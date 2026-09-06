use std::collections::VecDeque;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::os::raw::{c_char, c_uchar};
use std::str;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tungstenite::client::{client_with_config, IntoClientRequest};
use tungstenite::handshake::HandshakeError;
use tungstenite::protocol::{Message, WebSocket, WebSocketConfig};
use tungstenite::Error as WebSocketError;

use crate::{MAX_BUFFERED_PAYLOAD, MAX_EVENT_RECORDS, MAX_MESSAGE_BYTES, RESUME_CREDENTIAL_BYTES};

pub const CLIENT_ABI_VERSION: u32 = 1;
pub const STATUS_DISCONNECTED: i32 = 0;
pub const STATUS_CONNECTED: i32 = 1;
pub const STATUS_CONNECTING: i32 = 2;

const INVALID_ARGUMENT: i32 = -1;
const TRANSPORT_ERROR: i32 = -2;
const QUEUE_FULL: i32 = -3;
const INVALID_CREDENTIAL: i32 = -4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_POLL_INTERVAL: Duration = Duration::from_millis(25);
const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(2);

enum OpenError {
    Retry,
    Rejected,
    Cancelled,
}

#[derive(Clone)]
struct ClientConfig {
    address: SocketAddr,
    host: String,
    pairing_secret: String,
    resume_credential: [u8; RESUME_CREDENTIAL_BYTES],
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientConfig")
            .field("address", &self.address)
            .field("pairing_secret", &"[REDACTED]")
            .field("resume_credential", &"[REDACTED]")
            .finish()
    }
}

impl ClientConfig {
    fn from_join_url(join_url: &str) -> Result<Self, i32> {
        let rest = join_url.strip_prefix("http://").ok_or(INVALID_CREDENTIAL)?;
        let (authority, secret) = rest.split_once("/#secret=").ok_or(INVALID_CREDENTIAL)?;
        if authority.is_empty()
            || secret.len() < 32
            || secret.len() > 128
            || secret.len() % 2 != 0
            || !secret.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(INVALID_CREDENTIAL);
        }
        let (ip, port) = authority.rsplit_once(':').ok_or(INVALID_CREDENTIAL)?;
        let ip = ip.parse::<Ipv4Addr>().map_err(|_| INVALID_CREDENTIAL)?;
        let port = port.parse::<u16>().map_err(|_| INVALID_CREDENTIAL)?;
        if port == 0 {
            return Err(INVALID_CREDENTIAL);
        }
        let mut resume_credential = [0_u8; RESUME_CREDENTIAL_BYTES];
        getrandom::fill(&mut resume_credential).map_err(|_| TRANSPORT_ERROR)?;
        Ok(Self {
            address: SocketAddr::V4(SocketAddrV4::new(ip, port)),
            host: authority.to_string(),
            pairing_secret: secret.to_ascii_lowercase(),
            resume_credential,
        })
    }

    fn request(&self) -> Result<tungstenite::http::Request<()>, i32> {
        let mut request = format!("ws://{}/session", self.host)
            .into_client_request()
            .map_err(|_| INVALID_CREDENTIAL)?;
        request.headers_mut().insert(
            "Origin",
            format!("http://{}", self.host)
                .parse()
                .map_err(|_| INVALID_CREDENTIAL)?,
        );
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!(
                "stasis-v1, {}, stasis-resume-v1.{}",
                self.pairing_secret,
                hex_encode(&self.resume_credential)
            )
            .parse()
            .map_err(|_| INVALID_CREDENTIAL)?,
        );
        Ok(request)
    }
}

struct IncomingQueue {
    records: VecDeque<Vec<u8>>,
    bytes: usize,
}

impl IncomingQueue {
    fn clear(&mut self) {
        self.records.clear();
        self.bytes = 0;
    }
}

struct Shared {
    desired: AtomicBool,
    background: AtomicBool,
    accepting_incoming: AtomicBool,
    status: AtomicI32,
    error: AtomicI32,
    outbound_bytes: AtomicUsize,
    outbound_records: AtomicUsize,
    generation: AtomicUsize,
    incoming: Mutex<IncomingQueue>,
    resume_seat: AtomicI32,
    last_sequence: AtomicI32,
}

enum Command {
    Wake,
    Disconnect,
    Background(bool),
    Send { generation: usize, payload: Vec<u8> },
    Shutdown,
}

pub struct NetworkClient {
    shared: Arc<Shared>,
    commands: SyncSender<Command>,
    thread: Mutex<Option<JoinHandle<()>>>,
    config: ClientConfig,
}

impl fmt::Debug for NetworkClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkClient")
            .field("config", &self.config)
            .field("status", &self.status())
            .field("resume_seat", &self.resume_seat())
            .field("last_sequence", &self.last_sequence())
            .finish()
    }
}

impl NetworkClient {
    pub fn new(join_url: &str) -> Result<Self, i32> {
        let config = ClientConfig::from_join_url(join_url)?;
        let shared = Arc::new(Shared {
            desired: AtomicBool::new(false),
            background: AtomicBool::new(false),
            accepting_incoming: AtomicBool::new(false),
            status: AtomicI32::new(STATUS_DISCONNECTED),
            error: AtomicI32::new(0),
            outbound_bytes: AtomicUsize::new(0),
            outbound_records: AtomicUsize::new(0),
            generation: AtomicUsize::new(1),
            incoming: Mutex::new(IncomingQueue {
                records: VecDeque::new(),
                bytes: 0,
            }),
            resume_seat: AtomicI32::new(-1),
            last_sequence: AtomicI32::new(0),
        });
        let (command_tx, command_rx) = mpsc::sync_channel(MAX_EVENT_RECORDS);
        let worker_shared = Arc::clone(&shared);
        let worker_config = config.clone();
        let worker = thread::Builder::new()
            .name("stasis-network-client".to_string())
            .spawn(move || worker_loop(worker_config, worker_shared, command_rx))
            .map_err(|_| TRANSPORT_ERROR)?;
        Ok(Self {
            shared,
            commands: command_tx,
            thread: Mutex::new(Some(worker)),
            config,
        })
    }

    pub fn connect(&self) -> i32 {
        self.shared.desired.store(true, Ordering::Release);
        if self.shared.background.load(Ordering::Acquire) {
            return 0;
        }
        if self.shared.status.load(Ordering::Acquire) == STATUS_CONNECTED {
            return 0;
        }
        self.shared
            .status
            .store(STATUS_CONNECTING, Ordering::Release);
        self.shared.error.store(0, Ordering::Release);
        match self.commands.try_send(Command::Wake) {
            Ok(()) | Err(TrySendError::Full(_)) => 0,
            Err(TrySendError::Disconnected(_)) => TRANSPORT_ERROR,
        }
    }

    pub fn disconnect(&self) -> i32 {
        self.shared.desired.store(false, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared
            .accepting_incoming
            .store(false, Ordering::Release);
        self.shared
            .status
            .store(STATUS_DISCONNECTED, Ordering::Release);
        self.shared.error.store(0, Ordering::Release);
        clear_incoming(&self.shared);
        match self.commands.try_send(Command::Disconnect) {
            Ok(()) => 0,
            Err(TrySendError::Full(_)) => 0,
            Err(TrySendError::Disconnected(_)) => TRANSPORT_ERROR,
        }
    }

    pub fn set_background(&self, background: bool) -> i32 {
        if self.shared.background.swap(background, Ordering::AcqRel) == background {
            return 0;
        }
        if background {
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
            self.shared
                .accepting_incoming
                .store(false, Ordering::Release);
            self.shared
                .status
                .store(STATUS_DISCONNECTED, Ordering::Release);
            self.shared.error.store(0, Ordering::Release);
            clear_incoming(&self.shared);
        } else if self.shared.desired.load(Ordering::Acquire) {
            self.shared
                .status
                .store(STATUS_CONNECTING, Ordering::Release);
        }
        match self.commands.try_send(Command::Background(background)) {
            Ok(()) => 0,
            Err(TrySendError::Full(_)) => 0,
            Err(TrySendError::Disconnected(_)) => TRANSPORT_ERROR,
        }
    }

    pub fn status(&self) -> i32 {
        self.shared.status.load(Ordering::Acquire)
    }

    pub fn poll(&self, out: &mut [u8]) -> i32 {
        let Ok(mut queue) = self.shared.incoming.lock() else {
            return TRANSPORT_ERROR;
        };
        let Some(next) = queue.records.front() else {
            return self.shared.error.load(Ordering::Acquire);
        };
        if next.len() > out.len() {
            return INVALID_ARGUMENT;
        }
        let next = queue.records.pop_front().expect("front record exists");
        queue.bytes -= next.len();
        out[..next.len()].copy_from_slice(&next);
        next.len() as i32
    }

    pub fn send(&self, payload: &[u8]) -> i32 {
        if payload.len() > MAX_MESSAGE_BYTES {
            return INVALID_ARGUMENT;
        }
        if !self.shared.desired.load(Ordering::Acquire)
            || self.shared.background.load(Ordering::Acquire)
        {
            return TRANSPORT_ERROR;
        }
        if reserve_outbound(&self.shared, payload.len()).is_err() {
            return QUEUE_FULL;
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        match self.commands.try_send(Command::Send {
            generation,
            payload: payload.to_vec(),
        }) {
            Ok(()) => 0,
            Err(TrySendError::Full(_)) => {
                release_outbound(&self.shared, payload.len());
                QUEUE_FULL
            }
            Err(TrySendError::Disconnected(_)) => {
                release_outbound(&self.shared, payload.len());
                TRANSPORT_ERROR
            }
        }
    }

    pub fn checkpoint(&self, seat: i32, sequence: i32) -> i32 {
        if !(-1..crate::MAX_SEATS as i32).contains(&seat) || sequence < 0 {
            self.shared.error.store(INVALID_ARGUMENT, Ordering::Release);
            return INVALID_ARGUMENT;
        }
        self.shared.resume_seat.store(seat, Ordering::Release);
        self.shared.last_sequence.store(sequence, Ordering::Release);
        0
    }

    pub fn resume_seat(&self) -> i32 {
        self.shared.resume_seat.load(Ordering::Acquire)
    }

    pub fn last_sequence(&self) -> i32 {
        self.shared.last_sequence.load(Ordering::Acquire)
    }

    fn shutdown(&self) {
        self.shared.desired.store(false, Ordering::Release);
        self.shared
            .accepting_incoming
            .store(false, Ordering::Release);
        let _ = self.commands.send(Command::Shutdown);
        if let Ok(mut slot) = self.thread.lock() {
            if let Some(worker) = slot.take() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for NetworkClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(config: ClientConfig, shared: Arc<Shared>, commands: Receiver<Command>) {
    let mut socket = None;
    let mut socket_generation = 0;
    let mut pending = VecDeque::new();
    let mut next_attempt = Instant::now();
    let mut backoff = INITIAL_BACKOFF;
    loop {
        while let Ok(command) = commands.try_recv() {
            if handle_command(command, &shared, &mut socket, &mut pending) {
                clear_pending(&shared, &mut pending, &commands);
                return;
            }
        }
        if socket.is_some() && socket_generation != shared.generation.load(Ordering::Acquire) {
            close_socket(&mut socket);
            shared.accepting_incoming.store(false, Ordering::Release);
            clear_pending_only(&shared, &mut pending);
        }
        let active =
            shared.desired.load(Ordering::Acquire) && !shared.background.load(Ordering::Acquire);
        if !active {
            close_socket(&mut socket);
            clear_pending_only(&shared, &mut pending);
            if shared.status.load(Ordering::Acquire) >= 0 {
                shared.status.store(STATUS_DISCONNECTED, Ordering::Release);
            }
            match commands.recv_timeout(IO_POLL_INTERVAL) {
                Ok(command) => {
                    if handle_command(command, &shared, &mut socket, &mut pending) {
                        clear_pending(&shared, &mut pending, &commands);
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }
        if socket.is_none() && Instant::now() >= next_attempt {
            let attempt_generation = shared.generation.load(Ordering::Acquire);
            shared.status.store(STATUS_CONNECTING, Ordering::Release);
            match open_socket(&config, &shared) {
                Ok(open)
                    if shared.desired.load(Ordering::Acquire)
                        && !shared.background.load(Ordering::Acquire)
                        && shared.generation.load(Ordering::Acquire) == attempt_generation =>
                {
                    socket_generation = attempt_generation;
                    socket = Some(open);
                    shared.error.store(0, Ordering::Release);
                    shared.status.store(STATUS_CONNECTED, Ordering::Release);
                    shared.accepting_incoming.store(true, Ordering::Release);
                    backoff = INITIAL_BACKOFF;
                }
                Ok(mut stale) => {
                    let _ = stale.close(None);
                    shared.status.store(STATUS_DISCONNECTED, Ordering::Release);
                }
                Err(OpenError::Cancelled) => {
                    shared.status.store(STATUS_DISCONNECTED, Ordering::Release);
                }
                Err(OpenError::Rejected) => {
                    shared.desired.store(false, Ordering::Release);
                    shared.error.store(INVALID_CREDENTIAL, Ordering::Release);
                    shared.status.store(INVALID_CREDENTIAL, Ordering::Release);
                    clear_pending_only(&shared, &mut pending);
                }
                Err(OpenError::Retry) => {
                    shared.error.store(TRANSPORT_ERROR, Ordering::Release);
                    next_attempt = Instant::now() + backoff;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
        if let Some(open) = socket.as_mut() {
            let failed = flush_pending(open, &shared, &mut pending, socket_generation)
                || read_one(open, &shared, socket_generation).is_err();
            if failed {
                close_socket(&mut socket);
                shared.generation.fetch_add(1, Ordering::AcqRel);
                clear_pending_only(&shared, &mut pending);
                connection_lost(&shared);
                next_attempt = Instant::now() + backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        } else {
            thread::sleep(
                IO_POLL_INTERVAL.min(next_attempt.saturating_duration_since(Instant::now())),
            );
        }
    }
}

fn handle_command(
    command: Command,
    shared: &Shared,
    socket: &mut Option<WebSocket<TcpStream>>,
    pending: &mut VecDeque<(usize, Vec<u8>)>,
) -> bool {
    match command {
        Command::Wake => {}
        Command::Disconnect | Command::Background(true) => {
            close_socket(socket);
            clear_pending_only(shared, pending);
            clear_incoming(shared);
            shared.status.store(STATUS_DISCONNECTED, Ordering::Release);
        }
        Command::Background(false) => {}
        Command::Send {
            generation,
            payload,
        } => {
            if generation == shared.generation.load(Ordering::Acquire) {
                pending.push_back((generation, payload));
            } else {
                release_outbound(shared, payload.len());
            }
        }
        Command::Shutdown => {
            close_socket(socket);
            return true;
        }
    }
    false
}

fn open_socket(config: &ClientConfig, shared: &Shared) -> Result<WebSocket<TcpStream>, OpenError> {
    let stream = TcpStream::connect_timeout(&config.address, CONNECT_TIMEOUT)
        .map_err(|_| OpenError::Retry)?;
    stream.set_nodelay(true).map_err(|_| OpenError::Retry)?;
    stream.set_nonblocking(true).map_err(|_| OpenError::Retry)?;
    let request = config.request().map_err(|_| OpenError::Rejected)?;
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(MAX_MESSAGE_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_BUFFERED_PAYLOAD)
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut handshake = client_with_config(request, stream, Some(websocket_config));
    let (socket, response) = loop {
        match handshake {
            Ok(result) => break result,
            Err(HandshakeError::Interrupted(mid)) => {
                if !shared.desired.load(Ordering::Acquire)
                    || shared.background.load(Ordering::Acquire)
                {
                    return Err(OpenError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(OpenError::Retry);
                }
                thread::sleep(Duration::from_millis(2));
                handshake = mid.handshake();
            }
            Err(HandshakeError::Failure(error)) => return Err(classify_handshake_error(error)),
        }
    };
    let selected = response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok());
    if selected != Some("stasis-v1") {
        return Err(OpenError::Rejected);
    }
    Ok(socket)
}

fn classify_handshake_error(error: WebSocketError) -> OpenError {
    match error {
        WebSocketError::Io(_) => OpenError::Retry,
        WebSocketError::Http(response)
            if response.body().as_deref() == Some(b"resume credential unavailable") =>
        {
            OpenError::Retry
        }
        _ => OpenError::Rejected,
    }
}

fn flush_pending(
    socket: &mut WebSocket<TcpStream>,
    shared: &Shared,
    pending: &mut VecDeque<(usize, Vec<u8>)>,
    generation: usize,
) -> bool {
    while let Some((payload_generation, payload)) = pending.pop_front() {
        let length = payload.len();
        if payload_generation != generation
            || shared.generation.load(Ordering::Acquire) != generation
            || !shared.desired.load(Ordering::Acquire)
            || shared.background.load(Ordering::Acquire)
        {
            release_outbound(shared, length);
            continue;
        }
        let result = socket.send(Message::Binary(payload.into()));
        release_outbound(shared, length);
        if result.is_err() {
            return true;
        }
    }
    false
}

fn read_one(
    socket: &mut WebSocket<TcpStream>,
    shared: &Shared,
    generation: usize,
) -> Result<(), ()> {
    match socket.read() {
        Ok(Message::Binary(payload)) => enqueue_incoming(shared, payload.to_vec(), generation),
        Ok(Message::Text(_)) => Err(()),
        Ok(Message::Ping(payload)) => socket.send(Message::Pong(payload)).map_err(|_| ()),
        Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => Ok(()),
        Ok(Message::Close(frame)) => {
            let _ = socket.close(frame);
            Err(())
        }
        Err(WebSocketError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut =>
        {
            thread::sleep(Duration::from_millis(2));
            Ok(())
        }
        Err(_) => Err(()),
    }
}

fn enqueue_incoming(shared: &Shared, payload: Vec<u8>, generation: usize) -> Result<(), ()> {
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(());
    }
    let Ok(mut queue) = shared.incoming.lock() else {
        return Err(());
    };
    if !shared.accepting_incoming.load(Ordering::Acquire)
        || !shared.desired.load(Ordering::Acquire)
        || shared.background.load(Ordering::Acquire)
        || shared.generation.load(Ordering::Acquire) != generation
    {
        return Ok(());
    }
    if queue.records.len() >= MAX_EVENT_RECORDS
        || queue.bytes.saturating_add(payload.len()) > MAX_BUFFERED_PAYLOAD
    {
        shared.error.store(QUEUE_FULL, Ordering::Release);
        return Ok(());
    }
    queue.bytes += payload.len();
    queue.records.push_back(payload);
    Ok(())
}

fn connection_lost(shared: &Shared) {
    shared.accepting_incoming.store(false, Ordering::Release);
    let status =
        if shared.desired.load(Ordering::Acquire) && !shared.background.load(Ordering::Acquire) {
            STATUS_CONNECTING
        } else {
            STATUS_DISCONNECTED
        };
    shared.status.store(status, Ordering::Release);
    shared.error.store(TRANSPORT_ERROR, Ordering::Release);
    clear_incoming(shared);
}

fn close_socket(socket: &mut Option<WebSocket<TcpStream>>) {
    if let Some(mut open) = socket.take() {
        let _ = open.close(None);
        let _ = open.get_ref().shutdown(std::net::Shutdown::Both);
    }
}

fn clear_incoming(shared: &Shared) {
    if let Ok(mut queue) = shared.incoming.lock() {
        queue.clear();
    }
}

fn clear_pending_only(shared: &Shared, pending: &mut VecDeque<(usize, Vec<u8>)>) {
    while let Some((_, payload)) = pending.pop_front() {
        release_outbound(shared, payload.len());
    }
}

fn clear_pending(
    shared: &Shared,
    pending: &mut VecDeque<(usize, Vec<u8>)>,
    commands: &Receiver<Command>,
) {
    clear_pending_only(shared, pending);
    loop {
        match commands.try_recv() {
            Ok(Command::Send { payload, .. }) => release_outbound(shared, payload.len()),
            Ok(_) => {}
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
        }
    }
}

fn reserve_outbound(shared: &Shared, amount: usize) -> Result<(), ()> {
    let records = shared.outbound_records.fetch_add(1, Ordering::AcqRel);
    if records >= MAX_EVENT_RECORDS {
        shared.outbound_records.fetch_sub(1, Ordering::AcqRel);
        return Err(());
    }
    let mut current = shared.outbound_bytes.load(Ordering::Acquire);
    loop {
        if current.saturating_add(amount) > MAX_BUFFERED_PAYLOAD {
            shared.outbound_records.fetch_sub(1, Ordering::AcqRel);
            return Err(());
        }
        match shared.outbound_bytes.compare_exchange(
            current,
            current + amount,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return Ok(()),
            Err(actual) => current = actual,
        }
    }
}

fn release_outbound(shared: &Shared, amount: usize) {
    shared.outbound_bytes.fetch_sub(amount, Ordering::AcqRel);
    shared.outbound_records.fetch_sub(1, Ordering::AcqRel);
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}

#[no_mangle]
pub extern "C" fn stasis_network_client_abi_version() -> u32 {
    CLIENT_ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_create(
    join_url: *const c_char,
    length: usize,
) -> *mut NetworkClient {
    if join_url.is_null() || length == 0 || length > 512 {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(join_url.cast::<u8>(), length) };
    let Ok(join_url) = str::from_utf8(bytes) else {
        return std::ptr::null_mut();
    };
    NetworkClient::new(join_url)
        .map(Box::new)
        .map(Box::into_raw)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_connect(client: *mut NetworkClient) -> i32 {
    client
        .as_ref()
        .map(NetworkClient::connect)
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_disconnect(client: *mut NetworkClient) -> i32 {
    client
        .as_ref()
        .map(NetworkClient::disconnect)
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_set_background(
    client: *mut NetworkClient,
    background: i32,
) -> i32 {
    if !matches!(background, 0 | 1) {
        return INVALID_ARGUMENT;
    }
    client
        .as_ref()
        .map(|client| client.set_background(background != 0))
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_status(client: *mut NetworkClient) -> i32 {
    client
        .as_ref()
        .map(NetworkClient::status)
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_poll(
    client: *mut NetworkClient,
    out: *mut c_uchar,
    capacity: usize,
) -> i32 {
    if client.is_null() || (out.is_null() && capacity != 0) {
        return INVALID_ARGUMENT;
    }
    let output = if capacity == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(out, capacity) }
    };
    unsafe { (&*client).poll(output) }
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_send(
    client: *mut NetworkClient,
    payload: *const c_uchar,
    length: usize,
) -> i32 {
    if client.is_null() || (payload.is_null() && length != 0) || length > MAX_MESSAGE_BYTES {
        return INVALID_ARGUMENT;
    }
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(payload, length) }
    };
    unsafe { (&*client).send(bytes) }
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_checkpoint(
    client: *mut NetworkClient,
    seat: i32,
    sequence: i32,
) -> i32 {
    client
        .as_ref()
        .map(|client| client.checkpoint(seat, sequence))
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_resume_seat(client: *mut NetworkClient) -> i32 {
    client
        .as_ref()
        .map(NetworkClient::resume_seat)
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_last_sequence(client: *mut NetworkClient) -> i32 {
    client
        .as_ref()
        .map(NetworkClient::last_sequence)
        .unwrap_or(INVALID_ARGUMENT)
}

#[no_mangle]
pub unsafe extern "C" fn stasis_network_client_destroy(client: *mut NetworkClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

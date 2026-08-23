#![deny(warnings)]

use std::collections::{BTreeMap, HashMap};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::raw::{c_char, c_uchar};
use std::str;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;
use tungstenite::handshake::server::{Request, Response};
use tungstenite::protocol::{Message, WebSocket, WebSocketConfig};
use tungstenite::{accept_hdr_with_config, Error as WebSocketError};

pub const ARCHIVE_MAGIC: [u8; 4] = *b"SGB1";
pub const ARCHIVE_VERSION: u16 = 1;
pub const MAX_FILES: usize = 256;
pub const MAX_PATH_BYTES: usize = 128;
pub const MAX_MIME_BYTES: usize = 64;
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub const ABI_VERSION: u32 = 1;
pub const RELEASE_ID: &str = "stasis_network-0.1";
const RELEASE_ID_BYTES: &[u8] = b"stasis_network-0.1\0";
pub const MAX_SEATS: usize = 8;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_BUFFERED_PAYLOAD: usize = 1024 * 1024;
pub const MAX_EVENT_RECORDS: usize = 256;
pub const MAX_HTTP_HEADER_BYTES: usize = 8 * 1024;
pub const SESSION_SECRET_BYTES: usize = 32;
pub const RESUME_CREDENTIAL_BYTES: usize = 16;
const SESSION_PATH: &str = "/session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum EventKind {
    Connected = 1,
    HttpRequest = 2,
    Message = 3,
    Disconnected = 4,
    Rejected = 5,
    Overflow = 6,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkEvent {
    pub kind: EventKind,
    pub connection: u32,
    pub payload: Vec<u8>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkError {
    InvalidArgument,
    AlreadyStopped,
    QueueFull,
    Io,
}

pub fn fill_session_secret(out: &mut [u8]) -> Result<(), NetworkError> {
    if out.len() < 16 {
        return Err(NetworkError::InvalidArgument);
    }
    getrandom::fill(out).map_err(|_| NetworkError::Io)
}

fn positive_seed_from_bytes(bytes: [u8; 4]) -> i32 {
    let value = u32::from_le_bytes(bytes) & 0x7fff_ffff;
    if value == 0 {
        1
    } else {
        value as i32
    }
}

/// Obtain the hidden deterministic-game seed at the native adapter boundary.
/// A failed OS CSPRNG call is reported as zero; callers must not substitute a
/// clock, tick count, or client-provided value.
pub fn random_game_seed() -> i32 {
    let mut bytes = [0_u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        return 0;
    }
    positive_seed_from_bytes(bytes)
}

#[derive(Debug, Clone)]
pub struct HostOptions {
    pub bind_addr: IpAddr,
    pub port: u16,
    pub bundle: StaticBundle,
    pub expected_origin: Option<String>,
    pub session_secret: Vec<u8>,
    pub max_connections: usize,
    pub max_buffered_bytes: usize,
}
impl HostOptions {
    pub fn loopback(port: u16, bundle: StaticBundle) -> Result<Self, NetworkError> {
        let mut secret = vec![0; SESSION_SECRET_BYTES];
        fill_session_secret(&mut secret)?;
        Ok(Self {
            bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            bundle,
            expected_origin: None,
            session_secret: secret,
            max_connections: MAX_SEATS,
            max_buffered_bytes: MAX_BUFFERED_PAYLOAD,
        })
    }
    fn validate(&self) -> Result<(), NetworkError> {
        if self.session_secret.len() < 16
            || self.session_secret.len() > 64
            || self.max_connections == 0
            || self.max_connections > MAX_SEATS
            || self.max_buffered_bytes < MAX_MESSAGE_BYTES
            || self.max_buffered_bytes > MAX_BUFFERED_PAYLOAD
        {
            return Err(NetworkError::InvalidArgument);
        }
        Ok(())
    }
}

enum HostCommand {
    Send { connection: u32, payload: Vec<u8> },
    Stop,
}
#[derive(Debug, Clone, Copy)]
struct ResumeRecord {
    handle: u32,
    active: bool,
}
struct Shared {
    stopped: AtomicBool,
    next_connection: AtomicU32,
    active_connections: AtomicUsize,
    buffered_bytes: AtomicUsize,
    overflow_count: AtomicUsize,
    max_connections: usize,
    max_buffered_bytes: usize,
    session_secret: Vec<u8>,
    expected_origin: Option<String>,
    bundle: StaticBundle,
    events: SyncSender<NetworkEvent>,
    writers: Mutex<HashMap<u32, SyncSender<Vec<u8>>>>,
    resume_credentials: Mutex<HashMap<[u8; RESUME_CREDENTIAL_BYTES], ResumeRecord>>,
}
pub struct NetworkHost {
    shared: Arc<Shared>,
    commands: SyncSender<HostCommand>,
    events: Receiver<NetworkEvent>,
    thread: Option<JoinHandle<()>>,
    address: SocketAddr,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BundleError {
    #[error("bundle header is truncated")]
    Truncated,
    #[error("bundle magic or version is invalid")]
    Header,
    #[error("bundle file count is invalid")]
    FileCount,
    #[error("bundle path is invalid")]
    Path,
    #[error("bundle MIME type is invalid")]
    Mime,
    #[error("bundle file length is invalid")]
    Length,
    #[error("bundle contains duplicate paths")]
    Duplicate,
    #[error("bundle has trailing bytes")]
    Trailing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFile {
    pub path: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticBundle {
    files: BTreeMap<String, BundleFile>,
}

impl StaticBundle {
    pub fn new(files: Vec<BundleFile>) -> Result<Self, BundleError> {
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(BundleError::FileCount);
        }
        let mut total = 0usize;
        let mut map = BTreeMap::new();
        for file in files {
            validate_path(&file.path)?;
            validate_mime(&file.mime)?;
            if file.bytes.len() > MAX_FILE_BYTES {
                return Err(BundleError::Length);
            }
            total = total
                .checked_add(file.bytes.len())
                .ok_or(BundleError::Length)?;
            if total > MAX_TOTAL_BYTES {
                return Err(BundleError::Length);
            }
            if map.insert(file.path.clone(), file).is_some() {
                return Err(BundleError::Duplicate);
            }
        }
        Ok(Self { files: map })
    }

    pub fn get(&self, path: &str) -> Option<&BundleFile> {
        self.files.get(path)
    }

    pub fn files(&self) -> impl Iterator<Item = &BundleFile> {
        self.files.values()
    }

    pub fn encode(&self) -> Result<Vec<u8>, BundleError> {
        let mut out = Vec::new();
        out.extend_from_slice(&ARCHIVE_MAGIC);
        out.extend_from_slice(&ARCHIVE_VERSION.to_be_bytes());
        out.extend_from_slice(&(self.files.len() as u16).to_be_bytes());
        for file in self.files.values() {
            let path = file.path.as_bytes();
            let mime = file.mime.as_bytes();
            out.extend_from_slice(&(path.len() as u16).to_be_bytes());
            out.extend_from_slice(&(mime.len() as u16).to_be_bytes());
            out.extend_from_slice(&(file.bytes.len() as u32).to_be_bytes());
            out.extend_from_slice(path);
            out.extend_from_slice(mime);
            out.extend_from_slice(&file.bytes);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BundleError> {
        let mut cursor = Cursor { bytes, offset: 0 };
        if cursor.take(4)? != ARCHIVE_MAGIC {
            return Err(BundleError::Header);
        }
        if cursor.u16()? != ARCHIVE_VERSION {
            return Err(BundleError::Header);
        }
        let count = cursor.u16()? as usize;
        if count == 0 || count > MAX_FILES {
            return Err(BundleError::FileCount);
        }
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            let path_len = cursor.u16()? as usize;
            let mime_len = cursor.u16()? as usize;
            let byte_len = cursor.u32()? as usize;
            if path_len == 0
                || path_len > MAX_PATH_BYTES
                || mime_len == 0
                || mime_len > MAX_MIME_BYTES
                || byte_len > MAX_FILE_BYTES
            {
                return Err(BundleError::Length);
            }
            let path = str::from_utf8(cursor.take(path_len)?)
                .map_err(|_| BundleError::Path)?
                .to_string();
            let mime = str::from_utf8(cursor.take(mime_len)?)
                .map_err(|_| BundleError::Mime)?
                .to_string();
            validate_path(&path)?;
            validate_mime(&mime)?;
            files.push(BundleFile {
                path,
                mime,
                bytes: cursor.take(byte_len)?.to_vec(),
            });
        }
        if cursor.offset != bytes.len() {
            return Err(BundleError::Trailing);
        }
        StaticBundle::new(files)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], BundleError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(BundleError::Truncated)?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or(BundleError::Truncated)?;
        self.offset = end;
        Ok(result)
    }

    fn u16(&mut self) -> Result<u16, BundleError> {
        Ok(u16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| BundleError::Truncated)?,
        ))
    }

    fn u32(&mut self) -> Result<u32, BundleError> {
        Ok(u32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| BundleError::Truncated)?,
        ))
    }
}

fn validate_path(path: &str) -> Result<(), BundleError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !path.is_ascii()
    {
        return Err(BundleError::Path);
    }
    Ok(())
}

fn validate_mime(mime: &str) -> Result<(), BundleError> {
    if mime.is_empty()
        || mime.len() > MAX_MIME_BYTES
        || !mime.is_ascii()
        || mime.bytes().any(|byte| byte < 0x20 || byte == 0x7f)
    {
        return Err(BundleError::Mime);
    }
    Ok(())
}

impl NetworkHost {
    pub fn bind(port: u16, bundle: StaticBundle) -> Result<Self, NetworkError> {
        Self::bind_with_options(HostOptions::loopback(port, bundle)?)
    }
    pub fn bind_with_options(options: HostOptions) -> Result<Self, NetworkError> {
        options.validate()?;
        let listener = TcpListener::bind(SocketAddr::new(options.bind_addr, options.port))
            .map_err(|_| NetworkError::Io)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Io)?;
        let address = listener.local_addr().map_err(|_| NetworkError::Io)?;
        let (event_tx, event_rx) = mpsc::sync_channel(MAX_EVENT_RECORDS);
        let (command_tx, command_rx) = mpsc::sync_channel(MAX_EVENT_RECORDS);
        let shared = Arc::new(Shared {
            stopped: AtomicBool::new(false),
            next_connection: AtomicU32::new(1),
            active_connections: AtomicUsize::new(0),
            buffered_bytes: AtomicUsize::new(0),
            overflow_count: AtomicUsize::new(0),
            max_connections: options.max_connections,
            max_buffered_bytes: options.max_buffered_bytes,
            session_secret: options.session_secret.clone(),
            expected_origin: options.expected_origin.clone(),
            bundle: options.bundle.clone(),
            events: event_tx,
            writers: Mutex::new(HashMap::new()),
            resume_credentials: Mutex::new(HashMap::new()),
        });
        let thread_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("stasis-network-listener".into())
            .spawn(move || listener_loop(listener, thread_shared, command_rx))
            .map_err(|_| NetworkError::Io)?;
        Ok(Self {
            shared,
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
            address,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn session_secret(&self) -> &[u8] {
        &self.shared.session_secret
    }
    pub fn join_url(&self) -> String {
        let host = match self.address.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => advertised_ipv4().to_string(),
            ip => ip.to_string(),
        };
        format!(
            "http://{}:{}/#secret={}",
            host,
            self.address.port(),
            hex_encode(&self.shared.session_secret)
        )
    }
    pub fn status(&self) -> u32 {
        if self.shared.stopped.load(Ordering::Acquire) {
            0
        } else {
            1
        }
    }
    pub fn overflow_count(&self) -> usize {
        self.shared.overflow_count.load(Ordering::Acquire)
    }
    pub fn poll(&self) -> Option<NetworkEvent> {
        let event = self.events.try_recv().ok()?;
        release(&self.shared, event.payload.len());
        Some(event)
    }
    pub fn send(&self, connection: u32, payload: &[u8]) -> Result<(), NetworkError> {
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(NetworkError::InvalidArgument);
        }
        reserve(&self.shared, payload.len())?;
        match self.commands.try_send(HostCommand::Send {
            connection,
            payload: payload.to_vec(),
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                release(&self.shared, payload.len());
                note_overflow(&self.shared, connection);
                Err(NetworkError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                release(&self.shared, payload.len());
                Err(NetworkError::AlreadyStopped)
            }
        }
    }
    pub fn stop(&mut self) -> Result<(), NetworkError> {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return Err(NetworkError::AlreadyStopped);
        }
        let _ = self.commands.try_send(HostCommand::Stop);
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| NetworkError::Io)?;
        }
        Ok(())
    }
}
impl Drop for NetworkHost {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn advertised_ipv4() -> Ipv4Addr {
    let Ok(probe) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return Ipv4Addr::LOCALHOST;
    };
    if probe.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).is_ok() {
        if let Ok(SocketAddr::V4(address)) = probe.local_addr() {
            if !address.ip().is_unspecified() {
                return *address.ip();
            }
        }
    }
    Ipv4Addr::LOCALHOST
}
fn reserve(shared: &Shared, amount: usize) -> Result<(), NetworkError> {
    loop {
        let current = shared.buffered_bytes.load(Ordering::Acquire);
        if amount > shared.max_buffered_bytes.saturating_sub(current) {
            return Err(NetworkError::QueueFull);
        }
        if shared
            .buffered_bytes
            .compare_exchange(
                current,
                current + amount,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            return Ok(());
        }
    }
}
fn release(shared: &Shared, amount: usize) {
    shared.buffered_bytes.fetch_sub(amount, Ordering::AcqRel);
}
fn note_overflow(shared: &Shared, connection: u32) {
    shared.overflow_count.fetch_add(1, Ordering::AcqRel);
    let _ = shared.events.try_send(NetworkEvent {
        kind: EventKind::Overflow,
        connection,
        payload: Vec::new(),
    });
}
fn publish(shared: &Shared, event: NetworkEvent) {
    let length = event.payload.len();
    if length > 0 && reserve(shared, length).is_err() {
        note_overflow(shared, event.connection);
        return;
    }
    if shared.events.try_send(event).is_err() {
        shared.overflow_count.fetch_add(1, Ordering::AcqRel);
        release(shared, length);
    }
}
fn claim_resume(
    shared: &Shared,
    credential: [u8; RESUME_CREDENTIAL_BYTES],
) -> Result<u32, NetworkError> {
    let mut records = shared
        .resume_credentials
        .lock()
        .map_err(|_| NetworkError::Io)?;
    if let Some(record) = records.get_mut(&credential) {
        if record.active {
            return Err(NetworkError::QueueFull);
        }
        record.active = true;
        return Ok(record.handle);
    }
    if records.len() >= MAX_SEATS {
        return Err(NetworkError::QueueFull);
    }
    let mut handle = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    if handle == 0 {
        handle = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    }
    records.insert(
        credential,
        ResumeRecord {
            handle,
            active: true,
        },
    );
    Ok(handle)
}
fn release_resume(shared: &Shared, handle: u32) {
    if let Ok(mut records) = shared.resume_credentials.lock() {
        if let Some(record) = records.values_mut().find(|r| r.handle == handle) {
            record.active = false;
        }
    }
}

fn listener_loop(listener: TcpListener, shared: Arc<Shared>, commands: Receiver<HostCommand>) {
    let mut workers = Vec::new();
    loop {
        match commands.try_recv() {
            Ok(HostCommand::Send {
                connection,
                payload,
            }) => {
                let payload_length = payload.len();
                let sender = shared
                    .writers
                    .lock()
                    .ok()
                    .and_then(|w| w.get(&connection).cloned());
                match sender {
                    Some(sender) => match sender.try_send(payload) {
                        Ok(()) => {}
                        Err(TrySendError::Full(payload)) => {
                            release(&shared, payload.len());
                            note_overflow(&shared, connection);
                        }
                        Err(TrySendError::Disconnected(payload)) => {
                            release(&shared, payload.len());
                            publish(
                                &shared,
                                NetworkEvent {
                                    kind: EventKind::Rejected,
                                    connection,
                                    payload: Vec::new(),
                                },
                            );
                        }
                    },
                    None => {
                        release(&shared, payload_length);
                        publish(
                            &shared,
                            NetworkEvent {
                                kind: EventKind::Rejected,
                                connection,
                                payload: Vec::new(),
                            },
                        );
                    }
                }
            }
            Ok(HostCommand::Stop) | Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                if shared.active_connections.load(Ordering::Acquire) >= shared.max_connections {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    note_overflow(&shared, 0);
                    continue;
                }
                shared.active_connections.fetch_add(1, Ordering::AcqRel);
                let id = shared.next_connection.fetch_add(1, Ordering::Relaxed);
                let worker_shared = Arc::clone(&shared);
                match thread::Builder::new()
                    .name(format!("stasis-network-connection-{id}"))
                    .spawn(move || connection_loop(stream, peer, id, worker_shared))
                {
                    Ok(worker) => workers.push(worker),
                    Err(_) => {
                        shared.active_connections.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(_) => break,
        }
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
    }
    for worker in workers {
        let _ = worker.join();
    }
}

fn connection_loop(mut stream: TcpStream, peer: SocketAddr, id: u32, shared: Arc<Shared>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut request = vec![0; MAX_HTTP_HEADER_BYTES];
    let header_length = loop {
        if shared.stopped.load(Ordering::Acquire) {
            finish(&shared);
            return;
        }
        match stream.peek(&mut request) {
            Ok(0) => {
                finish(&shared);
                return;
            }
            Ok(read) => {
                if let Some(end) = request[..read].windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
                if read == request.len() {
                    publish(
                        &shared,
                        NetworkEvent {
                            kind: EventKind::Rejected,
                            connection: id,
                            payload: Vec::new(),
                        },
                    );
                    finish(&shared);
                    return;
                }
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                continue
            }
            Err(_) => {
                finish(&shared);
                return;
            }
        }
    };
    let text = String::from_utf8_lossy(&request[..header_length]);
    let first = text.lines().next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let websocket_request = path == SESSION_PATH
        && text.lines().any(|line| {
            line.split_once(':')
                .map(|(n, v)| {
                    n.trim().eq_ignore_ascii_case("upgrade")
                        && v.split(',')
                            .any(|t| t.trim().eq_ignore_ascii_case("websocket"))
                })
                .unwrap_or(false)
        });
    if !websocket_request {
        let mut consumed = vec![0; header_length];
        if stream.read_exact(&mut consumed).is_err() {
            finish(&shared);
            return;
        }
    }
    if method != "GET" {
        write_http(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"not found\n",
        );
        publish(
            &shared,
            NetworkEvent {
                kind: EventKind::Rejected,
                connection: id,
                payload: Vec::new(),
            },
        );
        finish(&shared);
        return;
    }
    if websocket_request {
        let config = WebSocketConfig::default()
            .read_buffer_size(8 * 1024)
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_MESSAGE_BYTES + 1)
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let origin = shared.expected_origin.clone();
        let secret = shared.session_secret.clone();
        let claimed_handle = Arc::new(Mutex::new(None));
        let callback_handle = Arc::clone(&claimed_handle);
        let callback_shared = Arc::clone(&shared);
        let callback = move |request: &Request, mut response: Response| {
            let credential = validate_request(request, origin.as_deref(), &secret)?;
            let handle = claim_resume(&callback_shared, credential).map_err(|_| {
                Response::builder()
                    .status(403)
                    .body(Some("resume credential unavailable".to_string()))
                    .expect("response")
            })?;
            if let Ok(mut claimed) = callback_handle.lock() {
                *claimed = Some(handle);
            }
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                tungstenite::http::HeaderValue::from_static("stasis-v1"),
            );
            Ok(response)
        };
        match accept_hdr_with_config(stream, callback, Some(config)) {
            Ok(mut socket) => {
                if let Some(handle) = claimed_handle.lock().ok().and_then(|mut x| x.take()) {
                    websocket_loop(&mut socket, handle, peer, shared.clone());
                }
            }
            Err(_) => {
                if let Ok(mut claimed) = claimed_handle.lock() {
                    if let Some(handle) = claimed.take() {
                        release_resume(&shared, handle);
                    }
                }
                publish(
                    &shared,
                    NetworkEvent {
                        kind: EventKind::Rejected,
                        connection: id,
                        payload: Vec::new(),
                    },
                );
            }
        }
    } else if let Some(file) = static_file(&shared.bundle, path) {
        write_http(&mut stream, 200, &file.mime, &file.bytes);
        publish(
            &shared,
            NetworkEvent {
                kind: EventKind::HttpRequest,
                connection: id,
                payload: path.as_bytes().to_vec(),
            },
        );
    } else {
        write_http(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"not found\n",
        );
        publish(
            &shared,
            NetworkEvent {
                kind: EventKind::Rejected,
                connection: id,
                payload: Vec::new(),
            },
        );
    }
    finish(&shared);
}
fn static_file<'a>(bundle: &'a StaticBundle, path: &str) -> Option<&'a BundleFile> {
    let key = if path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/')?
    };
    if key.contains("/")
        && key
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    bundle.get(key)
}
fn write_http(stream: &mut TcpStream, status: u16, mime: &str, body: &[u8]) {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let header = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Type: {mime}\r\n\r\n", body.len());
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}
fn validate_request(
    request: &Request,
    expected_origin: Option<&str>,
    secret: &[u8],
) -> Result<[u8; RESUME_CREDENTIAL_BYTES], tungstenite::handshake::server::ErrorResponse> {
    let reject = || {
        Response::builder()
            .status(403)
            .body(Some("session rejected".to_string()))
            .expect("response")
    };
    if request.uri().path() != SESSION_PATH || request.headers().get("Host").is_none() {
        return Err(reject());
    }
    let Some(origin) = request
        .headers()
        .get("Origin")
        .and_then(|v| v.to_str().ok())
    else {
        return Err(reject());
    };
    let allowed = expected_origin
        .map(|value| value == origin)
        .unwrap_or_else(|| {
            request
                .headers()
                .get("Host")
                .and_then(|h| h.to_str().ok())
                .map(|h| format!("http://{h}") == origin)
                .unwrap_or(false)
        });
    if !allowed {
        return Err(reject());
    }
    let token = hex_encode(secret);
    let protocols = request
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').map(str::trim).collect::<Vec<_>>())
        .unwrap_or_default();
    if protocols.len() != 3
        || !protocols.iter().any(|p| *p == token)
        || !protocols.iter().any(|p| *p == "stasis-v1")
    {
        return Err(reject());
    }
    let resumes: Vec<_> = protocols
        .iter()
        .filter_map(|p| p.strip_prefix("stasis-resume-v1."))
        .collect();
    if resumes.len() != 1
        || resumes[0].len() != RESUME_CREDENTIAL_BYTES * 2
        || !resumes[0].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(reject());
    }
    let mut credential = [0; RESUME_CREDENTIAL_BYTES];
    for (i, pair) in resumes[0].as_bytes().chunks_exact(2).enumerate() {
        credential[i] = (hex_value(pair[0]) << 4) | hex_value(pair[1]);
    }
    Ok(credential)
}
fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}
fn websocket_loop(
    socket: &mut WebSocket<TcpStream>,
    id: u32,
    peer: SocketAddr,
    shared: Arc<Shared>,
) {
    let (send_tx, send_rx) = mpsc::sync_channel::<Vec<u8>>(32);
    if let Ok(mut writers) = shared.writers.lock() {
        writers.insert(id, send_tx);
    }
    publish(
        &shared,
        NetworkEvent {
            kind: EventKind::Connected,
            connection: id,
            payload: peer.to_string().into_bytes(),
        },
    );
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            let _ = socket.close(None);
            break;
        }
        while let Ok(payload) = send_rx.try_recv() {
            let length = payload.len();
            if socket.send(Message::Binary(payload.into())).is_err() {
                release(&shared, length);
                break;
            }
            release(&shared, length);
        }
        match socket.read() {
            Ok(Message::Binary(payload)) => publish(
                &shared,
                NetworkEvent {
                    kind: EventKind::Message,
                    connection: id,
                    payload: payload.to_vec(),
                },
            ),
            Ok(Message::Text(payload)) => publish(
                &shared,
                NetworkEvent {
                    kind: EventKind::Message,
                    connection: id,
                    payload: payload.as_bytes().to_vec(),
                },
            ),
            Ok(Message::Ping(payload)) => {
                let _ = socket.send(Message::Pong(payload));
            }
            Ok(Message::Pong(_)) => {}
            Ok(Message::Close(frame)) => {
                let _ = socket.close(frame);
                break;
            }
            Ok(Message::Frame(_)) => {}
            Err(WebSocketError::Io(error))
                if error.kind() == ErrorKind::WouldBlock || error.kind() == ErrorKind::TimedOut =>
            {
                continue
            }
            Err(_) => break,
        }
    }
    if let Ok(mut writers) = shared.writers.lock() {
        writers.remove(&id);
    }
    while let Ok(payload) = send_rx.try_recv() {
        release(&shared, payload.len());
    }
    publish(
        &shared,
        NetworkEvent {
            kind: EventKind::Disconnected,
            connection: id,
            payload: Vec::new(),
        },
    );
    release_resume(&shared, id);
}
fn finish(shared: &Shared) {
    shared.active_connections.fetch_sub(1, Ordering::AcqRel);
}
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        result.push(HEX[(b >> 4) as usize] as char);
        result.push(HEX[(b & 15) as usize] as char);
    }
    result
}

#[repr(C)]
pub struct StasisNetworkEvent {
    pub kind: u32,
    pub connection: u32,
    pub length: u32,
    pub payload: [c_uchar; MAX_MESSAGE_BYTES],
}
#[no_mangle]
pub extern "C" fn stasis_network_abi_version() -> u32 {
    ABI_VERSION
}
#[no_mangle]
pub extern "C" fn stasis_network_release_id() -> *const c_char {
    RELEASE_ID_BYTES.as_ptr().cast()
}
#[no_mangle]
pub extern "C" fn stasis_network_supported() -> i32 {
    if cfg!(any(
        target_os = "windows",
        target_os = "android",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos"
    )) {
        1
    } else {
        0
    }
}
#[no_mangle]
pub extern "C" fn stasis_network_random_seed() -> i32 {
    random_game_seed()
}
#[no_mangle]
pub extern "C" fn stasis_network_host_start(
    port: u16,
    content: *const c_uchar,
    content_len: usize,
    out_port: *mut u16,
) -> *mut NetworkHost {
    if content.is_null() || out_port.is_null() || content_len > MAX_TOTAL_BYTES + 1024 {
        return std::ptr::null_mut();
    }
    let Ok(bundle) =
        StaticBundle::decode(unsafe { std::slice::from_raw_parts(content, content_len) })
    else {
        return std::ptr::null_mut();
    };
    match NetworkHost::bind(port, bundle) {
        Ok(host) => {
            unsafe {
                *out_port = host.address().port();
            }
            Box::into_raw(Box::new(host))
        }
        Err(_) => std::ptr::null_mut(),
    }
}
#[no_mangle]
pub extern "C" fn stasis_network_host_start_bind(
    port: u16,
    bind_ipv4: u32,
    content: *const c_uchar,
    content_len: usize,
    out_port: *mut u16,
) -> *mut NetworkHost {
    if content.is_null() || out_port.is_null() || content_len > MAX_TOTAL_BYTES + 1024 {
        return std::ptr::null_mut();
    }
    let Ok(bundle) =
        StaticBundle::decode(unsafe { std::slice::from_raw_parts(content, content_len) })
    else {
        return std::ptr::null_mut();
    };
    let Ok(mut options) = HostOptions::loopback(port, bundle) else {
        return std::ptr::null_mut();
    };
    options.bind_addr = if bind_ipv4 == 0 {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        IpAddr::V4(Ipv4Addr::from(bind_ipv4.to_be_bytes()))
    };
    match NetworkHost::bind_with_options(options) {
        Ok(host) => {
            unsafe {
                *out_port = host.address().port();
            }
            Box::into_raw(Box::new(host))
        }
        Err(_) => std::ptr::null_mut(),
    }
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_poll(
    host: *mut NetworkHost,
    out: *mut StasisNetworkEvent,
) -> i32 {
    if host.is_null() || out.is_null() {
        return -1;
    }
    let Some(event) = (&*host).poll() else {
        return 0;
    };
    (*out).kind = event.kind as u32;
    (*out).connection = event.connection;
    (*out).length = event.payload.len() as u32;
    (&mut (*out).payload)[..event.payload.len()].copy_from_slice(&event.payload);
    1
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_send(
    host: *mut NetworkHost,
    connection: u32,
    payload: *const c_uchar,
    length: usize,
) -> i32 {
    if host.is_null() || payload.is_null() || length > MAX_MESSAGE_BYTES {
        return -1;
    }
    (&*host)
        .send(connection, std::slice::from_raw_parts(payload, length))
        .map(|_| 0)
        .unwrap_or(-2)
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_status(host: *mut NetworkHost) -> i32 {
    if host.is_null() {
        -1
    } else {
        (&*host).status() as i32
    }
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_overflow_count(host: *mut NetworkHost) -> u32 {
    if host.is_null() {
        0
    } else {
        (&*host).overflow_count().min(u32::MAX as usize) as u32
    }
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_port(host: *mut NetworkHost) -> u16 {
    if host.is_null() {
        0
    } else {
        (&*host).address().port()
    }
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_copy_join_url(
    host: *mut NetworkHost,
    out: *mut c_char,
    capacity: usize,
    out_length: *mut usize,
) -> i32 {
    if host.is_null() || out.is_null() || out_length.is_null() {
        return -1;
    }
    let bytes = (&*host).join_url();
    let bytes = bytes.as_bytes();
    if capacity <= bytes.len() {
        return -2;
    }
    unsafe {
        std::slice::from_raw_parts_mut(out.cast::<u8>(), bytes.len()).copy_from_slice(bytes);
        *out.add(bytes.len()) = 0;
        *out_length = bytes.len();
    }
    0
}
#[no_mangle]
pub unsafe extern "C" fn stasis_network_host_stop(host: *mut NetworkHost) {
    if !host.is_null() {
        let mut host = Box::from_raw(host);
        let _ = host.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::thread;
    use std::time::{Duration, Instant};
    use tungstenite::client::IntoClientRequest;
    use tungstenite::connect;

    #[test]
    fn positive_seed_mapping_is_bounded_and_nonzero() {
        assert_eq!(positive_seed_from_bytes([0, 0, 0, 0]), 1);
        assert_eq!(positive_seed_from_bytes([0xff, 0xff, 0xff, 0xff]), i32::MAX);
        assert!(positive_seed_from_bytes([1, 2, 3, 4]) > 0);
    }

    #[test]
    fn c_abi_random_seed_is_positive() {
        let seed = stasis_network_random_seed();
        assert!(seed > 0 && seed <= i32::MAX);
    }

    fn fixture() -> StaticBundle {
        StaticBundle::new(vec![
            BundleFile {
                path: "game.js".into(),
                mime: "text/javascript".into(),
                bytes: b"guest".to_vec(),
            },
            BundleFile {
                path: "index.html".into(),
                mime: "text/html; charset=utf-8".into(),
                bytes: b"<html/>".to_vec(),
            },
        ])
        .expect("fixture")
    }

    #[test]
    fn archive_round_trips_in_deterministic_path_order() {
        let bundle = fixture();
        let encoded = bundle.encode().expect("encode");
        let decoded = StaticBundle::decode(&encoded).expect("decode");
        assert_eq!(decoded, bundle);
        assert_eq!(
            decoded
                .files()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["game.js", "index.html"]
        );
    }

    #[test]
    fn archive_rejects_traversal_duplicate_trailing_and_truncation() {
        assert_eq!(
            StaticBundle::new(vec![BundleFile {
                path: "../x".into(),
                mime: "text/plain".into(),
                bytes: vec![]
            }]),
            Err(BundleError::Path)
        );
        assert_eq!(
            StaticBundle::new(vec![
                BundleFile {
                    path: "x".into(),
                    mime: "text/plain".into(),
                    bytes: vec![]
                },
                BundleFile {
                    path: "x".into(),
                    mime: "text/plain".into(),
                    bytes: vec![]
                }
            ]),
            Err(BundleError::Duplicate)
        );
        let encoded = fixture().encode().expect("encode");
        assert_eq!(
            StaticBundle::decode(&encoded[..encoded.len() - 1]),
            Err(BundleError::Truncated)
        );
        let mut trailing = encoded;
        trailing.push(0);
        assert_eq!(StaticBundle::decode(&trailing), Err(BundleError::Trailing));
    }

    #[test]
    fn archive_accepts_bounded_file_count_and_rejects_overflow() {
        let files = (0..MAX_FILES)
            .map(|index| BundleFile {
                path: format!("assets/{index}.bin"),
                mime: "application/octet-stream".into(),
                bytes: Vec::new(),
            })
            .collect::<Vec<_>>();
        let maximum = StaticBundle::new(files).expect("maximum file count");
        assert_eq!(maximum.files().count(), MAX_FILES);
        let encoded = maximum.encode().expect("encode maximum file count");
        assert_eq!(
            StaticBundle::decode(&encoded)
                .expect("decode maximum file count")
                .files()
                .count(),
            MAX_FILES
        );

        let mut too_many = (0..=MAX_FILES)
            .map(|index| BundleFile {
                path: format!("assets/{index}.bin"),
                mime: "application/octet-stream".into(),
                bytes: Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            StaticBundle::new(std::mem::take(&mut too_many)),
            Err(BundleError::FileCount)
        );
    }

    fn host_bundle() -> StaticBundle {
        StaticBundle::new(vec![
            BundleFile {
                path: "index.html".into(),
                mime: "text/html; charset=utf-8".into(),
                bytes: b"<html>guest</html>".to_vec(),
            },
            BundleFile {
                path: "game.wasm".into(),
                mime: "application/wasm".into(),
                bytes: b"wasm".to_vec(),
            },
            BundleFile {
                path: "assets/fonts/ui.ttf".into(),
                mime: "font/ttf".into(),
                bytes: b"font-bytes".to_vec(),
            },
        ])
        .expect("bundle")
    }

    fn wait_event(host: &NetworkHost, kind: EventKind) -> NetworkEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = host.poll() {
                if event.kind == kind {
                    return event;
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for {kind:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loopback_bundle_and_websocket_round_trip() {
        let mut host = NetworkHost::bind(0, host_bundle()).expect("bind");
        let mut http = TcpStream::connect(host.address()).expect("http connect");
        http.write_all(b"GET /game.wasm HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("request");
        let mut response = Vec::new();
        http.read_to_end(&mut response).expect("response");
        assert!(response.windows(b"200 OK".len()).any(|w| w == b"200 OK"));
        assert!(response.ends_with(b"wasm"));
        let _ = wait_event(&host, EventKind::HttpRequest);

        let mut asset_http = TcpStream::connect(host.address()).expect("asset http connect");
        asset_http
            .write_all(b"GET /assets/fonts/ui.ttf HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("asset request");
        let mut asset_response = Vec::new();
        asset_http
            .read_to_end(&mut asset_response)
            .expect("asset response");
        assert!(asset_response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(asset_response
            .windows(b"Content-Type: font/ttf\r\n".len())
            .any(|w| { w == b"Content-Type: font/ttf\r\n" }));
        assert!(asset_response.ends_with(b"font-bytes"));
        let _ = wait_event(&host, EventKind::HttpRequest);

        let url = format!("ws://{}/session", host.address());
        let token = hex_encode(host.session_secret());
        let mut request = url.into_client_request().expect("request");
        request.headers_mut().insert(
            "Origin",
            format!("http://{}", host.address())
                .parse()
                .expect("origin"),
        );
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("stasis-v1, {token}, stasis-resume-v1.00112233445566778899aabbccddeeff")
                .parse()
                .expect("protocol"),
        );
        let (mut socket, _) = connect(request).expect("websocket");
        let connected = wait_event(&host, EventKind::Connected);
        assert!(!connected.payload.is_empty());
        socket
            .send(Message::Binary(vec![1, 2, 3].into()))
            .expect("send");
        let event = wait_event(&host, EventKind::Message);
        assert_eq!(event.payload, vec![1, 2, 3]);
        host.send(event.connection, b"ack").expect("host send");
        assert_eq!(socket.read().expect("receive").into_data(), b"ack".to_vec());
        let _ = socket.close(None);
        host.stop().expect("stop");
    }

    #[test]
    fn malformed_resume_and_wrong_origin_are_rejected() {
        let mut host = NetworkHost::bind(0, host_bundle()).expect("bind");
        let token = hex_encode(host.session_secret());
        let mut request = format!("ws://{}/session", host.address())
            .into_client_request()
            .expect("request");
        request
            .headers_mut()
            .insert("Origin", "http://wrong.example".parse().expect("origin"));
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            format!("stasis-v1, {token}, stasis-resume-v1.0011")
                .parse()
                .expect("protocol"),
        );
        assert!(connect(request).is_err());
        assert_eq!(
            wait_event(&host, EventKind::Rejected).kind,
            EventKind::Rejected
        );
        host.stop().expect("stop");
    }

    #[test]
    fn wildcard_bind_never_advertises_unspecified_address() {
        let mut options = HostOptions::loopback(0, host_bundle()).expect("options");
        options.bind_addr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let mut host = NetworkHost::bind_with_options(options).expect("bind");
        let url = host.join_url();
        assert!(!url.contains("0.0.0.0"));
        // A sandbox with no route legitimately falls back to localhost; the
        // native Android overlay hides that value rather than presenting an
        // unusable address to nearby players.
        assert!(url.contains("localhost") || !url.contains("127.0.0.1"));
        host.stop().expect("stop");
    }
}

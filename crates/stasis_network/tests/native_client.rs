use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use stasis_network::client::{NetworkClient, STATUS_CONNECTED, STATUS_DISCONNECTED};
use stasis_network::{BundleFile, EventKind, NetworkEvent, NetworkHost, StaticBundle};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{accept_hdr, Message};

const TIMEOUT: Duration = Duration::from_secs(5);

fn host() -> NetworkHost {
    let bundle = StaticBundle::new(vec![BundleFile {
        path: "index.html".into(),
        mime: "text/html".into(),
        bytes: b"native client test".to_vec(),
    }])
    .expect("bundle");
    NetworkHost::bind(0, bundle).expect("host")
}

fn wait_status(client: &NetworkClient, expected: i32) {
    let deadline = Instant::now() + TIMEOUT;
    while client.status() != expected {
        assert!(
            Instant::now() < deadline,
            "client status remained {} instead of {expected}",
            client.status()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_event(host: &NetworkHost, kind: EventKind) -> NetworkEvent {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(event) = host.poll() {
            if event.kind == kind {
                return event;
            }
        }
        assert!(Instant::now() < deadline, "host event {kind:?} timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_payload(client: &NetworkClient, capacity: usize) -> Vec<u8> {
    let deadline = Instant::now() + TIMEOUT;
    let mut output = vec![0; capacity];
    loop {
        let result = client.poll(&mut output);
        if result > 0 {
            output.truncate(result as usize);
            return output;
        }
        assert!(matches!(result, 0 | -2), "unexpected poll result {result}");
        assert!(Instant::now() < deadline, "client payload timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn loopback_transport_matches_browser_handshake_and_resumes_identity() {
    let mut host = host();
    let client = NetworkClient::new(&host.join_url()).expect("client");
    assert_eq!(client.status(), STATUS_DISCONNECTED);
    assert_eq!(client.connect(), 0);
    wait_status(&client, STATUS_CONNECTED);
    let first = wait_event(&host, EventKind::Connected).connection;

    assert_eq!(client.send(b"guest command"), 0);
    let guest = wait_event(&host, EventKind::Message);
    assert_eq!(guest.connection, first);
    assert_eq!(guest.payload, b"guest command");

    host.send(first, b"authoritative snapshot")
        .expect("host send");
    assert_eq!(wait_payload(&client, 64), b"authoritative snapshot");
    assert_eq!(client.checkpoint(3, 41), 0);
    assert_eq!(client.resume_seat(), 3);
    assert_eq!(client.last_sequence(), 41);

    assert_eq!(client.disconnect(), 0);
    wait_status(&client, STATUS_DISCONNECTED);
    let disconnected = wait_event(&host, EventKind::Disconnected);
    assert_eq!(disconnected.connection, first);
    assert_eq!(client.connect(), 0);
    wait_status(&client, STATUS_CONNECTED);
    let resumed = wait_event(&host, EventKind::Connected);
    assert_eq!(resumed.connection, first);
    assert_eq!(client.resume_seat(), 3);
    assert_eq!(client.last_sequence(), 41);

    assert_eq!(client.disconnect(), 0);
    wait_event(&host, EventKind::Disconnected);
    drop(client);
    host.stop().expect("stop host");
}

#[test]
fn bounds_undersized_poll_and_lifecycle_drop_stale_data() {
    let mut host = host();
    let client = NetworkClient::new(&host.join_url()).expect("client");
    assert_eq!(client.connect(), 0);
    wait_status(&client, STATUS_CONNECTED);
    let connection = wait_event(&host, EventKind::Connected).connection;

    assert_eq!(client.send(&vec![0; 64 * 1024 + 1]), -1);
    assert_eq!(client.checkpoint(-1, i32::MAX), 0);

    host.send(connection, b"preserved").expect("host send");
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let mut short = [0_u8; 2];
        let result = client.poll(&mut short);
        if result == -1 {
            break;
        }
        assert!(Instant::now() < deadline, "undersized poll timed out");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(wait_payload(&client, 16), b"preserved");

    host.send(connection, b"stale before background")
        .expect("host send");
    thread::sleep(Duration::from_millis(75));
    assert_eq!(client.set_background(true), 0);
    wait_status(&client, STATUS_DISCONNECTED);
    assert_eq!(client.set_background(true), 0);
    assert_eq!(client.status(), STATUS_DISCONNECTED);
    assert_eq!(client.poll(&mut [0_u8; 64]), 0);
    wait_event(&host, EventKind::Disconnected);
    assert_eq!(client.send(b"not queued while backgrounded"), -2);

    assert_eq!(client.set_background(false), 0);
    wait_status(&client, STATUS_CONNECTED);
    let resumed = wait_event(&host, EventKind::Connected);
    assert_eq!(resumed.connection, connection);
    assert_eq!(client.set_background(false), 0);
    assert_eq!(client.status(), STATUS_CONNECTED);
    assert_eq!(client.set_background(true), 0);
    assert_eq!(client.set_background(false), 0);
    wait_event(&host, EventKind::Disconnected);
    let rapidly_resumed = wait_event(&host, EventKind::Connected);
    assert_eq!(rapidly_resumed.connection, connection);
    wait_status(&client, STATUS_CONNECTED);
    assert_eq!(client.poll(&mut [0_u8; 64]), 0);
    assert_eq!(client.checkpoint(-2, 0), -1);
    assert_eq!(client.checkpoint(0, -1), -1);
    assert_eq!(client.disconnect(), 0);
    wait_event(&host, EventKind::Disconnected);
    assert_eq!(client.send(b"not queued after disconnect"), -2);
    drop(client);
    host.stop().expect("stop host");
}

#[test]
fn rejects_wrong_protocol_and_redacts_credentials() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
    let address = listener.local_addr().expect("address");
    let secret = "a17c9e240d6b3f8152a48c70e93db6f1";
    let join_url = format!("http://{address}/#secret={secret}");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let callback = |_request: &Request, mut response: Response| {
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                secret.parse().expect("selected protocol"),
            );
            Ok(response)
        };
        if let Ok(mut socket) = accept_hdr(stream, callback) {
            let _ = socket.send(Message::Binary(b"must not be accepted".to_vec().into()));
        }
    });
    let client = NetworkClient::new(&join_url).expect("client");
    let debug = format!("{client:?}");
    assert!(!debug.contains(secret));
    assert!(!debug.contains("stasis-resume-v1"));
    assert_eq!(client.connect(), 0);
    server.join().expect("server");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(client.status(), -4);
    assert_eq!(client.poll(&mut [0_u8; 64]), -4);
    assert_eq!(client.disconnect(), 0);

    assert!(NetworkClient::new("https://127.0.0.1:1234/#secret=0011").is_err());
    assert!(
        NetworkClient::new("http://localhost:1234/#secret=00112233445566778899aabbccddeeff")
            .is_err()
    );
    assert!(
        NetworkClient::new("http://127.0.0.1:1234/#secret=00112233445566778899aabbccddeefg")
            .is_err()
    );
}

#[test]
fn outbound_caps_and_shutdown_interrupt_a_stalled_handshake() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
    let address = listener.local_addr().expect("address");
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        accepted_tx.send(()).expect("signal accept");
        let mut bytes = [0_u8; 256];
        while stream.read(&mut bytes).unwrap_or(0) != 0 {}
    });
    let join_url = format!("http://{address}/#secret=00112233445566778899aabbccddeeff");
    let client = NetworkClient::new(&join_url).expect("client");
    assert_eq!(client.connect(), 0);
    accepted_rx
        .recv_timeout(TIMEOUT)
        .expect("client reached stalled handshake");

    let full_message = vec![7_u8; 64 * 1024];
    for _ in 0..16 {
        assert_eq!(client.send(&full_message), 0);
    }
    assert_eq!(client.send(&[1]), -3, "one MiB outbound byte cap");
    for _ in 16..256 {
        assert_eq!(client.send(&[]), 0);
    }
    assert_eq!(client.send(&[]), -3, "256 outbound record cap");

    let shutdown_started = Instant::now();
    assert_eq!(client.disconnect(), 0);
    drop(client);
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(1),
        "stalled handshake delayed shutdown for {:?}",
        shutdown_started.elapsed()
    );
    server.join().expect("server");
}

#[test]
fn shutdown_interrupts_a_slow_incomplete_websocket_frame() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
    let address = listener.local_addr().expect("address");
    let (frame_tx, frame_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        let callback = |_request: &Request, mut response: Response| {
            response.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                "stasis-v1".parse().expect("selected protocol"),
            );
            Ok(response)
        };
        let mut socket = accept_hdr(stream, callback).expect("websocket");
        let mut header = vec![0x82, 0x7f];
        header.extend_from_slice(&(64_u64 * 1024).to_be_bytes());
        socket.get_mut().write_all(&header).expect("frame header");
        frame_tx.send(()).expect("signal frame");
        for _ in 0..64 * 1024 {
            if socket.get_mut().write_all(&[7]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let join_url = format!("http://{address}/#secret=00112233445566778899aabbccddeeff");
    let client = NetworkClient::new(&join_url).expect("client");
    assert_eq!(client.connect(), 0);
    wait_status(&client, STATUS_CONNECTED);
    frame_rx
        .recv_timeout(TIMEOUT)
        .expect("partial frame started");
    thread::sleep(Duration::from_millis(30));

    let shutdown_started = Instant::now();
    drop(client);
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(1),
        "partial frame delayed shutdown for {:?}",
        shutdown_started.elapsed()
    );
    server.join().expect("server");
}

#[test]
fn idle_connected_client_handles_disconnect_without_poll_delay() {
    let mut host = host();
    let client = NetworkClient::new(&host.join_url()).expect("client");
    assert_eq!(client.connect(), 0);
    wait_status(&client, STATUS_CONNECTED);
    wait_event(&host, EventKind::Connected);

    thread::sleep(Duration::from_millis(75));
    let disconnect_started = Instant::now();
    assert_eq!(client.disconnect(), 0);
    wait_event(&host, EventKind::Disconnected);
    assert!(
        disconnect_started.elapsed() < Duration::from_millis(250),
        "idle command handling took {:?}",
        disconnect_started.elapsed()
    );

    drop(client);
    host.stop().expect("stop host");
}

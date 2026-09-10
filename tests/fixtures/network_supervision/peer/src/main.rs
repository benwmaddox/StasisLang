use std::io::{self, BufRead, Read};
use std::thread;
use std::time::{Duration, Instant};

use stasis_network::client::{NetworkClient, STATUS_CONNECTED};

const HANDOFF_VERSION: &str = "stasis-network-supervision-v1\n";
const TIMEOUT: Duration = Duration::from_secs(20);

fn main() {
    if run().is_err() {
        std::process::exit(1);
    }
}

fn run() -> Result<(), ()> {
    if std::env::var_os("STASIS_NETWORK_SUPERVISION_HANDLE").is_some() {
        // Harness-only no-readiness authority used to prove the startup bound.
        thread::sleep(Duration::from_secs(60));
        return Err(());
    }
    let mode = match std::env::args().nth(1).as_deref() {
        None => Mode::Exchange,
        Some("--fail-after-invite") if std::env::args().nth(2).is_none() => Mode::Fail,
        Some("--hang-after-invite") if std::env::args().nth(2).is_none() => Mode::Hang,
        Some("--crash-authority") if std::env::args().nth(2).is_none() => Mode::CrashAuthority,
        Some(_) => return Err(()),
    };
    let stdin = io::stdin();
    let input = stdin.lock();
    let mut input = input.take(1024);
    let mut version = String::new();
    if input.read_line(&mut version).map_err(|_| ())? == 0 || version != HANDOFF_VERSION {
        return Err(());
    }
    let mut join_url = String::new();
    let length = input.read_line(&mut join_url).map_err(|_| ())?;
    if length < 2 || length > 513 || !join_url.ends_with('\n') || join_url.contains('\r') {
        return Err(());
    }
    join_url.pop();
    let mut trailing = [0_u8; 1];
    if input.read(&mut trailing).map_err(|_| ())? != 0 {
        return Err(());
    }

    let client = NetworkClient::new(&join_url).map_err(|_| ())?;
    // Drop our only copy of the private URL before the socket exchange.
    join_url.clear();
    match mode {
        Mode::Fail => return Err(()),
        Mode::Hang => {
            thread::sleep(Duration::from_secs(60));
            return Err(());
        }
        Mode::Exchange | Mode::CrashAuthority => {}
    }
    if client.connect() != 0 {
        return Err(());
    }
    let deadline = Instant::now() + TIMEOUT;
    while client.status() != STATUS_CONNECTED {
        if client.status() < 0 || Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(5));
    }
    if mode == Mode::CrashAuthority {
        if client.send(&[1, 1, 0]) != 0 {
            return Err(());
        }
        thread::sleep(Duration::from_secs(60));
        return Err(());
    }

    exchange(&client, &[1, 1, 7], &[1, 2, 7], deadline)?;
    exchange(&client, &[1, 1, 5], &[1, 2, 12], deadline)?;
    if client.disconnect() != 0 {
        return Err(());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Exchange,
    Fail,
    Hang,
    CrashAuthority,
}

fn exchange(
    client: &NetworkClient,
    command: &[u8],
    expected_projection: &[u8],
    deadline: Instant,
) -> Result<(), ()> {
    if client.send(command) != 0 {
        return Err(());
    }
    let mut payload = [0_u8; 64];
    loop {
        let received = client.poll(&mut payload);
        if received > 0 {
            if received as usize != expected_projection.len()
                || &payload[..received as usize] != expected_projection
            {
                return Err(());
            }
            break;
        }
        if received < 0 || Instant::now() >= deadline {
            return Err(());
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

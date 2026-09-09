//! Bounded launcher for a packaged Stasis authority and one external peer.

use std::path::PathBuf;
use std::time::Duration;

pub const PEER_PROTOCOL_LINE: &str = "stasis-network-supervision-v1";
pub const MAX_BOUND: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub authority: PathBuf,
    pub peer: PathBuf,
    pub peer_args: Vec<String>,
    pub startup: Duration,
    pub action: Duration,
    pub shutdown: Duration,
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut authority = None;
    let mut peer = None;
    let mut startup = None;
    let mut action = None;
    let mut shutdown = None;
    let mut args = args.into_iter();
    let mut peer_args = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--" {
            peer_args.extend(args);
            break;
        }
        let value = args.next().ok_or("missing option value")?;
        match arg.as_str() {
            "--authority" => authority = Some(PathBuf::from(value)),
            "--peer" => peer = Some(PathBuf::from(value)),
            "--startup-ms" => startup = Some(parse_bound(&value, &arg)?),
            "--action-ms" => action = Some(parse_bound(&value, &arg)?),
            "--shutdown-ms" => shutdown = Some(parse_bound(&value, &arg)?),
            _ => return Err("unknown option".into()),
        }
    }
    Ok(Options {
        authority: authority.ok_or("missing --authority")?,
        peer: peer.ok_or("missing --peer")?,
        peer_args,
        startup: startup.ok_or("missing --startup-ms")?,
        action: action.ok_or("missing --action-ms")?,
        shutdown: shutdown.ok_or("missing --shutdown-ms")?,
    })
}

fn parse_bound(value: &str, option: &str) -> Result<Duration, String> {
    let milliseconds = value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {option}"))?;
    let bound = Duration::from_millis(milliseconds);
    if milliseconds == 0 || bound > MAX_BOUND {
        return Err(format!("{option} must be between 1 and 900000"));
    }
    Ok(bound)
}

#[cfg(windows)]
pub fn run(options: &Options) -> Result<(), String> {
    windows::run(options)
}

#[cfg(not(windows))]
pub fn run(_options: &Options) -> Result<(), String> {
    Err("production authority supervision is available only on Windows".into())
}

#[cfg(windows)]
mod windows {
    use super::{Options, PEER_PROTOCOL_LINE};
    use crate::{supervision_windows, SUPERVISION_FRAME_MAGIC, SUPERVISION_HANDLE_ENV};
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    pub fn run(options: &Options) -> Result<(), String> {
        let job = supervision_windows::Job::new()
            .map_err(|error| format!("could not create cleanup job: {error}"))?;
        let result = run_session(options, &job);
        let cleanup = shutdown(&job, options.shutdown);
        match (result, cleanup) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Ok(())) => {
                eprintln!("stasis-network-supervise: complete");
                Ok(())
            }
        }
    }

    fn run_session(options: &Options, job: &supervision_windows::Job) -> Result<(), String> {
        let startup_deadline = Instant::now() + options.startup;
        let (mut invite_read, invite_write) = supervision_windows::pipe_to_child()
            .map_err(|error| format!("could not create readiness pipe: {error}"))?;
        let inherited_handle = supervision_windows::raw_handle(&invite_write).to_string();
        let environment = [(
            OsString::from(SUPERVISION_HANDLE_ENV),
            OsString::from(inherited_handle),
        )];
        let authority = job
            .spawn(
                &options.authority,
                &[],
                &environment,
                None,
                &[&invite_write],
            )
            .map_err(|error| format!("could not start authority: {error}"))?;
        drop(invite_write);

        let (frame_tx, frame_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = read_invite_frame(&mut invite_read);
            let _ = frame_tx.send(result);
        });
        let startup_remaining = startup_deadline.saturating_duration_since(Instant::now());
        if startup_remaining.is_zero() {
            return Err("authority readiness timed out".into());
        }
        let join_url = match frame_rx.recv_timeout(startup_remaining) {
            Ok(Ok(url)) => url,
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err("authority readiness timed out".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("authority readiness channel failed".into())
            }
        };
        if Instant::now() > startup_deadline {
            return Err("authority readiness timed out".into());
        }
        if let Some(status) = authority
            .try_wait()
            .map_err(|error| format!("could not inspect authority: {error}"))?
        {
            return Err(format!("authority exited during startup ({status})"));
        }
        eprintln!("stasis-network-supervise: authority-ready");

        let action_deadline = Instant::now() + options.action;
        let (peer_stdin, mut peer_input) = supervision_windows::pipe_from_parent()
            .map_err(|error| format!("could not create peer handoff pipe: {error}"))?;
        let peer = job
            .spawn(
                &options.peer,
                &options.peer_args,
                &[],
                Some(&peer_stdin),
                &[],
            )
            .map_err(|error| format!("could not start peer: {error}"))?;
        drop(peer_stdin);
        eprintln!("stasis-network-supervise: peer-started");
        if peer_input
            .write_all(format!("{PEER_PROTOCOL_LINE}\n{join_url}\n").as_bytes())
            .is_err()
        {
            return Err("peer rejected the private invite handoff".into());
        }
        drop(peer_input);
        drop(join_url);

        let peer_status = wait_for_peer(&authority, &peer, action_deadline)?;
        if let Some(status) = authority
            .try_wait()
            .map_err(|error| format!("could not inspect authority: {error}"))?
        {
            return Err(format!(
                "authority exited before peer completion ({status})"
            ));
        }
        if peer_status == 0 {
            Ok(())
        } else {
            Err(format!("peer failed ({peer_status})"))
        }
    }

    fn read_invite_frame(reader: &mut impl Read) -> Result<String, String> {
        let mut magic = vec![0; SUPERVISION_FRAME_MAGIC.len()];
        reader
            .read_exact(&mut magic)
            .map_err(|_| "authority exited before readiness".to_string())?;
        if magic != SUPERVISION_FRAME_MAGIC {
            return Err("authority sent an invalid readiness frame".into());
        }
        let mut length = [0; 4];
        reader
            .read_exact(&mut length)
            .map_err(|_| "authority truncated its readiness frame".to_string())?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > 512 {
            return Err("authority sent an invalid private invite length".into());
        }
        let mut url = vec![0; length];
        reader
            .read_exact(&mut url)
            .map_err(|_| "authority truncated its private invite".to_string())?;
        let url = String::from_utf8(url)
            .map_err(|_| "authority sent a non-UTF-8 private invite".to_string())?;
        if url
            .bytes()
            .any(|byte| byte == 0 || byte == b'\r' || byte == b'\n')
        {
            return Err("authority sent an invalid private invite".into());
        }
        Ok(url)
    }

    fn wait_for_peer(
        authority: &supervision_windows::Process,
        peer: &supervision_windows::Process,
        deadline: Instant,
    ) -> Result<u32, String> {
        loop {
            if Instant::now() >= deadline {
                return Err("peer action timed out".into());
            }
            if let Some(status) = peer
                .try_wait()
                .map_err(|error| format!("could not inspect peer: {error}"))?
            {
                return Ok(status);
            }
            if let Some(status) = authority
                .try_wait()
                .map_err(|error| format!("could not inspect authority: {error}"))?
            {
                return Err(format!("authority crashed during peer action ({status})"));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn shutdown(job: &supervision_windows::Job, bound: Duration) -> Result<(), String> {
        let deadline = Instant::now() + bound;
        if job
            .active_processes()
            .map_err(|_| "could not inspect supervised job".to_string())?
            == 0
        {
            return Ok(());
        }
        job.terminate()
            .map_err(|_| "could not terminate supervised job".to_string())?;
        while Instant::now() < deadline {
            if job
                .active_processes()
                .map_err(|_| "could not inspect supervised job".to_string())?
                == 0
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err("supervised job exceeded the shutdown bound".into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;

        #[test]
        fn readiness_frame_rejects_malformed_and_line_injected_invites() {
            assert!(read_invite_frame(&mut Cursor::new(b"wrong".to_vec())).is_err());

            let url = b"http://127.0.0.1:9/#secret=value\nsecond-line";
            let mut frame = SUPERVISION_FRAME_MAGIC.to_vec();
            frame.extend_from_slice(&(url.len() as u32).to_be_bytes());
            frame.extend_from_slice(url);
            assert!(read_invite_frame(&mut Cursor::new(frame)).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_contract_and_peer_args() {
        let options = parse_args(
            [
                "--authority",
                "authority.exe",
                "--peer",
                "peer.exe",
                "--startup-ms",
                "1000",
                "--action-ms",
                "2000",
                "--shutdown-ms",
                "3000",
                "--",
                "one",
                "two",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert_eq!(options.peer_args, ["one", "two"]);
        assert_eq!(options.action, Duration::from_secs(2));
    }

    #[test]
    fn rejects_unbounded_or_incomplete_contract() {
        assert!(parse_args(["--startup-ms", "900001"].into_iter().map(str::to_string)).is_err());
        assert!(parse_args(std::iter::empty()).is_err());
        assert_eq!(
            parse_args(["private-invite"].into_iter().map(str::to_string)),
            Err("missing option value".into())
        );
    }
}

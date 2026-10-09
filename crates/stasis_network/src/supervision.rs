//! Bounded launcher for a packaged Stasis authority and one external peer.

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(any(windows, test))]
use std::path::Path;

pub const PEER_PROTOCOL_LINE: &str = "stasis-network-supervision-v1";
pub const MAX_BOUND: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authority {
    Packaged {
        exe: PathBuf,
    },
    Live {
        toolchain: PathBuf,
        workspace: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub authority: Authority,
    pub evidence_root: Option<PathBuf>,
    pub peer: PathBuf,
    pub peer_args: Vec<String>,
    pub startup: Duration,
    pub action: Duration,
    pub shutdown: Duration,
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut authority = None;
    let mut toolchain = None;
    let mut workspace = None;
    let mut evidence_root = None;
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
            "--toolchain" => toolchain = Some(PathBuf::from(value)),
            "--workspace" => workspace = Some(PathBuf::from(value)),
            "--evidence-root" => evidence_root = Some(PathBuf::from(value)),
            "--peer" => peer = Some(PathBuf::from(value)),
            "--startup-ms" => startup = Some(parse_bound(&value, &arg)?),
            "--action-ms" => action = Some(parse_bound(&value, &arg)?),
            "--shutdown-ms" => shutdown = Some(parse_bound(&value, &arg)?),
            _ => return Err("unknown option".into()),
        }
    }
    let authority = match (authority, toolchain, workspace) {
        (Some(exe), None, None) if evidence_root.is_none() => Authority::Packaged { exe },
        (None, Some(toolchain), Some(workspace)) if evidence_root.is_some() => Authority::Live {
            toolchain,
            workspace,
        },
        (None, None, None) => return Err("missing authority mode".into()),
        _ => return Err("authority options are incomplete or mutually exclusive".into()),
    };
    Ok(Options {
        authority,
        evidence_root,
        peer: peer.ok_or("missing --peer")?,
        peer_args,
        startup: startup.ok_or("missing --startup-ms")?,
        action: action.ok_or("missing --action-ms")?,
        shutdown: shutdown.ok_or("missing --shutdown-ms")?,
    })
}

pub struct ChildPipes {
    control: File,
    response: File,
    capture_root: PathBuf,
}

impl ChildPipes {
    pub fn into_parts(self) -> (File, File, PathBuf) {
        (self.control, self.response, self.capture_root)
    }
}

pub fn take_child_pipes() -> Result<ChildPipes, &'static str> {
    #[cfg(windows)]
    {
        use crate::{
            supervision_windows, SUPERVISION_CAPTURE_ROOT_ENV, SUPERVISION_CONTROL_HANDLE_ENV,
            SUPERVISION_HANDLE_ENV, SUPERVISION_RESPONSE_HANDLE_ENV,
        };
        supervision_windows::validate_inherited_pipes(&[
            SUPERVISION_HANDLE_ENV,
            SUPERVISION_CONTROL_HANDLE_ENV,
            SUPERVISION_RESPONSE_HANDLE_ENV,
        ])
        .map_err(|_| "supervised live pipes are invalid")?;
        let control = supervision_windows::take_inherited_pipe(SUPERVISION_CONTROL_HANDLE_ENV)
            .map_err(|_| "supervised live control pipe is unavailable")?;
        let response = supervision_windows::take_inherited_pipe(SUPERVISION_RESPONSE_HANDLE_ENV)
            .map_err(|_| "supervised live response pipe is unavailable")?;
        let capture_root = std::env::var_os(SUPERVISION_CAPTURE_ROOT_ENV)
            .map(PathBuf::from)
            .ok_or("supervised live capture root is unavailable")?;
        std::env::remove_var(SUPERVISION_CAPTURE_ROOT_ENV);
        validate_capture_root(&capture_root)?;
        Ok(ChildPipes {
            control,
            response,
            capture_root,
        })
    }
    #[cfg(not(windows))]
    {
        Err("supervised live pipes are available only on Windows")
    }
}

#[cfg(any(windows, test))]
fn validate_capture_root(root: &Path) -> Result<(), &'static str> {
    if !root.is_absolute() {
        return Err("supervised live capture root is invalid");
    }
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|_| "supervised live capture root is unavailable")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("supervised live capture root is invalid");
    }
    #[cfg(windows)]
    if crate::supervision_windows::is_reparse_point(root).unwrap_or(true) {
        return Err("supervised live capture root is invalid");
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|_| "supervised live capture root is invalid")?;
    let temp_root = std::env::temp_dir()
        .canonicalize()
        .map_err(|_| "supervised live capture root is invalid")?;
    if canonical_root == temp_root || !canonical_root.starts_with(temp_root) {
        return Err("supervised live capture root is invalid");
    }
    Ok(())
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
    use super::{Authority, Options, PEER_PROTOCOL_LINE};
    use crate::{supervision_windows, SUPERVISION_FRAME_MAGIC, SUPERVISION_HANDLE_ENV};
    use std::ffi::OsString;
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    pub fn run(options: &Options) -> Result<(), String> {
        let job = supervision_windows::Job::new()
            .map_err(|error| format!("could not create cleanup job: {error}"))?;
        let live_shutdown_deadline = std::cell::Cell::new(None);
        let result = match &options.authority {
            Authority::Packaged { .. } => run_packaged_session(options, &job),
            #[cfg(feature = "supervision-cli")]
            Authority::Live { .. } => live::run_session(options, &job, &live_shutdown_deadline),
            #[cfg(not(feature = "supervision-cli"))]
            Authority::Live { .. } => Err("live supervision CLI is disabled".into()),
        };
        let cleanup_deadline = live_shutdown_deadline
            .get()
            .unwrap_or_else(|| Instant::now() + options.shutdown);
        let cleanup = shutdown_until(&job, cleanup_deadline);
        match (result, cleanup) {
            (_, Err(error)) => Err(error),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Ok(())) => {
                eprintln!("stasis-network-supervise: complete");
                Ok(())
            }
        }
    }

    fn run_packaged_session(
        options: &Options,
        job: &supervision_windows::Job,
    ) -> Result<(), String> {
        let startup_deadline = Instant::now() + options.startup;
        let (mut invite_read, invite_write) = supervision_windows::pipe_to_child()
            .map_err(|error| format!("could not create readiness pipe: {error}"))?;
        let inherited_handle = supervision_windows::raw_handle(&invite_write).to_string();
        let environment = [(
            OsString::from(SUPERVISION_HANDLE_ENV),
            OsString::from(inherited_handle),
        )];
        let Authority::Packaged { exe } = &options.authority else {
            return Err("packaged authority mode is invalid".into());
        };
        let authority = job
            .spawn(exe, &[], &environment, None, None, &[&invite_write])
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
                None,
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

    fn shutdown_until(job: &supervision_windows::Job, deadline: Instant) -> Result<(), String> {
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
            thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
        if job
            .active_processes()
            .map_err(|_| "could not inspect supervised job".to_string())?
            == 0
        {
            return Ok(());
        }
        Err("supervised job exceeded the shutdown bound".into())
    }

    #[cfg(feature = "supervision-cli")]
    mod live {
        use super::{read_invite_frame, Options, PEER_PROTOCOL_LINE};
        use crate::{
            supervision_windows, SUPERVISION_CAPTURE_ROOT_ENV, SUPERVISION_CONTROL_HANDLE_ENV,
            SUPERVISION_HANDLE_ENV, SUPERVISION_RESPONSE_HANDLE_ENV,
        };
        use sha2::{Digest, Sha256};
        use stasis_runner::supervised_live::{
            artifact_name, response_for_request, SupervisedLiveErrorCode, SupervisedLiveEvent,
            SupervisedLiveRequest, SupervisedLiveRequestBudget, SupervisedLiveResponse,
            MAX_SUPERVISED_LIVE_CAPTURE_BYTES, MAX_SUPERVISED_LIVE_LINE_BYTES,
            MAX_TOTAL_SUPERVISED_LIVE_CAPTURE_BYTES,
        };
        use std::collections::BTreeSet;
        use std::ffi::OsString;
        use std::fs::{self, File, OpenOptions};
        use std::io::{BufRead, BufReader, Read, Write};
        use std::path::Path;
        use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
        use std::thread;
        use std::time::{Duration, Instant};

        const MAX_PEER_RECEIPT_BYTES: usize = 256 * 1024;
        const MAX_PEER_RECEIPT_LINES: usize = 256;

        fn validate_evidence_root(root: &Path) -> Result<(), String> {
            if !root.is_absolute() {
                return Err("live evidence root is invalid".into());
            }
            let metadata = fs::symlink_metadata(root)
                .map_err(|_| "live evidence root is unavailable".to_string())?;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || supervision_windows::is_reparse_point(root).unwrap_or(true)
            {
                return Err("live evidence root is invalid".into());
            }
            let canonical = root
                .canonicalize()
                .map_err(|_| "live evidence root is invalid".to_string())?;
            let temp = std::env::temp_dir()
                .canonicalize()
                .map_err(|_| "live evidence root is invalid".to_string())?;
            if canonical == temp || !canonical.starts_with(&temp) {
                return Err("live evidence root is invalid".into());
            }
            let mut entries = fs::read_dir(&canonical)
                .map_err(|_| "live evidence root is unavailable".to_string())?;
            if entries
                .next()
                .transpose()
                .map_err(|_| "live evidence root is unavailable".to_string())?
                .is_some()
            {
                return Err("live evidence root must be empty".into());
            }
            Ok(())
        }

        pub(super) fn run_session(
            options: &Options,
            job: &supervision_windows::Job,
            shutdown_deadline: &std::cell::Cell<Option<Instant>>,
        ) -> Result<(), String> {
            let capture_root = options
                .evidence_root
                .as_deref()
                .ok_or_else(|| "live evidence root is required".to_string())?;
            validate_evidence_root(capture_root)?;
            run_session_inner(options, job, capture_root, shutdown_deadline)
        }

        fn run_session_inner(
            options: &Options,
            job: &supervision_windows::Job,
            capture_root: &Path,
            shutdown_deadline: &std::cell::Cell<Option<Instant>>,
        ) -> Result<(), String> {
            let super::super::Authority::Live {
                toolchain,
                workspace,
            } = &options.authority
            else {
                return Err("live authority mode is invalid".into());
            };
            let workspace = workspace
                .to_str()
                .ok_or_else(|| "live workspace path is invalid".to_string())?;
            let (mut readiness_reader, readiness_writer) = supervision_windows::pipe_to_child()
                .map_err(|_| "live readiness pipe is unavailable".to_string())?;
            let (control_reader, control_writer) = supervision_windows::pipe_from_parent()
                .map_err(|_| "live control pipe is unavailable".to_string())?;
            let (response_reader, response_writer) = supervision_windows::pipe_to_child()
                .map_err(|_| "live response pipe is unavailable".to_string())?;
            let environment = [
                (
                    OsString::from(SUPERVISION_HANDLE_ENV),
                    OsString::from(supervision_windows::raw_handle(&readiness_writer).to_string()),
                ),
                (
                    OsString::from(SUPERVISION_CONTROL_HANDLE_ENV),
                    OsString::from(supervision_windows::raw_handle(&control_reader).to_string()),
                ),
                (
                    OsString::from(SUPERVISION_RESPONSE_HANDLE_ENV),
                    OsString::from(supervision_windows::raw_handle(&response_writer).to_string()),
                ),
                (
                    OsString::from(SUPERVISION_CAPTURE_ROOT_ENV),
                    capture_root.as_os_str().to_os_string(),
                ),
            ];
            let args = vec![
                "--workspace".to_string(),
                workspace.to_string(),
                "live".to_string(),
                "--live-stdio".to_string(),
                "--supervised-live".to_string(),
                "--evidence-root".to_string(),
                capture_root.to_string_lossy().into_owned(),
            ];
            let authority = job
                .spawn(
                    toolchain,
                    &args,
                    &environment,
                    None,
                    None,
                    &[&readiness_writer, &control_reader, &response_writer],
                )
                .map_err(|_| "live authority could not be started".to_string())?;
            drop((readiness_writer, control_reader, response_writer));

            let (readiness_tx, readiness_rx) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let result = read_invite_frame(&mut readiness_reader);
                let _ = readiness_tx.send(result);
            });
            let startup_deadline = Instant::now() + options.startup;
            let join_url = loop {
                if Instant::now() >= startup_deadline {
                    return Err("live authority readiness timed out".into());
                }
                if authority
                    .try_wait()
                    .map_err(|_| "live authority status is unavailable".to_string())?
                    .is_some()
                {
                    return Err("live authority exited before readiness".into());
                }
                match readiness_rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(Ok(url)) => break url,
                    Ok(Err(_)) => return Err("live authority readiness was invalid".into()),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("live authority readiness channel failed".into());
                    }
                }
            };
            if authority
                .try_wait()
                .map_err(|_| "live authority status is unavailable".to_string())?
                .is_some()
            {
                return Err("live authority exited during startup".into());
            }

            let (peer_stdin, peer_input) = supervision_windows::pipe_from_parent()
                .map_err(|_| "live peer handoff pipe is unavailable".to_string())?;
            let (peer_receipts_read, peer_stdout_writer) = supervision_windows::pipe_to_child()
                .map_err(|_| "live peer receipt pipe is unavailable".to_string())?;
            let peer = job
                .spawn(
                    &options.peer,
                    &options.peer_args,
                    &[],
                    Some(&peer_stdin),
                    Some(&peer_stdout_writer),
                    &[&peer_stdout_writer],
                )
                .map_err(|_| "live peer could not be started".to_string())?;
            drop((peer_stdin, peer_stdout_writer));
            let receipts_path = capture_root.join("peer-receipts.jsonl");
            let (receipts_tx, receipts_rx) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let result = collect_peer_receipts(peer_receipts_read, &receipts_path);
                let _ = receipts_tx.send(result);
            });
            let peer_input = BoundedWriter::new(peer_input);
            let mut invite_bytes = join_url.into_bytes();
            let mut handoff =
                Vec::with_capacity(PEER_PROTOCOL_LINE.len() + 1 + invite_bytes.len() + 1);
            handoff.extend_from_slice(PEER_PROTOCOL_LINE.as_bytes());
            handoff.push(b'\n');
            handoff.extend_from_slice(&invite_bytes);
            handoff.push(b'\n');
            invite_bytes.fill(0);
            let handoff_result = peer_input.write(&handoff, Instant::now() + options.action);
            handoff.fill(0);
            handoff_result?;
            drop(peer_input);

            let (input_tx, input_rx) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let stdin = std::io::stdin();
                let mut reader = BufReader::with_capacity(1024, stdin.lock());
                loop {
                    let message = read_bounded_line(&mut reader, MAX_SUPERVISED_LIVE_LINE_BYTES);
                    let done = !matches!(&message, Ok(Some(_)));
                    if input_tx.send(message).is_err() || done {
                        break;
                    }
                }
            });
            let (response_tx, response_rx) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let mut reader = BufReader::with_capacity(1024, response_reader);
                loop {
                    let message = read_bounded_line(&mut reader, MAX_SUPERVISED_LIVE_LINE_BYTES);
                    let done = !matches!(&message, Ok(Some(_)));
                    if response_tx.send(message).is_err() || done {
                        break;
                    }
                }
            });

            let control_writer = BoundedWriter::new(control_writer);
            let harness_output = BoundedWriter::new(
                supervision_windows::duplicate_stdout_file()
                    .map_err(|_| "live response output is unavailable".to_string())?,
            );
            let mut budget = SupervisedLiveRequestBudget::new(Instant::now());
            let mut request_ids = BTreeSet::new();
            let mut peer_status = None;
            let mut peer_receipts = None;
            let mut capture_bytes = 0_u64;
            let mut idle_deadline = Instant::now() + options.action;
            loop {
                check_processes(&authority, &peer, &mut peer_status, true)?;
                poll_peer_receipts(&receipts_rx, &mut peer_receipts)?;
                let now = Instant::now();
                if now >= idle_deadline {
                    return Err("live control session timed out".into());
                }
                match input_rx.recv_timeout(
                    idle_deadline
                        .saturating_duration_since(now)
                        .min(Duration::from_millis(10)),
                ) {
                    Ok(Ok(Some(line))) => {
                        let budget_result = budget.record(Instant::now());
                        let request = SupervisedLiveRequest::parse_line(&line)
                            .map_err(|_| "live control request was invalid".to_string())?;
                        if !request_ids.insert(request.request_id) {
                            let rejected = response_for_request(
                                &request,
                                0,
                                SupervisedLiveEvent::Rejected,
                                None,
                                Some(SupervisedLiveErrorCode::InvalidRequest),
                            )
                            .map_err(|_| "live control request was invalid".to_string())?;
                            write_response(
                                &harness_output,
                                &rejected,
                                Instant::now() + options.action,
                            )?;
                            return Err("live control request was duplicated".into());
                        }
                        if let Err(error) = budget_result {
                            let rejected = response_for_request(
                                &request,
                                0,
                                SupervisedLiveEvent::Rejected,
                                None,
                                Some(error),
                            )
                            .map_err(|_| "live control request was invalid".to_string())?;
                            write_response(
                                &harness_output,
                                &rejected,
                                Instant::now() + options.action,
                            )?;
                            return Err("live control request limit was reached".into());
                        }
                        if let Err(error) = budget.capture_attempt(&request) {
                            let rejected = response_for_request(
                                &request,
                                0,
                                SupervisedLiveEvent::Rejected,
                                None,
                                Some(error),
                            )
                            .map_err(|_| "live control request was invalid".to_string())?;
                            write_response(
                                &harness_output,
                                &rejected,
                                Instant::now() + options.action,
                            )?;
                            return Err("live capture request limit was reached".into());
                        }
                        let action_deadline = Instant::now() + options.action;
                        send_control(&control_writer, &request, action_deadline)?;
                        let is_quit = matches!(
                            request.command,
                            stasis_runner::supervised_live::SupervisedLiveCommand::Quit
                        );
                        let response = receive_response(
                            &response_rx,
                            &request,
                            &authority,
                            &peer,
                            &mut peer_status,
                            action_deadline,
                            is_quit,
                        )?;
                        let current_capture_bytes =
                            verify_capture(capture_root, &request, &response)?;
                        remaining_until(action_deadline)?;
                        capture_bytes = capture_bytes
                            .checked_add(current_capture_bytes)
                            .ok_or_else(|| {
                                "live capture evidence exceeded its bound".to_string()
                            })?;
                        if capture_bytes > MAX_TOTAL_SUPERVISED_LIVE_CAPTURE_BYTES {
                            return Err("live capture evidence exceeded its bound".into());
                        }
                        write_response(&harness_output, &response, action_deadline)?;
                        idle_deadline = Instant::now() + options.action;
                        if is_quit {
                            if !response.ok {
                                return Err("live authority rejected quit".into());
                            }
                            let deadline = Instant::now() + options.shutdown;
                            shutdown_deadline.set(Some(deadline));
                            control_writer.finish(deadline)?;
                            harness_output.finish(deadline)?;
                            wait_for_authority(&authority, deadline)?;
                            wait_for_peer(&peer, &mut peer_status, deadline)?;
                            poll_peer_receipts(&receipts_rx, &mut peer_receipts)?;
                            let receipts =
                                wait_for_peer_receipts(&receipts_rx, peer_receipts, deadline)?;
                            if !receipts.complete {
                                return Err("live peer receipt stream did not complete".into());
                            }
                            return Ok(());
                        }
                    }
                    Ok(Ok(None)) => {
                        let deadline = Instant::now() + options.shutdown;
                        shutdown_deadline.set(Some(deadline));
                        let quit = next_internal_quit(&request_ids);
                        let _ = send_control(&control_writer, &quit, deadline);
                        let _ = receive_response(
                            &response_rx,
                            &quit,
                            &authority,
                            &peer,
                            &mut peer_status,
                            deadline,
                            true,
                        );
                        control_writer.finish(deadline)?;
                        harness_output.finish(deadline)?;
                        wait_for_authority(&authority, deadline)?;
                        wait_for_peer(&peer, &mut peer_status, deadline)?;
                        poll_peer_receipts(&receipts_rx, &mut peer_receipts)?;
                        let receipts =
                            wait_for_peer_receipts(&receipts_rx, peer_receipts, deadline)?;
                        if receipts.complete {
                            return Ok(());
                        }
                        return Err("live harness input ended before peer completion".into());
                    }
                    Ok(Err(_)) => return Err("live harness input line exceeded its bound".into()),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("live harness input channel failed".into());
                    }
                }
            }
        }

        fn read_bounded_line<R: BufRead>(
            reader: &mut R,
            max_bytes: usize,
        ) -> Result<Option<Vec<u8>>, &'static str> {
            let mut line = Vec::with_capacity(128);
            loop {
                let available = reader.fill_buf().map_err(|_| "pipe read failed")?;
                if available.is_empty() {
                    return if line.is_empty() {
                        Ok(None)
                    } else {
                        Err("unterminated pipe line")
                    };
                }
                let newline = available.iter().position(|byte| *byte == b'\n');
                let consumed = newline.map_or(available.len(), |index| index + 1);
                let content = newline.map_or(consumed, |index| index);
                if line.len().saturating_add(content) > max_bytes {
                    return Err("pipe line exceeded its bound");
                }
                line.extend_from_slice(&available[..content]);
                reader.consume(consumed);
                if newline.is_some() {
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    return Ok(Some(line));
                }
            }
        }

        struct BoundedWriter {
            sender: std::sync::Mutex<Option<SyncSender<(Vec<u8>, SyncSender<bool>)>>>,
            worker: std::sync::Mutex<Option<thread::JoinHandle<()>>>,
        }

        impl BoundedWriter {
            fn new<W: Write + Send + 'static>(mut writer: W) -> Self {
                let (sender, receiver) = mpsc::sync_channel::<(Vec<u8>, SyncSender<bool>)>(1);
                let worker = thread::spawn(move || {
                    while let Ok((bytes, completion)) = receiver.recv() {
                        let written = writer
                            .write_all(&bytes)
                            .and_then(|()| writer.flush())
                            .is_ok();
                        let _ = completion.send(written);
                        if !written {
                            break;
                        }
                    }
                });
                Self {
                    sender: std::sync::Mutex::new(Some(sender)),
                    worker: std::sync::Mutex::new(Some(worker)),
                }
            }

            fn write(&self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
                let (completion, written) = mpsc::sync_channel(1);
                let send_result = self
                    .sender
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .ok_or_else(|| "supervised pipe writer is closed".to_string())?
                    .try_send((bytes.to_vec(), completion));
                if send_result.is_err() {
                    return if self.cancel_and_join() {
                        Err("supervised pipe write queue is unavailable".into())
                    } else {
                        Err("supervised pipe write queue failed and writer shutdown failed".into())
                    };
                }
                match written.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        if self.cancel_and_join() {
                            Err("supervised pipe write failed".into())
                        } else {
                            Err("supervised pipe write failed and writer shutdown timed out".into())
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if self.cancel_and_join() {
                            Err("supervised pipe write timed out".into())
                        } else {
                            Err("supervised pipe write timed out and writer shutdown failed".into())
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        if self.cancel_and_join() {
                            Err("supervised pipe write failed".into())
                        } else {
                            Err("supervised pipe writer shutdown failed".into())
                        }
                    }
                }
            }

            fn cancel_and_join(&self) -> bool {
                self.cancel_and_join_until(Instant::now() + Duration::from_millis(250))
            }

            fn cancel_and_join_until(&self, deadline: Instant) -> bool {
                self.sender
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let mut worker_slot = self
                    .worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(worker) = worker_slot.as_mut() else {
                    return true;
                };
                while !worker.is_finished() && Instant::now() < deadline {
                    let _ = supervision_windows::cancel_synchronous_io(worker);
                    thread::sleep(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(5)),
                    );
                }
                if worker.is_finished() {
                    if let Some(worker) = worker_slot.take() {
                        let _ = worker.join();
                    }
                    true
                } else {
                    false
                }
            }

            fn finish(&self, deadline: Instant) -> Result<(), String> {
                if self.cancel_and_join_until(deadline) {
                    Ok(())
                } else {
                    Err("supervised pipe writer shutdown failed".into())
                }
            }
        }

        impl Drop for BoundedWriter {
            fn drop(&mut self) {
                self.sender
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                let worker_slot = self
                    .worker
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(worker) = worker_slot.as_mut() {
                    if !worker.is_finished() {
                        let _ = supervision_windows::cancel_synchronous_io(worker);
                    }
                    if worker.is_finished() {
                        if let Some(worker) = worker_slot.take() {
                            let _ = worker.join();
                        }
                    }
                }
            }
        }

        #[derive(Clone, Copy)]
        struct PeerReceiptSummary {
            complete: bool,
        }

        fn collect_peer_receipts<R: Read>(
            reader: R,
            path: &Path,
        ) -> Result<PeerReceiptSummary, String> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .map_err(|_| "live peer receipt file could not be created".to_string())?;
            let mut reader = BufReader::with_capacity(1024, reader);
            let mut total_bytes = 0_usize;
            let mut total_lines = 0_usize;
            let mut complete = false;
            loop {
                let line = read_bounded_line(&mut reader, MAX_SUPERVISED_LIVE_LINE_BYTES)
                    .map_err(|_| "live peer receipt was invalid".to_string())?;
                let Some(line) = line else {
                    break;
                };
                if complete {
                    return Err("live peer receipt followed completion".into());
                }
                total_bytes = total_bytes
                    .checked_add(line.len() + 1)
                    .ok_or_else(|| "live peer receipts exceeded their bound".to_string())?;
                total_lines += 1;
                if total_bytes > MAX_PEER_RECEIPT_BYTES || total_lines > MAX_PEER_RECEIPT_LINES {
                    return Err("live peer receipts exceeded their bound".into());
                }
                let (receipt, is_complete) = parse_peer_receipt(&line)?;
                let mut canonical = serde_json::to_vec(&receipt)
                    .map_err(|_| "live peer receipt was invalid".to_string())?;
                canonical.push(b'\n');
                file.write_all(&canonical)
                    .and_then(|()| file.flush())
                    .map_err(|_| "live peer receipt could not be retained".to_string())?;
                complete = is_complete;
            }
            if !complete {
                return Err("live peer receipt stream ended before completion".into());
            }
            Ok(PeerReceiptSummary { complete })
        }

        fn parse_peer_receipt(line: &[u8]) -> Result<(serde_json::Value, bool), String> {
            let value: serde_json::Value = serde_json::from_slice(line)
                .map_err(|_| "live peer receipt was invalid".to_string())?;
            let object = value
                .as_object()
                .ok_or_else(|| "live peer receipt was invalid".to_string())?;
            let event = object
                .get("event")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "live peer receipt was invalid".to_string())?;
            let fields: &[&str] = match event {
                "sent_action" => &["sequence", "command", "arg"],
                "join_auth" => &["seat", "ack"],
                "ack" | "duplicate_rejected" => &["sequence", "ack", "result", "revision", "hash"],
                "malformed_rejected" => &["code", "revision", "hash"],
                "snapshot" => &[
                    "game_id",
                    "phase",
                    "terminal_phase",
                    "screen",
                    "revision",
                    "hash",
                    "sequence",
                    "current",
                    "winner",
                    "moves",
                ],
                "sent_malformed" => &["sequence", "kind"],
                "complete" => &["revision", "hash", "sequence"],
                _ => return Err("live peer receipt event was invalid".into()),
            };
            if object.len() != fields.len() + 1 {
                return Err("live peer receipt fields were invalid".into());
            }
            let mut projected = serde_json::Map::new();
            for field in fields {
                let number = object
                    .get(*field)
                    .and_then(serde_json::Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| "live peer receipt fields were invalid".to_string())?;
                projected.insert((*field).to_string(), serde_json::json!(number));
            }
            let complete = event == "complete";
            Ok((
                serde_json::json!({
                    "event": event,
                    "fields": projected,
                }),
                complete,
            ))
        }

        fn poll_peer_receipts(
            receiver: &Receiver<Result<PeerReceiptSummary, String>>,
            summary: &mut Option<PeerReceiptSummary>,
        ) -> Result<(), String> {
            if summary.is_some() {
                return Ok(());
            }
            match receiver.try_recv() {
                Ok(Ok(receipts)) => {
                    *summary = Some(receipts);
                    Ok(())
                }
                Ok(Err(_)) => Err("live peer receipt stream was invalid".into()),
                Err(TryRecvError::Empty) => Ok(()),
                Err(TryRecvError::Disconnected) => Err("live peer receipt stream failed".into()),
            }
        }

        fn wait_for_peer_receipts(
            receiver: &Receiver<Result<PeerReceiptSummary, String>>,
            summary: Option<PeerReceiptSummary>,
            deadline: Instant,
        ) -> Result<PeerReceiptSummary, String> {
            if let Some(summary) = summary {
                return Ok(summary);
            }
            match receiver.recv_timeout(remaining_until(deadline)?) {
                Ok(Ok(summary)) => Ok(summary),
                Ok(Err(_)) => Err("live peer receipt stream was invalid".into()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    Err("live peer receipt stream timed out".into())
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err("live peer receipt stream failed".into())
                }
            }
        }

        fn wait_for_peer(
            peer: &supervision_windows::Process,
            peer_status: &mut Option<u32>,
            deadline: Instant,
        ) -> Result<(), String> {
            loop {
                if let Some(status) = peer
                    .try_wait()
                    .map_err(|_| "live peer status is unavailable".to_string())?
                {
                    *peer_status = Some(status);
                }
                if let Some(status) = *peer_status {
                    return if status == 0 {
                        Ok(())
                    } else {
                        Err("live peer failed".into())
                    };
                }
                if Instant::now() >= deadline {
                    return Err("live peer shutdown timed out".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn send_control(
            writer: &BoundedWriter,
            request: &SupervisedLiveRequest,
            deadline: Instant,
        ) -> Result<(), String> {
            let mut line = serde_json::to_vec(request)
                .map_err(|_| "live control request could not be serialized".to_string())?;
            if line.len() > MAX_SUPERVISED_LIVE_LINE_BYTES {
                return Err("live control request exceeded its bound".into());
            }
            line.push(b'\n');
            writer.write(&line, deadline)
        }

        fn receive_response(
            reader: &Receiver<Result<Option<Vec<u8>>, &'static str>>,
            request: &SupervisedLiveRequest,
            authority: &supervision_windows::Process,
            peer: &supervision_windows::Process,
            peer_status: &mut Option<u32>,
            deadline: Instant,
            allow_authority_exit: bool,
        ) -> Result<SupervisedLiveResponse, String> {
            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Err("live authority response timed out".into());
                }
                match reader.recv_timeout(
                    deadline
                        .saturating_duration_since(now)
                        .min(Duration::from_millis(10)),
                ) {
                    Ok(Ok(Some(line))) => {
                        let response = SupervisedLiveResponse::parse_line(&line)
                            .map_err(|_| "live authority response was invalid".to_string())?;
                        response.validate_for_request(request).map_err(|_| {
                            "live authority response did not match its request".to_string()
                        })?;
                        return Ok(response);
                    }
                    Ok(Ok(None)) => return Err("live authority response pipe closed".into()),
                    Ok(Err(_)) => return Err("live authority response exceeded its bound".into()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        check_processes(authority, peer, peer_status, !allow_authority_exit)?;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("live authority response channel failed".into());
                    }
                }
            }
        }

        fn write_response(
            writer: &BoundedWriter,
            response: &SupervisedLiveResponse,
            deadline: Instant,
        ) -> Result<(), String> {
            let mut line = serde_json::to_vec(response)
                .map_err(|_| "live response could not be serialized".to_string())?;
            if line.len() > MAX_SUPERVISED_LIVE_LINE_BYTES {
                return Err("live response exceeded its bound".into());
            }
            line.push(b'\n');
            writer.write(&line, deadline)
        }

        fn check_processes(
            authority: &supervision_windows::Process,
            peer: &supervision_windows::Process,
            peer_status: &mut Option<u32>,
            check_authority: bool,
        ) -> Result<(), String> {
            if check_authority {
                if authority
                    .try_wait()
                    .map_err(|_| "live authority status is unavailable".to_string())?
                    .is_some()
                {
                    return Err("live authority exited during control session".into());
                }
            }
            if peer_status.is_none() {
                if let Some(status) = peer
                    .try_wait()
                    .map_err(|_| "live peer status is unavailable".to_string())?
                {
                    if status != 0 {
                        return Err("live peer failed".into());
                    }
                    *peer_status = Some(status);
                }
            }
            Ok(())
        }

        fn wait_for_authority(
            authority: &supervision_windows::Process,
            deadline: Instant,
        ) -> Result<(), String> {
            loop {
                if let Some(status) = authority
                    .try_wait()
                    .map_err(|_| "live authority status is unavailable".to_string())?
                {
                    return if status == 0 {
                        Ok(())
                    } else {
                        Err("live authority exited with an error".into())
                    };
                }
                if Instant::now() >= deadline {
                    return Err("live authority shutdown timed out".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        fn remaining_until(deadline: Instant) -> Result<Duration, String> {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                Err("supervised session deadline expired".into())
            } else {
                Ok(remaining)
            }
        }

        fn next_internal_quit(request_ids: &BTreeSet<u32>) -> SupervisedLiveRequest {
            let mut request_id = u32::MAX;
            while request_ids.contains(&request_id) {
                request_id -= 1;
            }
            SupervisedLiveRequest {
                schema_version: stasis_runner::supervised_live::SUPERVISED_LIVE_SCHEMA_VERSION,
                request_id,
                command: stasis_runner::supervised_live::SupervisedLiveCommand::Quit,
            }
        }

        fn verify_capture(
            root: &Path,
            request: &SupervisedLiveRequest,
            response: &SupervisedLiveResponse,
        ) -> Result<u64, String> {
            if !response.ok || response.event != SupervisedLiveEvent::CaptureSaved {
                return Ok(0);
            }
            let (expected_id, capture) = match (&request.command, &response.capture) {
                (
                    stasis_runner::supervised_live::SupervisedLiveCommand::CaptureFrame {
                        artifact_id,
                    },
                    Some(capture),
                ) if capture.artifact_id == *artifact_id => (*artifact_id, capture),
                _ => return Err("live capture response was invalid".into()),
            };
            let name = artifact_name(expected_id)
                .ok_or_else(|| "live capture artifact ID was invalid".to_string())?;
            let root = root
                .canonicalize()
                .map_err(|_| "live capture root is unavailable".to_string())?;
            let path = root.join(format!("{name}.png"));
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| "live capture artifact is unavailable".to_string())?;
            if metadata.file_type().is_symlink()
                || supervision_windows::is_reparse_point(&path).unwrap_or(true)
                || !metadata.is_file()
                || metadata.len() == 0
                || metadata.len() > u64::from(MAX_SUPERVISED_LIVE_CAPTURE_BYTES)
                || metadata.len() != u64::from(capture.byte_length)
            {
                return Err("live capture artifact metadata was invalid".into());
            }
            let canonical = path
                .canonicalize()
                .map_err(|_| "live capture artifact is invalid".to_string())?;
            if canonical.parent() != Some(root.as_path())
                || canonical.file_name() != Some(std::ffi::OsStr::new(&format!("{name}.png")))
            {
                return Err("live capture artifact path was invalid".into());
            }
            let file = File::open(&canonical)
                .map_err(|_| "live capture artifact is unavailable".to_string())?;
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            file.take(u64::from(MAX_SUPERVISED_LIVE_CAPTURE_BYTES) + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "live capture artifact could not be read".to_string())?;
            if bytes.len() as u64 != metadata.len()
                || bytes.len() < 24
                || bytes[..8] != [137, 80, 78, 71, 13, 10, 26, 10]
                || bytes[12..16] != *b"IHDR"
            {
                return Err("live capture artifact bytes were invalid".into());
            }
            let width = u32::from_be_bytes(bytes[16..20].try_into().expect("four width bytes"));
            let height = u32::from_be_bytes(bytes[20..24].try_into().expect("four height bytes"));
            let hash = format!("{:x}", Sha256::digest(&bytes));
            if width != u32::from(capture.width)
                || height != u32::from(capture.height)
                || hash != capture.sha256
            {
                return Err("live capture evidence did not match its artifact".into());
            }
            Ok(metadata.len())
        }

        #[cfg(test)]
        mod tests {
            use super::*;
            use std::io::Cursor;

            #[test]
            fn bounded_jsonl_reader_accepts_crlf_but_preserves_embedded_carriage_returns() {
                let mut crlf = BufReader::new(Cursor::new(b"{\"schema_version\":1}\r\n"));
                assert_eq!(
                    read_bounded_line(&mut crlf, MAX_SUPERVISED_LIVE_LINE_BYTES).unwrap(),
                    Some(b"{\"schema_version\":1}".to_vec())
                );

                let mut embedded = BufReader::new(Cursor::new(b"a\rb\n"));
                let embedded_line =
                    read_bounded_line(&mut embedded, MAX_SUPERVISED_LIVE_LINE_BYTES)
                        .unwrap()
                        .expect("embedded-CR line");
                assert_eq!(embedded_line, b"a\rb");

                let mut invalid_jsonl = BufReader::new(Cursor::new(
                    b"{\"schema_version\":1,\r\"request_id\":1,\"command\":{\"type\":\"pause\"}}\n",
                ));
                let invalid_line =
                    read_bounded_line(&mut invalid_jsonl, MAX_SUPERVISED_LIVE_LINE_BYTES)
                        .unwrap()
                        .expect("embedded-CR JSONL line");
                assert!(SupervisedLiveRequest::parse_line(&invalid_line).is_err());
            }

            #[test]
            fn peer_receipt_projection_rejects_unknown_and_non_numeric_fields() {
                let (receipt, complete) = parse_peer_receipt(
                    br#"{"event":"snapshot","game_id":1,"phase":2,"terminal_phase":3,"screen":1,"revision":4,"hash":5,"sequence":6,"current":1,"winner":0,"moves":7}"#,
                )
                .unwrap();
                assert!(!complete);
                assert_eq!(receipt["event"], "snapshot");
                assert_eq!(receipt["fields"]["revision"], 4);
                assert!(parse_peer_receipt(
                    br#"{"event":"snapshot","game_id":1,"phase":2,"terminal_phase":3,"screen":1,"revision":4,"hash":5,"sequence":6,"current":1,"winner":0,"moves":7,"path":"C:\\secret"}"#,
                )
                .is_err());
                assert!(parse_peer_receipt(
                    br#"{"event":"ack","sequence":"secret","ack":1,"result":0,"revision":4,"hash":5}"#,
                )
                .is_err());
            }

            #[test]
            fn nonreading_control_and_stdout_pipes_have_bounded_writes() {
                let (_control_reader, control_pipe_writer) =
                    supervision_windows::pipe_from_parent().unwrap();
                let control_writer = BoundedWriter::new(control_pipe_writer);
                let oversized_request = vec![b'x'; MAX_SUPERVISED_LIVE_LINE_BYTES + 1];
                let started = Instant::now();
                assert!(control_writer
                    .write(
                        &oversized_request,
                        Instant::now() + Duration::from_millis(30)
                    )
                    .is_err());
                assert!(started.elapsed() < Duration::from_secs(1));
                assert!(control_writer
                    .worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_none());

                let (_stdout_reader, stdout_pipe_writer) =
                    supervision_windows::pipe_to_child().unwrap();
                let stdout_writer = BoundedWriter::new(File::from(stdout_pipe_writer));
                let response = vec![b'x'; 64 * 1024];
                let started = Instant::now();
                assert!(stdout_writer
                    .write(&response, Instant::now() + Duration::from_millis(30))
                    .is_err());
                assert!(started.elapsed() < Duration::from_secs(1));
                assert!(stdout_writer
                    .worker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_none());
            }
        }
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
    use std::fs;
    use std::path::Path;

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
        assert_eq!(
            options.authority,
            Authority::Packaged {
                exe: PathBuf::from("authority.exe")
            }
        );
        assert_eq!(options.action, Duration::from_secs(2));
    }

    #[test]
    fn parses_live_toolchain_authority_without_changing_packaged_mode() {
        let options = parse_args(
            [
                "--toolchain",
                "stasis.exe",
                "--workspace",
                "project",
                "--evidence-root",
                "C:\\temp\\evidence",
                "--peer",
                "peer.exe",
                "--startup-ms",
                "1000",
                "--action-ms",
                "2000",
                "--shutdown-ms",
                "3000",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert_eq!(
            options.authority,
            Authority::Live {
                toolchain: PathBuf::from("stasis.exe"),
                workspace: PathBuf::from("project"),
            }
        );
        assert_eq!(
            options.evidence_root,
            Some(PathBuf::from("C:\\temp\\evidence"))
        );
    }

    #[test]
    fn rejects_unbounded_or_incomplete_contract() {
        assert!(parse_args(["--startup-ms", "900001"].into_iter().map(str::to_string)).is_err());
        assert!(parse_args(std::iter::empty()).is_err());
        assert!(parse_args(
            [
                "--authority",
                "authority.exe",
                "--toolchain",
                "stasis.exe",
                "--workspace",
                "project",
                "--peer",
                "peer.exe",
                "--startup-ms",
                "1",
                "--action-ms",
                "1",
                "--shutdown-ms",
                "1",
            ]
            .into_iter()
            .map(str::to_string)
        )
        .is_err());
        assert_eq!(
            parse_args(["private-invite"].into_iter().map(str::to_string)),
            Err("missing option value".into())
        );
    }

    #[test]
    fn capture_root_validation_accepts_temporary_directories_and_rejects_invalid_paths() {
        assert_eq!(
            validate_capture_root(Path::new("relative-capture-root")),
            Err("supervised live capture root is invalid")
        );
        assert_eq!(
            validate_capture_root(&std::env::temp_dir()),
            Err("supervised live capture root is invalid")
        );

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "stasis-capture-root-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        assert_eq!(validate_capture_root(&root), Ok(()));

        let file = root.join("not-a-directory");
        fs::write(&file, b"capture root test").unwrap();
        assert_eq!(
            validate_capture_root(&file),
            Err("supervised live capture root is invalid")
        );
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

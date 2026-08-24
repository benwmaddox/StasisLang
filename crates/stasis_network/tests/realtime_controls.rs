use stasis_network::realtime::{
    AdmissionOutcome, AuthoritativeSnapshot, ConfigError, ControlEnvelope, ControlState,
    HashRecordError, RealtimeConfig, RealtimeSession, ReplayError, ReplayEvent, ReplayRecord,
    ScheduledTransition, SnapshotError, MAX_PENDING_TRANSITIONS, REALTIME_MAX_SEATS,
};
use stasis_network::{
    stasis_realtime_advance, stasis_realtime_current_tick, stasis_realtime_read_control,
    stasis_realtime_schedule, stasis_realtime_start, stasis_realtime_stop,
    stasis_realtime_submit_payload, BundleFile, EventKind, NetworkHost, StaticBundle,
};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::{connect, Message};

fn native_realtime_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native realtime test lock")
}

fn transition(
    seat: u8,
    sequence: u32,
    apply_tick: u64,
    state: ControlState,
) -> ScheduledTransition {
    ScheduledTransition {
        seat,
        epoch: 1,
        sequence,
        apply_tick,
        state,
    }
}

fn transition_in_epoch(
    seat: u8,
    epoch: u32,
    sequence: u32,
    apply_tick: u64,
    state: ControlState,
) -> ScheduledTransition {
    ScheduledTransition {
        seat,
        epoch,
        sequence,
        apply_tick,
        state,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct World {
    position: i32,
    world_tick: u64,
}

fn simulate(world: &mut World, controls: &[ControlState; REALTIME_MAX_SEATS]) {
    world.world_tick += 1;
    world.position += i32::from(controls[0].axis_x);
}

fn world_hash(world: World, controls: &[ControlState; REALTIME_MAX_SEATS]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in world
        .position
        .to_le_bytes()
        .into_iter()
        .chain(world.world_tick.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for control in controls {
        hash ^= u64::from(control.buttons);
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= control.axis_x as i64 as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        hash ^= control.axis_y as i64 as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn rtc1_payload_crosses_the_existing_network_host_websocket_path() {
    let bundle = StaticBundle::new(vec![BundleFile {
        path: "index.html".into(),
        mime: "text/html; charset=utf-8".into(),
        bytes: b"<html/>".to_vec(),
    }])
    .expect("bundle");
    let mut host = NetworkHost::bind(0, bundle).expect("host");
    let mut request = format!("ws://{}/session", host.address())
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "Origin",
        format!("http://{}", host.address())
            .parse()
            .expect("origin"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!(
            "stasis-v1, {}, stasis-resume-v1.00112233445566778899aabbccddeeff",
            hex(host.session_secret())
        )
        .parse()
        .expect("protocol"),
    );
    let (mut socket, _) = connect(request).expect("websocket");
    let deadline = Instant::now() + Duration::from_secs(2);
    let connection = loop {
        if let Some(event) = host.poll() {
            if event.kind == EventKind::Connected {
                break event.connection;
            }
        }
        assert!(Instant::now() < deadline, "connection event timeout");
        thread::sleep(Duration::from_millis(2));
    };
    let envelope =
        ControlEnvelope::from_transition(transition(0, 1, 2, ControlState::new(1, 1, 0)))
            .expect("envelope");
    socket
        .send(Message::Binary(envelope.encode().into()))
        .expect("send RTC1");
    let deadline = Instant::now() + Duration::from_secs(2);
    let payload = loop {
        if let Some(event) = host.poll() {
            if event.kind == EventKind::Message && event.connection == connection {
                break event.payload;
            }
        }
        assert!(Instant::now() < deadline, "message event timeout");
        thread::sleep(Duration::from_millis(2));
    };
    let config = RealtimeConfig::new(60, 60, 20, 1).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("receiver");
    assert_eq!(
        session.submit_payload(&payload).outcomes(),
        &[AdmissionOutcome::Accepted]
    );
    socket.close(None).expect("close");
    host.stop().expect("stop");
}

#[test]
fn networkhost_payload_reaches_the_same_native_realtime_session() {
    let _guard = native_realtime_test_lock();
    let bundle = StaticBundle::new(vec![BundleFile {
        path: "index.html".into(),
        mime: "text/html; charset=utf-8".into(),
        bytes: b"<html/>".to_vec(),
    }])
    .expect("bundle");
    let mut host = NetworkHost::bind(0, bundle).expect("host");
    let mut request = format!("ws://{}/session", host.address())
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "Origin",
        format!("http://{}", host.address())
            .parse()
            .expect("origin"),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        format!(
            "stasis-v1, {}, stasis-resume-v1.00112233445566778899aabbccddeeff",
            hex(host.session_secret())
        )
        .parse()
        .expect("protocol"),
    );
    let (mut socket, _) = connect(request).expect("websocket");
    let deadline = Instant::now() + Duration::from_secs(2);
    let connection = loop {
        if let Some(event) = host.poll() {
            if event.kind == EventKind::Connected {
                break event.connection;
            }
        }
        assert!(Instant::now() < deadline, "connection event timeout");
        thread::sleep(Duration::from_millis(2));
    };
    let envelope =
        ControlEnvelope::from_transition(transition(0, 1, 2, ControlState::new(7, 1, 0)))
            .expect("envelope");
    socket
        .send(Message::Binary(envelope.encode().into()))
        .expect("send RTC1");
    let deadline = Instant::now() + Duration::from_secs(2);
    let payload = loop {
        if let Some(event) = host.poll() {
            if event.kind == EventKind::Message && event.connection == connection {
                break event.payload;
            }
        }
        assert!(Instant::now() < deadline, "message event timeout");
        thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(stasis_realtime_stop(), 0);
    assert_eq!(stasis_realtime_start(60, 120, 20, 1, 1), 0);
    let guest_payload: Vec<i32> = payload.iter().copied().map(i32::from).collect();
    assert_eq!(
        unsafe {
            stasis_realtime_submit_payload(guest_payload.as_ptr(), guest_payload.len() as i32)
        },
        0
    );
    assert_eq!(stasis_realtime_current_tick(), 0);
    assert_eq!(stasis_realtime_advance(), 0);
    assert_eq!(stasis_realtime_advance(), 0);
    assert_eq!(stasis_realtime_current_tick(), 2);
    assert_eq!(stasis_realtime_stop(), 0);
    socket.close(None).expect("close");
    host.stop().expect("stop");
}

#[test]
fn native_guest_payload_bounds_and_mixed_outcomes_are_visible() {
    let _guard = native_realtime_test_lock();
    let encode = |envelope: &ControlEnvelope| {
        envelope
            .encode()
            .into_iter()
            .map(i32::from)
            .collect::<Vec<_>>()
    };

    assert_eq!(stasis_realtime_stop(), 0);
    assert_eq!(stasis_realtime_start(60, 60, 60, 1, 1), 0);
    let mut mixed = ControlEnvelope::new();
    mixed
        .push(transition_in_epoch(0, 1, 1, 1, ControlState::new(1, 1, 0)))
        .expect("accepted transition");
    mixed
        .push(transition_in_epoch(0, 2, 2, 1, ControlState::neutral()))
        .expect("stale transition");
    let mixed = encode(&mixed);
    assert_eq!(
        unsafe { stasis_realtime_submit_payload(mixed.as_ptr(), mixed.len() as i32) },
        -7,
        "later stale outcome must not be hidden by the first acceptance"
    );
    assert_eq!(stasis_realtime_advance(), 0);
    let (mut buttons, mut axis_x, mut axis_y) = (0, 0, 0);
    assert_eq!(
        unsafe { stasis_realtime_read_control(0, &mut buttons, &mut axis_x, &mut axis_y) },
        0
    );
    assert_eq!((buttons, axis_x, axis_y), (1, 1, 0));
    assert_eq!(stasis_realtime_stop(), 0);

    assert_eq!(stasis_realtime_start(60, 60, 60, 1, 1), 0);
    let hostile = ControlEnvelope::from_transition(transition_in_epoch(
        0,
        1,
        i32::MAX as u32 + 1,
        1,
        ControlState::new(1, 1, 0),
    ))
    .expect("wire-valid hostile transition");
    let hostile = encode(&hostile);
    assert_eq!(
        unsafe { stasis_realtime_submit_payload(hostile.as_ptr(), hostile.len() as i32) },
        -1
    );
    assert_eq!(stasis_realtime_schedule(0, 1, 1, 1, 1, 1, 0), 0);
    assert_eq!(stasis_realtime_stop(), 0);
}

#[test]
fn rates_are_independent_and_delay_is_genuinely_future() {
    let config = RealtimeConfig::new(60, 30, 120, 3).expect("independent rates");
    assert_eq!(config.simulation_hz(), 60);
    assert_eq!(config.presentation_hz(), 30);
    assert_eq!(config.control_hz(), 120);
    assert_eq!(config.input_delay_ticks(), 3);
    assert_eq!(
        RealtimeConfig::new(0, 60, 60, 3),
        Err(ConfigError::ZeroRate)
    );
    assert_eq!(
        RealtimeConfig::new(60, 60, 60, 0),
        Err(ConfigError::DelayZero)
    );
    let session = RealtimeSession::new(config, 2).expect("session");
    assert_eq!(session.earliest_apply_tick(), 3);
    assert_eq!(session.latest_apply_tick(), 240);
}

#[test]
fn two_devices_run_a_real_world_for_60_ticks_with_loss_recovery_and_read_only_presentation() {
    let config = RealtimeConfig::new(60, 120, 20, 3).expect("realtime rates");
    let press = transition(0, 1, 3, ControlState::new(1, 1, 0));
    let change = transition(0, 2, 6, ControlState::new(1, -1, 0));
    let release = transition(0, 3, 9, ControlState::neutral());
    let mut device_a = RealtimeSession::new(config, 2).expect("device A");
    let mut device_b = RealtimeSession::new(config, 2).expect("device B");
    assert_eq!(
        device_a.submit_transition(press),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        device_b.submit_transition(press),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        device_b.submit_transition(press),
        AdmissionOutcome::Duplicate
    );
    assert_eq!(
        device_a.submit_transition(release),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        device_b.submit_transition(release),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        device_a.submit_transition(change),
        AdmissionOutcome::AcceptedReordered
    );

    let mut world_a = World::default();
    let mut world_b = World::default();
    let mut presentation_frames = 0_u64;
    for tick in 1..=60 {
        if tick == 3 {
            // Device B's first copy of the direction change was lost. A
            // retransmission arrives before its authoritative tick.
            assert_eq!(
                device_b.submit_transition(change),
                AdmissionOutcome::AcceptedReordered
            );
        }
        let advance_a = device_a.advance_tick().expect("tick A");
        let advance_b = device_b.advance_tick().expect("tick B");
        simulate(&mut world_a, advance_a.controls());
        simulate(&mut world_b, advance_b.controls());
        assert_eq!(advance_a.tick, tick);
        assert_eq!(world_a, world_b);
        assert_eq!(advance_a.controls(), advance_b.controls());
        assert_eq!(
            advance_a.applied_transitions().len(),
            if [3, 6, 9].contains(&tick) { 1 } else { 0 }
        );
        let expected = match tick {
            1 | 2 => ControlState::neutral(),
            3..=5 => ControlState::new(1, 1, 0),
            6..=8 => ControlState::new(1, -1, 0),
            _ => ControlState::neutral(),
        };
        assert_eq!(advance_a.control(0), Some(expected));
        let hash_a = world_hash(world_a, advance_a.controls());
        let hash_b = world_hash(world_b, advance_b.controls());
        assert_eq!(hash_a, hash_b);
        device_a
            .record_authoritative_hash(tick, hash_a)
            .expect("hash A");
        device_b
            .record_authoritative_hash(tick, hash_b)
            .expect("hash B");
        for _ in 0..2 {
            let before_tick = device_a.current_tick();
            let before_controls = *device_a.controls();
            let before_world = world_a;
            presentation_frames += 1;
            assert_eq!(device_a.current_tick(), before_tick);
            assert_eq!(*device_a.controls(), before_controls);
            assert_eq!(world_a, before_world);
        }
    }
    assert_eq!(world_a.world_tick, 60);
    assert_eq!(presentation_frames, 120);
    assert_eq!(world_a.position, 0);
    let mut replay_world = World::default();
    device_a
        .replay()
        .replay(config, 2, |tick, controls| {
            simulate(&mut replay_world, controls);
            assert_eq!(replay_world.world_tick, tick);
            world_hash(replay_world, controls)
        })
        .expect("replay reproduces world hashes");
    assert_eq!(replay_world, world_a);
}

#[test]
fn pending_conflicts_quarantine_both_arrival_orders_to_the_same_result() {
    let config = RealtimeConfig::new(60, 60, 60, 1).expect("config");
    let left = transition(0, 1, 3, ControlState::new(1, 1, 0));
    let right = transition(0, 1, 3, ControlState::new(1, -1, 0));
    let mut first_left = RealtimeSession::new(config, 1).expect("session");
    let mut first_right = RealtimeSession::new(config, 1).expect("session");
    assert_eq!(
        first_left.submit_transition(left),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        first_left.submit_transition(right),
        AdmissionOutcome::Conflict
    );
    assert_eq!(
        first_right.submit_transition(right),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        first_right.submit_transition(left),
        AdmissionOutcome::Conflict
    );
    for tick in 1..=3 {
        let left_tick = first_left.advance_tick().expect("tick left");
        let right_tick = first_right.advance_tick().expect("tick right");
        assert_eq!(left_tick.controls(), right_tick.controls());
        let hash = world_hash(
            World {
                world_tick: tick,
                ..World::default()
            },
            left_tick.controls(),
        );
        first_left
            .record_authoritative_hash(tick, hash)
            .expect("hash");
        first_right
            .record_authoritative_hash(tick, hash)
            .expect("hash");
    }
    assert_eq!(first_left.control(0), Some(ControlState::neutral()));
    assert_eq!(first_left.submit_transition(right), AdmissionOutcome::Late);
    assert!(first_left
        .replay()
        .records()
        .any(|record| matches!(record, ReplayRecord::Quarantine { .. })));
}

#[test]
fn envelope_and_future_bounds_are_bounded_and_malformed_inputs_are_stable() {
    let scheduled = transition(1, 7, 10, ControlState::new(0x22, -3, 4));
    let envelope = ControlEnvelope::from_transition(scheduled).expect("transition");
    let bytes = envelope.encode();
    assert_eq!(
        ControlEnvelope::decode(&bytes)
            .expect("round trip")
            .transitions(),
        &[scheduled]
    );
    assert_eq!(
        ControlEnvelope::decode(&bytes)
            .expect("epoch round trip")
            .transitions()[0]
            .epoch,
        1
    );
    let mut malformed = bytes.clone();
    malformed[0] = b'X';
    assert!(ControlEnvelope::decode(&malformed).is_err());
    let config = RealtimeConfig::new(60, 60, 30, 2).expect("config");
    let mut session = RealtimeSession::new(config, 2).expect("session");
    assert_eq!(
        session.submit_payload(&malformed).outcomes(),
        &[AdmissionOutcome::Malformed]
    );
    assert_eq!(
        session.submit_transition(transition(2, 1, 3, ControlState::neutral())),
        AdmissionOutcome::Malformed
    );
    assert_eq!(
        session.submit_transition(transition(0, 0, 3, ControlState::neutral())),
        AdmissionOutcome::Malformed
    );
    assert_eq!(
        session.submit_transition(transition(0, 1, 1, ControlState::neutral())),
        AdmissionOutcome::Late
    );
    assert_eq!(
        session.submit_transition(transition(
            0,
            1,
            session.latest_apply_tick() + 1,
            ControlState::neutral()
        )),
        AdmissionOutcome::TooFar
    );
}

#[test]
fn lifecycle_and_snapshot_validation_preserve_sequence_floors() {
    let config = RealtimeConfig::new(60, 60, 20, 2).expect("config");
    let mut session = RealtimeSession::new(config, 2).expect("session");
    assert_eq!(
        session.submit_transition(transition(0, 1, 2, ControlState::new(1, 1, 0))),
        AdmissionOutcome::Accepted
    );
    session.disconnect(0).expect("disconnect");
    assert_eq!(session.pending_len(), 0);
    assert_eq!(session.control(0), Some(ControlState::neutral()));
    assert_eq!(
        session.submit_transition(transition(0, 1, 2, ControlState::new(1, 1, 0))),
        AdmissionOutcome::Inactive
    );
    session.reconnect(0).expect("reconnect");
    assert_eq!(
        session.submit_transition(transition(0, 99, 2, ControlState::new(1, 1, 0))),
        AdmissionOutcome::Stale
    );
    session.pause().expect("pause");
    session.focus_lost().expect("focus");
    let mut controls = [ControlState::neutral(); REALTIME_MAX_SEATS];
    controls[1] = ControlState::new(4, 0, -1);
    let mut sequences = [0; REALTIME_MAX_SEATS];
    sequences[0] = 9;
    let snapshot = AuthoritativeSnapshot::new(
        1,
        40,
        controls,
        sequences,
        [5, 3, 1, 1, 1, 1, 1, 1],
        [true, true, false, false, false, false, false, false],
    );
    session
        .apply_authoritative_snapshot(snapshot)
        .expect("snapshot");
    assert_eq!(session.current_tick(), 40);
    assert_eq!(session.controls(), &controls);
    assert_eq!(
        session.submit_transition(transition(0, 8, 42, ControlState::neutral())),
        AdmissionOutcome::Stale
    );
    assert_eq!(
        session.submit_transition(transition_in_epoch(
            0,
            5,
            10,
            42,
            ControlState::new(2, 1, 0),
        )),
        AdmissionOutcome::Accepted
    );
    controls[2] = ControlState::new(1, 0, 0);
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            2,
            41,
            controls,
            sequences,
            [5, 1, 1, 1, 1, 1, 1, 1],
            [true, true, false, false, false, false, false, false],
        )),
        Err(SnapshotError::InactiveSeatState { seat: 2 })
    );
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            3,
            u64::MAX,
            [ControlState::neutral(); REALTIME_MAX_SEATS],
            [0; REALTIME_MAX_SEATS],
            [1; REALTIME_MAX_SEATS],
            [true, true, false, false, false, false, false, false],
        )),
        Err(SnapshotError::TickOutOfRange)
    );
    let mut exhausted_epochs = [1; REALTIME_MAX_SEATS];
    exhausted_epochs[0] = u32::MAX;
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            4,
            40,
            [ControlState::neutral(); REALTIME_MAX_SEATS],
            [0; REALTIME_MAX_SEATS],
            exhausted_epochs,
            [true, true, false, false, false, false, false, false],
        )),
        Err(SnapshotError::EpochExhausted)
    );
    session.rematch().expect("rematch");
    assert_eq!(session.current_tick(), 0);
    assert_eq!(
        session.controls(),
        &[ControlState::neutral(); REALTIME_MAX_SEATS]
    );
}

#[test]
fn pause_and_focus_advance_all_epochs_and_reject_unseen_old_packets() {
    let config = RealtimeConfig::new(60, 60, 20, 1).expect("config");
    let mut session = RealtimeSession::new(config, 2).expect("session");
    let old_epoch = session.epoch(0).expect("epoch");
    assert_eq!(
        session.submit_transition(transition(0, 99, 1, ControlState::new(1, 0, 0))),
        AdmissionOutcome::Accepted
    );
    session.pause().expect("pause");
    assert_eq!(session.epoch(0), Some(old_epoch + 1));
    assert_eq!(session.epoch(1), Some(2));
    assert_eq!(
        session.submit_transition(transition_in_epoch(
            0,
            old_epoch,
            100,
            1,
            ControlState::new(1, 0, 0)
        )),
        AdmissionOutcome::Stale
    );
    session.focus_lost().expect("focus");
    assert_eq!(session.epoch(0), Some(old_epoch + 2));
    assert_eq!(
        session.submit_transition(transition_in_epoch(
            0,
            old_epoch + 1,
            1,
            1,
            ControlState::new(1, 0, 0)
        )),
        AdmissionOutcome::Stale
    );
}

#[test]
fn rematch_replay_reuses_epoch_reset_semantics() {
    let config = RealtimeConfig::new(60, 60, 20, 1).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("session");
    session.rematch().expect("rematch");
    let epoch = session.epoch(0).expect("epoch");
    assert_eq!(
        session.submit_transition(transition_in_epoch(
            0,
            epoch,
            1,
            1,
            ControlState::new(1, 1, 0)
        )),
        AdmissionOutcome::Accepted
    );
    session.advance_tick().expect("tick");
    session.record_authoritative_hash(1, 42).expect("hash");
    session
        .replay()
        .replay(config, 1, |_tick, _controls| 42)
        .expect("rematch replay");
}

#[test]
fn snapshots_are_monotonic_and_new_epochs_may_reset_floors() {
    let config = RealtimeConfig::new(60, 60, 20, 2).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("session");
    let controls = [ControlState::neutral(); REALTIME_MAX_SEATS];
    let active = [true, false, false, false, false, false, false, false];
    let epochs = [1, 1, 1, 1, 1, 1, 1, 1];
    let mut sequences = [0; REALTIME_MAX_SEATS];
    sequences[0] = 4;
    session
        .apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            1, 10, controls, sequences, epochs, active,
        ))
        .expect("first snapshot");
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            1, 11, controls, sequences, epochs, active,
        )),
        Err(SnapshotError::RevisionRegression)
    );
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            2, 9, controls, sequences, epochs, active,
        )),
        Err(SnapshotError::TickRegression)
    );
    let mut lower_epochs = epochs;
    lower_epochs[0] = 0;
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            3,
            11,
            controls,
            sequences,
            lower_epochs,
            active,
        )),
        Err(SnapshotError::EpochRegression)
    );
    let mut lower_sequences = sequences;
    lower_sequences[0] = 3;
    assert_eq!(
        session.apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            4,
            11,
            controls,
            lower_sequences,
            epochs,
            active,
        )),
        Err(SnapshotError::SequenceRegression)
    );
    let mut new_epochs = epochs;
    new_epochs[0] = 2;
    session
        .apply_authoritative_snapshot(AuthoritativeSnapshot::new(
            5,
            1,
            controls,
            [0; REALTIME_MAX_SEATS],
            new_epochs,
            active,
        ))
        .expect("new epoch reset");
    assert_eq!(session.epoch(0), Some(2));
    assert_eq!(session.current_tick(), 1);
}

#[test]
fn quarantine_capacity_fails_closed_until_new_snapshot_or_rematch() {
    let config = RealtimeConfig::new(60, 60, 60, 1).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("session");
    for sequence in 1..=(MAX_PENDING_TRANSITIONS as u32) {
        let left = transition_in_epoch(
            0,
            1,
            sequence,
            sequence as u64 + 1,
            ControlState::new(1, 1, 0),
        );
        let right = transition_in_epoch(
            0,
            1,
            sequence,
            sequence as u64 + 1,
            ControlState::new(1, -1, 0),
        );
        assert_eq!(session.submit_transition(left), AdmissionOutcome::Accepted);
        assert_eq!(session.submit_transition(right), AdmissionOutcome::Conflict);
    }
    assert!(!session.resync_required());
    assert_eq!(
        session.submit_transition(transition_in_epoch(
            0,
            1,
            (MAX_PENDING_TRANSITIONS + 1) as u32,
            130,
            ControlState::neutral(),
        )),
        AdmissionOutcome::ResyncRequired
    );
    assert!(session.resync_required());
    assert_eq!(
        session.submit_transition(transition_in_epoch(0, 1, 999, 130, ControlState::neutral())),
        AdmissionOutcome::ResyncRequired
    );
    let snapshot = AuthoritativeSnapshot::new(
        1,
        0,
        [ControlState::neutral(); REALTIME_MAX_SEATS],
        [0; REALTIME_MAX_SEATS],
        [2; REALTIME_MAX_SEATS],
        [true, false, false, false, false, false, false, false],
    );
    session
        .apply_authoritative_snapshot(snapshot)
        .expect("snapshot recovery");
    assert!(!session.resync_required());
    assert_eq!(
        session.submit_transition(transition_in_epoch(0, 2, 1, 1, ControlState::new(1, 1, 0),)),
        AdmissionOutcome::Accepted
    );
}

#[test]
fn replay_notifies_game_of_reset_and_snapshot_before_later_hashes() {
    use std::cell::RefCell;
    let config = RealtimeConfig::new(60, 60, 20, 1).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("session");
    assert_eq!(
        session.submit_transition(transition_in_epoch(0, 1, 1, 2, ControlState::new(1, 1, 0),)),
        AdmissionOutcome::Accepted
    );
    let first = session.advance_tick().expect("first tick");
    let mut world = World::default();
    simulate(&mut world, first.controls());
    session
        .record_authoritative_hash(1, world_hash(world, first.controls()))
        .expect("first hash");
    session.pause().expect("pause");
    world = World::default();
    let mut epochs = [1; REALTIME_MAX_SEATS];
    epochs[0] = 3;
    let snapshot = AuthoritativeSnapshot::new(
        1,
        0,
        [ControlState::neutral(); REALTIME_MAX_SEATS],
        [0; REALTIME_MAX_SEATS],
        epochs,
        [true, false, false, false, false, false, false, false],
    )
    .with_game_token(&[7])
    .expect("token");
    session
        .apply_authoritative_snapshot(snapshot)
        .expect("snapshot");
    world.position = 7;
    assert_eq!(
        session.submit_transition(transition_in_epoch(0, 3, 1, 1, ControlState::new(1, 1, 0),)),
        AdmissionOutcome::Accepted
    );
    for tick in 1..=2 {
        let advance = session.advance_tick().expect("later tick");
        simulate(&mut world, advance.controls());
        session
            .record_authoritative_hash(tick, world_hash(world, advance.controls()))
            .expect("later hash");
    }
    let mut events = Vec::new();
    let replay_world = RefCell::new(World::default());
    session
        .replay()
        .replay_with_events(
            config,
            1,
            |event| {
                match event {
                    ReplayEvent::Reset(_) => *replay_world.borrow_mut() = World::default(),
                    ReplayEvent::Snapshot(snapshot) => {
                        *replay_world.borrow_mut() = World {
                            position: i32::from(snapshot.game_token[0]),
                            ..World::default()
                        };
                    }
                    ReplayEvent::Quarantine { .. } => {}
                }
                events.push(event);
            },
            |_tick, controls| {
                let mut world = replay_world.borrow_mut();
                simulate(&mut world, controls);
                world_hash(*world, controls)
            },
        )
        .expect("replay with lifecycle events");
    assert!(events
        .iter()
        .any(|event| matches!(event, ReplayEvent::Reset(_))));
    assert!(events.iter().any(|event| matches!(
        event,
        ReplayEvent::Snapshot(snapshot)
            if snapshot.game_token_len == 1 && snapshot.game_token[0] == 7
    )));
}

#[test]
fn queue_full_replay_overflow_and_hash_attachment_are_explicit() {
    let config = RealtimeConfig::new(60, 60, 60, 1).expect("config");
    let mut session = RealtimeSession::new(config, 1).expect("session");
    assert_eq!(
        session.submit_transition(transition(0, 1, 1, ControlState::neutral())),
        AdmissionOutcome::Accepted
    );
    assert_eq!(
        session.submit_transition(transition(0, 1, 1, ControlState::new(1, 0, 0))),
        AdmissionOutcome::Conflict
    );
    for sequence in 2..=((MAX_PENDING_TRANSITIONS + 1) as u32) {
        assert_eq!(
            session.submit_transition(transition(
                0,
                sequence,
                sequence as u64,
                ControlState::neutral()
            )),
            AdmissionOutcome::Accepted
        );
    }
    assert_eq!(session.pending_len(), MAX_PENDING_TRANSITIONS);
    assert_eq!(
        session.submit_transition(transition(
            0,
            10_000,
            session.latest_apply_tick(),
            ControlState::neutral(),
        )),
        AdmissionOutcome::Full
    );
    let advance = session.advance_tick().expect("tick");
    assert_eq!(
        session.record_authoritative_hash(advance.tick + 1, 1),
        Err(HashRecordError::WrongTick {
            expected: advance.tick,
            received: advance.tick + 1
        })
    );
    assert_eq!(session.record_authoritative_hash(advance.tick, 1), Ok(()));
    assert_eq!(
        session.record_authoritative_hash(advance.tick, 2),
        Err(HashRecordError::AlreadyRecorded)
    );

    let mut missing = RealtimeSession::new(config, 1).expect("missing hash session");
    missing.advance_tick().expect("tick");
    assert_eq!(
        missing.replay().replay(config, 1, |_tick, _controls| 0),
        Err(ReplayError::MissingHash { index: 0, tick: 1 })
    );
    let mut reset_hash = RealtimeSession::new(config, 1).expect("reset hash session");
    reset_hash.advance_tick().expect("tick");
    reset_hash.pause().expect("pause");
    assert_eq!(
        reset_hash.record_authoritative_hash(1, 1),
        Err(HashRecordError::NoUnfinishedTick)
    );
    let mut overflow =
        RealtimeSession::new_with_replay_limit(config, 1, 4).expect("overflow session");
    for _ in 0..=4 {
        overflow.advance_tick().expect("tick");
    }
    assert!(!overflow.replay().is_complete());
    assert_eq!(
        overflow.replay().replay(config, 1, |_tick, _controls| 0),
        Err(ReplayError::Incomplete)
    );
}

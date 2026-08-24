//! Fixed-tick realtime control scheduling.
//!
//! This module owns the simulation-side contract only.  A transport may carry
//! [`ControlEnvelope::encode`] bytes in its normal message envelope, but this
//! module never reads a socket and never performs rendering work.

use thiserror::Error;

pub const REALTIME_MAX_SEATS: usize = 8;
pub const MAX_ENVELOPE_TRANSITIONS: usize = 16;
pub const MAX_PENDING_TRANSITIONS: usize = 128;
pub const DEFAULT_REPLAY_RECORDS: usize = 65_536;
pub const MAX_REPLAY_RECORDS: usize = 1_000_000;
pub const MAX_DELAY_TICKS: u32 = 240;
pub const MAX_FUTURE_TICKS: u64 = 240;
pub const MAX_SNAPSHOT_TOKEN_BYTES: usize = 64;
pub const MAX_RATE_HZ: u32 = 1_000;
/// Native/JIT guests expose ticks and epochs as signed 32-bit values.
pub const GUEST_MAX_TICK: u64 = i32::MAX as u64;
pub const GUEST_MAX_EPOCH: u32 = i32::MAX as u32;
pub const REALTIME_ENVELOPE_MAGIC: [u8; 4] = *b"RTC1";
pub const REALTIME_ENVELOPE_VERSION: u16 = 1;
pub const ENVELOPE_HEADER_BYTES: usize = 8;
pub const TRANSITION_BYTES: usize = 21;
pub const MAX_ENVELOPE_BYTES: usize =
    ENVELOPE_HEADER_BYTES + MAX_ENVELOPE_TRANSITIONS * TRANSITION_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlState {
    pub buttons: u16,
    pub axis_x: i8,
    pub axis_y: i8,
}

impl ControlState {
    pub const fn neutral() -> Self {
        Self {
            buttons: 0,
            axis_x: 0,
            axis_y: 0,
        }
    }

    pub const fn new(buttons: u16, axis_x: i8, axis_y: i8) -> Self {
        Self {
            buttons,
            axis_x,
            axis_y,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScheduledTransition {
    pub seat: u8,
    pub epoch: u32,
    pub sequence: u32,
    pub apply_tick: u64,
    pub state: ControlState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeConfig {
    simulation_hz: u32,
    presentation_hz: u32,
    control_hz: u32,
    input_delay_ticks: u32,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    #[error("simulation, presentation, and control rates must be positive")]
    ZeroRate,
    #[error("rate exceeds the bounded realtime limit")]
    RateTooHigh,
    #[error("input delay must be positive")]
    DelayZero,
    #[error("input delay exceeds the bounded realtime limit")]
    DelayTooHigh,
}

impl RealtimeConfig {
    pub fn new(
        simulation_hz: u32,
        presentation_hz: u32,
        control_hz: u32,
        input_delay_ticks: u32,
    ) -> Result<Self, ConfigError> {
        if simulation_hz == 0 || presentation_hz == 0 || control_hz == 0 {
            return Err(ConfigError::ZeroRate);
        }
        if simulation_hz > MAX_RATE_HZ || presentation_hz > MAX_RATE_HZ || control_hz > MAX_RATE_HZ
        {
            return Err(ConfigError::RateTooHigh);
        }
        if input_delay_ticks == 0 {
            return Err(ConfigError::DelayZero);
        }
        if input_delay_ticks > MAX_DELAY_TICKS {
            return Err(ConfigError::DelayTooHigh);
        }
        Ok(Self {
            simulation_hz,
            presentation_hz,
            control_hz,
            input_delay_ticks,
        })
    }

    pub const fn simulation_hz(self) -> u32 {
        self.simulation_hz
    }

    pub const fn presentation_hz(self) -> u32 {
        self.presentation_hz
    }

    pub const fn control_hz(self) -> u32 {
        self.control_hz
    }

    pub const fn input_delay_ticks(self) -> u32 {
        self.input_delay_ticks
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionOutcome {
    Accepted,
    AcceptedReordered,
    Inactive,
    Duplicate,
    Stale,
    Conflict,
    Late,
    TooFar,
    ResyncRequired,
    Malformed,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionReport {
    outcomes: [AdmissionOutcome; MAX_ENVELOPE_TRANSITIONS],
    count: usize,
}

impl AdmissionReport {
    fn one(outcome: AdmissionOutcome) -> Self {
        let mut report = Self {
            outcomes: [AdmissionOutcome::Malformed; MAX_ENVELOPE_TRANSITIONS],
            count: 1,
        };
        report.outcomes[0] = outcome;
        report
    }

    pub fn len(self) -> usize {
        self.count
    }

    pub fn is_empty(self) -> bool {
        self.count == 0
    }

    pub fn outcomes(&self) -> &[AdmissionOutcome] {
        &self.outcomes[..self.count]
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    #[error("control envelope has an invalid transition")]
    InvalidTransition,
    #[error("control envelope is full")]
    Full,
    #[error("control envelope has a duplicate transition identity")]
    Duplicate,
    #[error("control envelope has a conflicting transition identity")]
    Conflict,
    #[error("control envelope is malformed")]
    Malformed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlEnvelope {
    count: usize,
    transitions: [ScheduledTransition; MAX_ENVELOPE_TRANSITIONS],
}

impl Default for ControlEnvelope {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlEnvelope {
    pub const fn new() -> Self {
        Self {
            count: 0,
            transitions: [ScheduledTransition {
                seat: 0,
                epoch: 0,
                sequence: 0,
                apply_tick: 0,
                state: ControlState::neutral(),
            }; MAX_ENVELOPE_TRANSITIONS],
        }
    }

    pub fn from_transition(transition: ScheduledTransition) -> Result<Self, EnvelopeError> {
        let mut envelope = Self::new();
        envelope.push(transition)?;
        Ok(envelope)
    }

    pub fn push(&mut self, transition: ScheduledTransition) -> Result<(), EnvelopeError> {
        validate_transition(transition)?;
        for existing in self.transitions.iter().take(self.count) {
            if same_sequence(*existing, transition) {
                if same_identity(*existing, transition) && *existing == transition {
                    return Err(EnvelopeError::Duplicate);
                }
                return Err(EnvelopeError::Conflict);
            }
        }
        if self.count == MAX_ENVELOPE_TRANSITIONS {
            return Err(EnvelopeError::Full);
        }
        self.transitions[self.count] = transition;
        self.count += 1;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn transitions(&self) -> &[ScheduledTransition] {
        &self.transitions[..self.count]
    }

    /// Encode the bounded transport payload.  The production network layer
    /// can pass these bytes through its existing message envelope unchanged.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ENVELOPE_HEADER_BYTES + self.count * TRANSITION_BYTES);
        bytes.extend_from_slice(&REALTIME_ENVELOPE_MAGIC);
        bytes.extend_from_slice(&REALTIME_ENVELOPE_VERSION.to_be_bytes());
        bytes.push(self.count as u8);
        bytes.push(0);
        for transition in self.transitions() {
            encode_transition(*transition, &mut bytes);
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        if bytes.len() < ENVELOPE_HEADER_BYTES || bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(EnvelopeError::Malformed);
        }
        if bytes[..4] != REALTIME_ENVELOPE_MAGIC
            || u16::from_be_bytes([bytes[4], bytes[5]]) != REALTIME_ENVELOPE_VERSION
            || bytes[7] != 0
        {
            return Err(EnvelopeError::Malformed);
        }
        let count = bytes[6] as usize;
        if count > MAX_ENVELOPE_TRANSITIONS
            || bytes.len() != ENVELOPE_HEADER_BYTES + count * TRANSITION_BYTES
        {
            return Err(EnvelopeError::Malformed);
        }
        let mut envelope = Self::new();
        for index in 0..count {
            let start = ENVELOPE_HEADER_BYTES + index * TRANSITION_BYTES;
            envelope.push(decode_transition(&bytes[start..start + TRANSITION_BYTES])?)?;
        }
        Ok(envelope)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickAdvance {
    pub tick: u64,
    controls: [ControlState; REALTIME_MAX_SEATS],
    applied: [ScheduledTransition; MAX_PENDING_TRANSITIONS],
    applied_count: usize,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TickError {
    #[error("authoritative simulation tick is exhausted")]
    Exhausted,
}

impl TickAdvance {
    pub fn controls(&self) -> &[ControlState; REALTIME_MAX_SEATS] {
        &self.controls
    }

    pub fn control(&self, seat: usize) -> Option<ControlState> {
        self.controls.get(seat).copied()
    }

    pub fn applied_transitions(&self) -> &[ScheduledTransition] {
        &self.applied[..self.applied_count]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthoritativeSnapshot {
    pub revision: u64,
    pub tick: u64,
    pub controls: [ControlState; REALTIME_MAX_SEATS],
    pub sequences: [u32; REALTIME_MAX_SEATS],
    pub epochs: [u32; REALTIME_MAX_SEATS],
    pub active: [bool; REALTIME_MAX_SEATS],
    pub game_token: [u8; MAX_SNAPSHOT_TOKEN_BYTES],
    pub game_token_len: u8,
}

impl AuthoritativeSnapshot {
    pub const fn new(
        revision: u64,
        tick: u64,
        controls: [ControlState; REALTIME_MAX_SEATS],
        sequences: [u32; REALTIME_MAX_SEATS],
        epochs: [u32; REALTIME_MAX_SEATS],
        active: [bool; REALTIME_MAX_SEATS],
    ) -> Self {
        Self {
            revision,
            tick,
            controls,
            sequences,
            epochs,
            active,
            game_token: [0; MAX_SNAPSHOT_TOKEN_BYTES],
            game_token_len: 0,
        }
    }

    pub fn with_game_token(mut self, token: &[u8]) -> Result<Self, SnapshotError> {
        if token.len() > MAX_SNAPSHOT_TOKEN_BYTES {
            return Err(SnapshotError::TokenTooLarge);
        }
        self.game_token[..token.len()].copy_from_slice(token);
        self.game_token_len = token.len() as u8;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    Disconnect { seat: u8 },
    Reconnect { seat: u8 },
    Pause,
    FocusLost,
    Rematch,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleError {
    #[error("seat is outside the configured seat count")]
    InvalidSeat,
    #[error("replay record limit is outside the bounded range")]
    InvalidReplayLimit,
    #[error("seat epoch is exhausted")]
    EpochExhausted,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("snapshot revision must be newer than the current revision")]
    RevisionRegression,
    #[error("snapshot tick does not leave room for bounded future scheduling")]
    TickOutOfRange,
    #[error("snapshot tick regresses within an unchanged seat epoch")]
    TickRegression,
    #[error("snapshot seat epoch regresses")]
    EpochRegression,
    #[error("snapshot seat epoch cannot be advanced safely")]
    EpochExhausted,
    #[error("snapshot sequence floor regresses within an unchanged seat epoch")]
    SequenceRegression,
    #[error("snapshot contains state for inactive seat {seat}")]
    InactiveSeatState { seat: usize },
    #[error("snapshot token is too large")]
    TokenTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayRecord {
    Scheduled(ScheduledTransition),
    Tick {
        tick: u64,
        controls: [ControlState; REALTIME_MAX_SEATS],
        authoritative_hash: Option<u64>,
    },
    Reset(ResetReason),
    Snapshot(AuthoritativeSnapshot),
    Quarantine {
        seat: u8,
        epoch: u32,
        sequence: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayEvent {
    Reset(ResetReason),
    Snapshot(AuthoritativeSnapshot),
    Quarantine { seat: u8, epoch: u32, sequence: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayLog {
    records: Vec<ReplayRecord>,
    limit: usize,
    complete: bool,
}

impl Default for ReplayLog {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayLog {
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_REPLAY_RECORDS).expect("default replay limit is valid")
    }

    pub fn with_limit(limit: usize) -> Result<Self, LifecycleError> {
        if limit == 0 || limit > MAX_REPLAY_RECORDS {
            return Err(LifecycleError::InvalidReplayLimit);
        }
        Ok(Self {
            records: Vec::new(),
            limit,
            complete: true,
        })
    }

    fn push(&mut self, record: ReplayRecord) -> bool {
        if self.records.len() == self.limit {
            self.complete = false;
            return false;
        }
        self.records.push(record);
        true
    }

    fn record_hash(&mut self, tick: u64, hash: u64) -> Result<(), HashRecordError> {
        let Some(ReplayRecord::Tick {
            tick: latest_tick,
            authoritative_hash,
            ..
        }) = self.records.last_mut()
        else {
            return Err(HashRecordError::NoUnfinishedTick);
        };
        if *latest_tick != tick {
            return Err(HashRecordError::WrongTick {
                expected: *latest_tick,
                received: tick,
            });
        }
        if authoritative_hash.is_some() {
            return Err(HashRecordError::AlreadyRecorded);
        }
        *authoritative_hash = Some(hash);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn records(&self) -> impl Iterator<Item = &ReplayRecord> {
        self.records.iter()
    }

    /// Replay the control schedule and verify each caller-supplied hash.
    /// Hashing remains the game's responsibility; the callback receives the
    /// exact post-tick control snapshot used by the simulation.
    pub fn replay<F>(
        &self,
        config: RealtimeConfig,
        seats: usize,
        hash: F,
    ) -> Result<(), ReplayError>
    where
        F: FnMut(u64, &[ControlState; REALTIME_MAX_SEATS]) -> u64,
    {
        self.replay_with_events(config, seats, |_| {}, hash)
    }

    pub fn replay_with_events<E, F>(
        &self,
        config: RealtimeConfig,
        seats: usize,
        mut event: E,
        mut hash: F,
    ) -> Result<(), ReplayError>
    where
        E: FnMut(ReplayEvent),
        F: FnMut(u64, &[ControlState; REALTIME_MAX_SEATS]) -> u64,
    {
        if !self.complete {
            return Err(ReplayError::Incomplete);
        }
        let mut session =
            RealtimeSession::new(config, seats).map_err(|_| ReplayError::InvalidSetup)?;
        for (index, record) in self.records().enumerate() {
            match *record {
                ReplayRecord::Scheduled(transition) => {
                    let outcome = session.submit_transition(transition);
                    if !matches!(
                        outcome,
                        AdmissionOutcome::Accepted | AdmissionOutcome::AcceptedReordered
                    ) {
                        return Err(ReplayError::Admission { index, outcome });
                    }
                }
                ReplayRecord::Tick {
                    tick,
                    controls,
                    authoritative_hash,
                } => {
                    let advance = session
                        .advance_tick()
                        .map_err(|_| ReplayError::TickExhausted { index })?;
                    if advance.tick != tick || advance.controls != controls {
                        return Err(ReplayError::TickMismatch { index, tick });
                    }
                    let Some(expected) = authoritative_hash else {
                        return Err(ReplayError::MissingHash { index, tick });
                    };
                    let observed = hash(tick, &controls);
                    if observed != expected {
                        return Err(ReplayError::HashMismatch {
                            index,
                            tick,
                            expected,
                            observed,
                        });
                    }
                }
                ReplayRecord::Reset(reason) => {
                    event(ReplayEvent::Reset(reason));
                    session.apply_reset(reason)?;
                }
                ReplayRecord::Snapshot(snapshot) => {
                    event(ReplayEvent::Snapshot(snapshot));
                    session.apply_snapshot(snapshot);
                }
                ReplayRecord::Quarantine {
                    seat,
                    epoch,
                    sequence,
                } => {
                    event(ReplayEvent::Quarantine {
                        seat,
                        epoch,
                        sequence,
                    });
                    if !session.quarantine_identity(seat, epoch, sequence) {
                        return Err(ReplayError::InvalidSetup);
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HashRecordError {
    #[error("there is no immediately preceding unfinished simulation tick")]
    NoUnfinishedTick,
    #[error("hash belongs to tick {expected}, not tick {received}")]
    WrongTick { expected: u64, received: u64 },
    #[error("the current simulation tick already has a hash")]
    AlreadyRecorded,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    #[error("replay log is incomplete")]
    Incomplete,
    #[error("replay setup is invalid")]
    InvalidSetup,
    #[error("replay admission failed at record {index}: {outcome:?}")]
    Admission {
        index: usize,
        outcome: AdmissionOutcome,
    },
    #[error("replay tick {tick} differs at record {index}")]
    TickMismatch { index: usize, tick: u64 },
    #[error("replay tick could not advance at record {index}")]
    TickExhausted { index: usize },
    #[error("replay tick {tick} has no authoritative hash at record {index}")]
    MissingHash { index: usize, tick: u64 },
    #[error("replay hash differs at tick {tick}: expected {expected}, observed {observed}")]
    HashMismatch {
        index: usize,
        tick: u64,
        expected: u64,
        observed: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeSession {
    config: RealtimeConfig,
    seats: usize,
    current_tick: u64,
    snapshot_revision: u64,
    controls: [ControlState; REALTIME_MAX_SEATS],
    epochs: [u32; REALTIME_MAX_SEATS],
    active: [bool; REALTIME_MAX_SEATS],
    highest_applied_sequence: [u32; REALTIME_MAX_SEATS],
    highest_sequence: [u32; REALTIME_MAX_SEATS],
    quarantined: [(u8, u32, u32); MAX_PENDING_TRANSITIONS],
    quarantined_count: usize,
    resync_required: bool,
    pending: [Option<ScheduledTransition>; MAX_PENDING_TRANSITIONS],
    pending_count: usize,
    recent: [Option<ScheduledTransition>; MAX_PENDING_TRANSITIONS],
    recent_next: usize,
    replay: ReplayLog,
}

impl RealtimeSession {
    pub fn new(config: RealtimeConfig, seats: usize) -> Result<Self, LifecycleError> {
        Self::new_with_replay_limit(config, seats, DEFAULT_REPLAY_RECORDS)
    }

    pub fn new_with_replay_limit(
        config: RealtimeConfig,
        seats: usize,
        replay_limit: usize,
    ) -> Result<Self, LifecycleError> {
        if seats == 0 || seats > REALTIME_MAX_SEATS {
            return Err(LifecycleError::InvalidSeat);
        }
        let mut session = Self {
            config,
            seats,
            current_tick: 0,
            snapshot_revision: 0,
            controls: [ControlState::neutral(); REALTIME_MAX_SEATS],
            epochs: [1; REALTIME_MAX_SEATS],
            active: [false; REALTIME_MAX_SEATS],
            highest_applied_sequence: [0; REALTIME_MAX_SEATS],
            highest_sequence: [0; REALTIME_MAX_SEATS],
            quarantined: [(0, 0, 0); MAX_PENDING_TRANSITIONS],
            quarantined_count: 0,
            resync_required: false,
            pending: [None; MAX_PENDING_TRANSITIONS],
            pending_count: 0,
            recent: [None; MAX_PENDING_TRANSITIONS],
            recent_next: 0,
            replay: ReplayLog::with_limit(replay_limit)?,
        };
        session.active[..seats].fill(true);
        Ok(session)
    }

    pub const fn config(&self) -> RealtimeConfig {
        self.config
    }

    pub const fn seats(&self) -> usize {
        self.seats
    }

    pub const fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub const fn snapshot_revision(&self) -> u64 {
        self.snapshot_revision
    }

    pub fn epoch(&self, seat: usize) -> Option<u32> {
        self.epochs.get(seat).copied()
    }

    pub fn is_active(&self, seat: usize) -> Option<bool> {
        self.active.get(seat).copied()
    }

    pub const fn resync_required(&self) -> bool {
        self.resync_required
    }

    pub fn controls(&self) -> &[ControlState; REALTIME_MAX_SEATS] {
        &self.controls
    }

    pub fn control(&self, seat: usize) -> Option<ControlState> {
        self.controls.get(seat).copied()
    }

    pub fn pending_len(&self) -> usize {
        self.pending_count
    }

    pub fn replay(&self) -> &ReplayLog {
        &self.replay
    }

    pub fn earliest_apply_tick(&self) -> u64 {
        self.current_tick
            .saturating_add(u64::from(self.config.input_delay_ticks))
    }

    pub fn latest_apply_tick(&self) -> u64 {
        self.current_tick.saturating_add(MAX_FUTURE_TICKS)
    }

    pub fn submit_transition(&mut self, transition: ScheduledTransition) -> AdmissionOutcome {
        if validate_transition(transition).is_err() || transition.seat as usize >= self.seats {
            return AdmissionOutcome::Malformed;
        }
        if self.resync_required {
            return AdmissionOutcome::ResyncRequired;
        }
        let seat = transition.seat as usize;
        if !self.active[seat] {
            return AdmissionOutcome::Inactive;
        }
        if transition.epoch != self.epochs[seat] {
            return AdmissionOutcome::Stale;
        }
        if transition.apply_tick < self.earliest_apply_tick() {
            return AdmissionOutcome::Late;
        }
        if transition.apply_tick > self.latest_apply_tick() {
            return AdmissionOutcome::TooFar;
        }
        if self
            .quarantined
            .iter()
            .take(self.quarantined_count)
            .any(|(seat, epoch, sequence)| {
                *seat == transition.seat
                    && *epoch == transition.epoch
                    && *sequence == transition.sequence
            })
        {
            return AdmissionOutcome::Conflict;
        }
        for existing in self.pending.iter().flatten() {
            if same_sequence(*existing, transition) {
                if same_identity(*existing, transition) && *existing == transition {
                    return AdmissionOutcome::Duplicate;
                }
                if !self.quarantine_identity(transition.seat, transition.epoch, transition.sequence)
                {
                    self.resync_required = true;
                    return AdmissionOutcome::ResyncRequired;
                }
                self.replay.push(ReplayRecord::Quarantine {
                    seat: transition.seat,
                    epoch: transition.epoch,
                    sequence: transition.sequence,
                });
                return AdmissionOutcome::Conflict;
            }
        }
        for existing in self.recent.iter().flatten() {
            if same_sequence(*existing, transition) {
                if same_identity(*existing, transition) && *existing == transition {
                    return AdmissionOutcome::Duplicate;
                }
                return if transition.apply_tick <= self.current_tick {
                    AdmissionOutcome::Late
                } else {
                    AdmissionOutcome::Stale
                };
            }
        }
        if transition.sequence <= self.highest_applied_sequence[seat] {
            return AdmissionOutcome::Stale;
        }
        if self.quarantined_count == MAX_PENDING_TRANSITIONS {
            self.resync_required = true;
            return AdmissionOutcome::ResyncRequired;
        }
        if self.pending_count == MAX_PENDING_TRANSITIONS || !self.replay.has_capacity() {
            return AdmissionOutcome::Full;
        }
        let reordered = transition.sequence < self.highest_sequence[seat];
        let slot = self
            .pending
            .iter()
            .position(Option::is_none)
            .expect("pending_count and slots remain consistent");
        self.pending[slot] = Some(transition);
        self.pending_count += 1;
        self.highest_sequence[seat] = self.highest_sequence[seat].max(transition.sequence);
        self.replay.push(ReplayRecord::Scheduled(transition));
        if reordered {
            AdmissionOutcome::AcceptedReordered
        } else {
            AdmissionOutcome::Accepted
        }
    }

    pub fn submit_envelope(&mut self, envelope: &ControlEnvelope) -> AdmissionReport {
        let mut report = AdmissionReport {
            outcomes: [AdmissionOutcome::Malformed; MAX_ENVELOPE_TRANSITIONS],
            count: envelope.count,
        };
        for (index, transition) in envelope.transitions().iter().enumerate() {
            report.outcomes[index] = self.submit_transition(*transition);
        }
        report
    }

    pub fn submit_payload(&mut self, payload: &[u8]) -> AdmissionReport {
        match ControlEnvelope::decode(payload) {
            Ok(envelope) => self.submit_envelope(&envelope),
            Err(_) => AdmissionReport::one(AdmissionOutcome::Malformed),
        }
    }

    /// Advance exactly one authoritative simulation tick.  No network read,
    /// packet wait, or rendering operation occurs here.
    pub fn advance_tick(&mut self) -> Result<TickAdvance, TickError> {
        self.advance_tick_inner()
    }

    /// Attach a hash computed from the returned post-tick state to the
    /// immediately preceding tick record.
    pub fn record_authoritative_hash(
        &mut self,
        tick: u64,
        authoritative_hash: u64,
    ) -> Result<(), HashRecordError> {
        self.replay.record_hash(tick, authoritative_hash)
    }

    pub fn disconnect(&mut self, seat: usize) -> Result<(), LifecycleError> {
        self.lifecycle_epoch(seat, false, ResetReason::Disconnect { seat: seat as u8 })
    }

    pub fn reconnect(&mut self, seat: usize) -> Result<(), LifecycleError> {
        self.lifecycle_epoch(seat, true, ResetReason::Reconnect { seat: seat as u8 })
    }

    pub fn pause(&mut self) -> Result<(), LifecycleError> {
        self.reset_all(ResetReason::Pause)
    }

    pub fn focus_lost(&mut self) -> Result<(), LifecycleError> {
        self.reset_all(ResetReason::FocusLost)
    }

    pub fn rematch(&mut self) -> Result<(), LifecycleError> {
        self.reset_all(ResetReason::Rematch)
    }

    pub fn apply_authoritative_snapshot(
        &mut self,
        snapshot: AuthoritativeSnapshot,
    ) -> Result<(), SnapshotError> {
        if snapshot.revision == 0 || snapshot.revision <= self.snapshot_revision {
            return Err(SnapshotError::RevisionRegression);
        }
        if snapshot.tick > u64::MAX - MAX_FUTURE_TICKS {
            return Err(SnapshotError::TickOutOfRange);
        }
        for seat in 0..REALTIME_MAX_SEATS {
            if (seat >= self.seats || !snapshot.active[seat])
                && (snapshot.controls[seat] != ControlState::neutral()
                    || snapshot.sequences[seat] != 0
                    || snapshot.active[seat])
            {
                return Err(SnapshotError::InactiveSeatState { seat });
            }
        }
        if snapshot.game_token_len as usize > MAX_SNAPSHOT_TOKEN_BYTES {
            return Err(SnapshotError::TokenTooLarge);
        }
        for seat in 0..self.seats {
            if snapshot.epochs[seat] == u32::MAX {
                return Err(SnapshotError::EpochExhausted);
            }
            if snapshot.epochs[seat] < self.epochs[seat] {
                return Err(SnapshotError::EpochRegression);
            }
            if snapshot.epochs[seat] == self.epochs[seat] {
                if snapshot.tick < self.current_tick {
                    return Err(SnapshotError::TickRegression);
                }
                if snapshot.sequences[seat] < self.highest_sequence[seat] {
                    return Err(SnapshotError::SequenceRegression);
                }
            }
        }
        self.apply_snapshot(snapshot);
        self.replay.push(ReplayRecord::Snapshot(snapshot));
        Ok(())
    }

    fn advance_tick_inner(&mut self) -> Result<TickAdvance, TickError> {
        self.current_tick = self
            .current_tick
            .checked_add(1)
            .ok_or(TickError::Exhausted)?;
        let mut due = [0usize; MAX_PENDING_TRANSITIONS];
        let mut due_count = 0;
        for (index, pending) in self.pending.iter().enumerate() {
            if pending.is_some_and(|transition| transition.apply_tick == self.current_tick) {
                due[due_count] = index;
                due_count += 1;
            }
        }
        due[..due_count].sort_by_key(|index| {
            let transition = self.pending[*index].expect("due index is populated");
            (transition.seat, transition.sequence, transition.apply_tick)
        });
        let mut applied = [ScheduledTransition::default(); MAX_PENDING_TRANSITIONS];
        for (applied_index, index) in due.into_iter().take(due_count).enumerate() {
            let transition = self.pending[index].take().expect("due index is populated");
            self.pending_count -= 1;
            self.controls[transition.seat as usize] = transition.state;
            let seat = transition.seat as usize;
            self.highest_applied_sequence[seat] =
                self.highest_applied_sequence[seat].max(transition.sequence);
            self.compact_quarantine(seat);
            self.recent[self.recent_next] = Some(transition);
            self.recent_next = (self.recent_next + 1) % MAX_PENDING_TRANSITIONS;
            applied[applied_index] = transition;
        }
        let applied_count = due_count;
        let advance = TickAdvance {
            tick: self.current_tick,
            controls: self.controls,
            applied,
            applied_count,
        };
        self.replay.push(ReplayRecord::Tick {
            tick: advance.tick,
            controls: advance.controls,
            authoritative_hash: None,
        });
        Ok(advance)
    }

    fn lifecycle_epoch(
        &mut self,
        seat: usize,
        active: bool,
        reason: ResetReason,
    ) -> Result<(), LifecycleError> {
        if seat >= self.seats {
            return Err(LifecycleError::InvalidSeat);
        }
        let next_epoch = self.next_epoch(seat)?;
        self.epochs[seat] = next_epoch;
        self.active[seat] = active;
        self.controls[seat] = ControlState::neutral();
        self.remove_pending_seat(seat);
        self.highest_sequence[seat] = 0;
        self.highest_applied_sequence[seat] = 0;
        self.recent
            .iter_mut()
            .filter(|entry| entry.is_some_and(|t| t.seat as usize == seat))
            .for_each(|entry| *entry = None);
        let mut retained = [(0, 0, 0); MAX_PENDING_TRANSITIONS];
        let mut retained_count = 0;
        for entry in self.quarantined.iter().take(self.quarantined_count) {
            if entry.0 as usize != seat {
                retained[retained_count] = *entry;
                retained_count += 1;
            }
        }
        self.quarantined = retained;
        self.quarantined_count = retained_count;
        self.replay.push(ReplayRecord::Reset(reason));
        Ok(())
    }

    fn reset_all(&mut self, reason: ResetReason) -> Result<(), LifecycleError> {
        let mut next_epochs = self.epochs;
        for (seat, next_epoch) in next_epochs.iter_mut().enumerate().take(self.seats) {
            *next_epoch = self.next_epoch(seat)?;
        }
        self.epochs = next_epochs;
        if reason == ResetReason::Rematch {
            self.current_tick = 0;
            self.active[..self.seats].fill(true);
        }
        self.controls = [ControlState::neutral(); REALTIME_MAX_SEATS];
        self.pending = [None; MAX_PENDING_TRANSITIONS];
        self.pending_count = 0;
        for seat in 0..self.seats {
            self.highest_sequence[seat] = 0;
            self.highest_applied_sequence[seat] = 0;
        }
        self.quarantined = [(0, 0, 0); MAX_PENDING_TRANSITIONS];
        self.quarantined_count = 0;
        self.resync_required = false;
        self.recent = [None; MAX_PENDING_TRANSITIONS];
        self.recent_next = 0;
        self.replay.push(ReplayRecord::Reset(reason));
        Ok(())
    }

    fn remove_pending_seat(&mut self, seat: usize) {
        for pending in &mut self.pending {
            if pending.is_some_and(|transition| transition.seat as usize == seat) {
                *pending = None;
                self.pending_count -= 1;
            }
        }
    }

    fn apply_reset(&mut self, reason: ResetReason) -> Result<(), ReplayError> {
        match reason {
            ResetReason::Disconnect { seat } | ResetReason::Reconnect { seat } => self
                .lifecycle_epoch(
                    seat as usize,
                    matches!(reason, ResetReason::Reconnect { .. }),
                    reason,
                )
                .map_err(|_| ReplayError::InvalidSetup),
            ResetReason::Pause | ResetReason::FocusLost | ResetReason::Rematch => self
                .reset_all(reason)
                .map_err(|_| ReplayError::InvalidSetup),
        }
    }

    fn quarantine_identity(&mut self, seat: u8, epoch: u32, sequence: u32) -> bool {
        self.remove_pending_sequence(seat, epoch, sequence);
        if self
            .quarantined
            .iter()
            .take(self.quarantined_count)
            .any(|identity| *identity == (seat, epoch, sequence))
        {
            return true;
        }
        if self.quarantined_count < MAX_PENDING_TRANSITIONS {
            self.quarantined[self.quarantined_count] = (seat, epoch, sequence);
            self.quarantined_count += 1;
            return true;
        }
        false
    }

    fn remove_pending_sequence(&mut self, seat: u8, epoch: u32, sequence: u32) {
        for pending in &mut self.pending {
            if pending.is_some_and(|transition| {
                transition.seat == seat
                    && transition.epoch == epoch
                    && transition.sequence == sequence
            }) {
                *pending = None;
                self.pending_count -= 1;
            }
        }
    }

    fn next_epoch(&self, seat: usize) -> Result<u32, LifecycleError> {
        self.epochs[seat]
            .checked_add(1)
            .ok_or(LifecycleError::EpochExhausted)
    }

    fn compact_quarantine(&mut self, seat: usize) {
        let floor = self.highest_applied_sequence[seat];
        let epoch = self.epochs[seat];
        let mut retained = [(0, 0, 0); MAX_PENDING_TRANSITIONS];
        let mut retained_count = 0;
        for identity in self.quarantined.iter().take(self.quarantined_count) {
            if !(identity.0 as usize == seat && identity.1 == epoch && identity.2 <= floor) {
                retained[retained_count] = *identity;
                retained_count += 1;
            }
        }
        self.quarantined = retained;
        self.quarantined_count = retained_count;
    }

    fn apply_snapshot(&mut self, snapshot: AuthoritativeSnapshot) {
        self.snapshot_revision = snapshot.revision;
        self.current_tick = snapshot.tick;
        self.controls = snapshot.controls;
        self.highest_applied_sequence = snapshot.sequences;
        self.highest_sequence = snapshot.sequences;
        self.epochs = snapshot.epochs;
        self.active = snapshot.active;
        self.pending = [None; MAX_PENDING_TRANSITIONS];
        self.pending_count = 0;
        self.recent = [None; MAX_PENDING_TRANSITIONS];
        self.recent_next = 0;
        self.quarantined = [(0, 0, 0); MAX_PENDING_TRANSITIONS];
        self.quarantined_count = 0;
        self.resync_required = false;
    }
}

impl ReplayLog {
    fn has_capacity(&self) -> bool {
        self.records.len() < self.limit
    }
}

fn validate_transition(transition: ScheduledTransition) -> Result<(), EnvelopeError> {
    if transition.seat as usize >= REALTIME_MAX_SEATS
        || transition.epoch == 0
        || transition.sequence == 0
    {
        return Err(EnvelopeError::InvalidTransition);
    }
    Ok(())
}

fn same_identity(left: ScheduledTransition, right: ScheduledTransition) -> bool {
    left.seat == right.seat
        && left.epoch == right.epoch
        && left.sequence == right.sequence
        && left.apply_tick == right.apply_tick
}

fn same_sequence(left: ScheduledTransition, right: ScheduledTransition) -> bool {
    left.seat == right.seat && left.epoch == right.epoch && left.sequence == right.sequence
}

fn encode_transition(transition: ScheduledTransition, bytes: &mut Vec<u8>) {
    bytes.push(transition.seat);
    bytes.extend_from_slice(&transition.epoch.to_be_bytes());
    bytes.extend_from_slice(&transition.sequence.to_be_bytes());
    bytes.extend_from_slice(&transition.apply_tick.to_be_bytes());
    bytes.extend_from_slice(&transition.state.buttons.to_be_bytes());
    bytes.push(transition.state.axis_x as u8);
    bytes.push(transition.state.axis_y as u8);
}

fn decode_transition(bytes: &[u8]) -> Result<ScheduledTransition, EnvelopeError> {
    if bytes.len() != TRANSITION_BYTES {
        return Err(EnvelopeError::Malformed);
    }
    let transition = ScheduledTransition {
        seat: bytes[0],
        epoch: u32::from_be_bytes(bytes[1..5].try_into().expect("fixed transition width")),
        sequence: u32::from_be_bytes(bytes[5..9].try_into().expect("fixed transition width")),
        apply_tick: u64::from_be_bytes(bytes[9..17].try_into().expect("fixed transition width")),
        state: ControlState {
            buttons: u16::from_be_bytes(bytes[17..19].try_into().expect("fixed transition width")),
            axis_x: bytes[19] as i8,
            axis_y: bytes[20] as i8,
        },
    };
    validate_transition(transition)?;
    Ok(transition)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_exhaustion_does_not_emit_duplicate_max_tick() {
        let config = RealtimeConfig::new(60, 30, 120, 1).expect("config");
        let mut session = RealtimeSession::new(config, 1).expect("session");
        session.current_tick = u64::MAX - 1;
        assert_eq!(session.advance_tick().expect("maximum tick").tick, u64::MAX);
        assert_eq!(session.advance_tick(), Err(TickError::Exhausted));
        assert_eq!(
            session
                .replay
                .records()
                .filter(|record| matches!(record, ReplayRecord::Tick { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn all_seat_reset_preflights_epoch_exhaustion() {
        let config = RealtimeConfig::new(60, 60, 60, 1).expect("config");
        let mut session = RealtimeSession::new(config, 2).expect("session");
        session.epochs[1] = u32::MAX;
        let before = session.clone();
        assert_eq!(session.pause(), Err(LifecycleError::EpochExhausted));
        assert_eq!(session, before);
    }
}

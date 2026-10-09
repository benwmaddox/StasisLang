use crate::live::{LiveCommand, LivePointerInput, LiveRequest};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const SUPERVISED_LIVE_SCHEMA_VERSION: u16 = 1;
pub const MAX_SUPERVISED_LIVE_LINE_BYTES: usize = 4096;
pub const MAX_SUPERVISED_LIVE_REQUESTS: u32 = 512;
pub const MAX_SUPERVISED_LIVE_REQUESTS_PER_SECOND: u16 = 120;
pub const MAX_SUPERVISED_LIVE_POINTERS: usize = 8;
pub const MAX_SUPERVISED_LIVE_CAPTURES: u8 = 16;
pub const MAX_SUPERVISED_LIVE_CAPTURE_BYTES: u32 = 16 * 1024 * 1024;
pub const MAX_TOTAL_SUPERVISED_LIVE_CAPTURE_BYTES: u64 =
    MAX_SUPERVISED_LIVE_CAPTURE_BYTES as u64 * MAX_SUPERVISED_LIVE_CAPTURES as u64;
pub const MAX_SUPERVISED_LIVE_CAPTURE_WIDTH: u16 = 1920;
pub const MAX_SUPERVISED_LIVE_CAPTURE_HEIGHT: u16 = 1080;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisedLiveRequest {
    pub schema_version: u16,
    pub request_id: u32,
    pub command: SupervisedLiveCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SupervisedLiveCommand {
    Pause,
    Resume,
    Step { ticks: u8 },
    SetInputState { pointers: Vec<LivePointerInput> },
    CaptureFrame { artifact_id: u8 },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisedLiveEvent {
    Paused,
    Resumed,
    StepScheduled,
    InputApplied,
    CaptureSaved,
    Quit,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisedLiveErrorCode {
    InvalidRequest,
    RequestLimit,
    RateLimited,
    StartupFailed,
    RuntimeFailed,
    CaptureRejected,
    ChildExited,
    TimedOut,
    ShutdownFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisedLiveCapture {
    pub artifact_id: u8,
    pub width: u16,
    pub height: u16,
    pub byte_length: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisedLiveResponse {
    pub schema_version: u16,
    pub request_id: u32,
    pub tick: u64,
    pub ok: bool,
    pub event: SupervisedLiveEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<SupervisedLiveCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<SupervisedLiveErrorCode>,
}

impl SupervisedLiveRequest {
    pub fn parse_line(line: &[u8]) -> Result<Self, SupervisedLiveErrorCode> {
        if line.is_empty()
            || line.len() > MAX_SUPERVISED_LIVE_LINE_BYTES
            || line.iter().any(|byte| matches!(byte, b'\n' | b'\r'))
        {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        let shape: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| SupervisedLiveErrorCode::InvalidRequest)?;
        let Some(envelope) = shape.as_object() else {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        };
        if envelope.len() != 3
            || !envelope.contains_key("schema_version")
            || !envelope.contains_key("request_id")
            || !envelope.contains_key("command")
        {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        let Some(command) = envelope
            .get("command")
            .and_then(serde_json::Value::as_object)
        else {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        };
        let Some(command_type) = command.get("type").and_then(serde_json::Value::as_str) else {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        };
        let expected_fields: &[&str] = match command_type {
            "pause" | "resume" | "quit" => &["type"],
            "step" => &["type", "ticks"],
            "set_input_state" => &["type", "pointers"],
            "capture_frame" => &["type", "artifact_id"],
            _ => return Err(SupervisedLiveErrorCode::InvalidRequest),
        };
        if command.len() != expected_fields.len()
            || command
                .keys()
                .any(|key| !expected_fields.contains(&key.as_str()))
        {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        let request: Self =
            serde_json::from_slice(line).map_err(|_| SupervisedLiveErrorCode::InvalidRequest)?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), SupervisedLiveErrorCode> {
        if self.schema_version != SUPERVISED_LIVE_SCHEMA_VERSION || self.request_id == 0 {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        match &self.command {
            SupervisedLiveCommand::Step { ticks } if *ticks != 1 => {
                Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            SupervisedLiveCommand::SetInputState { pointers }
                if pointers.len() > MAX_SUPERVISED_LIVE_POINTERS =>
            {
                Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            SupervisedLiveCommand::SetInputState { pointers } if !valid_pointers(pointers) => {
                Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            SupervisedLiveCommand::CaptureFrame { artifact_id }
                if *artifact_id >= MAX_SUPERVISED_LIVE_CAPTURES =>
            {
                Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            _ => Ok(()),
        }
    }

    pub fn to_live_request(&self) -> LiveRequest {
        let command = match &self.command {
            SupervisedLiveCommand::Pause => LiveCommand::Pause,
            SupervisedLiveCommand::Resume => LiveCommand::Resume,
            SupervisedLiveCommand::Step { ticks } => LiveCommand::Step {
                ticks: u32::from(*ticks),
            },
            SupervisedLiveCommand::SetInputState { pointers } => LiveCommand::SetInputState {
                pointers: pointers.clone(),
            },
            SupervisedLiveCommand::CaptureFrame { artifact_id } => LiveCommand::CaptureFrame {
                artifact: format!("supervised-capture-{artifact_id:02}"),
            },
            SupervisedLiveCommand::Quit => LiveCommand::Quit,
        };
        LiveRequest::new(u64::from(self.request_id), command)
    }
}

impl SupervisedLiveResponse {
    pub fn validate(&self) -> Result<(), SupervisedLiveErrorCode> {
        if self.schema_version != SUPERVISED_LIVE_SCHEMA_VERSION || self.request_id == 0 {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        match (self.ok, self.event, self.error_code, &self.capture) {
            (true, SupervisedLiveEvent::Rejected, _, _) | (true, _, Some(_), _) => {
                return Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            (false, SupervisedLiveEvent::Rejected, Some(_), None) => {}
            (false, _, _, _) => return Err(SupervisedLiveErrorCode::InvalidRequest),
            (true, SupervisedLiveEvent::CaptureSaved, None, Some(capture)) => {
                if capture.artifact_id >= MAX_SUPERVISED_LIVE_CAPTURES
                    || capture.width == 0
                    || capture.width > MAX_SUPERVISED_LIVE_CAPTURE_WIDTH
                    || capture.height == 0
                    || capture.height > MAX_SUPERVISED_LIVE_CAPTURE_HEIGHT
                    || capture.byte_length == 0
                    || capture.byte_length > MAX_SUPERVISED_LIVE_CAPTURE_BYTES
                    || capture.sha256.len() != 64
                    || !capture
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                {
                    return Err(SupervisedLiveErrorCode::InvalidRequest);
                }
            }
            (true, SupervisedLiveEvent::CaptureSaved, _, None) | (true, _, _, Some(_)) => {
                return Err(SupervisedLiveErrorCode::InvalidRequest)
            }
            (true, _, None, None) => {}
        }
        Ok(())
    }

    pub fn validate_for_request(
        &self,
        request: &SupervisedLiveRequest,
    ) -> Result<(), SupervisedLiveErrorCode> {
        self.validate()?;
        if self.request_id != request.request_id {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        let expected = match &request.command {
            SupervisedLiveCommand::Pause => SupervisedLiveEvent::Paused,
            SupervisedLiveCommand::Resume => SupervisedLiveEvent::Resumed,
            SupervisedLiveCommand::Step { .. } => SupervisedLiveEvent::StepScheduled,
            SupervisedLiveCommand::SetInputState { .. } => SupervisedLiveEvent::InputApplied,
            SupervisedLiveCommand::CaptureFrame { .. } => SupervisedLiveEvent::CaptureSaved,
            SupervisedLiveCommand::Quit => SupervisedLiveEvent::Quit,
        };
        if self.ok {
            if self.event != expected {
                return Err(SupervisedLiveErrorCode::InvalidRequest);
            }
            if let SupervisedLiveCommand::CaptureFrame { artifact_id } = &request.command {
                if self.capture.as_ref().map(|capture| capture.artifact_id) != Some(*artifact_id) {
                    return Err(SupervisedLiveErrorCode::InvalidRequest);
                }
            }
        }
        Ok(())
    }

    pub fn parse_line(line: &[u8]) -> Result<Self, SupervisedLiveErrorCode> {
        if line.is_empty()
            || line.len() > MAX_SUPERVISED_LIVE_LINE_BYTES
            || line.iter().any(|byte| matches!(byte, b'\n' | b'\r'))
        {
            return Err(SupervisedLiveErrorCode::InvalidRequest);
        }
        let response: Self =
            serde_json::from_slice(line).map_err(|_| SupervisedLiveErrorCode::InvalidRequest)?;
        response.validate()?;
        Ok(response)
    }
}

fn valid_pointers(pointers: &[LivePointerInput]) -> bool {
    let mut ids = std::collections::BTreeSet::new();
    pointers.iter().all(|pointer| {
        pointer.id >= 0
            && pointer.x >= 0
            && pointer.y >= 0
            && ids.insert(pointer.id)
            && (!pointer.went_down || (pointer.is_down && !pointer.went_up))
            && (!pointer.went_up || !pointer.is_down)
    })
}

#[derive(Debug, Clone)]
pub struct SupervisedLiveRequestBudget {
    attempts: u32,
    window_started: Instant,
    window_count: u16,
    capture_count: u8,
    capture_ids: std::collections::BTreeSet<u8>,
}

impl SupervisedLiveRequestBudget {
    pub fn new(now: Instant) -> Self {
        Self {
            attempts: 0,
            window_started: now,
            window_count: 0,
            capture_count: 0,
            capture_ids: std::collections::BTreeSet::new(),
        }
    }

    pub fn record(&mut self, now: Instant) -> Result<(), SupervisedLiveErrorCode> {
        if self.attempts >= MAX_SUPERVISED_LIVE_REQUESTS {
            return Err(SupervisedLiveErrorCode::RequestLimit);
        }
        self.attempts += 1;
        if now.saturating_duration_since(self.window_started) >= Duration::from_secs(1) {
            self.window_started = now;
            self.window_count = 0;
        }
        if self.window_count >= MAX_SUPERVISED_LIVE_REQUESTS_PER_SECOND {
            self.window_count = self.window_count.saturating_add(1);
            return Err(SupervisedLiveErrorCode::RateLimited);
        }
        self.window_count += 1;
        Ok(())
    }

    pub fn accepted(&self) -> u32 {
        self.attempts
    }

    pub fn capture_attempt(
        &mut self,
        request: &SupervisedLiveRequest,
    ) -> Result<(), SupervisedLiveErrorCode> {
        if let SupervisedLiveCommand::CaptureFrame { artifact_id } = &request.command {
            let artifact_id = *artifact_id;
            if self.capture_count >= MAX_SUPERVISED_LIVE_CAPTURES {
                return Err(SupervisedLiveErrorCode::CaptureRejected);
            }
            if !self.capture_ids.insert(artifact_id) {
                return Err(SupervisedLiveErrorCode::CaptureRejected);
            }
            self.capture_count += 1;
        }
        Ok(())
    }
}

pub fn response_for_request(
    request: &SupervisedLiveRequest,
    tick: u64,
    event: SupervisedLiveEvent,
    capture: Option<SupervisedLiveCapture>,
    error_code: Option<SupervisedLiveErrorCode>,
) -> Result<SupervisedLiveResponse, SupervisedLiveErrorCode> {
    let response = SupervisedLiveResponse {
        schema_version: SUPERVISED_LIVE_SCHEMA_VERSION,
        request_id: request.request_id,
        tick,
        ok: error_code.is_none(),
        event,
        capture,
        error_code,
    };
    response.validate_for_request(request)?;
    Ok(response)
}

pub fn artifact_name(artifact_id: u8) -> Option<String> {
    (artifact_id < MAX_SUPERVISED_LIVE_CAPTURES)
        .then(|| format!("supervised-capture-{artifact_id:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(command: SupervisedLiveCommand) -> SupervisedLiveRequest {
        SupervisedLiveRequest {
            schema_version: SUPERVISED_LIVE_SCHEMA_VERSION,
            request_id: 1,
            command,
        }
    }

    #[test]
    fn parser_accepts_only_the_fixed_live_command_allowlist() {
        let pause = br#"{"schema_version":1,"request_id":1,"command":{"type":"pause"}}"#;
        assert!(matches!(
            SupervisedLiveRequest::parse_line(pause).unwrap().command,
            SupervisedLiveCommand::Pause
        ));
        for line in [
            br#"{"schema_version":1,"request_id":1,"command":{"type":"evaluate","code":"1"}}"#
                .as_slice(),
            br#"{"schema_version":1,"request_id":1,"command":{"type":"pause","extra":true}}"#,
            br#"{"schema_version":1,"request_id":1,"extra":true,"command":{"type":"pause"}}"#,
        ] {
            assert_eq!(
                SupervisedLiveRequest::parse_line(line),
                Err(SupervisedLiveErrorCode::InvalidRequest)
            );
        }
    }

    #[test]
    fn requests_are_bounded_before_json_parsing_and_mapping() {
        assert_eq!(
            SupervisedLiveRequest::parse_line(&vec![b' '; MAX_SUPERVISED_LIVE_LINE_BYTES + 1]),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            SupervisedLiveRequest::parse_line(b"{\"schema_version\":1,\n\"request_id\":1}"),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            request(SupervisedLiveCommand::Step { ticks: 2 }).validate(),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            request(SupervisedLiveCommand::CaptureFrame { artifact_id: 16 }).validate(),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            request(SupervisedLiveCommand::SetInputState {
                pointers: vec![
                    LivePointerInput {
                        id: 0,
                        x: 0,
                        y: 0,
                        is_down: false,
                        went_down: false,
                        went_up: false,
                    };
                    MAX_SUPERVISED_LIVE_POINTERS + 1
                ],
            })
            .validate(),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            request(SupervisedLiveCommand::SetInputState {
                pointers: vec![LivePointerInput {
                    id: -1,
                    x: 0,
                    y: 0,
                    is_down: false,
                    went_down: false,
                    went_up: false,
                }],
            })
            .validate(),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            request(SupervisedLiveCommand::SetInputState {
                pointers: vec![
                    LivePointerInput {
                        id: 0,
                        x: 0,
                        y: 0,
                        is_down: false,
                        went_down: false,
                        went_up: false,
                    };
                    2
                ],
            })
            .validate(),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn request_budget_enforces_total_and_rate_bounds() {
        let start = Instant::now();
        let mut budget = SupervisedLiveRequestBudget::new(start);
        for _ in 0..MAX_SUPERVISED_LIVE_REQUESTS_PER_SECOND {
            budget.record(start).unwrap();
        }
        assert_eq!(
            budget.record(start),
            Err(SupervisedLiveErrorCode::RateLimited)
        );
        budget
            .record(start + Duration::from_secs(1))
            .expect("new one-second window");
        assert_eq!(budget.accepted(), 122);
        for artifact_id in 0..MAX_SUPERVISED_LIVE_CAPTURES {
            let capture = request(SupervisedLiveCommand::CaptureFrame { artifact_id });
            budget.capture_attempt(&capture).unwrap();
        }
        let capture = request(SupervisedLiveCommand::CaptureFrame {
            artifact_id: MAX_SUPERVISED_LIVE_CAPTURES,
        });
        assert_eq!(
            budget.capture_attempt(&capture),
            Err(SupervisedLiveErrorCode::CaptureRejected)
        );
        let duplicate = request(SupervisedLiveCommand::CaptureFrame { artifact_id: 0 });
        let mut capture_budget = SupervisedLiveRequestBudget::new(start);
        capture_budget.capture_attempt(&duplicate).unwrap();
        assert_eq!(
            capture_budget.capture_attempt(&duplicate),
            Err(SupervisedLiveErrorCode::CaptureRejected)
        );
    }

    #[test]
    fn live_mapping_uses_only_existing_safe_host_input_commands() {
        let input = request(SupervisedLiveCommand::CaptureFrame { artifact_id: 3 });
        assert_eq!(
            input.to_live_request().command,
            LiveCommand::CaptureFrame {
                artifact: "supervised-capture-03".into()
            }
        );
        assert_eq!(artifact_name(15).as_deref(), Some("supervised-capture-15"));
        assert_eq!(artifact_name(16), None);
    }

    #[test]
    fn response_contract_contains_only_fixed_receipts_and_error_codes() {
        let pause_request = request(SupervisedLiveCommand::Pause);
        let capture_request = request(SupervisedLiveCommand::CaptureFrame { artifact_id: 2 });
        let response =
            response_for_request(&pause_request, 4, SupervisedLiveEvent::Paused, None, None)
                .unwrap();
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value,
            json!({
                "schema_version": 1,
                "request_id": 1,
                "tick": 4,
                "ok": true,
                "event": "paused"
            })
        );
        let capture = SupervisedLiveCapture {
            artifact_id: 2,
            width: 640,
            height: 360,
            byte_length: 1024,
            sha256: "a".repeat(64),
        };
        let response = response_for_request(
            &capture_request,
            4,
            SupervisedLiveEvent::CaptureSaved,
            Some(capture),
            None,
        )
        .unwrap();
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("path"));
        assert!(!json.contains("invite"));
        assert!(!json.contains("runtime_identity"));
        assert!(
            response_for_request(&pause_request, 4, SupervisedLiveEvent::Rejected, None, None,)
                .is_err()
        );
        assert!(response_for_request(
            &pause_request,
            4,
            SupervisedLiveEvent::Paused,
            None,
            Some(SupervisedLiveErrorCode::RuntimeFailed),
        )
        .is_err());
        let wrong_id = SupervisedLiveResponse {
            request_id: 9,
            ..response
        };
        assert_eq!(
            wrong_id.validate_for_request(&capture_request),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
        assert_eq!(
            SupervisedLiveResponse::parse_line(
                br#"{"schema_version":1,"request_id":1,"tick":4,"ok":true,"event":"paused","extra":true}"#
            ),
            Err(SupervisedLiveErrorCode::InvalidRequest)
        );
    }
}

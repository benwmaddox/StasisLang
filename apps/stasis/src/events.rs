use serde::Serialize;

pub const RUNNER_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunnerEvent {
    CompileResult {
        request_id: u64,
        status: String,
        diagnostics: Vec<String>,
        compile_duration_ms: Option<u64>,
    },
    HookResult {
        request_id: u64,
        symbol: String,
        status: String,
        error: Option<String>,
    },
    SwapIndicatorArmed {
        request_id: u64,
        ticks: u32,
    },
    SwapCommitResult {
        request_id: u64,
        status: String,
        swapped_fn_ids: Vec<u32>,
        new_generation: Option<u64>,
        error: Option<String>,
        commit_duration_ms: Option<u64>,
    },
    Summary {
        ticks_executed: u32,
        compile_successes: u32,
        compile_failures: u32,
        swap_commit_successes: u32,
        swap_commit_failures: u32,
        swap_indicator_armed_count: u32,
        swap_flash_peak_ticks: u32,
        swap_flash_ticks_remaining: u32,
        window_width: Option<u32>,
        window_height: Option<u32>,
        has_in_flight_work: bool,
        last_compile_duration_ms: Option<u64>,
        last_commit_duration_ms: Option<u64>,
    },
}

#[derive(Debug, Serialize)]
pub struct VersionedRunnerEvent<'a> {
    pub schema_version: u16,
    #[serde(flatten)]
    pub event: &'a RunnerEvent,
}

impl RunnerEvent {
    pub fn with_schema_version(&self) -> VersionedRunnerEvent<'_> {
        VersionedRunnerEvent {
            schema_version: RUNNER_EVENT_SCHEMA_VERSION,
            event: self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_event_serialization_includes_schema_version_and_tag() {
        let event = RunnerEvent::CompileResult {
            request_id: 7,
            status: "success".to_string(),
            diagnostics: Vec::new(),
            compile_duration_ms: Some(12),
        };

        let json = serde_json::to_string(&event.with_schema_version())
            .expect("event serialization should succeed");
        assert!(json.contains("\"schema_version\":1"));
        assert!(json.contains("\"event\":\"compile_result\""));
    }
}

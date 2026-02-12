use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunnerEvent {
    CompileResult {
        request_id: u64,
        status: String,
        diagnostics: Vec<String>,
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
        has_in_flight_work: bool,
    },
}

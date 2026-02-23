use crate::RunnerConfig;
use std::path::PathBuf;
use stasis_runner::swap::contracts::TargetMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
}

impl WindowConfig {
    pub fn is_vertical(self) -> bool {
        self.height > self.width
    }
}

pub const BRICKOUT_REVENGE_V1_WINDOW: WindowConfig = WindowConfig {
    width: 720,
    height: 1280,
};

pub fn brickout_revenge_v1_runner_config(max_ticks: u32) -> RunnerConfig {
    RunnerConfig {
        max_ticks,
        tick_sleep_micros: 0,
        window: Some(BRICKOUT_REVENGE_V1_WINDOW),
        inject_file_change: Some(PathBuf::from(
            "samples/brickout_revenge/brickout_revenge_v1.stasis",
        )),
        watch_directory: None,
        target_mode: TargetMode::JitDev,
        fail_compile: false,
        disable_on_code_swap_hook: false,
        hook_failure_reason: None,
        swap_failure_reason: None,
        runtime_launch: false,
        aot_probe_loadability: false,
    }
}

pub mod aot;
pub(crate) mod emit;
pub mod jit;
mod reachability;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineEntrypoints {
    pub tick: String,
    pub render: String,
    pub on_code_swap: Option<String>,
}

impl EngineEntrypoints {
    pub fn runtime_default() -> Self {
        Self {
            tick: "tick".to_string(),
            render: "render".to_string(),
            on_code_swap: Some("on_code_swap".to_string()),
        }
    }
}

impl Default for EngineEntrypoints {
    fn default() -> Self {
        Self::runtime_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AotOptimizationProfile {
    None,
    Speed,
    SpeedAndSize,
}

impl AotOptimizationProfile {
    pub fn as_cranelift_opt_level(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Speed => "speed",
            Self::SpeedAndSize => "speed_and_size",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Speed => "speed",
            Self::SpeedAndSize => "speed_and_size",
        }
    }
}

impl Default for AotOptimizationProfile {
    fn default() -> Self {
        Self::Speed
    }
}

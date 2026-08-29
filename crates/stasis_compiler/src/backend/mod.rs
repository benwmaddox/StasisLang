pub mod aot;
pub mod assets;
pub(crate) mod compile_analysis;
pub mod development_swap;
pub(crate) mod emit;
pub mod jit;
pub mod patch_plan;
pub mod program_snapshot;
pub(crate) mod reachability;
mod runtime_exports;
pub mod state_layout;
pub mod state_migration;
mod state_query;
pub mod wasm;

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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    fn backend_source(path: &str) -> String {
        let mut full_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        full_path.push("src");
        full_path.push("backend");
        full_path.push(path);
        fs::read_to_string(&full_path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", full_path.display()))
    }

    fn assert_backend_wrapper_shape(path: &str) {
        let source = backend_source(path);
        for forbidden in [
            "parse_simple_statements_from_block_with(",
            "emit_simple_statements(",
            "emit_simple_expression(",
            "emit_simple_condition(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} should stay a thin wrapper; found forbidden lowering hook `{forbidden}`"
            );
        }
        assert!(
            source.contains("compile_function_with_module("),
            "{path} should route per-function compilation through compile_function_with_module"
        );
    }

    #[test]
    fn jit_backend_stays_a_thin_wrapper_over_shared_lowering() {
        assert_backend_wrapper_shape("jit.rs");
    }

    #[test]
    fn aot_backend_stays_a_thin_wrapper_over_shared_lowering() {
        assert_backend_wrapper_shape("aot.rs");
    }
}

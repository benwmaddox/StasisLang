#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use stasis_runner::swap::contracts::{CodeGeneration, FnId, FunctionPatchSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CodePtr(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    pub new_generation: CodeGeneration,
    pub swapped_fn_ids: Vec<FnId>,
    pub retired_generations: Vec<CodeGeneration>,
}

#[derive(Debug, Clone)]
pub struct AotCompileConfig {
    pub helper_path: Option<PathBuf>,
    pub target: String,
    pub module_name: String,
    pub opt_level: String,
}

impl Default for AotCompileConfig {
    fn default() -> Self {
        Self {
            helper_path: None,
            target: default_target_triple(),
            module_name: "stasis_module".to_string(),
            opt_level: "none".to_string(),
        }
    }
}

/// Dev/runtime-facing indirection table (`FnId -> code_ptr`) with simple
/// generation retirement bookkeeping.
pub struct FunctionPointerTable {
    entries: BTreeMap<FnId, CodePtr>,
    generation: u64,
    pending_retire: VecDeque<CodeGeneration>,
    safe_retire_window: usize,
}

impl FunctionPointerTable {
    pub fn new() -> Self {
        Self::with_safe_retire_window(2)
    }

    pub fn with_safe_retire_window(safe_retire_window: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            generation: 0,
            pending_retire: VecDeque::new(),
            safe_retire_window,
        }
    }

    pub fn generation(&self) -> CodeGeneration {
        CodeGeneration(self.generation)
    }

    pub fn code_ptr(&self, fn_id: FnId) -> Option<CodePtr> {
        self.entries.get(&fn_id).copied()
    }

    pub fn commit_patch_set(&mut self, patch_set: &FunctionPatchSet) -> CommitOutcome {
        self.generation += 1;
        let new_generation = CodeGeneration(self.generation);

        let mut swapped_fn_ids = Vec::with_capacity(patch_set.functions.len());
        for patch in &patch_set.functions {
            let fn_id = patch.fn_id;
            let code_ptr = make_code_ptr(new_generation, fn_id);
            self.entries.insert(fn_id, code_ptr);
            swapped_fn_ids.push(fn_id);
        }

        if self.generation > 1 {
            self.pending_retire
                .push_back(CodeGeneration(self.generation - 1));
        }

        let mut retired_generations = Vec::new();
        while self.pending_retire.len() > self.safe_retire_window {
            if let Some(retired) = self.pending_retire.pop_front() {
                retired_generations.push(retired);
            }
        }

        CommitOutcome {
            new_generation,
            swapped_fn_ids,
            retired_generations,
        }
    }
}

impl Default for FunctionPointerTable {
    fn default() -> Self {
        Self::new()
    }
}

fn make_code_ptr(generation: CodeGeneration, fn_id: FnId) -> CodePtr {
    CodePtr((generation.0 << 32) | u64::from(fn_id.0))
}

pub fn compile_clif_to_object(
    clif: &str,
    output_object: &Path,
    config: &AotCompileConfig,
) -> Result<(), String> {
    let helper_path = resolve_aot_helper_path(config)?;
    if !helper_path.exists() {
        return Err(format!(
            "missing Cranelift AOT helper at {}",
            helper_path.display()
        ));
    }

    if let Some(parent) = output_object.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create output directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("clock error while creating temp CLIF path: {e}"))?
        .as_nanos();
    let temp_input = std::env::temp_dir().join(format!("stasis_aot_{unique_suffix}.clif"));
    fs::write(&temp_input, clif).map_err(|e| {
        format!(
            "failed to write temporary CLIF input {}: {e}",
            temp_input.display()
        )
    })?;

    let output = Command::new(&helper_path)
        .arg("--input")
        .arg(&temp_input)
        .arg("--output")
        .arg(output_object)
        .arg("--target")
        .arg(&config.target)
        .arg("--module-name")
        .arg(&config.module_name)
        .arg("--opt-level")
        .arg(&config.opt_level)
        .output()
        .map_err(|e| format!("failed to execute AOT helper {}: {e}", helper_path.display()))?;

    let _ = fs::remove_file(&temp_input);

    if !output.status.success() {
        return Err(format!(
            "AOT helper failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    if !output_object.exists() {
        return Err(format!(
            "AOT helper reported success but did not produce object {}",
            output_object.display()
        ));
    }

    Ok(())
}

fn resolve_aot_helper_path(config: &AotCompileConfig) -> Result<PathBuf, String> {
    if let Some(path) = config.helper_path.as_ref() {
        return Ok(path.clone());
    }

    if let Ok(path) = std::env::var("STASIS_CRANELIFT_AOT") {
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    Ok(repo_root
        .join("tools")
        .join("cranelift-aot")
        .join("target")
        .join("debug")
        .join(default_aot_exe_name()))
}

fn default_target_triple() -> String {
    if cfg!(windows) {
        "x86_64-pc-windows-msvc".to_string()
    } else if cfg!(target_os = "linux") {
        "x86_64-unknown-linux-gnu".to_string()
    } else if cfg!(target_os = "macos") {
        "x86_64-apple-darwin".to_string()
    } else {
        "x86_64-unknown-unknown".to_string()
    }
}

fn default_aot_exe_name() -> &'static str {
    if cfg!(windows) {
        "stasis-cranelift-aot.exe"
    } else {
        "stasis-cranelift-aot"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_runner::swap::contracts::{FunctionPatch, FunctionPatchSet};
    use std::path::Path;

    fn patch_set(ids: &[u32]) -> FunctionPatchSet {
        FunctionPatchSet {
            functions: ids
                .iter()
                .copied()
                .map(|id| FunctionPatch { fn_id: FnId(id) })
                .collect(),
        }
    }

    #[test]
    fn initial_state_has_no_generation_and_no_entries() {
        let table = FunctionPointerTable::new();
        assert_eq!(table.generation(), CodeGeneration(0));
        assert_eq!(table.code_ptr(FnId(1)), None);
    }

    #[test]
    fn commit_updates_fn_ids_and_generation() {
        let mut table = FunctionPointerTable::new();
        let outcome = table.commit_patch_set(&patch_set(&[7, 11]));

        assert_eq!(outcome.new_generation, CodeGeneration(1));
        assert_eq!(outcome.swapped_fn_ids, vec![FnId(7), FnId(11)]);
        assert!(outcome.retired_generations.is_empty());
        assert_eq!(table.generation(), CodeGeneration(1));

        let ptr_7 = table.code_ptr(FnId(7)).expect("missing fn 7 code pointer");
        let ptr_11 = table.code_ptr(FnId(11)).expect("missing fn 11 code pointer");
        assert_eq!(ptr_7, CodePtr((1_u64 << 32) | 7));
        assert_eq!(ptr_11, CodePtr((1_u64 << 32) | 11));
    }

    #[test]
    fn repeated_commit_rewrites_code_ptr_for_same_fn_id() {
        let mut table = FunctionPointerTable::new();
        table.commit_patch_set(&patch_set(&[3]));
        let before = table.code_ptr(FnId(3)).expect("expected first code pointer");

        let outcome = table.commit_patch_set(&patch_set(&[3]));
        let after = table.code_ptr(FnId(3)).expect("expected rewritten code pointer");

        assert_eq!(outcome.new_generation, CodeGeneration(2));
        assert_ne!(before, after);
        assert_eq!(after, CodePtr((2_u64 << 32) | 3));
    }

    #[test]
    fn retires_old_generations_after_safe_window() {
        let mut table = FunctionPointerTable::with_safe_retire_window(2);

        let c1 = table.commit_patch_set(&patch_set(&[1]));
        let c2 = table.commit_patch_set(&patch_set(&[2]));
        let c3 = table.commit_patch_set(&patch_set(&[3]));
        let c4 = table.commit_patch_set(&patch_set(&[4]));

        assert!(c1.retired_generations.is_empty());
        assert!(c2.retired_generations.is_empty());
        assert!(c3.retired_generations.is_empty());
        assert_eq!(c4.retired_generations, vec![CodeGeneration(1)]);
    }

    #[test]
    fn resolve_helper_path_prefers_explicit_path() {
        let config = AotCompileConfig {
            helper_path: Some(Path::new("custom").join("helper.exe")),
            ..AotCompileConfig::default()
        };
        let resolved = resolve_aot_helper_path(&config).expect("resolution should succeed");
        assert!(resolved.ends_with(Path::new("custom").join("helper.exe")));
    }

    #[cfg(windows)]
    #[test]
    fn aot_helper_compiles_minimal_clif_to_object() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let helper = repo_root
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");

        if !helper.exists() {
            let build_output = Command::new("cargo")
                .arg("build")
                .arg("--manifest-path")
                .arg(
                    repo_root
                        .join("tools")
                        .join("cranelift-aot")
                        .join("Cargo.toml"),
                )
                .current_dir(&repo_root)
                .output()
                .expect("failed to build AOT helper");
            assert!(
                build_output.status.success(),
                "failed to build AOT helper\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&build_output.stdout),
                String::from_utf8_lossy(&build_output.stderr)
            );
        }

        let temp_dir = std::env::temp_dir().join(format!(
            "stasis_aot_smoke_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be valid")
                .as_millis()
        ));
        fs::create_dir_all(&temp_dir).expect("failed to create temp dir");
        let out_obj = temp_dir.join("main.obj");

        let clif = r#"function %main() -> i32 windows_fastcall {
block0:
v0 = iconst.i32 7
return v0
}
"#;
        let config = AotCompileConfig {
            helper_path: Some(helper),
            target: "x86_64-pc-windows-msvc".to_string(),
            module_name: "stasis_test".to_string(),
            opt_level: "none".to_string(),
        };

        compile_clif_to_object(clif, &out_obj, &config).expect("AOT compile should succeed");

        let metadata = fs::metadata(&out_obj).expect("missing AOT object output");
        assert!(metadata.len() > 0, "AOT object file should not be empty");

        let _ = fs::remove_file(out_obj);
        let _ = fs::remove_dir_all(temp_dir);
    }
}

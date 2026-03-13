#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

use stasis_runner::swap::contracts::{CodeGeneration, FnId, FunctionPatchSet, JitCodePtrOverride};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

fn default_aot_opt_level() -> String {
    if let Ok(value) = std::env::var("STASIS_AOT_OPT_LEVEL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            let normalized = trimmed.to_ascii_lowercase();
            match normalized.as_str() {
                "none" | "speed" | "speed_and_size" => return normalized,
                "speed-and-size" => return "speed_and_size".to_string(),
                _ => {}
            }
        }
    }
    "speed_and_size".to_string()
}

impl Default for AotCompileConfig {
    fn default() -> Self {
        Self {
            helper_path: None,
            target: default_target_triple(),
            module_name: "stasis_module".to_string(),
            opt_level: default_aot_opt_level(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AotLinkConfig {
    pub linker_path: Option<PathBuf>,
    pub runtime_lib_paths: Vec<PathBuf>,
}

impl Default for AotLinkConfig {
    fn default() -> Self {
        let linker_path = std::env::var("STASIS_AOT_LINKER")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self {
            linker_path,
            runtime_lib_paths: Vec::new(),
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

    pub fn preview_code_ptr_after_commit(
        &self,
        patch_set: &FunctionPatchSet,
        overrides: Option<&[JitCodePtrOverride]>,
        fn_id: FnId,
    ) -> Option<CodePtr> {
        // If a function is not part of the patch set, its code pointer will not change.
        if !patch_set.functions.iter().any(|patch| patch.fn_id == fn_id) {
            return self.code_ptr(fn_id);
        }

        if let Some(overrides) = overrides {
            if let Some(entry) = overrides.iter().find(|entry| entry.fn_id == fn_id) {
                return Some(CodePtr(entry.code_ptr));
            }
        }

        let next_generation = CodeGeneration(self.generation + 1);
        Some(make_code_ptr(next_generation, fn_id))
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

    pub fn commit_patch_set_with_overrides(
        &mut self,
        patch_set: &FunctionPatchSet,
        overrides: &[JitCodePtrOverride],
    ) -> CommitOutcome {
        self.generation += 1;
        let new_generation = CodeGeneration(self.generation);
        let override_by_fn: BTreeMap<FnId, CodePtr> = overrides
            .iter()
            .map(|entry| (entry.fn_id, CodePtr(entry.code_ptr)))
            .collect();

        let mut swapped_fn_ids = Vec::with_capacity(patch_set.functions.len());
        for patch in &patch_set.functions {
            let fn_id = patch.fn_id;
            let code_ptr = override_by_fn
                .get(&fn_id)
                .copied()
                .unwrap_or_else(|| make_code_ptr(new_generation, fn_id));
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

pub fn link_objects_to_dynamic_library(
    object_paths: &[PathBuf],
    output_library: &Path,
    export_symbols: &[String],
    config: &AotLinkConfig,
) -> Result<(), String> {
    if object_paths.is_empty() {
        return Err("cannot link dynamic library: object list is empty".to_string());
    }

    if let Some(parent) = output_library.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create dynamic library output directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let linker = resolve_linker_path(config);
    let mut args: Vec<String> = Vec::new();
    if cfg!(windows) {
        args.push("/NOLOGO".to_string());
        args.push("/DLL".to_string());
        args.push(format!("/OUT:{}", output_library.display()));
        for symbol in export_symbols {
            args.push(format!("/EXPORT:{symbol}"));
        }
        let windows_lib_paths = resolve_windows_link_lib_paths();
        for lib_path in &windows_lib_paths {
            args.push(format!("/LIBPATH:{}", lib_path.display()));
        }
        if let Some(kernel32) = resolve_kernel32_lib_path(&windows_lib_paths) {
            args.push(kernel32.display().to_string());
        } else {
            args.push("kernel32.lib".to_string());
        }
        // When linking against Rust `staticlib` runtime shims (e.g. `stasis_dynload.lib`),
        // lld-link does not automatically pull in the CRT/system libraries that those objects
        // depend on. Add the common MSVC + Windows SDK libraries so AOT bundles link cleanly.
        args.push("ucrt.lib".to_string());
        args.push("vcruntime.lib".to_string());
        args.push("msvcrt.lib".to_string());
        args.push("legacy_stdio_definitions.lib".to_string());
        args.push("advapi32.lib".to_string());
        args.push("bcrypt.lib".to_string());
        args.push("dbghelp.lib".to_string());
        args.push("ntdll.lib".to_string());
        args.push("ole32.lib".to_string());
        args.push("oleaut32.lib".to_string());
        args.push("psapi.lib".to_string());
        args.push("secur32.lib".to_string());
        args.push("shell32.lib".to_string());
        args.push("user32.lib".to_string());
        args.push("userenv.lib".to_string());
        args.push("ws2_32.lib".to_string());
    } else {
        args.push("-shared".to_string());
        args.push("-o".to_string());
        args.push(output_library.display().to_string());
    }
    for object_path in object_paths {
        args.push(object_path.display().to_string());
    }
    for runtime_lib in &config.runtime_lib_paths {
        args.push(runtime_lib.display().to_string());
    }

    run_link_command_with_args(
        &linker,
        &args,
        "dynamic library link",
        &output_library.with_extension("link.rsp"),
    )?;
    if !output_library.exists() {
        return Err(format!(
            "link step reported success but did not produce {}",
            output_library.display()
        ));
    }
    Ok(())
}

pub fn link_objects_to_executable(
    object_paths: &[PathBuf],
    output_executable: &Path,
    entry_symbol: &str,
    config: &AotLinkConfig,
) -> Result<(), String> {
    if object_paths.is_empty() {
        return Err("cannot link executable: object list is empty".to_string());
    }
    if entry_symbol.is_empty() {
        return Err("cannot link executable: entry symbol is empty".to_string());
    }

    if let Some(parent) = output_executable.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create executable output directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let linker = resolve_linker_path(config);
    let mut args: Vec<String> = Vec::new();
    if cfg!(windows) {
        args.push("/NOLOGO".to_string());
        args.push(format!("/OUT:{}", output_executable.display()));
        args.push(format!("/ENTRY:{entry_symbol}"));
        args.push("/SUBSYSTEM:CONSOLE".to_string());
        let windows_lib_paths = resolve_windows_link_lib_paths();
        for lib_path in &windows_lib_paths {
            args.push(format!("/LIBPATH:{}", lib_path.display()));
        }
        if let Some(kernel32) = resolve_kernel32_lib_path(&windows_lib_paths) {
            args.push(kernel32.display().to_string());
        } else {
            args.push("kernel32.lib".to_string());
        }
    } else {
        args.push("-o".to_string());
        args.push(output_executable.display().to_string());
        args.push(format!("-Wl,-e,{entry_symbol}"));
    }
    for object_path in object_paths {
        args.push(object_path.display().to_string());
    }
    for runtime_lib in &config.runtime_lib_paths {
        args.push(runtime_lib.display().to_string());
    }

    run_link_command_with_args(
        &linker,
        &args,
        "executable link",
        &output_executable.with_extension("link.rsp"),
    )?;
    if !output_executable.exists() {
        return Err(format!(
            "link step reported success but did not produce {}",
            output_executable.display()
        ));
    }
    Ok(())
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
        .map_err(|e| {
            format!(
                "failed to execute AOT helper {}: {e}",
                helper_path.display()
            )
        })?;

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

    let mut candidates: Vec<PathBuf> = Vec::new();

    // Release-friendly default: ship the helper next to the stasis executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(default_aot_exe_name()));
        }
    }

    // Dev-friendly default: locate the helper under the repo tree.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    candidates.push(
        repo_root
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("release")
            .join(default_aot_exe_name()),
    );
    candidates.push(
        repo_root
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join(default_aot_exe_name()),
    );

    // Alternate dev-friendly default: allow building the helper into the workspace target dir.
    // This avoids relying on `tools/cranelift-aot/target`, which can be blocked by path-based
    // execution policies in some Windows environments.
    candidates.push(
        repo_root
            .join("target")
            .join("cranelift-aot")
            .join("release")
            .join(default_aot_exe_name()),
    );
    candidates.push(
        repo_root
            .join("target")
            .join("cranelift-aot")
            .join("debug")
            .join(default_aot_exe_name()),
    );

    // Allow running with the helper on PATH / in the current directory.
    candidates.push(PathBuf::from(default_aot_exe_name()));

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    // No candidate exists; return the most likely location so the caller reports a concrete path.
    Ok(candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(default_aot_exe_name())))
}

fn resolve_linker_path(config: &AotLinkConfig) -> PathBuf {
    if let Some(path) = config.linker_path.as_ref() {
        return path.clone();
    }
    if cfg!(windows) {
        PathBuf::from("lld-link.exe")
    } else {
        PathBuf::from("cc")
    }
}

#[cfg(windows)]
fn resolve_windows_link_lib_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    if let Ok(lib_env) = std::env::var("LIB") {
        for raw in lib_env.split(';') {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let path = PathBuf::from(trimmed);
            if path.exists() && !out.contains(&path) {
                out.push(path);
            }
        }
    }

    if let Ok(vc_tools) = std::env::var("VCToolsInstallDir") {
        let vc_lib = PathBuf::from(vc_tools).join("lib").join("x64");
        if vc_lib.exists() && !out.contains(&vc_lib) {
            out.push(vc_lib);
        }
    }

    let msvc_roots = [
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC",
        r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC",
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC",
        r"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC",
    ];
    for root in msvc_roots {
        if let Some(version_dir) = latest_child_dir(Path::new(root)) {
            let vc_lib = version_dir.join("lib").join("x64");
            if vc_lib.exists() && !out.contains(&vc_lib) {
                out.push(vc_lib);
            }
        }
    }

    let windows_kits_roots = [
        r"C:\Program Files (x86)\Windows Kits\10\Lib",
        r"C:\Program Files\Windows Kits\10\Lib",
    ];
    for root in windows_kits_roots {
        if let Some(version_dir) = latest_child_dir(Path::new(root)) {
            let um = version_dir.join("um").join("x64");
            if um.exists() && !out.contains(&um) {
                out.push(um);
            }
            let ucrt = version_dir.join("ucrt").join("x64");
            if ucrt.exists() && !out.contains(&ucrt) {
                out.push(ucrt);
            }
        }
    }

    out
}

#[cfg(not(windows))]
fn resolve_windows_link_lib_paths() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(windows)]
fn resolve_kernel32_lib_path(lib_paths: &[PathBuf]) -> Option<PathBuf> {
    for lib_path in lib_paths {
        let candidate = lib_path.join("kernel32.lib");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(windows))]
fn resolve_kernel32_lib_path(_lib_paths: &[PathBuf]) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn latest_child_dir(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    dirs.sort_by(|a, b| {
        let an = a
            .file_name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        let bn = b
            .file_name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        bn.cmp(&an)
    });
    dirs.into_iter().next()
}

fn run_link_command(command: &mut Command, mode: &str, linker: &Path) -> Result<(), String> {
    let output = command.output().map_err(|error| {
        format!(
            "failed to execute {mode} linker {}: {error}",
            linker.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "{mode} failed (status {:?})\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn run_link_command_with_args(
    linker: &Path,
    args: &[String],
    mode: &str,
    response_file_path: &Path,
) -> Result<(), String> {
    let mut command = Command::new(linker);
    if cfg!(windows) {
        let response_body = args
            .iter()
            .map(|arg| escape_linker_response_arg(arg))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(response_file_path, response_body).map_err(|error| {
            format!(
                "failed to write linker response file {}: {error}",
                response_file_path.display()
            )
        })?;
        command.arg(format!("@{}", response_file_path.display()));
        let result = run_link_command(&mut command, mode, linker);
        let _ = fs::remove_file(response_file_path);
        return result;
    }

    command.args(args);
    run_link_command(&mut command, mode, linker)
}

fn escape_linker_response_arg(arg: &str) -> String {
    if arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
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
        let ptr_11 = table
            .code_ptr(FnId(11))
            .expect("missing fn 11 code pointer");
        assert_eq!(ptr_7, CodePtr((1_u64 << 32) | 7));
        assert_eq!(ptr_11, CodePtr((1_u64 << 32) | 11));
    }

    #[test]
    fn repeated_commit_rewrites_code_ptr_for_same_fn_id() {
        let mut table = FunctionPointerTable::new();
        table.commit_patch_set(&patch_set(&[3]));
        let before = table
            .code_ptr(FnId(3))
            .expect("expected first code pointer");

        let outcome = table.commit_patch_set(&patch_set(&[3]));
        let after = table
            .code_ptr(FnId(3))
            .expect("expected rewritten code pointer");

        assert_eq!(outcome.new_generation, CodeGeneration(2));
        assert_ne!(before, after);
        assert_eq!(after, CodePtr((2_u64 << 32) | 3));
    }

    #[test]
    fn commit_with_overrides_applies_explicit_code_ptrs() {
        let mut table = FunctionPointerTable::new();
        let patch = patch_set(&[9, 11]);
        let overrides = vec![JitCodePtrOverride {
            fn_id: FnId(9),
            code_ptr: 0x1234,
        }];

        let outcome = table.commit_patch_set_with_overrides(&patch, &overrides);
        assert_eq!(outcome.swapped_fn_ids, vec![FnId(9), FnId(11)]);
        assert_eq!(table.code_ptr(FnId(9)), Some(CodePtr(0x1234)));
        assert_ne!(table.code_ptr(FnId(11)), Some(CodePtr(0x1234)));
    }

    #[test]
    fn preview_code_ptr_after_commit_prefers_override_for_patched_entry() {
        let table = FunctionPointerTable::new();
        let patch = patch_set(&[9]);
        let overrides = vec![JitCodePtrOverride {
            fn_id: FnId(9),
            code_ptr: 0x9988,
        }];
        assert_eq!(
            table.preview_code_ptr_after_commit(&patch, Some(&overrides), FnId(9)),
            Some(CodePtr(0x9988))
        );
    }

    #[test]
    fn preview_code_ptr_after_commit_returns_existing_entry_when_not_patched() {
        let mut table = FunctionPointerTable::new();
        table.commit_patch_set(&patch_set(&[3]));
        assert_eq!(
            table.preview_code_ptr_after_commit(&patch_set(&[7]), None, FnId(3)),
            Some(CodePtr((1_u64 << 32) | 3))
        );
    }

    #[test]
    fn preview_code_ptr_after_commit_returns_none_when_not_patched_and_missing() {
        let table = FunctionPointerTable::new();
        assert_eq!(
            table.preview_code_ptr_after_commit(&patch_set(&[1]), None, FnId(99)),
            None
        );
    }

    #[test]
    fn preview_code_ptr_after_commit_returns_synthetic_ptr_when_patched_without_override() {
        let table = FunctionPointerTable::new();
        assert_eq!(
            table.preview_code_ptr_after_commit(&patch_set(&[5]), None, FnId(5)),
            Some(CodePtr((1_u64 << 32) | 5))
        );
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

    #[test]
    fn aot_linker_can_be_driven_by_configured_fake_linker() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("stasis_aot_link_fake_{stamp}"));
        fs::create_dir_all(&temp_dir).expect("create fake link temp dir");

        let fake_linker = if cfg!(windows) {
            let path = temp_dir.join("fake-link.cmd");
            let script = r#"@echo off
setlocal EnableDelayedExpansion
set OUT=
for %%A in (%*) do (
  set ARG=%%~A
  echo !ARG! | findstr /B /C:"/OUT:" >nul
  if !errorlevel! == 0 (
    set OUT=!ARG:~5!
  )
)
if "%OUT%"=="" exit /b 2
echo fake-dll>"%OUT%"
exit /b 0
"#;
            fs::write(&path, script).expect("write fake windows linker");
            path
        } else {
            let path = temp_dir.join("fake-link.sh");
            let script = r#"#!/usr/bin/env sh
OUT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      OUT="$2"
      shift
      ;;
  esac
  shift
done
if [ -z "$OUT" ]; then
  exit 2
fi
echo "fake-shared" > "$OUT"
"#;
            fs::write(&path, script).expect("write fake unix linker");
            let status = Command::new("chmod")
                .arg("+x")
                .arg(&path)
                .status()
                .expect("chmod fake linker");
            assert!(status.success(), "chmod fake linker should succeed");
            path
        };

        let dummy_object = temp_dir.join("dummy.obj");
        fs::write(&dummy_object, "not-an-object").expect("write dummy object");
        let output_library = if cfg!(windows) {
            temp_dir.join("bundle.dll")
        } else if cfg!(target_os = "macos") {
            temp_dir.join("bundle.dylib")
        } else {
            temp_dir.join("bundle.so")
        };

        let config = AotLinkConfig {
            linker_path: Some(fake_linker),
            runtime_lib_paths: vec![],
        };
        link_objects_to_dynamic_library(
            &[dummy_object],
            &output_library,
            &["fn_1".to_string()],
            &config,
        )
        .expect("fake linker should succeed");
        assert!(output_library.exists(), "fake linker should create output");

        fs::remove_dir_all(&temp_dir).ok();
    }

    #[cfg(windows)]
    #[test]
    fn aot_helper_compiles_minimal_clif_to_object() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let helper_target_dir = repo_root.join("target").join("cranelift-aot");
        let helper = helper_target_dir.join("debug").join(default_aot_exe_name());

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
                .arg("--target-dir")
                .arg(&helper_target_dir)
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

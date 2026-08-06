#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

use stasis_runner::swap::contracts::{CodeGeneration, FnId, FunctionPatchSet, JitCodePtrOverride};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub opt_level: String,
    pub target: AotTarget,
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
            opt_level: default_aot_opt_level(),
            target: AotTarget::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AotTarget {
    Native,
    AndroidArm64 { min_sdk: u32 },
    AndroidX86_64 { min_sdk: u32 },
    IosArm64,
}

impl AotTarget {
    pub fn android_arm64_default() -> Self {
        Self::AndroidArm64 { min_sdk: 26 }
    }

    pub fn android_x86_64_default() -> Self {
        Self::AndroidX86_64 { min_sdk: 26 }
    }

    pub fn ios_arm64_default() -> Self {
        Self::IosArm64
    }

    pub fn object_triple(&self) -> Option<&'static str> {
        match self {
            Self::Native => None,
            Self::AndroidArm64 { .. } => Some("aarch64-linux-android"),
            Self::AndroidX86_64 { .. } => Some("x86_64-linux-android"),
            Self::IosArm64 => Some("aarch64-apple-ios"),
        }
    }

    pub fn clang_target(&self) -> Option<String> {
        match self {
            Self::Native => None,
            Self::AndroidArm64 { min_sdk } => Some(format!("aarch64-linux-android{min_sdk}")),
            Self::AndroidX86_64 { min_sdk } => Some(format!("x86_64-linux-android{min_sdk}")),
            Self::IosArm64 => Some("aarch64-apple-ios".to_string()),
        }
    }

    pub fn is_android(&self) -> bool {
        matches!(self, Self::AndroidArm64 { .. } | Self::AndroidX86_64 { .. })
    }

    pub fn requires_position_independent_code(&self) -> bool {
        !matches!(self, Self::Native) || cfg!(target_os = "macos")
    }
}

impl Default for AotTarget {
    fn default() -> Self {
        Self::Native
    }
}

#[derive(Debug, Clone)]
pub struct AotLinkConfig {
    pub linker_path: Option<PathBuf>,
    pub runtime_lib_paths: Vec<PathBuf>,
    pub target: AotTarget,
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
            target: AotTarget::default(),
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
    if uses_msvc_linker_syntax(config) {
        args.push("/NOLOGO".to_string());
        args.push("/DLL".to_string());
        args.push("/NOENTRY".to_string());
        args.push(format!("/OUT:{}", output_library.display()));
        for symbol in export_symbols {
            args.push(format!("/EXPORT:{symbol}"));
        }
        let windows_lib_paths = resolve_windows_link_lib_paths();
        for lib_path in &windows_lib_paths {
            args.push(format!("/LIBPATH:{}", lib_path.display()));
        }
    } else {
        args.push("-shared".to_string());
        args.push("-o".to_string());
        args.push(output_library.display().to_string());
        if let Some(target) = config.target.clang_target() {
            args.push(format!("--target={target}"));
        }
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
    let mut launcher_source = None;
    if uses_msvc_linker_syntax(config) {
        args.push("/NOLOGO".to_string());
        args.push(format!("/OUT:{}", output_executable.display()));
        args.push(format!("/ENTRY:{entry_symbol}"));
        args.push("/SUBSYSTEM:CONSOLE".to_string());
        let windows_lib_paths = resolve_windows_link_lib_paths();
        for lib_path in &windows_lib_paths {
            args.push(format!("/LIBPATH:{}", lib_path.display()));
        }
    } else if matches!(config.target, AotTarget::Native) {
        let source_path = output_executable.with_extension("entry.c");
        fs::write(
            &source_path,
            native_unix_executable_launcher_source(entry_symbol)?,
        )
        .map_err(|error| {
            format!(
                "failed to write native executable launcher {}: {error}",
                source_path.display()
            )
        })?;
        args.push("-o".to_string());
        args.push(output_executable.display().to_string());
        args.push(source_path.display().to_string());
        launcher_source = Some(source_path);
    } else {
        args.push("-o".to_string());
        args.push(output_executable.display().to_string());
        args.push(format!("-Wl,-e,{entry_symbol}"));
        if let Some(target) = config.target.clang_target() {
            args.push(format!("--target={target}"));
        }
    }
    for object_path in object_paths {
        args.push(object_path.display().to_string());
    }
    for runtime_lib in &config.runtime_lib_paths {
        args.push(runtime_lib.display().to_string());
    }
    if matches!(config.target, AotTarget::Native) && !cfg!(windows) {
        args.push("-lm".to_string());
    }

    let link_result = run_link_command_with_args(
        &linker,
        &args,
        "executable link",
        &output_executable.with_extension("link.rsp"),
    );
    if let Some(source_path) = launcher_source {
        let _ = fs::remove_file(source_path);
    }
    link_result?;
    if !output_executable.exists() {
        return Err(format!(
            "link step reported success but did not produce {}",
            output_executable.display()
        ));
    }
    Ok(())
}

fn native_unix_executable_launcher_source(entry_symbol: &str) -> Result<String, String> {
    if !entry_symbol.bytes().enumerate().all(|(index, byte)| {
        byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
    }) {
        return Err(format!(
            "native executable entry symbol is not a C identifier: {entry_symbol}"
        ));
    }
    Ok(format!(
        "#include <stdint.h>\n\
#include <stdio.h>\n\
#include <string.h>\n\
#if defined(__APPLE__)\n\
#include <mach-o/dyld.h>\n\
#elif defined(__linux__)\n\
#include <unistd.h>\n\
#endif\n\
extern int32_t {entry_symbol}(void);\n\
static const char *stasis_executable_path(char *buffer, size_t capacity, const char *fallback) {{\n\
#if defined(__APPLE__)\n\
    uint32_t size = (uint32_t)capacity;\n\
    if (_NSGetExecutablePath(buffer, &size) == 0) return buffer;\n\
#elif defined(__linux__)\n\
    ssize_t count = readlink(\"/proc/self/exe\", buffer, capacity - 1);\n\
    if (count > 0 && (size_t)count < capacity) {{ buffer[count] = 0; return buffer; }}\n\
#endif\n\
    return fallback ? fallback : \"\";\n\
}}\n\
static void stasis_log_package_provenance(const char *program) {{\n\
    char executable[4096];\n\
    char path[4096];\n\
    const char *resolved = stasis_executable_path(executable, sizeof(executable), program);\n\
    const char *slash = strrchr(resolved, '/');\n\
    size_t directory = slash ? (size_t)(slash - resolved + 1) : 0;\n\
    const char *name = \"stasis_provenance.json\";\n\
    if (directory + strlen(name) >= sizeof(path)) return;\n\
    if (directory) memcpy(path, resolved, directory);\n\
    strcpy(path + directory, name);\n\
    FILE *file = fopen(path, \"rb\");\n\
    if (!file) return;\n\
    char manifest[65537];\n\
    size_t count = fread(manifest, 1, sizeof(manifest) - 1, file);\n\
    int overflow = fgetc(file) != EOF;\n\
    fclose(file);\n\
    if (overflow) {{ fprintf(stderr, \"Stasis package provenance is invalid: manifest exceeds 65536 bytes path=%s\\n\", path); return; }}\n\
    manifest[count] = 0;\n\
    fprintf(stderr, \"Stasis package provenance: path=%s manifest=%s\\n\", path, manifest);\n\
}}\n\
int main(int argc, char **argv) {{ (void)argc; stasis_log_package_provenance(argv ? argv[0] : 0); return (int){entry_symbol}(); }}\n"
    ))
}

fn resolve_linker_path(config: &AotLinkConfig) -> PathBuf {
    if let Some(path) = config.linker_path.as_ref() {
        return path.clone();
    }
    if config.target.is_android() {
        PathBuf::from("clang")
    } else if cfg!(windows) {
        PathBuf::from("lld-link.exe")
    } else {
        PathBuf::from("cc")
    }
}

fn uses_msvc_linker_syntax(config: &AotLinkConfig) -> bool {
    matches!(config.target, AotTarget::Native) && cfg!(windows)
}

#[cfg(windows)]
fn resolve_windows_link_lib_paths() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(not(windows))]
fn resolve_windows_link_lib_paths() -> Vec<PathBuf> {
    Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_runner::swap::contracts::{FunctionPatch, FunctionPatchSet};
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn native_unix_launcher_calls_the_requested_entry_symbol() {
        let source = native_unix_executable_launcher_source("aot_fn_0")
            .expect("valid AOT symbol should produce launcher source");
        assert!(source.contains("extern int32_t aot_fn_0(void);"));
        assert!(source.contains("return (int)aot_fn_0();"));
        assert!(source.contains("stasis_provenance.json"));
        assert!(source.contains("manifest exceeds 65536 bytes"));
        assert!(source.contains("/proc/self/exe"));
        assert!(source.contains("_NSGetExecutablePath"));
    }

    #[test]
    fn native_unix_launcher_rejects_non_identifier_entry_symbol() {
        let error = native_unix_executable_launcher_source("aot_fn_0; injected")
            .expect_err("invalid C symbol should be rejected");
        assert!(error.contains("not a C identifier"));
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
  if "!ARG:~0,1!"=="@" (
    for /f "usebackq delims=" %%L in ("!ARG:~1!") do (
      set LINE=%%~L
      echo !LINE! | findstr /B /C:"/OUT:" >nul
      if !errorlevel! == 0 (
        set OUT=!LINE:~5!
      )
    )
  ) else (
    echo !ARG! | findstr /B /C:"/OUT:" >nul
    if !errorlevel! == 0 (
      set OUT=!ARG:~5!
    )
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
            target: AotTarget::default(),
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

    #[test]
    fn android_aot_linker_defaults_to_clang_when_unset() {
        let config = AotLinkConfig {
            linker_path: None,
            runtime_lib_paths: vec![],
            target: AotTarget::android_arm64_default(),
        };

        assert_eq!(resolve_linker_path(&config), PathBuf::from("clang"));
    }

    #[test]
    fn aot_target_reports_position_independent_code_requirement() {
        assert_eq!(
            AotTarget::Native.requires_position_independent_code(),
            cfg!(target_os = "macos")
        );
        assert!(AotTarget::android_arm64_default().requires_position_independent_code());
        assert!(AotTarget::android_x86_64_default().requires_position_independent_code());
        assert!(AotTarget::ios_arm64_default().requires_position_independent_code());
    }

    #[test]
    fn android_x86_64_target_reports_emulator_triples() {
        let target = AotTarget::android_x86_64_default();
        assert_eq!(target.object_triple(), Some("x86_64-linux-android"));
        assert_eq!(
            target.clang_target().as_deref(),
            Some("x86_64-linux-android26")
        );
        assert!(target.is_android());
    }

    #[test]
    fn ios_aot_target_reports_apple_arm64_triple() {
        let target = AotTarget::ios_arm64_default();
        assert_eq!(target.object_triple(), Some("aarch64-apple-ios"));
        assert_eq!(target.clang_target().as_deref(), Some("aarch64-apple-ios"));
    }

    #[test]
    fn native_aot_linker_default_stays_host_appropriate() {
        let config = AotLinkConfig {
            linker_path: None,
            runtime_lib_paths: vec![],
            target: AotTarget::default(),
        };
        let expected = if cfg!(windows) {
            PathBuf::from("lld-link.exe")
        } else {
            PathBuf::from("cc")
        };

        assert_eq!(resolve_linker_path(&config), expected);
    }
}

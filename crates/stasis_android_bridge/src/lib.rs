use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[cfg(test)]
unsafe extern "C" {
    /// The same native contract implementation used by the JIT extern.
    /// Keeping this declaration test-only prevents a second trace algorithm
    /// from entering the Android bridge while allowing the real sample test
    /// to validate the copied direct buffers.
    fn stasis_render_trace_native(
        cmd_i32: *const i32,
        cmd_f32: *const f32,
        cmd_u8: *const u8,
    ) -> u32;
}

use stasis_assets::{AssetFormat, AssetHandle, AssetLimits, ResolvedAssetManifest};
#[cfg(test)]
use stasis_compiler::backend::development_swap::DevelopmentSwapStatus;
use stasis_compiler::backend::development_swap::{
    commit_development_swap, DevelopmentSwapDescriptor, DevelopmentSwapHost, DevelopmentSwapReceipt,
};
use stasis_compiler::backend::jit::JitProcess;
#[cfg(test)]
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
use stasis_compiler::frontend::workshop::{
    find_workshop_references, load_workshop_edit_workspace, load_workshop_project_with_diagnostic,
    plan_workshop_semantic_edits, workshop_reachable_files, workshop_source_items,
    write_workshop_semantic_plan, write_workshop_semantic_receipt, WorkshopReload,
    WorkshopSemanticEditBatch, WorkshopSemanticEditPlan, WorkshopSourceFile,
};
#[cfg(test)]
use stasis_compiler::frontend::workshop::{
    WorkshopSemanticEdit, WorkshopSemanticEditOperation, WorkshopSourceItemKind,
    WorkshopSymbolSelector,
};
use stasis_dynload::StasisAudioHostApi;

pub const ANDROID_RENDER_COMMAND_CAPACITY: usize = 8;
pub const ANDROID_RENDER_FRAME_HEADER_SIZE: usize = 6;
pub const ANDROID_RENDER_COMMAND_STRIDE: usize = 13;
pub const ANDROID_RENDER_FRAME_I32_CAPACITY: usize = ANDROID_RENDER_FRAME_HEADER_SIZE
    + ANDROID_RENDER_COMMAND_CAPACITY * ANDROID_RENDER_COMMAND_STRIDE;
pub const ANDROID_RENDER_GFX_I32_CAPACITY: usize = stasis_dynload::STASIS_RENDER_I32_COUNT;
pub const ANDROID_RENDER_GFX_F32_CAPACITY: usize = stasis_dynload::STASIS_RENDER_F32_COUNT;
pub const ANDROID_RENDER_GFX_U8_CAPACITY: usize = stasis_dynload::STASIS_RENDER_U8_COUNT;
pub const ANDROID_RENDER_I_FRAME_TOKEN: usize = 26;

#[no_mangle]
pub extern "C" fn stasis_android_bridge_install_audio_api(api: *const StasisAudioHostApi) -> i32 {
    if api.is_null() {
        stasis_dynload::install_audio_host_api(None);
        return 1;
    }
    stasis_dynload::install_audio_host_api(Some(unsafe { *api }));
    1
}

#[derive(Debug)]
enum AndroidBridgeError {
    Plain(String),
    Source(stasis_compiler::SourceDiagnostic),
    Phase {
        stage: &'static str,
        symbol: &'static str,
        detail: String,
        resource: Option<String>,
    },
}

impl From<String> for AndroidBridgeError {
    fn from(value: String) -> Self {
        Self::Plain(value)
    }
}

impl From<&str> for AndroidBridgeError {
    fn from(value: &str) -> Self {
        Self::Plain(value.to_string())
    }
}

impl AndroidBridgeError {
    fn phase(
        stage: &'static str,
        symbol: &'static str,
        detail: impl Into<String>,
        resource: Option<String>,
    ) -> Self {
        Self::Phase {
            stage,
            symbol,
            detail: detail.into(),
            resource,
        }
    }
}

/// Stable identity for the real Rust bridge loaded by the Workshop JNI shim.
/// The pointer is backed by a static NUL-terminated package-version literal.
#[no_mangle]
pub extern "C" fn stasis_android_bridge_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidBridgeTickInput {
    pub touch_x: i32,
    pub touch_y: i32,
    pub touch_active: i32,
    pub screen_w: i32,
    pub screen_h: i32,
}

#[derive(Debug, Clone, Copy)]
struct AndroidDisplayMetrics {
    logical_w: i32,
    logical_h: i32,
    native_w: i32,
    native_h: i32,
    drawable_w: i32,
    drawable_h: i32,
    viewport_x: f32,
    viewport_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    content_scale: f32,
    raster_scale: f32,
}

impl AndroidDisplayMetrics {
    fn new(logical_w: i32, logical_h: i32, native_w: i32, native_h: i32) -> Self {
        let logical_w = logical_w.max(1);
        let logical_h = logical_h.max(1);
        let native_w = native_w.max(1);
        let native_h = native_h.max(1);
        let fit_scale =
            (native_w as f32 / logical_w as f32).min(native_h as f32 / logical_h as f32);
        let viewport_w = (logical_w as f32 * fit_scale).round().max(1.0);
        let viewport_h = (logical_h as f32 * fit_scale).round().max(1.0);
        let viewport_x = ((native_w as i32 - viewport_w as i32) / 2) as f32;
        let viewport_y = ((native_h as i32 - viewport_h as i32) / 2) as f32;
        let content_scale = (viewport_w / logical_w as f32).min(viewport_h / logical_h as f32);
        Self {
            logical_w,
            logical_h,
            native_w,
            native_h,
            drawable_w: native_w,
            drawable_h: native_h,
            viewport_x,
            viewport_y,
            viewport_w,
            viewport_h,
            content_scale,
            raster_scale: content_scale.clamp(1.0, 8.0),
        }
    }

    fn native_to_logical(self, x: i32, y: i32) -> (f32, f32) {
        let x = ((x as f32 - self.viewport_x) * self.logical_w as f32 / self.viewport_w)
            .clamp(0.0, self.logical_w as f32);
        let y = ((y as f32 - self.viewport_y) * self.logical_h as f32 / self.viewport_h)
            .clamp(0.0, self.logical_h as f32);
        (x, y)
    }

    fn signature(self) -> [i32; 6] {
        [
            self.logical_w,
            self.logical_h,
            self.native_w,
            self.native_h,
            self.drawable_w,
            self.drawable_h,
        ]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AndroidBridgeRenderCommand {
    pub kind: i32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub color: i32,
    pub asset: i32,
    pub rotation_degrees: i32,
    pub alpha: i32,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: i32,
    pub clip_h: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidBridgeRunTickResult {
    pub tick_count: i32,
    pub recompiled: bool,
    pub initialized: bool,
    pub observed_game_tick_count: i32,
    pub render_command_count: i32,
    pub render_commands: [AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY],
}

struct AndroidRuntimeSession {
    project_root: PathBuf,
    source_fingerprint: u64,
    generation: u64,
    jit: JitProcess,
    initialized: bool,
    pending_candidate: Option<JitProcess>,
    pending_source_fingerprint: Option<u64>,
    pending_resource_catalog: Option<EmbeddedResourceCatalog>,
    last_swap_receipt: Option<DevelopmentSwapReceipt>,
    tick_count: i32,
    previous_input: Option<AndroidBridgeTickInput>,
    display_metrics: AndroidDisplayMetrics,
    display_signature: [i32; 6],
    display_generation: i32,
    density_scale_bits: u32,
    density_generation: i32,
}

thread_local! {
    static RUNTIME_SESSION: RefCell<Option<AndroidRuntimeSession>> = const { RefCell::new(None) };
    static LAST_FRAME_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    #[cfg(test)]
    static FORCE_NEXT_MANIFEST_COMMIT_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    #[cfg(test)]
    static ANDROID_ARTIFACT_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidBridgeCompileResult {
    pub status: i32,
    pub reload: WorkshopReload,
    pub manifest_path: PathBuf,
    pub runtime_state_path: PathBuf,
    pub compiled_function_count: usize,
    pub compile_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct AndroidJitArtifactSummary {
    symbol_id: String,
    function_id: u32,
    source_path: String,
    name: String,
    signature_hash: u64,
    slot: u32,
    body_hash: u64,
    executable_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct AndroidJitArtifactManifestV1 {
    schema_version: u32,
    artifacts: Vec<AndroidJitArtifactSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidJitCompileSummary {
    source_revision: u64,
    layout_hash: u64,
    emitted_function_count: usize,
    reused_function_count: usize,
    executable_bytes: usize,
    artifacts: Vec<AndroidJitArtifactSummary>,
    entrypoints: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreviousAndroidCompile {
    project_hash: u64,
    layout_hash: u64,
}

pub fn load_android_workshop_asset_manifest(
    project_root: impl AsRef<Path>,
) -> Result<ResolvedAssetManifest, String> {
    stasis_assets::load_project_asset_manifest(project_root, AssetLimits::default())
        .map_err(|error| error.to_string())
}

pub fn resolve_android_workshop_sprite_asset(
    project_root: impl AsRef<Path>,
    handle: i32,
) -> Result<serde_json::Value, String> {
    let handle = AssetHandle::from_i32(handle)
        .ok_or_else(|| "sprite asset handle must be nonzero".to_string())?;
    let manifest = load_android_workshop_asset_manifest(project_root)?;
    let asset = manifest.by_handle(handle).ok_or_else(|| {
        format!(
            "sprite asset handle {} is not in the manifest",
            handle.get()
        )
    })?;
    let (encoding, width, height, layout) = match asset.entry.format {
        AssetFormat::Sprite {
            encoding,
            width,
            height,
            layout,
        } => (
            format!("{encoding:?}").to_ascii_lowercase(),
            width,
            height,
            layout,
        ),
        AssetFormat::Audio { .. } => {
            return Err(format!(
                "asset handle {} identifies audio, not a sprite",
                handle.get()
            ));
        }
        AssetFormat::Font { .. } => {
            return Err(format!(
                "asset handle {} identifies a font, not a sprite",
                handle.get()
            ));
        }
    };
    Ok(serde_json::json!({
        "status": "ok",
        "handle": handle.as_i32(),
        "id": asset.entry.id,
        "path": asset.absolute_path,
        "content_sha256": asset.entry.content_sha256,
        "byte_length": asset.byte_length,
        "encoding": encoding,
        "width": width,
        "height": height,
        "layout": layout.map(|layout| serde_json::json!({
            "columns": layout.columns,
            "rows": layout.rows,
        })),
    }))
}

pub fn run_android_workshop_stasis_tests(
    project_root: impl AsRef<Path>,
) -> Result<serde_json::Value, String> {
    let project_root = project_root.as_ref();
    let test_root = project_root.join("tests");
    let mut test_files = Vec::new();
    collect_stasis_test_files(&test_root, &mut test_files)?;
    test_files.sort();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::new();
    for path in test_files {
        let relative_path = path
            .strip_prefix(project_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed reading Stasis test {}: {error}", path.display()))?;
        let (rewritten, tests) = match rewrite_top_level_test_declarations(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                failed += 1;
                let offset = diagnostic_offset(&source, &error);
                let (line, column) = source_line_column(&source, offset);
                results.push(serde_json::json!({
                    "file": relative_path,
                    "line": line,
                    "column": column,
                    "name": diagnostic_symbol(&source, offset, &error),
                    "passed": false,
                    "status": "compile_failed",
                    "error": error,
                }));
                continue;
            }
        };
        if tests.is_empty() {
            continue;
        }
        let mut jit = JitProcess::new();
        jit.set_local_runtime_helper_trampolines(true);
        jit.set_project_root(project_root.to_string_lossy().to_string())?;
        jit.upsert_file(relative_path.clone(), rewritten);
        jit.set_required_emit_roots(
            &tests
                .iter()
                .map(|test| test.generated_function_name.clone())
                .collect::<Vec<_>>(),
        );
        if let Err(error) = jit.compile() {
            failed += tests.len();
            let message = format!("{error:?}");
            let diagnostic = jit.last_source_diagnostic();
            let test = diagnostic.and_then(|diagnostic| {
                tests
                    .iter()
                    .find(|test| test.generated_function_name == diagnostic.symbol)
            });
            let offset = test
                .map(|test| test.declaration_range.start)
                .unwrap_or_else(|| diagnostic_offset(&source, &message));
            let (line, column) = source_line_column(&source, offset);
            results.push(serde_json::json!({
                "file": relative_path,
                "line": line,
                "column": column,
                "name": test
                    .map(|test| test.display_name.clone())
                    .unwrap_or_else(|| diagnostic_symbol(&source, offset, &message)),
                "passed": false,
                "status": "compile_failed",
                "error": message,
            }));
            continue;
        }
        for test in tests {
            let line = 1 + source[..test.declaration_range.start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            match jit.execute_bool_noarg_by_name(&test.generated_function_name) {
                Ok(true) => {
                    passed += 1;
                    results.push(serde_json::json!({"file": relative_path, "line": line, "name": test.display_name, "passed": true}));
                }
                Ok(false) => {
                    failed += 1;
                    results.push(serde_json::json!({"file": relative_path, "line": line, "name": test.display_name, "passed": false}));
                }
                Err(error) => {
                    failed += 1;
                    results.push(serde_json::json!({"file": relative_path, "line": line, "name": test.display_name, "passed": false, "error": error}));
                }
            }
        }
    }
    Ok(
        serde_json::json!({"kind": "stasis_test_run", "passed": passed, "failed": failed, "all_passed": failed == 0 && passed > 0, "results": results}),
    )
}

pub fn android_workshop_source_items(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
) -> Result<serde_json::Value, String> {
    let files = load_workshop_edit_workspace(project_root.as_ref(), entry_file.as_ref())?;
    let editable = files
        .into_iter()
        .filter(|file| {
            let path = file.path.replace('\\', "/");
            path.starts_with("src/") || path.starts_with("tests/")
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema_version": 1,
        "items": workshop_source_items(&editable)?,
    }))
}

pub fn android_workshop_references(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    symbol: &str,
    limit: usize,
) -> Result<serde_json::Value, String> {
    let files = load_workshop_edit_workspace(project_root.as_ref(), entry_file.as_ref())?;
    let references = find_workshop_references(&files, symbol, limit)?
        .into_iter()
        .map(|reference| {
            serde_json::json!({
                "kind": reference.kind,
                "file": reference.file,
                "containing_kind": reference.containing_kind,
                "containing_name": reference.containing_name,
                "containing_signature": reference.containing_signature,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema_version": 1,
        "symbol": symbol,
        "references": references,
    }))
}

pub fn execute_android_workshop_semantic_edit(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    batch: &WorkshopSemanticEditBatch,
    dry_run: bool,
    validate: bool,
    run_tests: bool,
) -> Result<serde_json::Value, String> {
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();
    let files = load_workshop_edit_workspace(project_root, entry_file)?;
    let (after, plan) = plan_workshop_semantic_edits(&files, batch)?;
    let reachable = workshop_reachable_files(&after, entry_file)?;
    let source_fingerprint = fingerprint_workshop_sources(&reachable);
    build_runtime_session(project_root, &reachable, source_fingerprint)
        .map_err(|error| format_android_bridge_error(project_root, error))?;
    if dry_run {
        return Ok(serde_json::json!({
            "schema_version": 1,
            "status": "preview",
            "validated": true,
            "plan": plan,
        }));
    }

    write_workshop_semantic_plan(project_root, &plan, false)?;
    if !validate {
        return Ok(serde_json::json!({
            "schema_version": 1,
            "status": "applied",
            "validation": "pending_batch_compile",
            "plan": plan,
        }));
    }

    let validation = (|| {
        let compile = compile_android_workshop_project(project_root, entry_file)?;
        if run_tests {
            let tests = run_android_workshop_stasis_tests(project_root)?;
            if tests["all_passed"] != true {
                return Err(format!("Stasis tests failed: {tests}"));
            }
            Ok(serde_json::json!({
                "compiler": "passed",
                "reload": compile.reload,
                "tests": tests,
            }))
        } else {
            Ok(serde_json::json!({
                "compiler": "passed",
                "reload": compile.reload,
                "tests": "skipped",
            }))
        }
    })();
    let validation = match validation {
        Ok(value) => value,
        Err(error) => {
            write_workshop_semantic_plan(project_root, &plan, true).map_err(|rollback| {
                format!("semantic edit failed: {error}; rollback failed: {rollback}")
            })?;
            compile_android_workshop_project(project_root, entry_file).map_err(|restore| {
                format!("semantic edit failed: {error}; restored compile failed: {restore}")
            })?;
            return Err(format!(
                "semantic edit validation failed and sources were rolled back: {error}"
            ));
        }
    };
    let receipt = match write_android_semantic_receipt(project_root, &plan) {
        Ok(receipt) => receipt,
        Err(error) => {
            write_workshop_semantic_plan(project_root, &plan, true).map_err(|rollback| {
                format!(
                    "failed writing semantic edit receipt: {error}; rollback failed: {rollback}"
                )
            })?;
            compile_android_workshop_project(project_root, entry_file).map_err(|restore| {
                format!(
                    "failed writing semantic edit receipt: {error}; restored compile failed: {restore}"
                )
            })?;
            return Err(format!(
                "failed writing semantic edit receipt; sources and runtime rolled back: {error}"
            ));
        }
    };
    Ok(serde_json::json!({
        "schema_version": 1,
        "status": "applied",
        "validation": validation,
        "receipt": receipt,
        "plan": plan,
    }))
}

fn write_android_semantic_receipt(
    project_root: &Path,
    plan: &WorkshopSemanticEditPlan,
) -> Result<String, String> {
    write_workshop_semantic_receipt(project_root, Path::new("build/semantic-edits"), plan)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn collect_stasis_test_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)
        .map_err(|error| format!("failed reading test directory {}: {error}", root.display()))?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            collect_stasis_test_files(&path, out)?;
        } else if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".test.stasis"))
        {
            out.push(path);
        }
    }
    Ok(())
}

pub fn compile_android_workshop_project(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
) -> Result<AndroidBridgeCompileResult, String> {
    let started = Instant::now();
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();
    let files = load_workshop_project_with_diagnostic(project_root, entry_file)
        .map_err(|diagnostic| format_compiler_source_diagnostic(project_root, &diagnostic))?;
    let source_fingerprint = fingerprint_workshop_sources(&files);
    discard_pending_runtime_candidate_if_different(project_root, source_fingerprint);
    let previous = read_previous_android_plan(project_root)?;
    let had_runtime_session = has_runtime_session_for_project(project_root);
    warm_or_reload_runtime_session(project_root, &files, source_fingerprint)
        .map_err(|error| format_android_bridge_error(project_root, error))?;
    let finalized = (|| {
        let summary = current_android_jit_compile_summary(project_root, source_fingerprint)?;
        let reload = match (had_runtime_session, previous) {
            (false, _) => WorkshopReload::InitialCompile,
            (true, None) => WorkshopReload::InitialCompile,
            (true, Some(previous)) if previous.project_hash == source_fingerprint => {
                WorkshopReload::NoChange
            }
            (true, Some(previous)) if previous.layout_hash == summary.layout_hash => {
                WorkshopReload::FastReload
            }
            (true, Some(_)) => WorkshopReload::ResetRequired,
        };
        let manifest_path = project_root.join("build/native_compile_manifest.txt");
        let runtime_state_path = project_root.join("build/runtime_state.txt");
        if let Some(parent) = manifest_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed creating Android manifest directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        remove_legacy_stub_artifacts(project_root)?;
        let previous_runtime_state = fs::read(&runtime_state_path).ok();
        if matches!(
            reload,
            WorkshopReload::InitialCompile | WorkshopReload::ResetRequired
        ) {
            if let Some(parent) = runtime_state_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "failed creating Android runtime state directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            let runtime_state = format!(
                "status=RuntimeStateReady\nproject_hash={source_fingerprint:016x}\nreload={reload:?}\ntick_count=0\n"
            );
            if let Err(error) = fs::write(&runtime_state_path, runtime_state.as_bytes()) {
                let primary = format!(
                    "failed writing Android runtime state {}: {error}",
                    runtime_state_path.display()
                );
                return Err(restore_after_error(
                    primary,
                    &runtime_state_path,
                    previous_runtime_state.as_deref(),
                ));
            }
        }
        let artifact_manifest = serde_json::to_string_pretty(&AndroidJitArtifactManifestV1 {
            schema_version: 1,
            artifacts: summary.artifacts.clone(),
        })
        .map_err(|error| format!("failed serializing Android artifact manifest: {error}"))?;
        let artifact_hash = hex_sha256(artifact_manifest.as_bytes());
        let artifact_file_name =
            format!("native_compile_artifacts.v1.{source_fingerprint:016x}.{artifact_hash}.json");
        let artifact_manifest_path = project_root.join("build").join(&artifact_file_name);
        #[cfg(test)]
        if take_android_artifact_fault(1) {
            return Err("forced failure before immutable artifact publication".to_string());
        }
        if let Err(primary) =
            write_immutable_android_artifact(&artifact_manifest_path, artifact_manifest.as_bytes())
        {
            return Err(restore_after_error(
                primary,
                &runtime_state_path,
                previous_runtime_state.as_deref(),
            ));
        }
        #[cfg(test)]
        if take_android_artifact_fault(2) {
            return Err(restore_after_error(
                "forced failure after immutable artifact publication".to_string(),
                &runtime_state_path,
                previous_runtime_state.as_deref(),
            ));
        }
        let manifest = render_android_jit_manifest(
            project_root,
            source_fingerprint,
            reload,
            &summary,
            &artifact_file_name,
            &artifact_hash,
        );
        #[cfg(test)]
        if take_android_artifact_fault(3) {
            return Err(restore_after_error(
                "forced failure before authoritative manifest publication".to_string(),
                &runtime_state_path,
                previous_runtime_state.as_deref(),
            ));
        }
        if let Err(primary) = write_android_manifest_atomically(&manifest_path, &manifest, true) {
            return Err(restore_after_error(
                primary,
                &runtime_state_path,
                previous_runtime_state.as_deref(),
            ));
        }
        Ok::<_, String>((summary, reload, manifest_path, runtime_state_path))
    })();
    let (summary, reload, manifest_path, runtime_state_path) = match finalized {
        Ok(finalized) => finalized,
        Err(error) => {
            reject_staged_runtime_compile(project_root, source_fingerprint, had_runtime_session);
            return Err(error);
        }
    };

    Ok(AndroidBridgeCompileResult {
        status: 0,
        reload,
        manifest_path,
        runtime_state_path,
        compiled_function_count: summary.artifacts.len(),
        compile_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
    })
}

#[cfg(test)]
fn take_android_artifact_fault(expected: u8) -> bool {
    ANDROID_ARTIFACT_FAULT.with(|fault| {
        if fault.get() == expected {
            fault.set(0);
            true
        } else {
            false
        }
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_immutable_android_artifact(path: &Path, contents: &[u8]) -> Result<(), String> {
    if path.exists() {
        let existing = fs::read(path).map_err(|error| {
            format!(
                "failed reading immutable artifact {}: {error}",
                path.display()
            )
        })?;
        return if existing == contents {
            Ok(())
        } else {
            Err(format!(
                "immutable Android artifact collision at {}",
                path.display()
            ))
        };
    }
    write_android_manifest_atomically(
        path,
        std::str::from_utf8(contents)
            .map_err(|error| format!("artifact manifest is not UTF-8: {error}"))?,
        false,
    )
}

fn restore_after_error(primary: String, path: &Path, contents: Option<&[u8]>) -> String {
    match restore_optional_file(path, contents) {
        Ok(()) => primary,
        Err(rollback) => format!("{primary}; rollback failed: {rollback}"),
    }
}

fn restore_optional_file(path: &Path, contents: Option<&[u8]>) -> Result<(), String> {
    if let Some(contents) = contents {
        fs::write(path, contents)
            .map_err(|error| format!("failed restoring {}: {error}", path.display()))
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("failed removing {}: {error}", path.display())),
        }
    }
}

fn write_android_manifest_atomically(
    path: &Path,
    manifest: &str,
    allow_forced_failure: bool,
) -> Result<(), String> {
    use std::io::Write;
    #[cfg(not(test))]
    let _ = allow_forced_failure;

    let mut file = atomic_write_file::AtomicWriteFile::open(path).map_err(|error| {
        format!(
            "failed staging Android manifest {}: {error}",
            path.display()
        )
    })?;
    file.write_all(manifest.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "failed staging Android manifest {}: {error}",
                path.display()
            )
        })?;
    #[cfg(test)]
    if allow_forced_failure
        && FORCE_NEXT_MANIFEST_COMMIT_FAILURE.with(|forced| forced.replace(false))
    {
        return Err(format!(
            "failed writing Android manifest {}: forced atomic commit failure",
            path.display()
        ));
    }
    file.commit().map_err(|error| {
        format!(
            "failed writing Android manifest {}: {error}",
            path.display()
        )
    })
}

pub fn run_android_workshop_tick(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    input: AndroidBridgeTickInput,
) -> Result<AndroidBridgeRunTickResult, String> {
    let project_root = project_root.as_ref();
    run_android_workshop_tick_internal(project_root, entry_file, input, true)
        .map_err(|error| format_android_bridge_error(project_root, error))
}

const MAX_EMBEDDED_FONTS: usize = 64;
const MAX_EMBEDDED_TEXT_RUNS: usize = 4096;
const MAX_EMBEDDED_SPRITES: usize = 4096;
const MAX_PENDING_SPRITE_RELEASES: usize = 256;

#[derive(Clone)]
struct EmbeddedFont {
    handle: i32,
    path: PathBuf,
    size: i32,
}

#[derive(Clone)]
struct EmbeddedTextRun {
    handle: i32,
    font: i32,
    text: String,
    measured_width: f32,
    measured_height: f32,
}

#[derive(Clone)]
struct EmbeddedSpriteRef {
    handle: i32,
    refs: usize,
}

struct EmbeddedResourceCatalog {
    project_root: PathBuf,
    assets: ResolvedAssetManifest,
    fonts: Vec<EmbeddedFont>,
    text_runs: Vec<EmbeddedTextRun>,
    sprite_refs: Vec<EmbeddedSpriteRef>,
    pending_sprite_releases: Vec<i32>,
    pending_sprite_release_cancellations: Vec<i32>,
    error: Option<EmbeddedResourceError>,
}

#[derive(Debug, Clone)]
struct EmbeddedResourceError {
    detail: String,
    resource: Option<String>,
}

fn embedded_resource_catalog() -> &'static Mutex<Option<EmbeddedResourceCatalog>> {
    static CATALOG: OnceLock<Mutex<Option<EmbeddedResourceCatalog>>> = OnceLock::new();
    CATALOG.get_or_init(|| Mutex::new(None))
}

fn install_embedded_resource_host(project_root: &Path) -> Result<(), String> {
    let catalog = prepare_embedded_resource_catalog(project_root, false)?;
    *embedded_resource_catalog()
        .lock()
        .map_err(|_| "embedded resource catalog mutex poisoned")? = Some(catalog);
    stasis_dynload::set_embedded_graphics_host(Some(stasis_dynload::EmbeddedGraphicsHost {
        load_sprite: embedded_load_sprite,
        release_sprite: embedded_release_sprite,
        load_font: embedded_load_font,
        measure_text: embedded_measure_text,
        cache_text: embedded_cache_text,
        measure_text_cached: embedded_measure_text_cached,
        measure_text_cached_height: embedded_measure_text_cached_height,
        poll_reload: |_| 0,
    }));
    Ok(())
}

fn prepare_embedded_resource_catalog(
    project_root: &Path,
    preserve_loaded_resources: bool,
) -> Result<EmbeddedResourceCatalog, String> {
    let project_root = project_root
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize embedded project root: {error}"))?;
    let manifest_path = project_root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH);
    let assets = if manifest_path.is_file() {
        load_android_workshop_asset_manifest(&project_root)?
    } else {
        ResolvedAssetManifest {
            manifest_path,
            dynamic_assets: Default::default(),
            assets: Vec::new(),
        }
    };
    let (
        fonts,
        text_runs,
        sprite_refs,
        pending_sprite_releases,
        pending_sprite_release_cancellations,
    ) = if preserve_loaded_resources {
        let slot = embedded_resource_catalog()
            .lock()
            .map_err(|_| "embedded resource catalog mutex poisoned")?;
        slot.as_ref()
            .filter(|catalog| catalog.project_root == project_root)
            .map(|catalog| {
                (
                    catalog.fonts.clone(),
                    catalog.text_runs.clone(),
                    catalog.sprite_refs.clone(),
                    catalog.pending_sprite_releases.clone(),
                    catalog.pending_sprite_release_cancellations.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    Vec::with_capacity(MAX_EMBEDDED_FONTS),
                    Vec::with_capacity(MAX_EMBEDDED_TEXT_RUNS),
                    Vec::with_capacity(MAX_EMBEDDED_SPRITES),
                    Vec::with_capacity(MAX_PENDING_SPRITE_RELEASES),
                    Vec::with_capacity(MAX_EMBEDDED_SPRITES),
                )
            })
    } else {
        (
            Vec::with_capacity(MAX_EMBEDDED_FONTS),
            Vec::with_capacity(MAX_EMBEDDED_TEXT_RUNS),
            Vec::with_capacity(MAX_EMBEDDED_SPRITES),
            Vec::with_capacity(MAX_PENDING_SPRITE_RELEASES),
            Vec::with_capacity(MAX_EMBEDDED_SPRITES),
        )
    };
    Ok(EmbeddedResourceCatalog {
        project_root,
        assets,
        fonts,
        text_runs,
        sprite_refs,
        pending_sprite_releases,
        pending_sprite_release_cancellations,
        error: None,
    })
}

fn embedded_path(catalog: &EmbeddedResourceCatalog, bytes: &[u8]) -> Option<PathBuf> {
    let path = std::str::from_utf8(bytes).ok()?;
    [
        catalog.project_root.join(path),
        catalog.project_root.join("src").join(path),
    ]
    .into_iter()
    .find_map(|candidate| {
        let absolute = candidate.canonicalize().ok()?;
        absolute
            .starts_with(&catalog.project_root)
            .then_some(absolute)
    })
}

fn set_embedded_resource_error(catalog: &mut EmbeddedResourceCatalog, message: String) {
    if catalog.error.is_none() {
        catalog.error = Some(EmbeddedResourceError {
            detail: message,
            resource: None,
        });
    }
}

fn set_embedded_resource_path_error(
    catalog: &mut EmbeddedResourceCatalog,
    message: String,
    resource: &str,
) {
    if catalog.error.is_none() {
        catalog.error = Some(EmbeddedResourceError {
            detail: message,
            resource: Some(resource.to_string()),
        });
    }
}

fn take_embedded_resource_error() -> Result<(), EmbeddedResourceError> {
    let mut slot = embedded_resource_catalog()
        .lock()
        .map_err(|_| EmbeddedResourceError {
            detail: "embedded resource catalog mutex poisoned".to_string(),
            resource: None,
        })?;
    let catalog = slot.as_mut().ok_or_else(|| EmbeddedResourceError {
        detail: "embedded resource catalog is not initialized".to_string(),
        resource: None,
    })?;
    match catalog.error.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn resource_phase_error(symbol: &'static str, error: EmbeddedResourceError) -> AndroidBridgeError {
    AndroidBridgeError::phase(
        "resource",
        symbol,
        format!("render resource error: {}", error.detail),
        error.resource,
    )
}

fn embedded_load_sprite(path: &[u8], _max_w: i32, _max_h: i32) -> i32 {
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return 0;
    };
    let Some(catalog) = slot.as_mut() else {
        return 0;
    };
    let display_path = String::from_utf8_lossy(path);
    let Some(absolute) = embedded_path(catalog, path) else {
        set_embedded_resource_path_error(
            catalog,
            format!("sprite path is invalid or missing: {display_path}"),
            &display_path,
        );
        return 0;
    };
    let handle = catalog
        .assets
        .assets
        .iter()
        .find(|asset| {
            asset.absolute_path == absolute
                && matches!(asset.entry.format, AssetFormat::Sprite { .. })
        })
        .map_or(0, |asset| asset.handle.as_i32());
    if handle == 0 {
        set_embedded_resource_path_error(
            catalog,
            format!("sprite is not declared in the asset manifest: {display_path}"),
            &display_path,
        );
    } else if !embedded_acquire_sprite(catalog, handle) {
        return 0;
    }
    handle
}

fn embedded_acquire_sprite(catalog: &mut EmbeddedResourceCatalog, handle: i32) -> bool {
    if let Some(index) = catalog
        .sprite_refs
        .iter()
        .position(|entry| entry.handle == handle)
    {
        let Some(next_refs) = catalog.sprite_refs[index].refs.checked_add(1) else {
            set_embedded_resource_error(catalog, "sprite reference count overflow".to_string());
            return false;
        };
        let cancellation_queued = catalog
            .pending_sprite_release_cancellations
            .contains(&handle);
        if !cancellation_queued
            && catalog.pending_sprite_release_cancellations.len() >= MAX_EMBEDDED_SPRITES
        {
            set_embedded_resource_error(
                catalog,
                "pending sprite release cancellation queue is full".to_string(),
            );
            return false;
        }
        catalog.sprite_refs[index].refs = next_refs;
        catalog
            .pending_sprite_releases
            .retain(|queued| *queued != handle);
        if !cancellation_queued {
            catalog.pending_sprite_release_cancellations.push(handle);
        }
        return true;
    }

    let cancellation_queued = catalog
        .pending_sprite_release_cancellations
        .contains(&handle);
    if !cancellation_queued
        && catalog.pending_sprite_release_cancellations.len() >= MAX_EMBEDDED_SPRITES
    {
        set_embedded_resource_error(
            catalog,
            "pending sprite release cancellation queue is full".to_string(),
        );
        return false;
    }
    if catalog.sprite_refs.len() >= MAX_EMBEDDED_SPRITES {
        set_embedded_resource_error(catalog, "sprite registry is full".to_string());
        return false;
    }
    if !cancellation_queued {
        catalog.pending_sprite_release_cancellations.push(handle);
    }
    catalog
        .sprite_refs
        .push(EmbeddedSpriteRef { handle, refs: 1 });
    true
}

fn embedded_release_sprite(handle: i32) {
    if handle == 0 {
        return;
    }
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return;
    };
    let Some(catalog) = slot.as_mut() else {
        return;
    };
    let Some(index) = catalog
        .sprite_refs
        .iter()
        .position(|entry| entry.handle == handle)
    else {
        // Handles supplied directly by raw code were never acquired by this
        // typed ownership table and are deliberately ignored.
        return;
    };
    if catalog.sprite_refs[index].refs == 0 {
        return;
    }
    catalog.sprite_refs[index].refs -= 1;
    if catalog.sprite_refs[index].refs != 0 {
        return;
    }
    if catalog.pending_sprite_releases.contains(&handle) {
        return;
    }
    if catalog.pending_sprite_releases.len() >= MAX_EMBEDDED_SPRITES {
        set_embedded_resource_error(catalog, "pending sprite release queue is full".to_string());
        return;
    }
    catalog
        .pending_sprite_release_cancellations
        .retain(|queued| *queued != handle);
    catalog.pending_sprite_releases.push(handle);
}

fn take_embedded_sprite_releases() -> Vec<i32> {
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return Vec::new();
    };
    let Some(catalog) = slot.as_mut() else {
        return Vec::new();
    };
    let count = catalog
        .pending_sprite_releases
        .len()
        .min(MAX_PENDING_SPRITE_RELEASES);
    let releases = catalog
        .pending_sprite_releases
        .drain(..count)
        .collect::<Vec<_>>();
    for handle in &releases {
        catalog
            .sprite_refs
            .retain(|entry| entry.handle != *handle || entry.refs != 0);
    }
    releases
}

fn take_embedded_sprite_release_cancellations() -> Vec<i32> {
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return Vec::new();
    };
    let Some(catalog) = slot.as_mut() else {
        return Vec::new();
    };
    let count = catalog
        .pending_sprite_release_cancellations
        .len()
        .min(MAX_PENDING_SPRITE_RELEASES);
    catalog
        .pending_sprite_release_cancellations
        .drain(..count)
        .collect()
}

fn embedded_load_font(path: &[u8], size: i32) -> i32 {
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return 0;
    };
    let Some(catalog) = slot.as_mut() else {
        return 0;
    };
    let display_path = String::from_utf8_lossy(path);
    if size <= 0 {
        set_embedded_resource_path_error(
            catalog,
            format!("font size must be positive: {display_path}"),
            &display_path,
        );
        return 0;
    }
    let Some(absolute) = embedded_path(catalog, path) else {
        set_embedded_resource_path_error(
            catalog,
            format!("font path is invalid or missing: {display_path}"),
            &display_path,
        );
        return 0;
    };
    if !absolute.is_file() {
        set_embedded_resource_path_error(
            catalog,
            format!("font file is missing: {display_path}"),
            &display_path,
        );
        return 0;
    }
    if let Some(font) = catalog
        .fonts
        .iter()
        .find(|font| font.path == absolute && font.size == size)
    {
        return font.handle;
    }
    if catalog.fonts.len() >= MAX_EMBEDDED_FONTS {
        set_embedded_resource_error(catalog, "font registry is full".to_string());
        return 0;
    }
    let handle = catalog.fonts.len() as i32 + 1;
    catalog.fonts.push(EmbeddedFont {
        handle,
        path: absolute,
        size,
    });
    handle
}

fn embedded_measure_text(font: i32, text: &[u8]) -> f32 {
    let Ok(slot) = embedded_resource_catalog().lock() else {
        return 0.0;
    };
    let Some(catalog) = slot.as_ref() else {
        return 0.0;
    };
    let Some(font) = catalog.fonts.iter().find(|entry| entry.handle == font) else {
        return 0.0;
    };
    text.len() as f32 * font.size as f32 * 0.6
}

fn embedded_cache_text(font: i32, text: &[u8]) -> i32 {
    let Ok(mut slot) = embedded_resource_catalog().lock() else {
        return 0;
    };
    let Some(catalog) = slot.as_mut() else {
        return 0;
    };
    let Ok(text) = std::str::from_utf8(text) else {
        set_embedded_resource_error(catalog, "cached text is not valid UTF-8".to_string());
        return 0;
    };
    let Some(font_entry) = catalog.fonts.iter().find(|entry| entry.handle == font) else {
        set_embedded_resource_error(catalog, format!("font handle {font} was not loaded"));
        return 0;
    };
    if let Some(run) = catalog
        .text_runs
        .iter()
        .find(|run| run.font == font && run.text == text)
    {
        return run.handle;
    }
    if catalog.text_runs.len() >= MAX_EMBEDDED_TEXT_RUNS {
        set_embedded_resource_error(catalog, "cached text registry is full".to_string());
        return 0;
    }
    let handle = catalog.text_runs.len() as i32 + 1;
    let measured_width = text.len() as f32 * font_entry.size as f32 * 0.6;
    catalog.text_runs.push(EmbeddedTextRun {
        handle,
        font,
        text: text.to_string(),
        measured_width,
        measured_height: font_entry.size as f32,
    });
    handle
}

fn embedded_measure_text_cached(handle: i32) -> f32 {
    let Ok(slot) = embedded_resource_catalog().lock() else {
        return 0.0;
    };
    slot.as_ref()
        .and_then(|catalog| catalog.text_runs.iter().find(|run| run.handle == handle))
        .map_or(0.0, |run| run.measured_width)
}

fn embedded_measure_text_cached_height(handle: i32) -> f32 {
    let Ok(slot) = embedded_resource_catalog().lock() else {
        return 0.0;
    };
    slot.as_ref()
        .and_then(|catalog| catalog.text_runs.iter().find(|run| run.handle == handle))
        .map_or(0.0, |run| run.measured_height)
}

fn resolve_embedded_text_run(
    project_root: &Path,
    handle: i32,
) -> Result<serde_json::Value, String> {
    let root = project_root
        .canonicalize()
        .map_err(|error| format!("invalid project root: {error}"))?;
    let slot = embedded_resource_catalog()
        .lock()
        .map_err(|_| "embedded resource catalog mutex poisoned")?;
    let catalog = slot
        .as_ref()
        .ok_or_else(|| "embedded resource catalog is not initialized".to_string())?;
    if catalog.project_root != root {
        return Err("cached text belongs to a different project".to_string());
    }
    let run = catalog
        .text_runs
        .iter()
        .find(|run| run.handle == handle)
        .ok_or_else(|| format!("cached text handle {handle} was not loaded"))?;
    let font = catalog
        .fonts
        .iter()
        .find(|font| font.handle == run.font)
        .ok_or_else(|| format!("font handle {} was not loaded", run.font))?;
    Ok(serde_json::json!({
        "status": "ok",
        "handle": run.handle,
        "font": font.handle,
        "font_path": font.path,
        "font_size": font.size,
        "text": run.text,
        "measured_width": run.measured_width,
    }))
}

fn resolve_embedded_font(project_root: &Path, handle: i32) -> Result<serde_json::Value, String> {
    let root = project_root
        .canonicalize()
        .map_err(|error| format!("invalid project root: {error}"))?;
    let slot = embedded_resource_catalog()
        .lock()
        .map_err(|_| "embedded resource catalog mutex poisoned")?;
    let catalog = slot
        .as_ref()
        .ok_or_else(|| "embedded resource catalog is not initialized".to_string())?;
    if catalog.project_root != root {
        return Err("font belongs to a different project".to_string());
    }
    let font = catalog
        .fonts
        .iter()
        .find(|font| font.handle == handle)
        .ok_or_else(|| format!("font handle {handle} was not loaded"))?;
    Ok(serde_json::json!({
        "status": "ok",
        "handle": font.handle,
        "font_path": font.path,
        "font_size": font.size,
    }))
}

fn run_android_workshop_tick_internal(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    input: AndroidBridgeTickInput,
    read_workshop_render_commands: bool,
) -> Result<AndroidBridgeRunTickResult, AndroidBridgeError> {
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();

    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        let mut recompiled = false;
        let needs_lazy_build = session_slot
            .as_ref()
            .is_none_or(|session| session.project_root != project_root);
        if needs_lazy_build {
            let files = load_workshop_project_with_diagnostic(project_root, entry_file)
                .map_err(AndroidBridgeError::Source)?;
            let source_fingerprint = fingerprint_workshop_sources(&files);
            *session_slot = Some(build_runtime_session(
                project_root,
                &files,
                source_fingerprint,
            )?);
            recompiled = true;
        }

        let session = session_slot
            .as_mut()
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
        let swapped_code = activate_pending_runtime_candidate(session)?;
        if swapped_code {
            recompiled = true;
        }
        let initialized = if session.initialized {
            false
        } else {
            write_production_host_frame(session, input)?;
            execute_lifecycle_noarg(&session.jit, "main")
                .map_err(|error| AndroidBridgeError::phase("runtime_entry", "main", error, None))?;
            take_embedded_resource_error().map_err(|error| resource_phase_error("main", error))?;
            session.initialized = true;
            session.previous_input = None;
            session.display_generation = 0;
            session.density_generation = 0;
            true
        };
        let metrics = write_production_host_frame(session, input)?;
        if read_workshop_render_commands {
            let (touch_x, touch_y) = metrics.native_to_logical(input.touch_x, input.touch_y);
            session
                .jit
                .write_i32_global_path("Input.touch_x", touch_x.round() as i32);
            session
                .jit
                .write_i32_global_path("Input.touch_y", touch_y.round() as i32);
            session
                .jit
                .write_i32_global_path("Input.touch_active", input.touch_active);
            session
                .jit
                .write_i32_global_path("Input.screen_w", metrics.logical_w);
            session
                .jit
                .write_i32_global_path("Input.screen_h", metrics.logical_h);
        }
        execute_lifecycle_noarg(&session.jit, "tick")
            .map_err(|error| AndroidBridgeError::phase("runtime_entry", "tick", error, None))?;
        take_embedded_resource_error().map_err(|error| resource_phase_error("tick", error))?;
        session.tick_count = session.tick_count.saturating_add(1);
        execute_optional_lifecycle_noarg(&session.jit, "render")
            .map_err(|error| AndroidBridgeError::phase("runtime_entry", "render", error, None))?;
        take_embedded_resource_error().map_err(|error| resource_phase_error("render", error))?;
        let write_runtime_state = should_write_jit_runtime_state(initialized, recompiled);
        let observed_game_tick_count = if read_workshop_render_commands || write_runtime_state {
            session.jit.read_i32_global_path("GameState.tick_count")
        } else {
            0
        };
        let (render_command_count, render_commands) = if read_workshop_render_commands {
            (
                session.jit.read_i32_global_path("Render.command_count"),
                read_render_commands(&session.jit),
            )
        } else {
            (
                0,
                [AndroidBridgeRenderCommand::default(); ANDROID_RENDER_COMMAND_CAPACITY],
            )
        };
        if write_runtime_state {
            write_jit_runtime_state(
                project_root,
                session.tick_count,
                observed_game_tick_count,
                render_command_count,
                &render_commands,
            )?;
        }

        Ok(AndroidBridgeRunTickResult {
            tick_count: session.tick_count,
            recompiled,
            initialized,
            observed_game_tick_count,
            render_command_count,
            render_commands,
        })
    })
}

fn hash_global_path(path: &str) -> i32 {
    let mut hash = 2_166_136_261u32;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

fn write_production_host_frame(
    session: &mut AndroidRuntimeSession,
    input: AndroidBridgeTickInput,
) -> Result<AndroidDisplayMetrics, String> {
    const HOST_I32_COUNT: i32 = 768;
    const HOST_F32_COUNT: i32 = 64;
    const POINTER_I32_BASE: usize = 544;
    let host_i32_ptr = stasis_dynload::stasis_jit_global_i32_array_ptr(
        hash_global_path("host_i32"),
        0,
        HOST_I32_COUNT,
    );
    let host_f32_ptr = stasis_dynload::stasis_jit_global_f32_array_ptr(
        hash_global_path("host_f32"),
        0,
        HOST_F32_COUNT,
    );
    if host_i32_ptr.is_null() || host_f32_ptr.is_null() {
        return Err("production host frame buffers were not registered".to_string());
    }
    let host_i32 = unsafe { std::slice::from_raw_parts_mut(host_i32_ptr, HOST_I32_COUNT as usize) };
    let host_f32 = unsafe { std::slice::from_raw_parts_mut(host_f32_ptr, HOST_F32_COUNT as usize) };
    let requested_w = session.jit.read_i32_global_path("host_req_window_w_px");
    let requested_h = session.jit.read_i32_global_path("host_req_window_h_px");
    let metrics = AndroidDisplayMetrics::new(
        if requested_w > 0 {
            requested_w
        } else {
            input.screen_w
        },
        if requested_h > 0 {
            requested_h
        } else {
            input.screen_h
        },
        input.screen_w,
        input.screen_h,
    );
    let signature = metrics.signature();
    let resized = session.display_generation == 0 || signature != session.display_signature;
    if resized {
        session.display_generation = session.display_generation.saturating_add(1);
        session.display_signature = signature;
    }
    let raster_scale_bits = metrics.raster_scale.to_bits();
    if session.density_generation == 0 || raster_scale_bits != session.density_scale_bits {
        session.density_generation = session.density_generation.saturating_add(1);
        session.density_scale_bits = raster_scale_bits;
    }
    session.display_metrics = metrics;

    let previous = session.previous_input;
    let was_down = previous.is_some_and(|value| value.touch_active != 0);
    let is_down = input.touch_active != 0;
    let (touch_x, touch_y) = metrics.native_to_logical(input.touch_x, input.touch_y);
    // A new pointer contact starts a fresh gesture, so its delta lane is
    // deterministic zero even when the runtime previously rendered idle
    // frames (for example, the preceding ABI acceptance call).
    let (previous_x, previous_y) = if !was_down {
        (touch_x, touch_y)
    } else {
        previous.map_or((touch_x, touch_y), |value| {
            metrics.native_to_logical(value.touch_x, value.touch_y)
        })
    };
    host_i32[0] = stasis_dynload::stasis_get_time_ms();
    host_i32[7] = 1;
    host_i32[8] = 0;
    host_i32[9] = 0;
    host_i32[10] = session.tick_count;
    host_i32[11] = i32::from(resized);
    host_i32[12] = metrics.native_w;
    host_i32[13] = metrics.native_h;
    host_i32[14] = 3;
    host_i32[15] = 0;
    host_i32[16] = 60;
    host_i32[17] = 1;
    host_i32[18] = 0;
    host_i32[19] = stasis_dynload::stasis_get_time_us();
    host_i32[22] = metrics.native_w;
    host_i32[23] = metrics.native_h;
    host_i32[24] = metrics.drawable_w;
    host_i32[25] = metrics.drawable_h;
    host_i32[30] = session.display_generation;
    host_i32[31] = session.density_generation;
    host_i32[POINTER_I32_BASE] = 0;
    host_i32[POINTER_I32_BASE + 1] = i32::from(is_down);
    host_i32[POINTER_I32_BASE + 2] = i32::from(is_down && !was_down);
    host_i32[POINTER_I32_BASE + 3] = i32::from(!is_down && was_down);
    host_f32[0] = touch_x;
    host_f32[1] = touch_y;
    host_f32[2] = touch_x - previous_x;
    host_f32[3] = touch_y - previous_y;
    host_f32[4] = touch_x / metrics.logical_w as f32;
    host_f32[5] = touch_y / metrics.logical_h as f32;
    host_f32[48] = metrics.content_scale;
    host_f32[49] = metrics.raster_scale;
    host_f32[50] = metrics.logical_w as f32;
    host_f32[51] = metrics.logical_h as f32;
    host_f32[52] = 0.0;
    host_f32[53] = 0.0;
    host_f32[54] = metrics.logical_w as f32;
    host_f32[55] = metrics.logical_h as f32;
    session.previous_input = Some(input);
    Ok(metrics)
}

fn write_android_display_metadata(out: &mut [i32]) -> Result<(), String> {
    if out.len() <= ANDROID_RENDER_I_FRAME_TOKEN {
        return Err("render header is too small for display metadata".to_string());
    }
    let (metrics, display_generation, density_generation, frame_token) =
        RUNTIME_SESSION.with(|slot| {
            let slot = slot.borrow();
            let session = slot
                .as_ref()
                .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
            Ok::<_, String>((
                session.display_metrics,
                session.display_generation,
                session.density_generation,
                session.tick_count,
            ))
        })?;
    out[10] = metrics.logical_w;
    out[11] = metrics.logical_h;
    out[12] = metrics.native_w;
    out[13] = metrics.native_h;
    out[14] = metrics.drawable_w;
    out[15] = metrics.drawable_h;
    out[16] = 0;
    out[17] = 0;
    out[18] = metrics.logical_w;
    out[19] = metrics.logical_h;
    out[20] = display_generation;
    out[21] = density_generation;
    out[ANDROID_RENDER_I_FRAME_TOKEN] = frame_token;
    Ok(())
}

fn with_initialized_runtime_session<R>(
    project_root: &Path,
    entry_file: &Path,
    operation: impl FnOnce(&mut AndroidRuntimeSession) -> Result<R, String>,
) -> Result<R, String> {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        let needs_lazy_build = session_slot
            .as_ref()
            .is_none_or(|session| session.project_root != project_root);
        if needs_lazy_build {
            let files = load_workshop_project_with_diagnostic(project_root, entry_file).map_err(
                |diagnostic| format_compiler_source_diagnostic(project_root, &diagnostic),
            )?;
            let source_fingerprint = fingerprint_workshop_sources(&files);
            *session_slot = Some(
                build_runtime_session(project_root, &files, source_fingerprint)
                    .map_err(|error| format_android_bridge_error(project_root, error))?,
            );
        }

        let session = session_slot
            .as_mut()
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
        activate_pending_runtime_candidate(session)
            .map_err(|error| format_android_bridge_error(project_root, error))?;
        if !session.initialized {
            execute_lifecycle_noarg(&session.jit, "main")?;
            session.initialized = true;
        }
        operation(session)
    })
}

pub fn set_android_workshop_i32_global(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    path: &str,
    value: i32,
) -> Result<(), String> {
    with_initialized_runtime_session(project_root.as_ref(), entry_file.as_ref(), |session| {
        session.jit.write_i32_global_path(path, value);
        Ok(())
    })
}

pub fn get_android_workshop_i32_global(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    path: &str,
) -> Result<i32, String> {
    with_initialized_runtime_session(project_root.as_ref(), entry_file.as_ref(), |session| {
        Ok(session.jit.read_i32_global_path(path))
    })
}
fn build_runtime_session(
    project_root: &Path,
    files: &[WorkshopSourceFile],
    source_fingerprint: u64,
) -> Result<AndroidRuntimeSession, AndroidBridgeError> {
    install_embedded_resource_host(project_root)?;
    let mut jit = JitProcess::new();
    jit.set_local_runtime_helper_trampolines(true);
    configure_runtime_jit(&mut jit, project_root, files)?;
    if let Err(error) = jit.compile() {
        return Err(jit
            .last_source_diagnostic()
            .cloned()
            .map(AndroidBridgeError::Source)
            .unwrap_or_else(|| {
                AndroidBridgeError::Plain(format!("Android JIT compile failed: {error:?}"))
            }));
    }
    let display_metrics = AndroidDisplayMetrics::new(1, 1, 1, 1);
    Ok(AndroidRuntimeSession {
        project_root: project_root.to_path_buf(),
        source_fingerprint,
        generation: 1,
        jit,
        initialized: false,
        pending_candidate: None,
        pending_source_fingerprint: None,
        pending_resource_catalog: None,
        last_swap_receipt: None,
        tick_count: 0,
        previous_input: None,
        display_metrics,
        display_signature: display_metrics.signature(),
        display_generation: 0,
        density_scale_bits: display_metrics.raster_scale.to_bits(),
        density_generation: 0,
    })
}

fn warm_or_reload_runtime_session(
    project_root: &Path,
    files: &[WorkshopSourceFile],
    source_fingerprint: u64,
) -> Result<(), AndroidBridgeError> {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        match session_slot.as_mut() {
            Some(session)
                if session.project_root == project_root
                    && session.pending_source_fingerprint == Some(source_fingerprint) =>
            {
                // A duplicate watcher notification must retain the already staged generation.
            }
            Some(session)
                if session.project_root == project_root
                    && session.source_fingerprint != source_fingerprint =>
            {
                recompile_runtime_session(session, project_root, files, source_fingerprint)?;
            }
            Some(session) if session.project_root == project_root => {}
            _ => {
                *session_slot = Some(build_runtime_session(
                    project_root,
                    files,
                    source_fingerprint,
                )?);
            }
        }
        Ok(())
    })
}

fn recompile_runtime_session(
    session: &mut AndroidRuntimeSession,
    project_root: &Path,
    files: &[WorkshopSourceFile],
    source_fingerprint: u64,
) -> Result<(), AndroidBridgeError> {
    session.pending_candidate = None;
    session.pending_source_fingerprint = None;
    session.pending_resource_catalog = None;
    // Android arm64 loads a complete development generation so state can migrate and
    // host entries can switch atomically without cross-generation code dependencies.
    let mut candidate = JitProcess::new();
    candidate.set_local_runtime_helper_trampolines(true);
    configure_runtime_jit(&mut candidate, project_root, files)?;
    if let Err(error) = candidate.compile_staged() {
        return Err(candidate
            .last_source_diagnostic()
            .cloned()
            .map(AndroidBridgeError::Source)
            .unwrap_or_else(|| {
                AndroidBridgeError::Plain(format!("Android JIT hot reload failed: {error:?}"))
            }));
    }
    candidate
        .validate_on_code_swap_signature()
        .map_err(|error| AndroidBridgeError::phase("runtime_entry", "on_code_swap", error, None))?;
    let resource_catalog = prepare_embedded_resource_catalog(project_root, true)?;
    session.pending_candidate = Some(candidate);
    session.pending_source_fingerprint = Some(source_fingerprint);
    session.pending_resource_catalog = Some(resource_catalog);
    Ok(())
}

impl std::fmt::Display for AndroidBridgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(detail) => formatter.write_str(detail),
            Self::Source(diagnostic) => formatter.write_str(&diagnostic.message),
            Self::Phase {
                stage,
                symbol,
                detail,
                resource,
            } => {
                write!(formatter, "{stage} '{symbol}' failed: {detail}")?;
                if let Some(resource) = resource {
                    write!(formatter, " (resource: {resource})")?;
                }
                Ok(())
            }
        }
    }
}

struct AndroidResourceStage {
    pending: Option<EmbeddedResourceCatalog>,
    previous: Option<Option<EmbeddedResourceCatalog>>,
}

struct AndroidResourcePublication {
    pending: Option<EmbeddedResourceCatalog>,
}

impl DevelopmentSwapHost for AndroidResourcePublication {
    type Staged = AndroidResourceStage;

    fn stage(
        &mut self,
        _candidate: &JitProcess,
        _descriptor: &DevelopmentSwapDescriptor,
    ) -> Result<Self::Staged, String> {
        Ok(AndroidResourceStage {
            pending: self.pending.take(),
            previous: None,
        })
    }

    fn publish(&mut self, staged: &mut Self::Staged) -> Result<(), String> {
        let Some(catalog) = staged.pending.take() else {
            return Ok(());
        };
        let mut slot = embedded_resource_catalog()
            .lock()
            .map_err(|_| "embedded resource catalog mutex poisoned".to_string())?;
        staged.previous = Some(slot.replace(catalog));
        Ok(())
    }

    fn restore(&mut self, staged: Self::Staged) -> Result<(), String> {
        if let Some(previous) = staged.previous {
            *embedded_resource_catalog()
                .lock()
                .map_err(|_| "embedded resource catalog mutex poisoned".to_string())? = previous;
        }
        Ok(())
    }
}

fn activate_pending_runtime_candidate(
    session: &mut AndroidRuntimeSession,
) -> Result<bool, AndroidBridgeError> {
    let Some(candidate) = session.pending_candidate.take() else {
        return Ok(false);
    };
    let pending_source_fingerprint = session
        .pending_source_fingerprint
        .take()
        .ok_or_else(|| "pending Android generation has no source fingerprint".to_string())?;
    let run_hook = session.initialized && candidate.has_on_code_swap();
    let descriptor = DevelopmentSwapDescriptor::for_candidate(&candidate, run_hook);
    let mut publication = AndroidResourcePublication {
        pending: session.pending_resource_catalog.take(),
    };
    let activation = commit_development_swap(
        &mut session.jit,
        candidate,
        descriptor,
        &mut publication,
        |candidate| {
            if run_hook {
                candidate.execute_optional_on_code_swap().map_err(|error| {
                    AndroidBridgeError::phase("runtime_entry", "on_code_swap", error, None)
                })?;
                take_embedded_resource_error()
                    .map_err(|error| resource_phase_error("on_code_swap", error))?;
            }
            Ok(())
        },
    );
    let receipt = match activation {
        Ok(receipt) => receipt,
        Err(failure) => {
            session.last_swap_receipt = Some(failure.receipt);
            return Err(failure
                .hook_error
                .unwrap_or_else(|| AndroidBridgeError::Plain(failure.error)));
        }
    };
    session.last_swap_receipt = Some(receipt);
    session.source_fingerprint = pending_source_fingerprint;
    session.generation = session.generation.saturating_add(1);
    Ok(true)
}

fn discard_pending_runtime_candidate_if_different(project_root: &Path, source_fingerprint: u64) {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        if let Some(session) = session_slot.as_mut().filter(|session| {
            session.project_root == project_root
                && session.pending_source_fingerprint != Some(source_fingerprint)
        }) {
            session.pending_candidate = None;
            session.pending_source_fingerprint = None;
            session.pending_resource_catalog = None;
        }
    });
}

fn reject_staged_runtime_compile(
    project_root: &Path,
    source_fingerprint: u64,
    had_runtime_session: bool,
) {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        let Some(session) = session_slot
            .as_mut()
            .filter(|session| session.project_root == project_root)
        else {
            return;
        };
        if session.pending_source_fingerprint == Some(source_fingerprint) {
            session.pending_candidate = None;
            session.pending_source_fingerprint = None;
            session.pending_resource_catalog = None;
        } else if !had_runtime_session && session.source_fingerprint == source_fingerprint {
            *session_slot = None;
        }
    });
}

fn configure_runtime_jit(
    jit: &mut JitProcess,
    project_root: &Path,
    files: &[WorkshopSourceFile],
) -> Result<(), String> {
    jit.set_project_root(project_root.to_string_lossy().to_string())?;
    jit.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    for file in files {
        jit.upsert_file(file.path.clone(), file.source.clone());
    }
    Ok(())
}

fn execute_lifecycle_noarg(jit: &JitProcess, name: &str) -> Result<(), String> {
    match jit.execute_void_noarg_by_name(name) {
        Ok(()) => Ok(()),
        Err(void_error) => jit
            .execute_i32_noarg_by_name(name)
            .map(|_| ())
            .map_err(|i32_error| {
                format!("failed executing lifecycle {name}: void={void_error}; i32={i32_error}")
            }),
    }
}

fn execute_optional_lifecycle_noarg(jit: &JitProcess, name: &str) -> Result<(), String> {
    match execute_lifecycle_noarg(jit, name) {
        Ok(()) => Ok(()),
        Err(error) if error.contains("function '") && error.contains("not found") => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_render_commands(
    jit: &JitProcess,
) -> [AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY] {
    let mut commands = [AndroidBridgeRenderCommand::default(); ANDROID_RENDER_COMMAND_CAPACITY];
    let schema_version = if jit.has_global_path("Render.command_schema_version") {
        jit.read_i32_global_path("Render.command_schema_version")
    } else {
        1
    };
    for (index, command) in commands.iter_mut().enumerate() {
        command.kind = jit.read_i32_global_path(&format!("Render.command{index}_kind"));
        command.x = jit.read_i32_global_path(&format!("Render.command{index}_x"));
        command.y = jit.read_i32_global_path(&format!("Render.command{index}_y"));
        command.w = jit.read_i32_global_path(&format!("Render.command{index}_w"));
        command.h = jit.read_i32_global_path(&format!("Render.command{index}_h"));
        command.color = jit.read_i32_global_path(&format!("Render.command{index}_color"));
        command.asset = jit.read_i32_global_path(&format!("Render.command{index}_asset"));
        let rotation_path = format!("Render.command{index}_rotation_degrees");
        if jit.has_global_path(&rotation_path) {
            command.rotation_degrees = jit.read_i32_global_path(&rotation_path);
        }
        command.alpha = if schema_version >= 2 {
            jit.read_i32_global_path(&format!("Render.command{index}_alpha"))
                .clamp(0, 255)
        } else {
            255
        };
        if schema_version >= 3 {
            command.clip_x = jit.read_i32_global_path(&format!("Render.command{index}_clip_x"));
            command.clip_y = jit.read_i32_global_path(&format!("Render.command{index}_clip_y"));
            command.clip_w = jit.read_i32_global_path(&format!("Render.command{index}_clip_w"));
            command.clip_h = jit.read_i32_global_path(&format!("Render.command{index}_clip_h"));
        }
    }
    commands
}

fn render_command_state_lines(
    render_command_count: i32,
    render_commands: &[AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY],
) -> String {
    let mut lines = format!("render_command_count={render_command_count}\n");
    let count = render_command_count.clamp(0, ANDROID_RENDER_COMMAND_CAPACITY as i32) as usize;
    for (index, command) in render_commands.iter().enumerate().take(count) {
        lines.push_str(&format!(
            "render{index}_kind={}\nrender{index}_x={}\nrender{index}_y={}\nrender{index}_w={}\nrender{index}_h={}\nrender{index}_color={}\nrender{index}_asset={}\nrender{index}_rotation_degrees={}\nrender{index}_alpha={}\nrender{index}_clip_x={}\nrender{index}_clip_y={}\nrender{index}_clip_w={}\nrender{index}_clip_h={}\n",
            command.kind, command.x, command.y, command.w, command.h, command.color,
            command.asset, command.rotation_degrees, command.alpha, command.clip_x,
            command.clip_y, command.clip_w, command.clip_h
        ));
    }
    lines
}

fn write_render_frame_i32s(
    out: &mut [i32],
    result: &AndroidBridgeRunTickResult,
) -> Result<(), String> {
    if out.len() < ANDROID_RENDER_FRAME_I32_CAPACITY {
        return Err(format!(
            "render frame output buffer too small: got {}, need {}",
            out.len(),
            ANDROID_RENDER_FRAME_I32_CAPACITY
        ));
    }
    out[0] = 0;
    out[1] = result.tick_count;
    out[2] = result.observed_game_tick_count;
    out[3] = if result.recompiled { 1 } else { 0 };
    out[4] = if result.initialized { 1 } else { 0 };
    out[5] = result
        .render_command_count
        .clamp(0, ANDROID_RENDER_COMMAND_CAPACITY as i32);
    let count = out[5] as usize;
    for index in 0..ANDROID_RENDER_COMMAND_CAPACITY {
        let base = ANDROID_RENDER_FRAME_HEADER_SIZE + index * ANDROID_RENDER_COMMAND_STRIDE;
        let command = if index < count {
            result.render_commands[index]
        } else {
            AndroidBridgeRenderCommand::default()
        };
        out[base] = command.kind;
        out[base + 1] = command.x;
        out[base + 2] = command.y;
        out[base + 3] = command.w;
        out[base + 4] = command.h;
        out[base + 5] = command.color;
        out[base + 6] = command.asset;
        out[base + 7] = command.rotation_degrees;
        out[base + 8] = command.alpha;
        out[base + 9] = command.clip_x;
        out[base + 10] = command.clip_y;
        out[base + 11] = command.clip_w;
        out[base + 12] = command.clip_h;
    }
    Ok(())
}

fn render_command_message_fields(
    render_command_count: i32,
    render_commands: &[AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY],
) -> String {
    let count = render_command_count.clamp(0, ANDROID_RENDER_COMMAND_CAPACITY as i32) as usize;
    let mut fields = format!("render_command_count={count}");
    for (index, command) in render_commands.iter().enumerate().take(count) {
        fields.push_str(&format!(
            " render{index}_kind={} render{index}_x={} render{index}_y={} render{index}_w={} render{index}_h={} render{index}_color={} render{index}_asset={} render{index}_rotation_degrees={} render{index}_alpha={} render{index}_clip_x={} render{index}_clip_y={} render{index}_clip_w={} render{index}_clip_h={}",
            command.kind, command.x, command.y, command.w, command.h, command.color,
            command.asset, command.rotation_degrees, command.alpha, command.clip_x,
            command.clip_y, command.clip_w, command.clip_h
        ));
    }
    fields
}

fn fingerprint_workshop_sources(files: &[WorkshopSourceFile]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for file in files {
        file.path.hash(&mut hasher);
        file.source.hash(&mut hasher);
    }
    hasher.finish()
}

fn has_runtime_session_for_project(project_root: &Path) -> bool {
    RUNTIME_SESSION.with(|session| {
        session
            .borrow()
            .as_ref()
            .is_some_and(|session| session.project_root == project_root)
    })
}

fn current_android_jit_compile_summary(
    project_root: &Path,
    source_fingerprint: u64,
) -> Result<AndroidJitCompileSummary, String> {
    RUNTIME_SESSION.with(|session| {
        let session = session.borrow();
        let session = session
            .as_ref()
            .filter(|session| session.project_root == project_root)
            .ok_or_else(|| "Android JIT compile produced no runtime session".to_string())?;
        let jit = if session.pending_source_fingerprint == Some(source_fingerprint) {
            session
                .pending_candidate
                .as_ref()
                .ok_or_else(|| "Android JIT compile produced no pending candidate".to_string())?
        } else if session.source_fingerprint == source_fingerprint {
            &session.jit
        } else {
            return Err(
                "Android JIT compile session does not match source fingerprint".to_string(),
            );
        };
        let metadata = jit
            .generation_metadata()
            .ok_or_else(|| "Android JIT compile produced no generation metadata".to_string())?;
        let snapshot = jit
            .program_snapshot()
            .ok_or_else(|| "Android JIT compile produced no ProgramSnapshot".to_string())?;
        let artifacts = jit
            .artifacts()
            .iter()
            .map(|artifact| {
                let function = snapshot
                    .function_by_id(artifact.function_id)
                    .ok_or_else(|| {
                        format!(
                            "Android JIT artifact {} has no canonical function",
                            artifact.function_id
                        )
                    })?;
                let source_path = snapshot
                    .files()
                    .get(function.file_id as usize)
                    .ok_or_else(|| {
                        format!(
                            "Android JIT function '{}' has no source file",
                            function.symbol_id
                        )
                    })?
                    .path
                    .clone();
                Ok(AndroidJitArtifactSummary {
                    symbol_id: function.symbol_id.to_string(),
                    function_id: function.id,
                    source_path,
                    name: function.name.clone(),
                    signature_hash: function.signature_hash,
                    slot: artifact.slot,
                    body_hash: artifact.body_hash,
                    executable_bytes: artifact.executable_bytes,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(AndroidJitCompileSummary {
            source_revision: metadata.source_revision,
            layout_hash: metadata.layout_hash,
            emitted_function_count: metadata.emitted_function_ids.len(),
            reused_function_count: metadata.reused_function_ids.len(),
            executable_bytes: metadata.executable_bytes,
            artifacts,
            entrypoints: metadata.host_export_signatures.keys().cloned().collect(),
        })
    })
}

fn render_android_jit_manifest(
    _project_root: &Path,
    project_hash: u64,
    reload: WorkshopReload,
    summary: &AndroidJitCompileSummary,
    artifact_file_name: &str,
    artifact_hash: &str,
) -> String {
    let mut manifest = format!(
        "status=CompileReady\nbackend=cranelift-jit\nartifact_kind=executable-memory\nartifact_manifest=build/{artifact_file_name}\nartifact_manifest_sha256={artifact_hash}\nreload={reload:?}\nproject_hash={project_hash:016x}\nlayout_hash={:016x}\nsource_revision={}\nfunctions={}\nemitted_functions={}\nreused_functions={}\nexecutable_bytes={}\nerrors=0\nruntime_state=build/runtime_state.txt\n",
        summary.layout_hash,
        summary.source_revision,
        summary.artifacts.len(),
        summary.emitted_function_count,
        summary.reused_function_count,
        summary.executable_bytes,
    );
    for entrypoint in &summary.entrypoints {
        manifest.push_str(&format!("entrypoint={entrypoint}\n"));
    }
    manifest
}

fn remove_legacy_stub_artifacts(project_root: &Path) -> Result<(), String> {
    let path = project_root.join("build/functions");
    if path.is_dir() {
        fs::remove_dir_all(&path)
    } else if path.is_file() {
        fs::remove_file(&path)
    } else {
        return Ok(());
    }
    .map_err(|error| {
        format!(
            "failed removing legacy stub artifacts {}: {error}",
            path.display()
        )
    })
}

fn should_write_jit_runtime_state(initialized: bool, recompiled: bool) -> bool {
    initialized || recompiled
}

fn write_jit_runtime_state(
    project_root: &Path,
    tick_count: i32,
    observed_game_tick_count: i32,
    render_command_count: i32,
    render_commands: &[AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY],
) -> Result<(), String> {
    let runtime_state_path = project_root.join("build/runtime_state.txt");
    if let Some(parent) = runtime_state_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed creating Android runtime state directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(
        &runtime_state_path,
        format!(
            "status=RuntimeStateReady\nmode=JitExecuted\ntick_count={tick_count}\ngame_tick_count={observed_game_tick_count}\n{}",
            render_command_state_lines(render_command_count, render_commands)
        ),
    )
    .map_err(|error| {
        format!(
            "failed writing Android runtime state {}: {error}",
            runtime_state_path.display()
        )
    })
}
fn read_previous_android_plan(
    project_root: &Path,
) -> Result<Option<PreviousAndroidCompile>, String> {
    let manifest_path = project_root.join("build/native_compile_manifest.txt");
    let manifest = match fs::read_to_string(&manifest_path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed reading previous Android manifest {}: {error}",
                manifest_path.display()
            ));
        }
    };
    let Some(project_hash) = parse_manifest_hex_u64(&manifest, "project_hash=") else {
        return Ok(None);
    };
    let Some(layout_hash) = parse_manifest_hex_u64(&manifest, "layout_hash=") else {
        return Ok(None);
    };
    Ok(Some(PreviousAndroidCompile {
        project_hash,
        layout_hash,
    }))
}

fn parse_manifest_hex_u64(manifest: &str, key: &str) -> Option<u64> {
    let line = manifest.lines().find(|line| line.starts_with(key))?;
    let value = line[key.len()..].trim();
    u64::from_str_radix(value, 16).ok()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_compile_project(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        compile_project_from_c(project_root, entry_file)
    }));
    let message = match result {
        Ok(Ok(result)) => format!(
            "CompileReady: backend=cranelift-jit reload={:?} status={} functions={} compile_us={} manifest={}",
            result.reload,
            result.status,
            result.compiled_function_count,
            result.compile_micros,
            result.manifest_path.display()
        ),
        Ok(Err(error)) => format!("CompileError: {error}"),
        Err(payload) => {
            let panic_message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            format!("CompileError: panic while compiling Android project: {panic_message}")
        }
    };
    CString::new(message.replace('\0', "%00"))
        .expect("compile diagnostic is NUL-safe")
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_run_tests(project_root: *const c_char) -> *mut c_char {
    let message = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() {
            return Err("null project root".to_string());
        }
        let root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        run_android_workshop_stasis_tests(root).map(|result| result.to_string())
    }));
    let message = match message {
        Ok(Ok(message)) => message,
        Ok(Err(error)) => serde_json::json!({"kind":"stasis_test_run","passed":0,"failed":1,"all_passed":false,"error":error}).to_string(),
        Err(_) => serde_json::json!({"kind":"stasis_test_run","passed":0,"failed":1,"all_passed":false,"error":"panic while running tests"}).to_string(),
    };
    CString::new(message).unwrap().into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_source_items(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        let (project_root, entry_file) = semantic_bridge_paths(project_root, entry_file)?;
        android_workshop_source_items(&project_root, &entry_file)
    }));
    semantic_bridge_json_result(result, "source item indexing")
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_find_references(
    project_root: *const c_char,
    entry_file: *const c_char,
    symbol: *const c_char,
    limit: usize,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        let (project_root, entry_file) = semantic_bridge_paths(project_root, entry_file)?;
        if symbol.is_null() {
            return Err("null reference symbol".to_string());
        }
        let symbol = CStr::from_ptr(symbol)
            .to_str()
            .map_err(|error| format!("reference symbol was not UTF-8: {error}"))?;
        android_workshop_references(&project_root, &entry_file, symbol, limit)
    }));
    semantic_bridge_json_result(result, "reference lookup")
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_semantic_edit(
    project_root: *const c_char,
    entry_file: *const c_char,
    request_json: *const c_char,
    dry_run: i32,
    validate: i32,
    run_tests: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        let (project_root, entry_file) = semantic_bridge_paths(project_root, entry_file)?;
        if request_json.is_null() {
            return Err("null semantic edit request".to_string());
        }
        let request_json = CStr::from_ptr(request_json)
            .to_str()
            .map_err(|error| format!("semantic edit request was not UTF-8: {error}"))?;
        let batch = serde_json::from_str::<WorkshopSemanticEditBatch>(request_json)
            .map_err(|error| format!("invalid semantic edit request: {error}"))?;
        execute_android_workshop_semantic_edit(
            &project_root,
            &entry_file,
            &batch,
            dry_run != 0,
            validate != 0,
            run_tests != 0,
        )
    }));
    semantic_bridge_json_result(result, "semantic edit")
}

unsafe fn semantic_bridge_paths(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> Result<(String, String), String> {
    if project_root.is_null() || entry_file.is_null() {
        return Err("null project root or entry file".to_string());
    }
    let project_root = CStr::from_ptr(project_root)
        .to_str()
        .map_err(|error| format!("project root was not UTF-8: {error}"))?;
    let entry_file = CStr::from_ptr(entry_file)
        .to_str()
        .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
    Ok((project_root.to_string(), entry_file.to_string()))
}

fn semantic_bridge_json_result(
    result: Result<Result<serde_json::Value, String>, Box<dyn std::any::Any + Send>>,
    operation: &str,
) -> *mut c_char {
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({
            "schema_version": 1,
            "status": "error",
            "error": error,
        }),
        Err(_) => serde_json::json!({
            "schema_version": 1,
            "status": "error",
            "error": format!("panic during Android {operation}"),
        }),
    };
    let message = serde_json::to_string(&value).unwrap_or_else(|_| {
        "{\"status\":\"error\",\"error\":\"serialization failed\"}".to_string()
    });
    CString::new(message)
        .unwrap_or_else(|_| CString::new("{\"status\":\"error\"}").unwrap())
        .into_raw()
}

unsafe fn compile_project_from_c(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> Result<AndroidBridgeCompileResult, String> {
    if project_root.is_null() || entry_file.is_null() {
        return Err("null project root or entry file".to_string());
    }
    let project_root = CStr::from_ptr(project_root)
        .to_str()
        .map_err(|error| format!("project root was not UTF-8: {error}"))?;
    let entry_file = CStr::from_ptr(entry_file)
        .to_str()
        .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
    compile_android_workshop_project(project_root, entry_file)
}

fn format_compiler_source_diagnostic(
    project_root: &Path,
    diagnostic: &stasis_compiler::SourceDiagnostic,
) -> String {
    let path = Path::new(&diagnostic.path);
    let disk_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let source = fs::read_to_string(&disk_path).unwrap_or_default();
    let start = diagnostic.start.min(source.len());
    let end = diagnostic.end.max(start).min(source.len());
    let symbol = diagnostic.symbol.clone();
    let (line, column) = source_line_column(&source, start);
    let (end_line, end_column) = source_line_column(&source, end);
    let file = disk_path
        .strip_prefix(project_root)
        .or_else(|_| disk_path.strip_prefix(&canonical_root))
        .unwrap_or(&disk_path)
        .to_string_lossy()
        .replace('\\', "/");
    let legacy = format!(
        "{}: {}|diagnostic_file={}|diagnostic_line={}|diagnostic_column={}|diagnostic_end_line={}|diagnostic_end_column={}|diagnostic_symbol={}|diagnostic_message={}",
        sanitize_legacy_prefix(&diagnostic.path),
        sanitize_legacy_prefix(&diagnostic.message),
        percent_encode(&file),
        line,
        column,
        end_line,
        end_column,
        percent_encode(&symbol),
        percent_encode(&diagnostic.message),
    );
    if matches!(
        diagnostic.code,
        stasis_compiler::SourceDiagnosticCode::Generic
    ) {
        return legacy;
    }
    let stage = diagnostic_stage(&diagnostic.code);
    let causes = [format!("{stage} phase"), diagnostic.message.clone()];
    format!(
        "{legacy}{}",
        format_native_diagnostic(
            stage,
            diagnostic.code.as_str(),
            &diagnostic.message,
            Some(&file),
            if symbol.is_empty() {
                None
            } else {
                Some(&symbol)
            },
            None,
            &causes,
        )
    )
}

fn sanitize_legacy_prefix(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\0' => sanitized.push_str("%00"),
            '|' => sanitized.push_str("%7C"),
            '\r' => sanitized.push_str("%0D"),
            '\n' => sanitized.push_str("%0A"),
            _ => sanitized.push(character),
        }
    }
    sanitized
}

fn diagnostic_stage(code: &stasis_compiler::SourceDiagnosticCode) -> &'static str {
    match code {
        stasis_compiler::SourceDiagnosticCode::Parse => "parse",
        stasis_compiler::SourceDiagnosticCode::UnresolvedExtern => "extern_resolution",
        _ => "compile",
    }
}

fn format_native_diagnostic(
    stage: &str,
    code: &str,
    detail: &str,
    file: Option<&str>,
    symbol: Option<&str>,
    resource: Option<&str>,
    causes: &[String],
) -> String {
    let cause_values = if causes.is_empty() {
        vec![detail.to_string()]
    } else {
        causes.to_vec()
    };
    let mut context = serde_json::Map::new();
    if let Some(file) = file {
        context.insert(
            "file".to_string(),
            serde_json::Value::String(file.to_string()),
        );
    }
    if let Some(symbol) = symbol {
        context.insert(
            "symbol".to_string(),
            serde_json::Value::String(symbol.to_string()),
        );
    }
    if let Some(resource) = resource {
        context.insert(
            "resource".to_string(),
            serde_json::Value::String(resource.to_string()),
        );
    }
    let envelope = serde_json::json!({
        "schema": "stasis.native_diagnostic.v1",
        "version": 1,
        "stage": stage,
        "code": code,
        "context": context,
        "detail": detail,
        "causes": &cause_values,
    })
    .to_string();
    format!(
        "|diagnostic_schema=stasis.native_diagnostic.v1|diagnostic_version=1|diagnostic_stage={}|diagnostic_code={}|diagnostic_detail={}|diagnostic_causes={}|diagnostic_envelope={}",
        percent_encode(stage),
        percent_encode(code),
        percent_encode(detail),
        percent_encode(&serde_json::to_string(&cause_values).unwrap_or_else(|_| "[]".to_string())),
        percent_encode(&envelope),
    )
}

fn format_runtime_diagnostic(
    stage: &str,
    code: &str,
    detail: &str,
    symbol: Option<&str>,
    resource: Option<&str>,
) -> String {
    format!(
        "{}{}",
        sanitize_legacy_prefix(detail),
        format_native_diagnostic(
            stage,
            code,
            detail,
            None,
            symbol,
            resource,
            &[format!("{stage} phase"), detail.to_string()],
        )
    )
}

fn format_android_bridge_error(project_root: &Path, error: AndroidBridgeError) -> String {
    match error {
        AndroidBridgeError::Source(diagnostic) => {
            format_compiler_source_diagnostic(project_root, &diagnostic)
        }
        AndroidBridgeError::Phase {
            stage,
            symbol,
            detail,
            resource,
        } => {
            let code = match stage {
                "runtime_entry" => "stasis.runtimeEntry",
                "render_schema" => "stasis.renderSchema",
                "resource" => "stasis.missingResource",
                _ => "stasis.runtime",
            };
            format_runtime_diagnostic(stage, code, &detail, Some(symbol), resource.as_deref())
        }
        AndroidBridgeError::Plain(detail) => {
            format_runtime_diagnostic("runtime", "stasis.runtime", &detail, None, None)
        }
    }
}

fn diagnostic_offset(source: &str, error: &str) -> usize {
    if let Some(token_name) = error.split(" but found ").nth(1) {
        let token = match token_name.split_whitespace().next().unwrap_or("") {
            "Colon" => Some(':'),
            "LParen" => Some('('),
            "RParen" => Some(')'),
            "LBrace" => Some('{'),
            "RBrace" => Some('}'),
            "Comma" => Some(','),
            "Semicolon" => Some(';'),
            _ => None,
        };
        if let Some(offset) = token.and_then(|token| source.find(token)) {
            return offset;
        }
    }
    if error.contains("unterminated string literal") {
        return source.rfind('"').unwrap_or(0);
    }
    if let Some(name) = error
        .split("missing closing '}' for function '")
        .nth(1)
        .and_then(|value| value.split('\'').next())
    {
        if let Some(offset) = source.find(&format!("function {name}")) {
            return offset;
        }
    }
    if let Some(name) = error
        .strip_prefix("test '")
        .and_then(|value| value.split('\'').next())
    {
        if let Some(offset) = source.find(&format!("test `{name}`")) {
            return offset;
        }
    }
    if error.contains("import") {
        return source.find("import").unwrap_or(0);
    }
    if error.contains("function") {
        return source.find("function").unwrap_or(0);
    }
    0
}

fn diagnostic_symbol(source: &str, offset: usize, error: &str) -> String {
    if let Some(name) = error
        .split("missing closing '}' for function '")
        .nth(1)
        .and_then(|value| value.split('\'').next())
    {
        return name.to_string();
    }
    if let Some(function_start) = source[..offset.min(source.len())].rfind("function ") {
        let name_start = function_start + "function ".len();
        let name_end = source[name_start..]
            .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map(|length| name_start + length)
            .unwrap_or(source.len());
        return source[name_start..name_end].to_string();
    }
    if let Some(name) = error
        .strip_prefix("test '")
        .and_then(|value| value.split('\'').next())
    {
        return name.to_string();
    }
    let prefix = &source[..offset.min(source.len())];
    for line in prefix.lines().rev() {
        let trimmed = line.trim_start();
        for keyword in ["function ", "test "] {
            if let Some(rest) = trimmed.strip_prefix(keyword) {
                let rest = rest.trim_start_matches('`');
                let end = rest
                    .find(|value: char| value == '`' || value == '(' || value.is_whitespace())
                    .unwrap_or(rest.len());
                return rest[..end].to_string();
            }
        }
    }
    String::new()
}

fn source_line_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = source[line_start..offset].chars().count() + 1;
    (line, column)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_set_i32_global(
    project_root: *const c_char,
    entry_file: *const c_char,
    path: *const c_char,
    value: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() || entry_file.is_null() || path.is_null() {
            return Err("null project root, entry file, or path".to_string());
        }
        let project_root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        let entry_file = CStr::from_ptr(entry_file)
            .to_str()
            .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
        let path = CStr::from_ptr(path)
            .to_str()
            .map_err(|error| format!("global path was not UTF-8: {error}"))?;
        set_android_workshop_i32_global(project_root, entry_file, path, value)?;
        Ok(format!("StateSet: path={path} value={value}"))
    }));
    let message = match result {
        Ok(Ok(message)) => message,
        Ok(Err(error)) => format!("StateError: {error}"),
        Err(_) => "StateError: panic while setting Android runtime global".to_string(),
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("StateError: invalid bridge message").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_get_i32_global(
    project_root: *const c_char,
    entry_file: *const c_char,
    path: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() || entry_file.is_null() || path.is_null() {
            return Err("null project root, entry file, or path".to_string());
        }
        let project_root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        let entry_file = CStr::from_ptr(entry_file)
            .to_str()
            .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
        let path = CStr::from_ptr(path)
            .to_str()
            .map_err(|error| format!("global path was not UTF-8: {error}"))?;
        let value = get_android_workshop_i32_global(project_root, entry_file, path)?;
        Ok(format!("StateGet: path={path} value={value}"))
    }));
    let message = match result {
        Ok(Ok(message)) => message,
        Ok(Err(error)) => format!("StateError: {error}"),
        Err(_) => "StateError: panic while getting Android runtime global".to_string(),
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("StateError: invalid bridge message").unwrap())
        .into_raw()
}
#[no_mangle]
pub extern "C" fn stasis_android_bridge_run_tick_frame(
    project_root: *const c_char,
    entry_file: *const c_char,
    touch_x: i32,
    touch_y: i32,
    touch_active: i32,
    screen_w: i32,
    screen_h: i32,
    out_values: *mut i32,
    out_len: usize,
) -> i32 {
    if out_values.is_null() {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        run_tick_from_c(
            project_root,
            entry_file,
            AndroidBridgeTickInput {
                touch_x,
                touch_y,
                touch_active,
                screen_w,
                screen_h,
            },
        )
    }));
    let out = unsafe { std::slice::from_raw_parts_mut(out_values, out_len) };
    match result {
        Ok(Ok(result)) => match write_render_frame_i32s(out, &result) {
            Ok(()) => 0,
            Err(_) => {
                if !out.is_empty() {
                    out[0] = -1;
                }
                -1
            }
        },
        Ok(Err(_)) | Err(_) => {
            if !out.is_empty() {
                out[0] = -1;
            }
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_run_tick_frame_v2(
    project_root: *const c_char,
    entry_file: *const c_char,
    touch_x: i32,
    touch_y: i32,
    touch_active: i32,
    screen_w: i32,
    screen_h: i32,
    out_i32: *mut i32,
    out_i32_len: usize,
    out_f32: *mut f32,
    out_f32_len: usize,
    out_u8: *mut u8,
    out_u8_len: usize,
) -> i32 {
    if out_i32.is_null()
        || out_f32.is_null()
        || out_u8.is_null()
        || out_i32_len < ANDROID_RENDER_GFX_I32_CAPACITY
        || out_f32_len < ANDROID_RENDER_GFX_F32_CAPACITY
        || out_u8_len < ANDROID_RENDER_GFX_U8_CAPACITY
    {
        return -1;
    }
    let mut diagnostic_project_root = PathBuf::from(".");
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() || entry_file.is_null() {
            return Err(AndroidBridgeError::from("null project root or entry file"));
        }
        let project_root = CStr::from_ptr(project_root).to_str().map_err(|error| {
            AndroidBridgeError::Plain(format!("project root was not UTF-8: {error}"))
        })?;
        diagnostic_project_root = PathBuf::from(project_root);
        let entry_file = CStr::from_ptr(entry_file).to_str().map_err(|error| {
            AndroidBridgeError::Plain(format!("entry file was not UTF-8: {error}"))
        })?;
        run_android_workshop_tick_internal(
            project_root,
            entry_file,
            AndroidBridgeTickInput {
                touch_x,
                touch_y,
                touch_active,
                screen_w,
                screen_h,
            },
            false,
        )?;
        let i32_values = std::slice::from_raw_parts_mut(out_i32, out_i32_len);
        let f32_values = std::slice::from_raw_parts_mut(out_f32, out_f32_len);
        let u8_values = std::slice::from_raw_parts_mut(out_u8, out_u8_len);
        stasis_dynload::copy_jit_render_active(i32_values, f32_values, u8_values)
            .map_err(|error| AndroidBridgeError::phase("render_schema", "render", error, None))?;
        write_android_display_metadata(i32_values).map_err(AndroidBridgeError::from)
    }));
    match result {
        Ok(Ok(())) => {
            LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = None);
            0
        }
        Ok(Err(error)) => {
            let diagnostic = format_android_bridge_error(&diagnostic_project_root, error);
            LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = Some(diagnostic));
            unsafe {
                *out_i32 = -1;
            }
            -1
        }
        Err(_) => {
            LAST_FRAME_ERROR.with(|slot| {
                *slot.borrow_mut() = Some(format_runtime_diagnostic(
                    "runtime_entry",
                    "stasis.runtimeEntry",
                    "panic while running Android preview frame",
                    None,
                    None,
                ));
            });
            unsafe {
                *out_i32 = -1;
            }
            -1
        }
    }
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_set_storage_root(storage_root: *const c_char) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if storage_root.is_null() {
            return Err("null storage root".to_string());
        }
        let root = CStr::from_ptr(storage_root)
            .to_str()
            .map_err(|error| format!("storage root was not UTF-8: {error}"))?;
        if root.is_empty() {
            return Err("empty storage root".to_string());
        }
        stasis_dynload::set_preference_storage_root(Some(PathBuf::from(root)));
        Ok(())
    }));
    matches!(result, Ok(Ok(()))) as i32
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_last_frame_error() -> *mut c_char {
    let message = LAST_FRAME_ERROR.with(|slot| {
        slot.borrow()
            .clone()
            .unwrap_or_else(|| "native preview frame failed".to_string())
    });
    CString::new(message.replace('\0', "%00"))
        .expect("NUL-safe frame diagnostic")
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_inspect_runtime_state(
    project_root: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() {
            return Err("null project root".to_string());
        }
        let project_root = Path::new(
            CStr::from_ptr(project_root)
                .to_str()
                .map_err(|error| format!("project root was not UTF-8: {error}"))?,
        );
        inspect_android_runtime_state(project_root)
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({"status": "RuntimeStateError", "error": error}),
        Err(_) => serde_json::json!({
            "status": "RuntimeStateError",
            "error": "panic while inspecting Android runtime state",
        }),
    };
    CString::new(value.to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"RuntimeStateError\"}").unwrap())
        .into_raw()
}

fn inspect_android_runtime_state(project_root: &Path) -> Result<serde_json::Value, String> {
    RUNTIME_SESSION.with(|session_cell| {
        let session_slot = session_cell.borrow();
        let session = session_slot
            .as_ref()
            .filter(|session| session.project_root == project_root)
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
        Ok(serde_json::json!({
            "status": "RuntimeStateReady",
            "mode": "JitExecuted",
            "source": "live_session",
            "tick_count": session.tick_count,
            "game_tick_count": session.jit.read_i32_global_path("GameState.tick_count"),
            "generation": session.generation,
            "source_fingerprint": format!("{:016x}", session.source_fingerprint),
            "initialized": session.initialized,
            "pending_candidate": session.pending_candidate.is_some(),
        }))
    })
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_run_tick(
    project_root: *const c_char,
    entry_file: *const c_char,
    touch_x: i32,
    touch_y: i32,
    touch_active: i32,
    screen_w: i32,
    screen_h: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        run_tick_from_c(
            project_root,
            entry_file,
            AndroidBridgeTickInput {
                touch_x,
                touch_y,
                touch_active,
                screen_w,
                screen_h,
            },
        )
    }));
    let message = match result {
        Ok(Ok(result)) => format!(
            "RunTick: tick_count={} game_tick_count={} mode=JitExecuted recompiled={} initialized={} {}",
            result.tick_count,
            result.observed_game_tick_count,
            result.recompiled,
            result.initialized,
            render_command_message_fields(result.render_command_count, &result.render_commands)
        ),
        Ok(Err(error)) => format!("RunError: {error}"),
        Err(payload) => {
            let panic_message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            format!("RunError: panic while running Android tick: {panic_message}")
        }
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("RunError: invalid bridge message").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_resolve_sprite_asset(
    project_root: *const c_char,
    handle: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() {
            return Err("null project root".to_string());
        }
        let project_root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        resolve_android_workshop_sprite_asset(project_root, handle)
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({ "status": "error", "error": error }),
        Err(_) => serde_json::json!({
            "status": "error",
            "error": "panic while resolving Android sprite asset"
        }),
    };
    let message = serde_json::to_string(&value).unwrap_or_else(|_| {
        "{\"status\":\"error\",\"error\":\"invalid sprite response\"}".to_string()
    });
    CString::new(message)
        .unwrap_or_else(|_| CString::new("{\"status\":\"error\"}").unwrap())
        .into_raw()
}

/// Drain typed sprite releases produced by the Android JIT. The stable
/// manifest handles are returned as a bounded JSON array and are consumed by
/// the Workshop GL thread; an empty array is a successful no-op.
#[no_mangle]
pub extern "C" fn stasis_android_bridge_drain_sprite_releases() -> *mut c_char {
    let releases = take_embedded_sprite_releases();
    CString::new(serde_json::json!({ "status": "ok", "handles": releases }).to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"ok\",\"handles\":[]}").unwrap())
        .into_raw()
}

/// Drain typed sprite release cancellations produced after a stable handle was
/// reacquired before its previously delivered GL release was applied.
#[no_mangle]
pub extern "C" fn stasis_android_bridge_poll_sprite_release_cancellations() -> *mut c_char {
    let handles = take_embedded_sprite_release_cancellations();
    CString::new(serde_json::json!({ "status": "ok", "handles": handles }).to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"ok\",\"handles\":[]}").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_resolve_cached_text(
    project_root: *const c_char,
    handle: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() {
            return Err("null project root".to_string());
        }
        let root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        resolve_embedded_text_run(Path::new(root), handle)
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({ "status": "error", "error": error }),
        Err(_) => {
            serde_json::json!({ "status": "error", "error": "panic while resolving cached text" })
        }
    };
    CString::new(value.to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"error\"}").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_resolve_font(
    project_root: *const c_char,
    handle: i32,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() {
            return Err("null project root".to_string());
        }
        let root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        resolve_embedded_font(Path::new(root), handle)
    }));
    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => serde_json::json!({ "status": "error", "error": error }),
        Err(_) => serde_json::json!({ "status": "error", "error": "panic while resolving font" }),
    };
    CString::new(value.to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"error\"}").unwrap())
        .into_raw()
}

unsafe fn run_tick_from_c(
    project_root: *const c_char,
    entry_file: *const c_char,
    input: AndroidBridgeTickInput,
) -> Result<AndroidBridgeRunTickResult, String> {
    if project_root.is_null() || entry_file.is_null() {
        return Err("null project root or entry file".to_string());
    }
    let project_root = CStr::from_ptr(project_root)
        .to_str()
        .map_err(|error| format!("project root was not UTF-8: {error}"))?;
    let entry_file = CStr::from_ptr(entry_file)
        .to_str()
        .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
    run_android_workshop_tick(project_root, entry_file, input)
}
#[no_mangle]
pub extern "C" fn stasis_android_bridge_free_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(value));
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    #[test]
    fn exports_the_real_bridge_package_version() {
        let version = unsafe { CStr::from_ptr(super::stasis_android_bridge_version()) };
        assert_eq!(
            version.to_str().expect("bridge version is UTF-8"),
            env!("CARGO_PKG_VERSION")
        );
    }
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn native_diagnostic_envelope_preserves_context_detail_and_cause_order() {
        let message = format_native_diagnostic(
            "resource",
            "stasis.missingResource",
            "sprite path is invalid or missing: assets/missing.svg",
            None,
            None,
            Some("assets/missing.svg"),
            &["outer cause".to_string(), "inner cause".to_string()],
        );
        assert!(message.contains("diagnostic_schema=stasis.native_diagnostic.v1"));
        assert!(message.contains("diagnostic_stage=resource"));
        assert!(message.contains("diagnostic_code=stasis.missingResource"));
        assert!(message.contains("diagnostic_envelope="));
        let encoded = message
            .split("diagnostic_envelope=")
            .nth(1)
            .expect("encoded diagnostic envelope");
        let decoded = percent_decode_for_test(encoded);
        let envelope: serde_json::Value = serde_json::from_str(&decoded).expect("valid envelope");
        assert_eq!(envelope["version"], 1);
        assert_eq!(envelope["context"]["resource"], "assets/missing.svg");
        assert_eq!(envelope["causes"][0], "outer cause");
        assert_eq!(envelope["causes"][1], "inner cause");
    }

    #[test]
    fn native_diagnostic_envelope_preserves_utf8_detail_and_context() {
        let message = format_native_diagnostic(
            "resource",
            "stasis.missingResource",
            "\u{8d44}\u{6e90} \u{2713}",
            None,
            None,
            Some("assets/\u{4e16}\u{754c}.svg"),
            &[
                "resource phase".to_string(),
                "\u{8d44}\u{6e90} \u{2713}".to_string(),
            ],
        );
        let encoded = message
            .split("diagnostic_envelope=")
            .nth(1)
            .expect("encoded diagnostic envelope");
        let decoded = percent_decode_for_test(encoded);
        let envelope: serde_json::Value = serde_json::from_str(&decoded).expect("valid envelope");
        assert_eq!(envelope["detail"], "\u{8d44}\u{6e90} \u{2713}");
        assert_eq!(
            envelope["context"]["resource"],
            "assets/\u{4e16}\u{754c}.svg"
        );
        assert_eq!(envelope["causes"][0], "resource phase");
        assert_eq!(envelope["causes"][1], "\u{8d44}\u{6e90} \u{2713}");
    }

    #[test]
    fn native_diagnostic_envelope_percent_encodes_delimiters_and_nul() {
        let message = format_native_diagnostic(
            "resource",
            "stasis.missingResource",
            "bad|detail=\u{0}tail",
            None,
            None,
            Some("assets/bad|name.svg"),
            &[
                "resource phase".to_string(),
                "bad|detail=\u{0}tail".to_string(),
            ],
        );
        assert!(!message.contains('\0'));
        let encoded = message
            .split("diagnostic_envelope=")
            .nth(1)
            .expect("encoded diagnostic envelope");
        let decoded = percent_decode_for_test(encoded);
        let envelope: serde_json::Value = serde_json::from_str(&decoded).expect("valid envelope");
        assert_eq!(envelope["detail"], "bad|detail=\u{0}tail");
        assert_eq!(envelope["context"]["resource"], "assets/bad|name.svg");
        LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = Some(message));
        let ptr = stasis_android_bridge_last_frame_error();
        let forwarded = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        stasis_android_bridge_free_string(ptr);
        LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = None);
        assert!(forwarded.contains("diagnostic_envelope="));
        assert!(!forwarded.contains("native preview frame failed"));
    }

    #[test]
    fn compile_legacy_diagnostic_boundary_encodes_nul_and_delimiter_text() {
        let diagnostic = stasis_compiler::SourceDiagnostic::new(
            "src/bad|diagnostic_envelope=\u{0}.stasis",
            0,
            1,
            "second",
            "detail|diagnostic_envelope=\u{0}tail",
        )
        .with_code(stasis_compiler::SourceDiagnosticCode::Parse);
        let message = format_compiler_source_diagnostic(Path::new("."), &diagnostic);
        assert!(!message.contains('\u{0}'));
        assert!(message.contains("src/bad%7Cdiagnostic_envelope=%00.stasis"));
        assert!(message.contains("detail%7Cdiagnostic_envelope=%00tail"));
        CString::new(message).expect("encoded compiler diagnostic crosses C boundary");
    }

    fn percent_decode_for_test(value: &str) -> String {
        let mut output = Vec::new();
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                let value = u8::from_str_radix(&value[index + 1..index + 3], 16).unwrap();
                output.push(value);
                index += 3;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        }
        String::from_utf8(output).expect("UTF-8 envelope")
    }

    #[test]
    fn typed_sprite_release_refcounts_and_cancels_pending_event_on_reacquire() {
        let root = std::env::temp_dir().join("stasis_android_sprite_release_refs");
        let catalog = EmbeddedResourceCatalog {
            project_root: root,
            assets: ResolvedAssetManifest {
                manifest_path: PathBuf::new(),
                dynamic_assets: Default::default(),
                assets: Vec::new(),
            },
            fonts: Vec::new(),
            text_runs: Vec::new(),
            sprite_refs: vec![EmbeddedSpriteRef {
                handle: 17,
                refs: 2,
            }],
            pending_sprite_releases: Vec::new(),
            pending_sprite_release_cancellations: Vec::new(),
            error: None,
        };
        *embedded_resource_catalog().lock().unwrap() = Some(catalog);
        embedded_release_sprite(17);
        assert!(take_embedded_sprite_releases().is_empty());
        embedded_release_sprite(17);
        // A zero-reference handle queues exactly once, and a reacquisition
        // removes that event before it can reach the GL thread.
        let mut slot = embedded_resource_catalog().lock().unwrap();
        let catalog = slot.as_mut().unwrap();
        assert!(embedded_acquire_sprite(catalog, 17));
        drop(slot);
        assert!(take_embedded_sprite_releases().is_empty());
        embedded_release_sprite(17);
        assert_eq!(take_embedded_sprite_releases(), vec![17]);
        let mut slot = embedded_resource_catalog().lock().unwrap();
        let catalog = slot.as_mut().unwrap();
        assert!(embedded_acquire_sprite(catalog, 17));
        drop(slot);
        assert_eq!(take_embedded_sprite_release_cancellations(), vec![17]);
        embedded_release_sprite(17);
        assert_eq!(take_embedded_sprite_releases(), vec![17]);
    }

    #[test]
    fn typed_sprite_release_refcount_overflow_is_a_resource_error() {
        let mut catalog = EmbeddedResourceCatalog {
            project_root: PathBuf::new(),
            assets: ResolvedAssetManifest {
                manifest_path: PathBuf::new(),
                dynamic_assets: Default::default(),
                assets: Vec::new(),
            },
            fonts: Vec::new(),
            text_runs: Vec::new(),
            sprite_refs: vec![EmbeddedSpriteRef {
                handle: 31,
                refs: usize::MAX,
            }],
            pending_sprite_releases: Vec::new(),
            pending_sprite_release_cancellations: Vec::new(),
            error: None,
        };
        assert!(!embedded_acquire_sprite(&mut catalog, 31));
        assert_eq!(
            catalog.error.as_ref().map(|error| error.detail.as_str()),
            Some("sprite reference count overflow")
        );
    }

    #[test]
    fn typed_sprite_release_delivery_batches_without_loss() {
        let _guard = bridge_runtime_test_guard();
        let catalog = EmbeddedResourceCatalog {
            project_root: PathBuf::new(),
            assets: ResolvedAssetManifest {
                manifest_path: PathBuf::new(),
                dynamic_assets: Default::default(),
                assets: Vec::new(),
            },
            fonts: Vec::new(),
            text_runs: Vec::new(),
            sprite_refs: (1..=300)
                .map(|handle| EmbeddedSpriteRef { handle, refs: 1 })
                .collect(),
            pending_sprite_releases: Vec::new(),
            pending_sprite_release_cancellations: Vec::new(),
            error: None,
        };
        *embedded_resource_catalog().lock().unwrap() = Some(catalog);
        for handle in 1..=300 {
            embedded_release_sprite(handle);
        }
        let first = take_embedded_sprite_releases();
        let second = take_embedded_sprite_releases();
        assert_eq!(first.len(), MAX_PENDING_SPRITE_RELEASES);
        assert_eq!(first, (1..=256).collect::<Vec<_>>());
        assert_eq!(second, (257..=300).collect::<Vec<_>>());
        assert!(take_embedded_sprite_releases().is_empty());
        *embedded_resource_catalog().lock().unwrap() = None;
    }

    #[test]
    fn typed_sprite_release_registry_reuses_slots_after_delivery() {
        let _guard = bridge_runtime_test_guard();
        let catalog = EmbeddedResourceCatalog {
            project_root: PathBuf::new(),
            assets: ResolvedAssetManifest {
                manifest_path: PathBuf::new(),
                dynamic_assets: Default::default(),
                assets: Vec::new(),
            },
            fonts: Vec::new(),
            text_runs: Vec::new(),
            sprite_refs: Vec::new(),
            pending_sprite_releases: Vec::new(),
            pending_sprite_release_cancellations: Vec::new(),
            error: None,
        };
        *embedded_resource_catalog().lock().unwrap() = Some(catalog);
        for _ in 0..5000 {
            let mut slot = embedded_resource_catalog().lock().unwrap();
            assert!(embedded_acquire_sprite(slot.as_mut().unwrap(), 17));
            drop(slot);
            assert_eq!(take_embedded_sprite_release_cancellations(), vec![17]);
            embedded_release_sprite(17);
            assert_eq!(take_embedded_sprite_releases(), vec![17]);
        }
        assert!(embedded_resource_catalog()
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .sprite_refs
            .is_empty());
        *embedded_resource_catalog().lock().unwrap() = None;
    }

    #[test]
    fn android_display_metrics_preserve_logical_canvas_and_round_trip_letterbox() {
        let metrics = AndroidDisplayMetrics::new(360, 720, 1080, 2400);
        assert_eq!(metrics.logical_w, 360);
        assert_eq!(metrics.logical_h, 720);
        assert_eq!(metrics.native_w, 1080);
        assert_eq!(metrics.drawable_h, 2400);
        assert!((metrics.content_scale - 3.0).abs() < 0.001);
        assert!((metrics.raster_scale - 3.0).abs() < 0.001);
        assert!((metrics.viewport_y - 120.0).abs() < 0.001);
        let (x, y) = metrics.native_to_logical(540, 1200);
        assert!((x - 180.0).abs() < 0.001);
        assert!((y - 360.0).abs() < 0.001);

        let landscape = AndroidDisplayMetrics::new(360, 720, 2400, 1080);
        assert!((landscape.content_scale - 1.5).abs() < 0.001);
        assert!((landscape.viewport_x - 930.0).abs() < 0.001);
        let (outside_x, outside_y) = landscape.native_to_logical(0, 540);
        assert_eq!(outside_x, 0.0);
        assert!((outside_y - 360.0).abs() < 0.001);

        let odd = AndroidDisplayMetrics::new(360, 720, 2400, 1081);
        assert_eq!(odd.viewport_x, 929.0);
        assert_eq!(odd.viewport_y, 0.0);
        assert_eq!(odd.viewport_w, 541.0);
        assert_eq!(odd.viewport_h, 1081.0);
        let (right, bottom) = odd.native_to_logical(1470, 1081);
        assert_eq!(right, 360.0);
        assert_eq!(bottom, 720.0);

        let vertical = AndroidDisplayMetrics::new(360, 720, 1080, 2401);
        assert_eq!(vertical.viewport_y, 120.0);
        assert_eq!(vertical.viewport_h, 2160.0);

        let narrow = AndroidDisplayMetrics::new(800, 200, 1, 100);
        assert_eq!(narrow.viewport_w, 1.0);
        assert_eq!(narrow.viewport_h, 1.0);
        assert_eq!(narrow.viewport_y, 49.0);
    }
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn bridge_runtime_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn clear_runtime_session_for_test() {
        RUNTIME_SESSION.with(|session| {
            *session.borrow_mut() = None;
        });
    }

    fn ffi_json(ptr: *mut c_char) -> serde_json::Value {
        assert!(!ptr.is_null());
        let source = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("FFI JSON UTF-8")
            .to_string();
        stasis_android_bridge_free_string(ptr);
        serde_json::from_str(&source).expect("valid FFI JSON")
    }

    fn default_tick_input() -> AndroidBridgeTickInput {
        AndroidBridgeTickInput {
            touch_x: 80,
            touch_y: 120,
            touch_active: 1,
            screen_w: 360,
            screen_h: 640,
        }
    }

    #[test]
    fn android_workshop_host_frame_preserves_edges_across_orientation_changes() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("orientation_host_frame");
        let source = format!(
            "{}\n\
             global observed_release_actions: i32;\n\
             global observed_resized: i32;\n\
             global observed_native_w: i32;\n\
             global observed_native_h: i32;\n\
             global observed_drawable_w: i32;\n\
             global observed_drawable_h: i32;\n\
             global observed_display_generation: i32;\n\
             global observed_density_generation: i32;\n\
             global observed_pointer_count: i32;\n\
             global observed_pointer_id: i32;\n\
             global observed_is_down: i32;\n\
             global observed_went_down: i32;\n\
             global observed_went_up: i32;\n\
             global observed_logical_ok: i32;\n\
             global observed_normalized_ok: i32;\n\
             global host_req_window_w_px: i32;\n\
             global host_req_window_h_px: i32;\n\
             function main(): void {{ observed_release_actions = 0; host_req_window_w_px = 360; host_req_window_h_px = 720; }}\n\
             function tick(): void {{\n\
                 observed_native_w = host_native_w_px();\n\
                 observed_native_h = host_native_h_px();\n\
                 observed_drawable_w = host_drawable_w_px();\n\
                 observed_drawable_h = host_drawable_h_px();\n\
                 observed_display_generation = host_display_generation();\n\
                 observed_density_generation = host_density_generation();\n\
                 observed_resized = 0;\n\
                 if (host_resized()) {{ observed_resized = 1; }}\n\
                 observed_pointer_count = host_pointer_count();\n\
                 observed_pointer_id = host_pointer_id(0);\n\
                 observed_is_down = 0;\n\
                 if (host_pointer_is_down(0)) {{ observed_is_down = 1; }}\n\
                 observed_went_down = 0;\n\
                 if (host_pointer_went_down(0)) {{ observed_went_down = 1; }}\n\
                 observed_went_up = 0;\n\
                 if (host_pointer_went_up(0)) {{\n\
                     observed_went_up = 1;\n\
                     observed_release_actions += 1;\n\
                 }}\n\
                 observed_logical_ok = 0;\n\
                 if (host_logical_width() == 360.0 && host_logical_height() == 720.0) {{ observed_logical_ok = 1; }}\n\
                 observed_normalized_ok = 0;\n\
                 if (host_pointer_x_n(0) == 0.5 && host_pointer_y_n(0) == 0.5) {{ observed_normalized_ok = 1; }}\n\
             }}\n",
            include_str!("../../../src/stdlib/internal/host_frame.stasis")
        );
        fs::write(root.join("src/main.stasis"), source).expect("write orientation fixture");
        let entry = Path::new("src/main.stasis");
        let frame = |touch_active: i32, screen_w: i32, screen_h: i32| {
            run_android_workshop_tick(
                &root,
                entry,
                AndroidBridgeTickInput {
                    touch_x: screen_w / 2,
                    touch_y: screen_h / 2,
                    touch_active,
                    screen_w,
                    screen_h,
                },
            )
            .expect("run orientation frame")
        };

        let portrait_down = frame(1, 360, 720);
        assert_eq!(portrait_down.tick_count, 1);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_w").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_h").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_w").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_h").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_id").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );

        frame(0, 360, 720);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );

        frame(1, 720, 360);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_w").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_h").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_w").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_h").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_id").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );

        frame(0, 720, 360);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );

        let restored = frame(0, 360, 720);
        assert_eq!(restored.tick_count, 5);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            3
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_w").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_native_h").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_w").unwrap(),
            360
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_drawable_h").unwrap(),
            720
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );

        let quiet = frame(0, 360, 720);
        assert_eq!(quiet.tick_count, 6);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_display_generation").unwrap(),
            3
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_density_generation").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_resized").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_pointer_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_is_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_went_up").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_release_actions").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_logical_ok").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "observed_normalized_ok").unwrap(),
            1
        );
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_bridge_uses_shared_asset_manifest_resolver() {
        let root = temp_project("shared_assets");
        fs::create_dir_all(root.join("assets")).expect("create assets");
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            r#"{"schema":"stasis-assets","version":1,"assets":[]}"#,
        )
        .expect("write manifest");

        let resolved = load_android_workshop_asset_manifest(&root).expect("resolve assets");
        assert!(resolved.assets.is_empty());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_bridge_resolves_sprite_metadata_by_stable_handle() {
        let root = temp_project("sprite_asset");
        fs::create_dir_all(root.join("assets")).expect("create assets");
        let pixels = b"representative sprite bytes";
        fs::write(root.join("assets/hero.png"), pixels).expect("write sprite");
        let hash = stasis_assets::sha256_bytes(pixels);
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            format!(r#"{{"schema":"stasis-assets","version":1,"assets":[{{"id":"hero","path":"assets/hero.png","content_sha256":"{hash}","format":{{"kind":"sprite","encoding":"png","width":4,"height":6,"layout":{{"columns":2,"rows":3}}}},"dependencies":[]}}]}}"#),
        )
        .expect("write manifest");

        let manifest = load_android_workshop_asset_manifest(&root).expect("load manifest");
        let handle = manifest.by_id("hero").expect("hero entry").handle.as_i32();
        let resolved =
            resolve_android_workshop_sprite_asset(&root, handle).expect("resolve sprite");
        assert_eq!(resolved["status"], "ok");
        assert_eq!(resolved["handle"], handle);
        assert_eq!(resolved["encoding"], "png");
        assert_eq!(resolved["width"], 4);
        assert_eq!(resolved["height"], 6);
        assert_eq!(resolved["layout"]["columns"], 2);
        assert_eq!(resolved["layout"]["rows"], 3);
        assert_eq!(resolved["content_sha256"], hash);
        assert!(resolved["path"]
            .as_str()
            .expect("path")
            .ends_with("hero.png"));

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_bridge_rejects_missing_sprite_handle() {
        let root = temp_project("missing_sprite_asset");
        fs::create_dir_all(root.join("assets")).expect("create assets");
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            r#"{"schema":"stasis-assets","version":1,"assets":[]}"#,
        )
        .expect("write manifest");

        let error = resolve_android_workshop_sprite_asset(&root, 7).expect_err("missing handle");
        assert!(error.contains("is not in the manifest"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_bridge_rejects_font_handle_as_sprite() {
        let root = temp_project("font_asset_as_sprite");
        fs::create_dir_all(root.join("assets")).expect("create assets");
        let bytes = b"representative font bytes";
        fs::write(root.join("assets/ui.ttf"), bytes).expect("write font");
        let hash = stasis_assets::sha256_bytes(bytes);
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            format!(r#"{{"schema":"stasis-assets","version":1,"assets":[{{"id":"ui","path":"assets/ui.ttf","content_sha256":"{hash}","format":{{"kind":"font","encoding":"ttf"}},"dependencies":[]}}]}}"#),
        )
        .expect("write manifest");

        let manifest = load_android_workshop_asset_manifest(&root).expect("load manifest");
        let handle = manifest.by_id("ui").expect("font entry").handle.as_i32();
        let error = resolve_android_workshop_sprite_asset(&root, handle)
            .expect_err("font handle must not resolve as a sprite");
        assert!(error.contains("identifies a font, not a sprite"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bundled_pong_sprite_uses_shared_manifest_handle() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("Pong template root");
        let resolved = resolve_android_workshop_sprite_asset(&root, -1_520_461_853)
            .expect("resolve bundled ball");
        assert_eq!(resolved["id"], "ball");
        assert_eq!(resolved["encoding"], "svg");
        assert_eq!(resolved["width"], 32);
        assert_eq!(resolved["height"], 32);
    }

    #[test]
    fn android_jit_preserves_negative_stable_sprite_handles() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("negative_sprite_handle");
        fs::create_dir_all(root.join("assets")).expect("create assets");
        let sprite = include_bytes!(
            "../../../mobile/android/app/src/main/assets/workshop_sample/assets/ball.svg"
        );
        fs::write(root.join("assets/ball.svg"), sprite).expect("write sprite");
        let hash = stasis_assets::sha256_bytes(sprite);
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            format!(r#"{{"schema":"stasis-assets","version":1,"assets":[{{"id":"ball","path":"assets/ball.svg","content_sha256":"{hash}","format":{{"kind":"sprite","encoding":"svg","width":32,"height":32}},"dependencies":[]}}]}}"#),
        )
        .expect("write manifest");
        let manifest = load_android_workshop_asset_manifest(&root).expect("load manifest");
        let handle = manifest.by_id("ball").expect("ball entry").handle.as_i32();
        assert!(handle < 0, "fixture must exercise a negative stable handle");
        fs::write(
            root.join("src/main.stasis"),
            "@link(\"stasis_graphics\");\nstruct Sprite { handle: i32; width: i32; height: i32; }\nglobal TestHost { sprite: Sprite; }\nfunction @extern(\"stasis_jit_sprite_load_from\") load_sprite_from(self: Sprite, path: string, width: i32, height: i32): bool;\nfunction main(): void { load_sprite_from(TestHost.sprite, \"assets/ball.svg\", 32, 32); }\nfunction tick(): void {}\n",
        )
        .expect("write source");

        run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
            .expect("load negative-handle sprite");

        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "TestHost.sprite.handle",
            )
            .expect("read sprite handle"),
            handle
        );
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_jit_typed_sprite_release_reaches_bridge_and_draws_signed_handle() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let source = r#"
import "/vendor/stasis/stdlib/graphics.stasis";

struct State {
    sprite: Sprite;
    loaded: i32;
    initial_handle: i32;
    initial_width: i32;
    initial_height: i32;
    draw_count: i32;
    draw_asset: i32;
    phase: i32;
}

global state: State;

function main(): void {
    if (state.sprite.load_sprite_from("assets/ball.svg", 32, 32)) {
        state.loaded = 1;
        state.initial_handle = state.sprite.handle;
        state.initial_width = state.sprite.width;
        state.initial_height = state.sprite.height;
    }
}

function tick(): void {
    gfx_cmd_begin();
    state.sprite.draw(4.0, 5.0, 200, 7);
    state.draw_count = gfx_cmd_sprite_count();
    state.draw_asset = gfx_cmd_i32[GFX_I_SPRITE_BASE];
    gfx_cmd_mark_present();
    if (state.phase == 0) {
        state.sprite.release();
        state.phase = 1;
    } else {
        state.sprite.release();
        state.phase = 2;
    }
}
"#;
        let (root, handle) = write_typed_sprite_project("typed_release", source);

        let _first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run typed sprite release tick");
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.draw_count"
            )
            .expect("read draw count"),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.draw_asset"
            )
            .expect("read draw asset"),
            handle
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "state.loaded")
                .expect("read loaded flag"),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.initial_handle",
            )
            .expect("read initial handle"),
            handle
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.initial_width",
            )
            .expect("read initial width"),
            32
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.initial_height",
            )
            .expect("read initial height"),
            32
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.sprite.handle"
            )
            .expect("read released handle"),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.sprite.width"
            )
            .expect("read released width"),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.sprite.height",
            )
            .expect("read released height"),
            0
        );
        assert_eq!(
            ffi_json(stasis_android_bridge_drain_sprite_releases())["handles"],
            serde_json::json!([handle])
        );

        let _duplicate =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run duplicate release tick");
        assert_eq!(
            ffi_json(stasis_android_bridge_drain_sprite_releases())["handles"],
            serde_json::json!([])
        );

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_jit_typed_sprite_release_reacquire_cancels_pending_event() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let source = r#"
import "/vendor/stasis/stdlib/graphics.stasis";

struct State {
    sprite: Sprite;
    loaded: i32;
    reloaded: i32;
    reload_handle: i32;
    reload_width: i32;
    reload_height: i32;
    draw_count: i32;
    draw_asset: i32;
    phase: i32;
}

global state: State;

function main(): void {
    state.phase = 0;
    if (state.sprite.load_sprite_from("assets/ball.svg", 32, 32)) {
        state.loaded = 1;
    }
}

function tick(): void {
    gfx_cmd_begin();
    if (state.phase == 0) {
        state.sprite.release();
        if (state.sprite.load_sprite_from("assets/ball.svg", 32, 32)) {
            state.reloaded = 1;
            state.reload_handle = state.sprite.handle;
            state.reload_width = state.sprite.width;
            state.reload_height = state.sprite.height;
        }
        state.sprite.draw(6.0, 7.0, 255, 0);
        state.draw_count = gfx_cmd_sprite_count();
        state.draw_asset = gfx_cmd_i32[GFX_I_SPRITE_BASE];
        state.phase = 1;
    } else {
        state.sprite.release();
        state.phase = 2;
    }
    gfx_cmd_mark_present();
}
"#;
        let (root, handle) = write_typed_sprite_project("typed_reacquire", source);

        let _reacquired =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run same-tick release and reacquire");
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.draw_count"
            )
            .expect("read draw count"),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.draw_asset"
            )
            .expect("read draw asset"),
            handle
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "state.loaded")
                .expect("read loaded flag"),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "state.reloaded")
                .expect("read reloaded flag"),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.reload_handle"
            )
            .expect("read reload handle"),
            handle
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.reload_width"
            )
            .expect("read reload width"),
            32
        );
        assert_eq!(
            get_android_workshop_i32_global(
                &root,
                Path::new("src/main.stasis"),
                "state.reload_height",
            )
            .expect("read reload height"),
            32
        );
        assert_eq!(
            ffi_json(stasis_android_bridge_drain_sprite_releases())["handles"],
            serde_json::json!([])
        );

        let _final_release =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run final release");
        assert_eq!(
            ffi_json(stasis_android_bridge_drain_sprite_releases())["handles"],
            serde_json::json!([handle])
        );
        assert_eq!(
            ffi_json(stasis_android_bridge_drain_sprite_releases())["handles"],
            serde_json::json!([])
        );

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_jit_direct_asset_tasks_release_overloads_compile() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let source = r#"
import "/vendor/stasis/stdlib/asset_tasks.stasis";

global image: ImageAsset;
global audio: AudioAsset;

function main(): void {
    image.release();
    audio.release();
}

function tick(): void {}
"#;
        let (root, _) = write_typed_sprite_project("direct_asset_tasks_release", source);
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
            .expect("compile and run direct asset_tasks release overloads");
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    fn temp_project(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_android_bridge_{name}_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("create project");
        root
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("create copied project directory");
        for entry in fs::read_dir(source).expect("read copied project directory") {
            let entry = entry.expect("read copied project entry");
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if source_path.is_dir() {
                copy_tree(&source_path, &destination_path);
            } else {
                fs::copy(&source_path, &destination_path).expect("copy project file");
            }
        }
    }

    fn stage_android_sample(sample_name: &str, project_name: &str) -> PathBuf {
        let root = temp_project(project_name);
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples")
            .join(sample_name)
            .canonicalize()
            .expect("Android seam sample root");
        copy_tree(&sample, &root);

        // The packaged Android sample resolves its canonical graphics import
        // from the project-owned vendor mount.  Keep the test source and
        // renderer exactly the checked-in sample while staging that mount in
        // the temporary project used by the live bridge.
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src/stdlib")
            .canonicalize()
            .expect("canonical stdlib root");
        copy_tree(&stdlib, &root.join("vendor/stasis/src/stdlib"));
        root
    }

    fn stage_android_aot_sample() -> PathBuf {
        stage_android_sample("android_aot_seam", "it017_aot_sample")
    }

    fn stage_android_touch_sample() -> PathBuf {
        stage_android_sample("android_touch_seam", "it018_touch_sample")
    }

    fn run_android_touch_slot_frame(
        project_root: &Path,
        input: AndroidBridgeTickInput,
        out_i32: &mut [i32],
        out_f32: &mut [f32],
        out_u8: &mut [u8],
    ) -> Result<u32, String> {
        RUNTIME_SESSION.with(|session_cell| -> Result<(), String> {
            let mut session_slot = session_cell.borrow_mut();
            let session = session_slot
                .as_mut()
                .filter(|session| session.project_root == project_root)
                .ok_or_else(|| "Android touch runtime session is not initialized".to_string())?;
            write_production_host_frame(session, input)?;

            let host_i32_ptr = stasis_dynload::stasis_jit_global_i32_array_ptr(
                hash_global_path("host_i32"),
                0,
                768,
            );
            let host_f32_ptr = stasis_dynload::stasis_jit_global_f32_array_ptr(
                hash_global_path("host_f32"),
                0,
                64,
            );
            if host_i32_ptr.is_null() || host_f32_ptr.is_null() {
                return Err("production host frame buffers were not registered".to_string());
            }
            unsafe {
                let host_i32 = std::slice::from_raw_parts_mut(host_i32_ptr, 768);
                let host_f32 = std::slice::from_raw_parts_mut(host_f32_ptr, 64);
                let touch_i32 = [host_i32[545], host_i32[546], host_i32[547]];
                let touch_f32 = [
                    host_f32[0],
                    host_f32[1],
                    host_f32[2],
                    host_f32[3],
                    host_f32[4],
                    host_f32[5],
                ];

                // Android reserves pointer slot zero for the mouse and reports
                // touch contacts starting at slot one.  The production bridge
                // API accepts one touch sample, so mirror the mobile host's
                // frame layout here without changing the production path.
                host_i32[7] = 2;
                host_i32[544..548].copy_from_slice(&[0, 0, 0, 0]);
                host_i32[548..552].copy_from_slice(&[1, touch_i32[0], touch_i32[1], touch_i32[2]]);
                host_f32[0..6].fill(0.0);
                host_f32[6..12].copy_from_slice(&touch_f32);
            }

            execute_lifecycle_noarg(&session.jit, "tick")?;
            take_embedded_resource_error()
                .map_err(|error| format!("touch tick resource error: {}", error.detail))?;
            session.tick_count = session.tick_count.saturating_add(1);
            execute_optional_lifecycle_noarg(&session.jit, "render")?;
            take_embedded_resource_error()
                .map_err(|error| format!("touch render resource error: {}", error.detail))?;
            Ok(())
        })?;

        stasis_dynload::copy_jit_render_active(out_i32, out_f32, out_u8)?;
        write_android_display_metadata(out_i32)?;
        Ok(unsafe {
            stasis_render_trace_native(out_i32.as_ptr(), out_f32.as_ptr(), out_u8.as_ptr())
        })
    }

    fn write_typed_sprite_project(name: &str, source: &str) -> (PathBuf, i32) {
        let root = temp_project(name);
        let stdlib_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/stdlib");
        for (relative, destination) in [
            ("stdlib.stasis", "stdlib.stasis"),
            ("memory.stasis", "memory.stasis"),
            ("graphics.stasis", "graphics.stasis"),
            ("asset_tasks.stasis", "asset_tasks.stasis"),
            ("internal/host_frame.stasis", "internal/host_frame.stasis"),
            ("internal/gfx_cmd.stasis", "internal/gfx_cmd.stasis"),
            (
                "internal/host_window_request.stasis",
                "internal/host_window_request.stasis",
            ),
        ] {
            let target = root.join("vendor/stasis/stdlib").join(destination);
            fs::create_dir_all(target.parent().expect("stdlib target parent"))
                .expect("create vendored stdlib directory");
            fs::copy(stdlib_root.join(relative), target).expect("vendor stdlib source");
        }
        fs::create_dir_all(root.join("assets")).expect("create sprite assets");
        let sprite = include_bytes!(
            "../../../mobile/android/app/src/main/assets/workshop_sample/assets/ball.svg"
        );
        fs::write(root.join("assets/ball.svg"), sprite).expect("write sprite asset");
        let hash = stasis_assets::sha256_bytes(sprite);
        fs::write(
            root.join(stasis_assets::DEFAULT_ASSET_MANIFEST_PATH),
            format!(r#"{{"schema":"stasis-assets","version":1,"assets":[{{"id":"ball","path":"assets/ball.svg","content_sha256":"{hash}","format":{{"kind":"sprite","encoding":"svg","width":32,"height":32}},"dependencies":[]}}]}}"#),
        )
        .expect("write sprite manifest");
        fs::write(root.join("src/main.stasis"), source).expect("write typed sprite source");
        let manifest = load_android_workshop_asset_manifest(&root).expect("load sprite manifest");
        let handle = manifest
            .by_id("ball")
            .expect("manifest sprite")
            .handle
            .as_i32();
        assert!(handle < 0, "fixture must exercise signed stable handle");
        (root, handle)
    }

    #[test]
    fn bridge_compiles_project_and_reports_real_jit_artifacts() {
        let root = temp_project("compile");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 7; }\n",
        )
        .expect("write source");

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile bridge");
        assert_eq!(result.status, 0);
        assert_eq!(result.reload, WorkshopReload::InitialCompile);
        assert!(result.compiled_function_count >= 2);

        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read manifest");
        assert!(manifest.contains("status=CompileReady"));
        assert!(manifest.contains("backend=cranelift-jit"));
        assert!(manifest.contains("artifact_kind=executable-memory"));
        assert!(manifest.contains("entrypoint=main"));
        assert!(manifest.contains("entrypoint=tick"));
        let artifact_relative = manifest
            .lines()
            .find_map(|line| line.strip_prefix("artifact_manifest="))
            .expect("artifact manifest path");
        assert!(artifact_relative.starts_with("build/native_compile_artifacts.v1."));
        let artifact_hash = manifest
            .lines()
            .find_map(|line| line.strip_prefix("artifact_manifest_sha256="))
            .expect("artifact manifest hash");
        assert!(manifest.contains("executable_bytes="));
        let artifact_manifest: AndroidJitArtifactManifestV1 = serde_json::from_str(
            &fs::read_to_string(root.join(artifact_relative)).expect("read artifact manifest"),
        )
        .expect("parse artifact manifest");
        assert_eq!(
            hex_sha256(&fs::read(root.join(artifact_relative)).expect("artifact bytes")),
            artifact_hash
        );
        assert_eq!(artifact_manifest.schema_version, 1);
        assert!(artifact_manifest.artifacts.iter().any(|artifact| {
            artifact.name == "tick" && artifact.symbol_id == "v1|function|src/main.stasis|tick|()"
        }));
        assert_eq!(
            serde_json::from_str::<AndroidJitArtifactManifestV1>(
                &serde_json::to_string(&artifact_manifest).expect("serialize manifest")
            )
            .expect("round trip manifest"),
            artifact_manifest
        );
        assert!(!root.join("build/functions").exists());
        RUNTIME_SESSION.with(|session| {
            assert_eq!(
                session
                    .borrow()
                    .as_ref()
                    .expect("compiled runtime")
                    .jit
                    .execute_i32_noarg_by_name("tick")
                    .expect("execute compiled tick"),
                7
            );
        });

        let state =
            fs::read_to_string(root.join("build/runtime_state.txt")).expect("read runtime state");
        assert!(state.contains("status=RuntimeStateReady"));
        assert!(state.contains("tick_count=0"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_accepts_valid_fields_through_the_production_jit() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("production_jit_authoritative");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { from_file: i32; }\nfunction main(): void { GameState.from_file = 3; }\nfunction tick(): void { GameState.from_file += 1; }\n",
        )
        .expect("write source");

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("production JIT compile");
        assert_eq!(result.status, 0);
        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read manifest");
        assert!(manifest.contains("errors=0"));

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_rejects_unsupported_program_without_stub_success() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("no_stub_fallback");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return missing(); }\n",
        )
        .expect("write unsupported source");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("missing reachable function must fail");
        assert!(error.contains("missing"), "{error}");
        assert!(!root.join("build/native_compile_manifest.txt").exists());
        assert!(!root.join("build/functions").exists());

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_rejects_duplicate_host_alias_without_replacing_active_generation() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("duplicate_host_alias");
        let main_path = root.join("src/main.stasis");
        fs::write(
            &main_path,
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 7; }\n",
        )
        .expect("write accepted source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile accepted generation");

        fs::write(
            root.join("src/duplicate.stasis"),
            "function tick(): i32 { return 99; }\n",
        )
        .expect("write duplicate source");
        fs::write(
            &main_path,
            "import \"duplicate.stasis\";\nfunction main(): i32 { return tick(); }\nfunction tick(): i32 { return 7; }\n",
        )
        .expect("import duplicate source");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("duplicate host alias must fail");
        assert!(error.contains("host ABI alias 'tick' requires exactly one canonical identity"));
        RUNTIME_SESSION.with(|session| {
            let session = session.borrow();
            let session = session.as_ref().expect("active runtime preserved");
            assert!(session.pending_candidate.is_none());
            assert_eq!(session.jit.execute_i32_noarg_by_name("tick"), Ok(7));
        });

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn manifest_write_failure_does_not_stage_runtime_candidate() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("artifact_write_rejection");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write initial source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("initial compile");
        let initial =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("initial tick");
        assert_eq!(initial.observed_game_tick_count, 11);
        let previous_runtime_state =
            fs::read(root.join("build/runtime_state.txt")).expect("read previous runtime state");
        let manifest_path = root.join("build/native_compile_manifest.txt");
        let previous_manifest = fs::read(&manifest_path).expect("read previous manifest");

        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 100; }\n",
        )
        .expect("write changed source");
        FORCE_NEXT_MANIFEST_COMMIT_FAILURE.with(|forced| forced.set(true));

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("manifest write must fail");
        assert!(error.contains("failed writing Android manifest"), "{error}");
        RUNTIME_SESSION.with(|session| {
            let session = session.borrow();
            let session = session.as_ref().expect("active runtime preserved");
            assert!(session.pending_candidate.is_none());
            assert!(session.pending_resource_catalog.is_none());
        });

        let after_failure =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("old runtime tick");
        assert!(!after_failure.recompiled);
        assert_eq!(after_failure.observed_game_tick_count, 12);
        assert_eq!(
            fs::read(root.join("build/runtime_state.txt")).expect("read restored runtime state"),
            previous_runtime_state
        );
        assert_eq!(
            fs::read(&manifest_path).expect("read preserved manifest"),
            previous_manifest
        );

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn artifact_transaction_faults_preserve_authoritative_manifest_and_referenced_json() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("artifact_transaction_faults");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "function main(): i32 { return tick(); } function tick(): i32 { return 1; }",
        )
        .expect("write initial source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("initial compile");
        let manifest_path = root.join("build/native_compile_manifest.txt");
        let accepted_manifest = fs::read(&manifest_path).expect("accepted manifest");
        let accepted_text = String::from_utf8(accepted_manifest.clone()).expect("manifest UTF-8");
        let accepted_artifact = accepted_text
            .lines()
            .find_map(|line| line.strip_prefix("artifact_manifest="))
            .expect("artifact path");
        let accepted_artifact_bytes =
            fs::read(root.join(accepted_artifact)).expect("accepted artifact JSON");
        serde_json::from_slice::<AndroidJitArtifactManifestV1>(&accepted_artifact_bytes)
            .expect("accepted artifact parses");

        fs::write(
            &source,
            "function main(): i32 { return tick(); } function tick(): i32 { return 2; }",
        )
        .expect("write changed source");
        for fault in 1..=3 {
            ANDROID_ARTIFACT_FAULT.with(|slot| slot.set(fault));
            compile_android_workshop_project(&root, Path::new("src/main.stasis"))
                .expect_err("fault must abort publication");
            assert_eq!(
                fs::read(&manifest_path).expect("manifest survives"),
                accepted_manifest
            );
            assert_eq!(
                fs::read(root.join(accepted_artifact)).expect("artifact survives"),
                accepted_artifact_bytes
            );
            serde_json::from_slice::<AndroidJitArtifactManifestV1>(
                &fs::read(root.join(accepted_artifact)).expect("artifact JSON"),
            )
            .expect("authoritative artifact remains valid");
        }

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn runtime_state_write_failure_preserves_published_compile() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("runtime_state_write_rejection");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write initial source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("initial compile");
        let manifest_path = root.join("build/native_compile_manifest.txt");
        let state_path = root.join("build/runtime_state.txt");
        let previous_manifest = fs::read(&manifest_path).expect("read previous manifest");
        let previous_state = fs::read(&state_path).expect("read previous runtime state");

        fs::write(
            &source,
            "global GameState { tick_count: i32; score: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 100; }\n",
        )
        .expect("write layout change");
        let mut permissions = fs::metadata(&state_path)
            .expect("runtime state metadata")
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&state_path, permissions).expect("make runtime state read-only");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("runtime state write must fail");
        assert!(
            error.contains("failed writing Android runtime state"),
            "{error}"
        );
        assert_eq!(
            fs::read(&manifest_path).expect("read preserved manifest"),
            previous_manifest
        );
        assert_eq!(
            fs::read(&state_path).expect("read preserved runtime state"),
            previous_state
        );

        let mut permissions = fs::metadata(&state_path)
            .expect("read-only runtime state metadata")
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&state_path, permissions).expect("restore runtime state permissions");
        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_preserves_runtime_state_for_fast_reload() {
        let root = temp_project("reload");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 1; }\n",
        )
        .expect("write first source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("first compile");
        fs::write(
            root.join("build/runtime_state.txt"),
            "status=RuntimeStateReady\ntick_count=41\n",
        )
        .expect("seed runtime state");

        fs::write(
            &source,
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 2; }\n",
        )
        .expect("write body change");
        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("second compile");
        assert_eq!(result.reload, WorkshopReload::FastReload);
        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read preserved runtime state");
        assert!(state.contains("tick_count=41"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_restart_rebuilds_initial_runtime_state() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("restart_state");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 1; }\n",
        )
        .expect("write source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("initial compile");
        fs::write(
            root.join("build/runtime_state.txt"),
            "status=RuntimeStateReady\ntick_count=41\n",
        )
        .expect("seed stale runtime state");
        clear_runtime_session_for_test();

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("restart compile");
        assert_eq!(result.reload, WorkshopReload::InitialCompile);
        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read restarted runtime state");
        assert!(state.contains("tick_count=0"));

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_reload_stages_a_complete_fresh_generation() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("full_generation_reload");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { score: i32; }\nfunction helper(): i32 { return 1; }\nfunction main(): void { GameState.score = helper(); }\nfunction tick(): void { GameState.score += helper(); }\n",
        )
        .expect("write active source");
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
            .expect("initialize active generation");
        let baseline = inspect_android_runtime_state(&root).expect("inspect baseline generation");
        assert_eq!(baseline["generation"], 1);
        let baseline_source = baseline["source_fingerprint"].clone();

        fs::write(
            &source,
            "global GameState { score: i32; }\nfunction helper(): i32 { return 2; }\nfunction main(): void { GameState.score = helper(); }\nfunction tick(): void { GameState.score += helper(); }\n",
        )
        .expect("write changed source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage complete generation");

        RUNTIME_SESSION.with(|session| {
            let session = session.borrow();
            let candidate = session
                .as_ref()
                .and_then(|session| session.pending_candidate.as_ref())
                .expect("pending generation");
            let metadata = candidate
                .generation_metadata()
                .expect("generation metadata");
            assert_eq!(metadata.emitted_function_ids.len(), 3);
            assert!(metadata.reused_function_ids.is_empty());
            assert_eq!(metadata.module_count, 1);
        });

        let activated =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("activate complete generation");
        assert!(activated.recompiled);
        let activated_state =
            inspect_android_runtime_state(&root).expect("inspect activated generation");
        assert_eq!(activated_state["generation"], 2);
        assert_ne!(activated_state["source_fingerprint"], baseline_source);
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.score")
                .expect("read migrated state"),
            4
        );

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_invalid_edit_preserves_active_generation_and_code() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("invalid_edit_preserves_generation");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write active source");
        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run active source");
        assert_eq!(first.observed_game_tick_count, 11);
        let active = inspect_android_runtime_state(&root).expect("inspect active source");

        fs::write(&source, "function main(): void {\n").expect("write invalid source");
        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("invalid edit must fail compilation");
        assert!(error.contains("diagnostic_file=src/main.stasis"));
        let preserved = inspect_android_runtime_state(&root).expect("inspect preserved source");
        assert_eq!(preserved["generation"], active["generation"]);
        assert_eq!(
            preserved["source_fingerprint"],
            active["source_fingerprint"]
        );

        let resumed =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("active code remains runnable after invalid edit");
        assert_eq!(resumed.observed_game_tick_count, 12);
        let after_frame = inspect_android_runtime_state(&root).expect("inspect after frame");
        assert_eq!(after_frame["generation"], active["generation"]);
        assert_eq!(
            after_frame["source_fingerprint"],
            active["source_fingerprint"]
        );

        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_no_change_compile_keeps_active_generation_without_staging() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("no_change_generation");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return 1; }\nfunction tick(): i32 { return 0; }\n",
        )
        .expect("write source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("initial compile");
        let active_pointers = RUNTIME_SESSION.with(|session| {
            session
                .borrow()
                .as_ref()
                .expect("active session")
                .jit
                .symbol_code_ptrs()
        });

        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("no-change compile");

        RUNTIME_SESSION.with(|session| {
            let session = session.borrow();
            let session = session.as_ref().expect("active session");
            assert!(session.pending_candidate.is_none());
            assert_eq!(session.jit.symbol_code_ptrs(), active_pointers);
        });

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn jit_runtime_state_write_policy_skips_ordinary_frames() {
        assert!(should_write_jit_runtime_state(true, false));
        assert!(should_write_jit_runtime_state(false, true));
        assert!(!should_write_jit_runtime_state(false, false));
    }

    #[test]
    fn bridge_can_set_and_get_i32_global_state_for_tests() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("state_set_get");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { score: i32; tick_count: i32; }\nfunction main(): void { GameState.score = 1; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile bridge");

        set_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.score", 42)
            .expect("set score");
        let score =
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.score")
                .expect("get score");
        assert_eq!(score, 42);
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_state_helper_activates_pending_candidate_before_write() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("state_set_pending_reload");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { score: i32; }\nfunction main(): void { GameState.score = 1; }\nfunction tick(): void { return; }\n",
        )
        .expect("write active source");
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
            .expect("initialize active runtime");

        fs::write(
            &source,
            "global GameState { score: i32; bonus: i32; }\nfunction main(): void { GameState.score = 1; }\nfunction tick(): void { return; }\n",
        )
        .expect("write layout-changing source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage layout-changing candidate");

        set_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.bonus", 42)
            .expect("state helper should activate candidate before write");
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.bonus")
                .expect("read migrated candidate state"),
            42
        );
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
            .expect("run already-activated candidate");
        assert_eq!(
            get_android_workshop_i32_global(&root, Path::new("src/main.stasis"), "GameState.bonus")
                .expect("write should survive next frame"),
            42
        );

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_run_tick_executes_real_void_lifecycle_functions() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("run_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write source");

        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("first real tick");
        assert_eq!(first.tick_count, 1);
        assert!(first.recompiled);
        assert!(first.initialized);
        assert_eq!(first.observed_game_tick_count, 11);

        let second =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("second real tick");
        assert_eq!(second.tick_count, 2);
        assert!(!second.recompiled);
        assert!(!second.initialized);
        assert_eq!(second.observed_game_tick_count, 12);

        let live_state = inspect_android_runtime_state(&root).expect("inspect live runtime state");
        assert_eq!(live_state["source"], "live_session");
        assert_eq!(live_state["tick_count"], 2);
        assert_eq!(live_state["game_tick_count"], 12);

        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read JIT runtime state");
        assert!(state.contains("mode=JitExecuted"));
        assert!(state.contains("tick_count=1"));
        assert!(state.contains("game_tick_count=11"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_run_tick_hot_reloads_without_rerunning_main() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("hot_reload_tick");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\nfunction on_code_swap(): void { GameState.tick_count += 100; }\n",
        )
        .expect("write first source");

        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("first real tick");
        assert_eq!(first.observed_game_tick_count, 11);

        let second =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("second real tick");
        assert_eq!(second.observed_game_tick_count, 12);

        fs::write(
            &source,
            "global GameState { tick_count: i32; bonus: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 2; }\nfunction on_code_swap(): void { GameState.tick_count += 100; GameState.bonus = 7; }\n",
        )
        .expect("write hot reload source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile hot reload source");

        let hot =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("hot reload tick");
        assert_eq!(hot.tick_count, 3);
        assert!(hot.recompiled);
        assert!(!hot.initialized);
        assert_eq!(hot.observed_game_tick_count, 114);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_hot_reload_hook_rejection_restores_old_code_and_state() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("hot_reload_rejection");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { print_string(\"live\"); GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write first source");

        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("first real tick");
        assert_eq!(first.observed_game_tick_count, 11);

        fs::write(
            &source,
            "extern function reject_code_swap(): void;\nglobal GameState { tick_count: i32; added: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 2; }\nfunction on_code_swap(): void { print_string(\"candidate\"); GameState.tick_count = 99; reject_code_swap(); return; }\n",
        )
        .expect("write rejecting hot reload source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage rejecting hot reload source");
        let live_literal = hash_global_path("live");
        let candidate_literal = hash_global_path("candidate");
        assert_eq!(
            stasis_dynload::jit_string_literal_value(live_literal).as_deref(),
            Some("live")
        );
        assert_eq!(
            stasis_dynload::jit_string_literal_value(candidate_literal),
            None
        );

        let error =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect_err("hook rejection should abort hot reload");
        assert!(error.contains("hook requested rejection"));
        RUNTIME_SESSION.with(|session| {
            assert_eq!(
                session
                    .borrow()
                    .as_ref()
                    .and_then(|session| session.last_swap_receipt.as_ref())
                    .map(|receipt| receipt.status),
                Some(DevelopmentSwapStatus::Rejected)
            );
        });
        assert_eq!(
            stasis_dynload::jit_string_literal_value(live_literal).as_deref(),
            Some("live")
        );
        assert_eq!(
            stasis_dynload::jit_string_literal_value(candidate_literal),
            None
        );

        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("retry identical rejected source");
        let retry_error =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect_err("retried hook rejection should run again");
        assert!(retry_error.contains("hook requested rejection"));

        let resumed =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("old runtime should remain usable");
        assert!(!resumed.recompiled);
        assert_eq!(resumed.observed_game_tick_count, 12);

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_code_only_reload_bypasses_oversized_state_snapshot() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let mut active = JitProcess::new();
        active.upsert_file(
            "android_large.stasis",
            "function main(): void { return; } function tick(): void { return; }",
        );
        active.compile().expect("compile active runtime");
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "android_large.stasis",
            "function main(): void { return; } function tick(): void { let value: i32 = 1; return; }",
        );
        candidate
            .compile_staged()
            .expect("compile code-only candidate");
        let oversized_len = MAX_STATE_SNAPSHOT_BYTES / std::mem::size_of::<i32>() + 1;
        stasis_dynload::ensure_jit_i32_array_capacity(91_153, 0, oversized_len)
            .expect("allocate oversized state fixture");
        assert!(
            stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES).is_err()
        );
        let mut session = AndroidRuntimeSession {
            project_root: PathBuf::from("android-large-state"),
            source_fingerprint: 1,
            generation: 1,
            jit: active,
            initialized: true,
            pending_candidate: Some(candidate),
            pending_source_fingerprint: Some(2),
            pending_resource_catalog: None,
            last_swap_receipt: None,
            tick_count: 0,
            previous_input: None,
            display_metrics: AndroidDisplayMetrics::new(1, 1, 1, 1),
            display_signature: [1, 1, 1, 1, 1, 1],
            display_generation: 0,
            density_scale_bits: 1.0f32.to_bits(),
            density_generation: 0,
        };

        assert!(activate_pending_runtime_candidate(&mut session)
            .expect("hook-free activation should bypass snapshot limit"));

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_array_global_table();
    }

    #[test]
    fn workshop_reload_emits_complete_reachable_android_generation() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("selective_warm_reload");
        let entry = Path::new("src/main.stasis");
        let source = root.join(entry);
        let before = "global GameState { tick_count: i32; }\nfunction helper(): i32 { return 1; }\nfunction untouched(): i32 { return 40; }\nfunction main(): void { GameState.tick_count = untouched(); }\nfunction tick(): void { GameState.tick_count += helper(); }\n";
        fs::write(&source, before).expect("write active source");
        compile_android_workshop_project(&root, entry).expect("compile active source");
        let baseline = run_android_workshop_tick(&root, entry, default_tick_input())
            .expect("initialize active source");
        assert_eq!(baseline.observed_game_tick_count, 41);

        fs::write(&source, before.replace("return 1", "return 2")).expect("write helper edit");
        compile_android_workshop_project(&root, entry).expect("stage complete generation");
        compile_android_workshop_project(&root, entry)
            .expect("duplicate notification keeps complete generation staged");

        RUNTIME_SESSION.with(|session_cell| {
            let session_slot = session_cell.borrow();
            let candidate = session_slot
                .as_ref()
                .and_then(|session| session.pending_candidate.as_ref())
                .expect("pending complete candidate");
            let metadata = candidate
                .generation_metadata()
                .expect("complete candidate metadata");
            let name_for_id = |id| {
                candidate
                    .artifacts()
                    .iter()
                    .find(|artifact| artifact.function_id == id)
                    .map(|artifact| artifact.function_key.name.as_str())
                    .expect("metadata function id should have an artifact")
            };
            let emitted: BTreeSet<&str> = metadata
                .emitted_function_ids
                .iter()
                .map(|id| name_for_id(*id))
                .collect();
            let reused: BTreeSet<&str> = metadata
                .reused_function_ids
                .iter()
                .map(|id| name_for_id(*id))
                .collect();
            assert_eq!(
                emitted,
                BTreeSet::from(["helper", "main", "tick", "untouched"])
            );
            assert!(reused.is_empty());
        });
        let activated = run_android_workshop_tick(&root, entry, default_tick_input())
            .expect("activate and execute complete generation");
        assert!(activated.recompiled);
        assert_eq!(activated.observed_game_tick_count, 43);

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_failed_newer_compile_discards_older_pending_candidate() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("stale_pending_candidate");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write active source");
        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("run active source");
        assert_eq!(first.observed_game_tick_count, 11);

        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 2; }\n",
        )
        .expect("write first candidate");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage first candidate");

        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { missing_target(); }\n",
        )
        .expect("write invalid newer candidate");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("newer compile should fail");

        let resumed =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("old runtime should remain active");
        assert!(!resumed.recompiled);
        assert_eq!(resumed.observed_game_tick_count, 12);

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn bridge_run_tick_passes_input_to_stasis_and_exports_render_commands() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("touch_render_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global Input { touch_x: i32; touch_y: i32; touch_active: i32; screen_w: i32; screen_h: i32; }\nglobal GameState { tick_count: i32; paddle_y: i32; }\nglobal Render { command_count: i32; command0_kind: i32; command0_x: i32; command0_y: i32; command0_w: i32; command0_h: i32; command0_color: i32; }\nfunction main(): void { GameState.paddle_y = 40; }\nfunction tick(): void { GameState.tick_count += 1; if (Input.touch_active != 0) { GameState.paddle_y = Input.touch_y; } }\nfunction render(): void { Render.command_count = 1; Render.command0_kind = 1; Render.command0_x = Input.touch_x; Render.command0_y = GameState.paddle_y; Render.command0_w = 8; Render.command0_h = 64; Render.command0_color = 65535; }\n",
        )
        .expect("write source");

        let result = run_android_workshop_tick(
            &root,
            Path::new("src/main.stasis"),
            AndroidBridgeTickInput {
                touch_x: 111,
                touch_y: 222,
                touch_active: 1,
                screen_w: 400,
                screen_h: 700,
            },
        )
        .expect("touch render tick");
        assert!(result.observed_game_tick_count >= 1);
        assert_eq!(result.render_command_count, 1);
        assert_eq!(result.render_commands[0].kind, 1);
        assert_eq!(result.render_commands[0].x, 111);
        assert_eq!(result.render_commands[0].y, 222);
        assert_eq!(result.render_commands[0].w, 8);
        assert_eq!(result.render_commands[0].h, 64);
        assert_eq!(result.render_commands[0].color, 65535);
        assert_eq!(result.render_commands[0].asset, 0);
        assert_eq!(result.render_commands[0].rotation_degrees, 0);
        assert_eq!(result.render_commands[0].alpha, 255);

        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read JIT runtime state");
        assert!(state.contains("render_command_count=1"));
        assert!(state.contains("render0_y=222"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn android_it027_touch_roundtrip_real_jit_preserves_host_lanes_and_marker() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("it027_touch_roundtrip");
        fs::write(
            root.join("src/main.stasis"),
            "global host_i32: i32[768];\nglobal host_f32: f32[64];\n\
             global Input { x: i32; y: i32; dx: i32; dy: i32; xn: i32; yn: i32; active: i32; down: i32; up: i32; checksum: i32; }\n\
             global Render { command_count: i32; command0_kind: i32; command0_x: i32; command0_y: i32; command0_w: i32; command0_h: i32; }\n\
             function main(): void { return; }\n\
             function tick(): void {\n\
                 Input.x.from_f32(host_f32[0]); Input.y.from_f32(host_f32[1]);\n\
                 Input.dx.from_f32(host_f32[2]); Input.dy.from_f32(host_f32[3]);\n\
                 Input.xn.from_f32(host_f32[4] * 1000.0); Input.yn.from_f32(host_f32[5] * 1000.0);\n\
                 Input.active = host_i32[545]; Input.down = host_i32[546]; Input.up = host_i32[547];\n\
                 Input.checksum = Input.x + Input.y * 3 + Input.dx * 5 + Input.dy * 7 + Input.xn * 11 + Input.yn * 13 + Input.active * 17 + Input.down * 19 + Input.up * 23;\n\
             }\n\
             function render(): void {\n\
                 Render.command_count = 1; Render.command0_kind = 1; Render.command0_x = Input.x - 8; Render.command0_y = Input.y - 8; Render.command0_w = 16; Render.command0_h = 16;\n\
             }\n",
        )
        .expect("write IT-027 fixture");
        let entry = Path::new("src/main.stasis");
        let frame = |x: i32, y: i32, active: i32| {
            run_android_workshop_tick(
                &root,
                entry,
                AndroidBridgeTickInput {
                    touch_x: x,
                    touch_y: y,
                    touch_active: active,
                    screen_w: 640,
                    screen_h: 360,
                },
            )
            .expect("run IT-027 fixture frame")
        };
        // An idle frame at a different coordinate must not leak into the next
        // contact's delta; the gesture edge starts a fresh zero-delta sample.
        frame(100, 50, 0);
        frame(160, 90, 1);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.dx").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.checksum").unwrap(),
            6466
        );
        let first = frame(320, 180, 1);
        assert_eq!(first.render_commands[0].x, 312);
        assert_eq!(first.render_commands[0].y, 172);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.dx").unwrap(),
            160
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.yn").unwrap(),
            500
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.checksum").unwrap(),
            14307
        );
        let last = frame(400, 225, 0);
        assert_eq!(last.render_commands[0].x, 392);
        assert_eq!(last.render_commands[0].y, 217);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.up").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.dy").unwrap(),
            45
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.yn").unwrap(),
            625
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "Input.checksum").unwrap(),
            16813
        );
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_it017_aot_sample_runtime_trace_matches_manifest() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = stage_android_aot_sample();
        let entry = Path::new("main.stasis");
        let expectations: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/android_aot_seam/android_seam_expectations.json"
        )))
        .expect("decode IT-017 expectations");
        assert_eq!(expectations["test_id"], "IT-017");
        let stable_frame = expectations["stable_frame"]
            .as_u64()
            .expect("IT-017 stable frame") as usize;
        let expected_checksum = expectations["state_checksum"]
            .as_i64()
            .expect("IT-017 state checksum") as i32;
        let expected_trace = expectations["command_trace"]
            .as_u64()
            .expect("IT-017 command trace") as u32;
        let logical_size = expectations["logical_size"]
            .as_array()
            .expect("IT-017 logical size");
        let logical_w = logical_size[0].as_i64().expect("IT-017 logical width") as i32;
        let logical_h = logical_size[1].as_i64().expect("IT-017 logical height") as i32;

        let mut frame_i32 = vec![0_i32; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut frame_f32 = vec![0.0_f32; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut frame_u8 = vec![0_u8; ANDROID_RENDER_GFX_U8_CAPACITY];
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("main.stasis").expect("entry cstr");
        for _ in 0..stable_frame {
            let status = stasis_android_bridge_run_tick_frame_v2(
                root_c.as_ptr(),
                entry_c.as_ptr(),
                0,
                0,
                0,
                logical_w,
                logical_h,
                frame_i32.as_mut_ptr(),
                frame_i32.len(),
                frame_f32.as_mut_ptr(),
                frame_f32.len(),
                frame_u8.as_mut_ptr(),
                frame_u8.len(),
            );
            assert_eq!(
                status,
                0,
                "IT-017 sample frame failed: {:?}",
                LAST_FRAME_ERROR.with(|slot| slot.borrow().clone())
            );
        }

        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_checksum")
                .expect("read IT-017 state checksum"),
            expected_checksum,
            "IT-017 stable frame must retain its checked-in state oracle"
        );
        assert_eq!(&frame_i32[0..5], &[1196967473, 6, 3, 0, 0]);
        assert_eq!(frame_i32[7], 0, "IT-017 sample must not emit text");
        assert_eq!(frame_i32[9], 0, "IT-017 sample must not emit text bytes");
        assert_eq!(frame_i32[10], logical_w);
        assert_eq!(frame_i32[11], logical_h);
        assert_eq!(frame_i32[22], 3, "IT-017 sample rectangle order count");
        assert_eq!(frame_i32[24], 3, "IT-017 sample rectangle count");
        assert_eq!(
            &frame_f32[79996..80004],
            &[64.0, 72.0, 192.0, 216.0, 0.90, 0.12, 0.16, 1.0]
        );
        assert_eq!(
            &frame_f32[79988..79996],
            &[384.0, 72.0, 192.0, 216.0, 0.10, 0.78, 0.72, 1.0]
        );
        assert_eq!(
            &frame_f32[79980..79988],
            &[296.0, 156.0, 48.0, 48.0, 0.95, 0.90, 0.30, 1.0]
        );
        let trace = unsafe {
            stasis_render_trace_native(frame_i32.as_ptr(), frame_f32.as_ptr(), frame_u8.as_ptr())
        };
        assert_eq!(
            trace, expected_trace,
            "IT-017 runtime trace must match manifest"
        );

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_it018_touch_sample_runtime_traces_match_manifest() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = stage_android_touch_sample();
        let entry = Path::new("main.stasis");
        let expectations: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/android_touch_seam/android_seam_expectations.json"
        )))
        .expect("decode IT-018 expectations");
        assert_eq!(expectations["test_id"], "IT-018");
        let stable_frame = expectations["stable_frame"]
            .as_u64()
            .expect("IT-018 stable frame") as usize;
        let expected_checksum = expectations["state_checksum"]
            .as_i64()
            .expect("IT-018 state checksum") as i32;
        let expected_trace = expectations["command_trace"]
            .as_u64()
            .expect("IT-018 stable command trace") as u32;
        let logical_size = expectations["logical_size"]
            .as_array()
            .expect("IT-018 logical size");
        let logical_w = logical_size[0].as_i64().expect("IT-018 logical width") as i32;
        let logical_h = logical_size[1].as_i64().expect("IT-018 logical height") as i32;
        let touch = expectations["touch"]
            .as_object()
            .expect("IT-018 touch contract");
        let expected_completion_sequence = touch["completion_sequence"]
            .as_i64()
            .expect("IT-018 completion sequence") as i32;
        let expected_final_trace = touch["final_command_trace"]
            .as_u64()
            .expect("IT-018 final command trace") as u32;
        let safe_viewport = touch["safe_viewport"]
            .as_array()
            .expect("IT-018 safe viewport");
        let safe_x = safe_viewport[0].as_f64().expect("IT-018 safe x") as f32;
        let safe_y = safe_viewport[1].as_f64().expect("IT-018 safe y") as f32;
        let safe_w = safe_viewport[2].as_f64().expect("IT-018 safe width") as f32;
        let safe_h = safe_viewport[3].as_f64().expect("IT-018 safe height") as f32;

        const NATIVE_W: i32 = 1080;
        const NATIVE_H: i32 = 2400;
        let mut frame_i32 = vec![0_i32; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut frame_f32 = vec![0.0_f32; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut frame_u8 = vec![0_u8; ANDROID_RENDER_GFX_U8_CAPACITY];
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("main.stasis").expect("entry cstr");
        for _ in 0..stable_frame {
            let status = stasis_android_bridge_run_tick_frame_v2(
                root_c.as_ptr(),
                entry_c.as_ptr(),
                0,
                0,
                0,
                NATIVE_W,
                NATIVE_H,
                frame_i32.as_mut_ptr(),
                frame_i32.len(),
                frame_f32.as_mut_ptr(),
                frame_f32.len(),
                frame_u8.as_mut_ptr(),
                frame_u8.len(),
            );
            assert_eq!(
                status,
                0,
                "IT-018 stable frame failed: {:?}",
                LAST_FRAME_ERROR.with(|slot| slot.borrow().clone())
            );
        }
        let stable_trace = unsafe {
            stasis_render_trace_native(frame_i32.as_ptr(), frame_f32.as_ptr(), frame_u8.as_ptr())
        };
        assert_eq!(stable_trace, expected_trace);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_checksum")
                .expect("read IT-018 stable checksum"),
            expected_checksum
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence")
                .expect("read IT-018 stable probe sequence"),
            0
        );
        assert_eq!(&frame_i32[..5], &[1196967473, 6, 3, 0, 0]);
        assert_eq!(
            &frame_i32[10..16],
            &[logical_w, logical_h, NATIVE_W, NATIVE_H, NATIVE_W, NATIVE_H]
        );
        assert_eq!(
            &frame_i32[16..20],
            &[safe_x as i32, safe_y as i32, safe_w as i32, safe_h as i32]
        );
        let global_f32 =
            |path: &str| stasis_dynload::stasis_jit_global_f32_load(hash_global_path(path));

        let outside_down = run_android_touch_slot_frame(
            &root,
            AndroidBridgeTickInput {
                touch_x: 540,
                touch_y: 60,
                touch_active: 1,
                screen_w: NATIVE_W,
                screen_h: NATIVE_H,
            },
            &mut frame_i32,
            &mut frame_f32,
            &mut frame_u8,
        )
        .expect("run IT-018 outside-letterbox down");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_kind").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_is_down").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_went_down").unwrap(),
            1
        );
        assert!((global_f32("seam_safe_x") - safe_x).abs() < 0.01);
        assert!((global_f32("seam_safe_y") - safe_y).abs() < 0.01);
        assert!((global_f32("seam_safe_w") - safe_w).abs() < 0.01);
        assert!((global_f32("seam_safe_h") - safe_h).abs() < 0.01);
        assert!((global_f32("seam_pointer_y") - 0.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_y_n") - 0.0).abs() < 0.01);

        let outside_up = run_android_touch_slot_frame(
            &root,
            AndroidBridgeTickInput {
                touch_x: 540,
                touch_y: 60,
                touch_active: 0,
                screen_w: NATIVE_W,
                screen_h: NATIVE_H,
            },
            &mut frame_i32,
            &mut frame_f32,
            &mut frame_u8,
        )
        .expect("run IT-018 outside-letterbox up");
        assert_eq!(
            outside_up, outside_down,
            "outside-letterbox gesture must not change the rendered state"
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_kind").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_went_up").unwrap(),
            1
        );

        let inside_down = run_android_touch_slot_frame(
            &root,
            AndroidBridgeTickInput {
                touch_x: 270,
                touch_y: 660,
                touch_active: 1,
                screen_w: NATIVE_W,
                screen_h: NATIVE_H,
            },
            &mut frame_i32,
            &mut frame_f32,
            &mut frame_u8,
        )
        .expect("run IT-018 inside-drag down");
        assert_ne!(inside_down, outside_up);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence").unwrap(),
            3
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_kind").unwrap(),
            3
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_input_phase").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_transitions").unwrap(),
            1
        );
        assert!((global_f32("seam_pointer_x") - 90.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_y") - 180.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_x_n") - 0.25).abs() < 0.01);
        assert!((global_f32("seam_pointer_y_n") - 0.25).abs() < 0.01);

        let inside_move = run_android_touch_slot_frame(
            &root,
            AndroidBridgeTickInput {
                touch_x: 810,
                touch_y: 1740,
                touch_active: 1,
                screen_w: NATIVE_W,
                screen_h: NATIVE_H,
            },
            &mut frame_i32,
            &mut frame_f32,
            &mut frame_u8,
        )
        .expect("run IT-018 inside-drag move");
        assert_ne!(inside_move, inside_down);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence").unwrap(),
            4
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_kind").unwrap(),
            4
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_input_phase").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_transitions").unwrap(),
            1
        );
        assert!((global_f32("seam_pointer_x") - 270.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_y") - 540.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_dx") - 180.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_dy") - 360.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_x_n") - 0.75).abs() < 0.01);
        assert!((global_f32("seam_pointer_y_n") - 0.75).abs() < 0.01);

        let final_trace = run_android_touch_slot_frame(
            &root,
            AndroidBridgeTickInput {
                touch_x: 810,
                touch_y: 1740,
                touch_active: 0,
                screen_w: NATIVE_W,
                screen_h: NATIVE_H,
            },
            &mut frame_i32,
            &mut frame_f32,
            &mut frame_u8,
        )
        .expect("run IT-018 inside-drag up");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_sequence").unwrap(),
            expected_completion_sequence
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_probe_kind").unwrap(),
            5
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_input_phase").unwrap(),
            3
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_transitions").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_checksum").unwrap(),
            3215
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_down_count").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_move_count").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_up_count").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_is_down").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_went_up").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_count").unwrap(),
            2
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_pointer_id").unwrap(),
            1
        );
        assert!((global_f32("seam_pointer_x") - 270.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_y") - 540.0).abs() < 0.01);
        assert!((global_f32("seam_pointer_x_n") - 0.75).abs() < 0.01);
        assert!((global_f32("seam_pointer_y_n") - 0.75).abs() < 0.01);
        assert_eq!(frame_i32[1], 6);
        assert_eq!(frame_i32[22], 3, "IT-018 final render order count");
        assert_eq!(frame_i32[24], 3, "IT-018 final render rectangle count");
        assert_eq!(
            &frame_f32[79996..80004],
            &[20.0, 20.0, 320.0, 680.0, 0.10, 0.12, 0.16, 1.0]
        );
        assert_eq!(
            &frame_f32[79988..79996],
            &[60.0, 260.0, 240.0, 200.0, 0.12, 0.84, 0.36, 1.0]
        );
        assert_eq!(
            &frame_f32[79980..79988],
            &[252.0, 522.0, 36.0, 36.0, 0.95, 0.20, 0.80, 1.0]
        );
        assert_eq!(final_trace, expected_final_trace);

        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_it027_render_parity_sample_real_jit_exports_marker_and_idle_trace() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../samples/render_parity")
            .canonicalize()
            .expect("render parity sample root");
        let entry = Path::new("main.stasis");
        let capture_manifest: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/render_parity/capture_manifest.json"
        )))
        .expect("decode render parity capture manifest");
        assert_eq!(
            capture_manifest["fixture"],
            "samples/render_parity/main.stasis"
        );
        let expected_workshop_trace = capture_manifest["workshop_command_trace"]
            .as_u64()
            .expect("render parity Workshop trace") as u32;
        let expected_state_checksum = capture_manifest["state_checksum"]
            .as_i64()
            .expect("render parity state checksum") as i32;
        let expected_render_contract_version = capture_manifest["render_contract_version"]
            .as_i64()
            .expect("render parity render contract version")
            as i32;
        let mut frame_i32 = vec![0_i32; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut frame_f32 = vec![0.0_f32; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut frame_u8 = vec![0_u8; ANDROID_RENDER_GFX_U8_CAPACITY];
        const RECT_COUNT: usize = 24;
        const ORDER_COUNT: usize = 22;
        const RECT_REVERSE_BASE: usize = 79_996;
        const FRAME_TOKEN: usize = ANDROID_RENDER_I_FRAME_TOKEN;

        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("main.stasis").expect("entry cstr");
        let run_frame = |x: i32,
                         y: i32,
                         active: i32,
                         frame_i32: &mut Vec<i32>,
                         frame_f32: &mut Vec<f32>,
                         frame_u8: &mut Vec<u8>|
         -> u32 {
            let status = stasis_android_bridge_run_tick_frame_v2(
                root_c.as_ptr(),
                entry_c.as_ptr(),
                x,
                y,
                active,
                640,
                360,
                frame_i32.as_mut_ptr(),
                frame_i32.len(),
                frame_f32.as_mut_ptr(),
                frame_f32.len(),
                frame_u8.as_mut_ptr(),
                frame_u8.len(),
            );
            assert_eq!(
                status,
                0,
                "render parity frame failed: {:?}",
                LAST_FRAME_ERROR.with(|slot| slot.borrow().clone())
            );
            assert_eq!(
                frame_i32[1], expected_render_contract_version,
                "render parity frame version must stay linked to the Workshop manifest"
            );
            let trace = unsafe {
                stasis_render_trace_native(
                    frame_i32.as_ptr(),
                    frame_f32.as_ptr(),
                    frame_u8.as_ptr(),
                )
            };
            assert_ne!(trace, 0, "render parity frame must have a canonical trace");
            trace
        };

        let down_trace = run_frame(160, 90, 1, &mut frame_i32, &mut frame_f32, &mut frame_u8);
        assert_eq!(frame_i32[RECT_COUNT], 2);
        assert_eq!(frame_i32[ORDER_COUNT], 11);
        assert_eq!(frame_i32[FRAME_TOKEN], 1);
        assert_eq!(
            &frame_f32[RECT_REVERSE_BASE - 8..RECT_REVERSE_BASE],
            &[152.0, 82.0, 16.0, 16.0, 1.0, 0.65, 0.08, 1.0]
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_x").unwrap(),
            160
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_y").unwrap(),
            90
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_dx").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_down_edge").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_marker_active").unwrap(),
            1
        );

        let move_trace = run_frame(320, 180, 1, &mut frame_i32, &mut frame_f32, &mut frame_u8);
        assert_ne!(move_trace, down_trace);
        assert_eq!(frame_i32[RECT_COUNT], 2);
        assert_eq!(frame_i32[ORDER_COUNT], 11);
        assert_eq!(frame_i32[FRAME_TOKEN], 2);
        assert_eq!(
            &frame_f32[RECT_REVERSE_BASE - 8..RECT_REVERSE_BASE],
            &[312.0, 172.0, 16.0, 16.0, 1.0, 0.65, 0.08, 1.0]
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_dx").unwrap(),
            160
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_dy").unwrap(),
            90
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_x_norm_x1000").unwrap(),
            500
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_y_norm_x1000").unwrap(),
            500
        );

        let up_trace = run_frame(400, 225, 0, &mut frame_i32, &mut frame_f32, &mut frame_u8);
        assert_ne!(up_trace, move_trace);
        assert_eq!(frame_i32[RECT_COUNT], 2);
        assert_eq!(frame_i32[ORDER_COUNT], 11);
        assert_eq!(frame_i32[FRAME_TOKEN], 3);
        assert_eq!(
            &frame_f32[RECT_REVERSE_BASE - 8..RECT_REVERSE_BASE],
            &[392.0, 217.0, 16.0, 16.0, 1.0, 0.65, 0.08, 1.0]
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_active").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_up_edge").unwrap(),
            1
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_dy").unwrap(),
            45
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_y_norm_x1000").unwrap(),
            625
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_checksum").unwrap(),
            16813
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_marker_active").unwrap(),
            1
        );

        let idle_trace = run_frame(0, 0, 0, &mut frame_i32, &mut frame_f32, &mut frame_u8);
        assert_eq!(idle_trace, expected_workshop_trace);
        assert_eq!(frame_i32[RECT_COUNT], 1);
        assert_eq!(frame_i32[ORDER_COUNT], 10);
        assert_eq!(frame_i32[FRAME_TOKEN], 4);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_touch_marker_active").unwrap(),
            0
        );
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "seam_state_checksum").unwrap(),
            expected_state_checksum,
            "render parity state oracle must stay linked to the capture manifest"
        );
        assert_eq!(
            idle_trace, expected_workshop_trace,
            "render parity idle trace must stay linked to the Workshop manifest"
        );
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_bundled_touch_pong_sample_real_jit_is_runnable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile bundled pong sample");
        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read bundled sample manifest");

        assert_eq!(result.status, 0, "{manifest}");
        assert!(result.compiled_function_count >= 5, "{manifest}");
    }

    #[test]
    fn android_bundled_touch_pong_sample_runs_and_exports_render_commands() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");

        let result = run_android_workshop_tick(
            &root,
            Path::new("src/main.stasis"),
            AndroidBridgeTickInput {
                touch_x: 180,
                touch_y: 240,
                touch_active: 1,
                screen_w: 360,
                screen_h: 640,
            },
        )
        .expect("bundled pong tick");

        assert_eq!(result.render_command_count, 5);
        assert_eq!(result.render_commands[0].kind, 1);
        assert_eq!(result.render_commands[0].w, 360);
        assert_eq!(result.render_commands[1].y, 204);
        assert_eq!(result.render_commands[3].kind, 2);
        assert_eq!(result.render_commands[3].asset, -1520461853);
        assert_eq!(result.render_commands[3].rotation_degrees, 3);
        assert_eq!(result.render_commands[3].alpha, 255);
        assert_eq!(result.render_commands[3].clip_x, 0);
        assert_eq!(result.render_commands[3].clip_y, 0);
        assert_eq!(result.render_commands[3].clip_w, 360);
        assert_eq!(result.render_commands[3].clip_h, 640);
        assert!(result.observed_game_tick_count >= 1);
    }

    #[test]
    fn android_exploration_template_accepts_touch_and_exports_render_commands() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/exploration_sample")
            .canonicalize()
            .expect("exploration template root");

        let result = run_android_workshop_tick(
            &root,
            Path::new("src/main.stasis"),
            AndroidBridgeTickInput {
                touch_x: 90,
                touch_y: 180,
                touch_active: 1,
                screen_w: 360,
                screen_h: 640,
            },
        )
        .expect("exploration touch tick");

        assert_eq!(result.render_command_count, 8);
        assert_eq!(result.render_commands[0].kind, 1);
        assert_eq!(result.render_commands[0].w, 360);
        assert_eq!(result.render_commands[4].kind, 2);
        assert_eq!(result.render_commands[4].x, 164);
        assert_eq!(result.render_commands[4].y, 302);
        assert_eq!(result.render_commands[4].asset, 1_921_230_027);
        assert_eq!(result.render_commands[5].kind, 2);
        assert_eq!(result.render_commands[5].x, 83);
        assert_eq!(result.render_commands[5].y, 174);
        assert_eq!(result.render_commands[5].asset, 476_662_006);
        assert!(result.observed_game_tick_count >= 1);
        clear_runtime_session_for_test();
    }
    #[test]
    fn android_bundled_touch_pong_enemy_paddle_speed_schedule_is_linear() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");
        let entry = Path::new("src/main.stasis");

        let first = run_android_workshop_tick(
            &root,
            entry,
            AndroidBridgeTickInput {
                touch_x: 180,
                touch_y: 240,
                touch_active: 1,
                screen_w: 360,
                screen_h: 640,
            },
        )
        .expect("initial pong tick");
        assert!(first.observed_game_tick_count >= 1);
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "GameState.enemy_paddle_speed_x100")
                .expect("initial enemy speed"),
            1500,
            "enemy paddle starts at 3x a 5px/tick ball speed"
        );

        set_android_workshop_i32_global(&root, entry, "GameState.ball_age_ticks", 1800)
            .expect("set half age");
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("half-age tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "GameState.enemy_paddle_speed_x100")
                .expect("half-age enemy speed"),
            875,
            "after 30 seconds, speed is halfway from 3x to 0.5x"
        );

        set_android_workshop_i32_global(&root, entry, "GameState.ball_age_ticks", 3600)
            .expect("set full age");
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("full-age tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "GameState.enemy_paddle_speed_x100")
                .expect("full-age enemy speed"),
            250,
            "after 60 seconds, speed reaches 0.5x a 5px/tick ball speed"
        );

        set_android_workshop_i32_global(&root, entry, "GameState.ball_age_ticks", 7200)
            .expect("set over age");
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("over-age tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "GameState.enemy_paddle_speed_x100")
                .expect("over-age enemy speed"),
            250,
            "after 60 seconds, speed stays clamped at 0.5x a 5px/tick ball speed"
        );

        set_android_workshop_i32_global(&root, entry, "GameState.ball_age_ticks", 1800)
            .expect("set stale age before reset");
        set_android_workshop_i32_global(&root, entry, "GameState.ball_x", 361)
            .expect("force ball reset");
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("reset tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "GameState.enemy_paddle_speed_x100")
                .expect("reset enemy speed"),
            1500,
            "each ball creation resets enemy paddle speed to 3x"
        );
        assert!(
            get_android_workshop_i32_global(&root, entry, "GameState.ball_age_ticks")
                .expect("reset ball age")
                <= 1,
            "ball age resets when a new ball is created"
        );

        clear_runtime_session_for_test();
    }
    #[test]
    fn android_bridge_runs_bundled_stasis_tests() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");
        let result = run_android_workshop_stasis_tests(&root).expect("run bundled Stasis tests");
        assert_eq!(result["passed"], 2, "{result}");
        assert_eq!(result["failed"], 0);
        assert_eq!(result["all_passed"], true);
        assert_eq!(
            result["results"][0]["file"],
            "tests/enemy_paddle_speed_schedule.test.stasis"
        );
        assert_eq!(result["results"][0]["line"], 3);
        clear_runtime_session_for_test();
    }

    #[test]
    fn android_bridge_runs_exploration_template_tests() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/exploration_sample")
            .canonicalize()
            .expect("exploration template root");
        let result =
            run_android_workshop_stasis_tests(&root).expect("run exploration Stasis tests");
        assert_eq!(result["passed"], 10);
        assert_eq!(result["failed"], 0);
        assert_eq!(result["all_passed"], true);
        assert_eq!(
            result["results"][0]["file"],
            "tests/exploration_gameplay.test.stasis"
        );
    }

    #[test]
    fn android_test_failure_reports_navigable_file_and_line() {
        let root = temp_project("test_failure_location");
        fs::create_dir_all(root.join("tests")).expect("create tests");
        fs::write(
            root.join("tests/failing.test.stasis"),
            "\n\ntest `intentional failure`(): bool {\n    return false;\n}\n",
        )
        .expect("write failing test");
        let result = run_android_workshop_stasis_tests(&root).expect("run Stasis tests");
        assert_eq!(result["failed"], 1);
        assert_eq!(result["results"][0]["file"], "tests/failing.test.stasis");
        assert_eq!(result["results"][0]["line"], 3);
        assert_eq!(result["results"][0]["name"], "intentional failure");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_test_compile_failure_reports_navigable_file_and_line() {
        let root = temp_project("test_compile_failure_location");
        fs::create_dir_all(root.join("tests")).expect("create tests");
        fs::write(
            root.join("tests/broken.test.stasis"),
            "\n\ntest `broken test`(): bool {\n    return true;\n",
        )
        .expect("write broken test");
        let result = run_android_workshop_stasis_tests(&root).expect("run Stasis tests");
        assert_eq!(result["failed"], 1);
        assert_eq!(result["results"][0]["file"], "tests/broken.test.stasis");
        assert_eq!(result["results"][0]["line"], 3);
        assert_eq!(result["results"][0]["column"], 1);
        assert_eq!(result["results"][0]["name"], "broken test");
        assert_eq!(result["results"][0]["status"], "compile_failed");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_generated_test_compile_failure_maps_back_to_test_declaration() {
        let root = temp_project("generated_test_compile_failure_location");
        fs::create_dir_all(root.join("tests")).expect("create tests");
        fs::write(
            root.join("tests/generated_failure.test.stasis"),
            "\n\ntest `missing helper`(): bool {\n    return missing();\n}\n",
        )
        .expect("write failing test");

        let result = run_android_workshop_stasis_tests(&root).expect("run Stasis tests");
        assert_eq!(result["failed"], 1);
        assert_eq!(
            result["results"][0]["file"],
            "tests/generated_failure.test.stasis"
        );
        assert_eq!(result["results"][0]["line"], 3);
        assert_eq!(result["results"][0]["column"], 1);
        assert_eq!(result["results"][0]["name"], "missing helper");
        assert_eq!(result["results"][0]["status"], "compile_failed");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_compile_failure_reports_imported_file_symbol_and_span() {
        let root = temp_project("cross_file_compile_diagnostic");
        fs::create_dir_all(root.join("src/systems")).expect("create systems");
        fs::write(
            root.join("src/main.stasis"),
            "import \"systems/broken.stasis\";\nfunction main(): void {}\nfunction tick(): void {}\n",
        )
        .expect("write entry");
        fs::write(
            root.join("src/systems/broken.stasis"),
            "\n\nfunction broken(: i32): void {}\n",
        )
        .expect("write broken import");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("compile should fail");
        assert!(
            error.contains("|diagnostic_file=src/systems/broken.stasis"),
            "{error}"
        );
        assert!(error.contains("|diagnostic_line=3"));
        assert!(error.contains("|diagnostic_column=17"));
        assert!(error.contains("|diagnostic_symbol=broken"));
        assert!(error.contains("|diagnostic_message="));
        assert!(error.contains("diagnostic_stage=parse"));
        assert!(error.contains("diagnostic_code=stasis.parse"));
        assert!(error.contains("diagnostic_causes="));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_parse_failure_preserves_parser_owned_final_function_span() {
        let root = temp_project("final_function_parse_diagnostic");
        let source = "function first(): void { return; }\n\nfunction final_hook(): void {\n";
        fs::write(root.join("src/main.stasis"), source).expect("write malformed source");
        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("missing final brace must fail");
        assert!(error.contains("diagnostic_file=src/main.stasis"), "{error}");
        assert!(error.contains("diagnostic_symbol=final_hook"), "{error}");
        assert!(error.contains("diagnostic_stage=parse"), "{error}");
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("parse envelope");
        assert_eq!(envelope["context"]["symbol"], "final_hook");
        assert_eq!(envelope["context"]["file"], "src/main.stasis");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_body_parse_failure_uses_typed_parse_envelope_and_function_context() {
        let root = temp_project("body_parse_diagnostic");
        let source = "function main(): void { let broken = ; }\n";
        fs::write(root.join("src/main.stasis"), source).expect("write malformed body");
        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("malformed body must fail");
        assert!(
            error.contains("diagnostic_schema=stasis.native_diagnostic.v1"),
            "{error}"
        );
        assert!(error.contains("diagnostic_stage=parse"), "{error}");
        assert!(error.contains("diagnostic_code=stasis.parse"), "{error}");
        assert!(error.contains("diagnostic_file=src/main.stasis"), "{error}");
        assert!(error.contains("diagnostic_symbol=main"), "{error}");
        assert!(error.contains("diagnostic_line=1"), "{error}");
        assert!(error.contains("diagnostic_end_line=1"), "{error}");
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("body parse envelope");
        assert_eq!(envelope["stage"], "parse");
        assert_eq!(envelope["code"], "stasis.parse");
        assert_eq!(envelope["context"]["file"], "src/main.stasis");
        assert_eq!(envelope["context"]["symbol"], "main");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_import_parse_failure_preserves_imported_file_diagnostic() {
        let root = temp_project("import_parse_diagnostic");
        fs::create_dir_all(root.join("src/systems")).expect("systems directory");
        fs::write(
            root.join("src/main.stasis"),
            "import \"systems/broken.stasis\";\nfunction main(): void {}\n",
        )
        .expect("write entry source");
        fs::write(
            root.join("src/systems/broken.stasis"),
            "import \"helper.txt\";\nfunction helper(): void {}\n",
        )
        .expect("write imported source");
        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("invalid imported target must fail");
        assert!(
            error.contains("diagnostic_file=src/systems/broken.stasis"),
            "{error}"
        );
        assert!(error.contains("diagnostic_stage=parse"), "{error}");
        assert!(error.contains("diagnostic_code=stasis.parse"), "{error}");
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("import parse envelope");
        assert_eq!(envelope["context"]["file"], "src/systems/broken.stasis");
        assert_eq!(envelope["context"]["symbol"], "helper.txt");
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_semantic_failure_reports_imported_function_span() {
        let root = temp_project("cross_file_backend_diagnostic");
        fs::create_dir_all(root.join("src/systems")).expect("create systems");
        fs::write(
            root.join("src/main.stasis"),
            "import \"systems/broken.stasis\";\nfunction main(): void {}\nfunction tick(): void {}\n",
        )
        .expect("write entry");
        fs::write(
            root.join("src/systems/broken.stasis"),
            "\n\nfunction on_code_swap(): void { missing_target(); }\n",
        )
        .expect("write imported semantic failure");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("compile should fail");
        assert!(
            error.contains("cannot%20resolve%20call%20%27missing_target%27"),
            "unexpected error: {error}"
        );
        assert!(error.contains("|diagnostic_file=src/systems/broken.stasis"));
        assert!(error.contains("|diagnostic_line=3"));
        assert!(error.contains("|diagnostic_symbol=on_code_swap"));
        fs::remove_dir_all(root).ok();
    }
    #[test]
    fn it031_extern_failure_preserves_typed_native_envelope() {
        let root = temp_project("it031_unresolved_extern");
        fs::write(
            root.join("src/main.stasis"),
            "extern function IT031_missing_extern(): void;\n\
function main(): void {}\n\
function tick(): void { IT031_missing_extern(); }\n\
function render(): void {}\n\
function on_code_swap(): void {}\n",
        )
        .expect("write source");
        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("unresolved extern must fail");
        assert!(
            error.contains("diagnostic_schema=stasis.native_diagnostic.v1"),
            "{error}"
        );
        assert!(
            error.contains("diagnostic_stage=extern_resolution"),
            "{error}"
        );
        assert!(
            error.contains("diagnostic_code=stasis.unresolvedExtern"),
            "{error}"
        );
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("extern envelope");
        assert_eq!(envelope["context"]["file"], "src/main.stasis");
        assert_eq!(envelope["context"]["symbol"], "IT031_missing_extern");
        assert_eq!(envelope["causes"][0], "extern_resolution phase");
        assert_eq!(
            envelope["causes"].as_array().unwrap().last().unwrap(),
            &envelope["detail"]
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn it031_runtime_and_render_resource_phase_tags_preserve_detail() {
        let runtime = format_android_bridge_error(
            Path::new("."),
            AndroidBridgeError::phase(
                "runtime_entry",
                "tick",
                "entrypoint signature mismatch",
                None,
            ),
        );
        let render = format_android_bridge_error(
            Path::new("."),
            AndroidBridgeError::phase(
                "render_schema",
                "render",
                "JIT frame is not a supported production gfx_cmd frame",
                None,
            ),
        );
        let resource = format_android_bridge_error(
            Path::new("."),
            AndroidBridgeError::phase(
                "resource",
                "render",
                "render resource error: sprite path is invalid or missing: assets/IT031_missing.svg",
                Some("assets/IT031_missing.svg".to_string()),
            ),
        );
        for (message, stage, code) in [
            (runtime, "runtime_entry", "stasis.runtimeEntry"),
            (render, "render_schema", "stasis.renderSchema"),
            (resource, "resource", "stasis.missingResource"),
        ] {
            let envelope = message
                .split("diagnostic_envelope=")
                .nth(1)
                .map(percent_decode_for_test)
                .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
                .expect("phase envelope");
            assert_eq!(envelope["stage"], stage);
            assert_eq!(envelope["code"], code);
            assert_eq!(envelope["causes"][0], format!("{stage} phase"));
            assert_eq!(
                envelope["causes"].as_array().unwrap().last().unwrap(),
                &envelope["detail"]
            );
        }
    }

    #[test]
    fn it031_runtime_entry_frame_failure_is_typed_at_tick_call_site() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("it031_runtime_entry");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): void {}\n\
function tick(value: i32): i32 { return value; }\n\
function render(): i32 { return 0; }\n\
function on_code_swap(): void {}\n",
        )
        .expect("write source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("runtime-entry source compiles before invocation");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut i32_values = vec![0; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut f32_values = vec![0.0; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut u8_values = vec![0; ANDROID_RENDER_GFX_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v2(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            0,
            0,
            0,
            320,
            240,
            i32_values.as_mut_ptr(),
            i32_values.len(),
            f32_values.as_mut_ptr(),
            f32_values.len(),
            u8_values.as_mut_ptr(),
            u8_values.len(),
        );
        assert_eq!(status, -1);
        let error_ptr = stasis_android_bridge_last_frame_error();
        let error = unsafe { CStr::from_ptr(error_ptr) }
            .to_string_lossy()
            .into_owned();
        stasis_android_bridge_free_string(error_ptr);
        assert!(error.contains("diagnostic_stage=runtime_entry"), "{error}");
        assert!(
            error.contains("diagnostic_code=stasis.runtimeEntry"),
            "{error}"
        );
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("runtime envelope");
        assert_eq!(envelope["context"]["symbol"], "tick");
        assert_eq!(envelope["causes"][0], "runtime_entry phase");
        assert_eq!(
            envelope["causes"].as_array().unwrap().last().unwrap(),
            &envelope["detail"]
        );
        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn it031_render_schema_frame_failure_is_typed_at_copy_call_site() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("it031_render_schema");
        fs::write(
            root.join("src/main.stasis"),
            "global gfx_cmd_i32: i32[35120];\n\
global gfx_cmd_f32: f32[126084];\n\
global gfx_cmd_u8: u8[65536];\n\
function main(): void {}\n\
function tick(): i32 { return 0; }\n\
function render(): i32 { gfx_cmd_i32[0] = 1196967473; gfx_cmd_i32[1] = 99; return 0; }\n\
function on_code_swap(): void {}\n",
        )
        .expect("write source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("render-schema source compiles before invocation");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut i32_values = vec![0; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut f32_values = vec![0.0; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut u8_values = vec![0; ANDROID_RENDER_GFX_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v2(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            0,
            0,
            0,
            320,
            240,
            i32_values.as_mut_ptr(),
            i32_values.len(),
            f32_values.as_mut_ptr(),
            f32_values.len(),
            u8_values.as_mut_ptr(),
            u8_values.len(),
        );
        assert_eq!(status, -1);
        let error_ptr = stasis_android_bridge_last_frame_error();
        let error = unsafe { CStr::from_ptr(error_ptr) }
            .to_string_lossy()
            .into_owned();
        stasis_android_bridge_free_string(error_ptr);
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("render envelope");
        assert_eq!(envelope["stage"], "render_schema");
        assert_eq!(envelope["code"], "stasis.renderSchema");
        assert_eq!(envelope["context"]["symbol"], "render");
        assert_eq!(envelope["causes"][0], "render_schema phase");
        assert_eq!(
            envelope["causes"].as_array().unwrap().last().unwrap(),
            &envelope["detail"]
        );
        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn c_bridge_run_tick_frame_writes_packed_render_data() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_frame_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { tick_count: i32; }
global Render { command_count: i32; command0_kind: i32; command0_x: i32; command0_y: i32; command0_w: i32; command0_h: i32; command0_color: i32; }
function main(): void { GameState.tick_count = 4; }
function tick(): void { GameState.tick_count += 1; }
function render(): void { Render.command_count = 1; Render.command0_kind = 1; Render.command0_x = 9; Render.command0_y = 8; Render.command0_w = 7; Render.command0_h = 6; Render.command0_color = 5; }
",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut frame = [0i32; ANDROID_RENDER_FRAME_I32_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            72,
            144,
            1,
            360,
            640,
            frame.as_mut_ptr(),
            frame.len(),
        );
        assert_eq!(status, 0);
        assert_eq!(frame[0], 0);
        assert_eq!(frame[1], 1);
        assert_eq!(frame[2], 5);
        assert_eq!(frame[5], 1);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE], 1);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 1], 9);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 5], 5);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 6], 0);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 7], 0);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 8], 255);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 9], 0);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 10], 0);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 11], 0);
        assert_eq!(frame[ANDROID_RENDER_FRAME_HEADER_SIZE + 12], 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_bridge_run_tick_frame_v2_copies_only_production_active_spans() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_production_frame_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global host_i32: i32[768];
global host_f32: f32[64];
global host_req_window_w_px: i32;
global host_req_window_h_px: i32;
global gfx_cmd_i32: i32[35120];
global gfx_cmd_f32: f32[126084];
global gfx_cmd_u8: u8[65536];
function main(): void { host_req_window_w_px = 360; host_req_window_h_px = 720; }
function tick(): void {}
function render(): void {
  gfx_cmd_i32[0] = 1196967473;
  gfx_cmd_i32[1] = 6;
  gfx_cmd_i32[2] = 3;
  gfx_cmd_i32[3] = 1;
  gfx_cmd_i32[4] = 1;
  gfx_cmd_i32[7] = 1;
  gfx_cmd_i32[9] = 2;
  gfx_cmd_i32[22] = 3;
  gfx_cmd_f32[0] = 0.1;
  gfx_cmd_f32[4] = host_f32[0];
  gfx_cmd_f32[5] = host_f32[1];
  gfx_cmd_f32[6] = 30.0;
  gfx_cmd_f32[7] = 40.0;
  gfx_cmd_f32[8] = 1.0;
  gfx_cmd_i32[32] = 77;
  gfx_cmd_i32[33] = 11;
  gfx_cmd_i32[34] = 255;
  gfx_cmd_i32[12320] = 5;
  gfx_cmd_i32[12321] = 0;
  gfx_cmd_i32[12322] = 1;
  gfx_cmd_i32[18464] = 32768;
  gfx_cmd_i32[18465] = 16384;
  gfx_cmd_i32[18466] = 49152;
  gfx_cmd_f32[80004] = 10.25;
  gfx_cmd_f32[80005] = 20.5;
  gfx_cmd_f32[80006] = 30.75;
  gfx_cmd_f32[80007] = 40.125;
  gfx_cmd_f32[80008] = 0.0;
  gfx_cmd_f32[80009] = 0.0;
  gfx_cmd_f32[80010] = 1.0;
  gfx_cmd_f32[80011] = 1.0;
  gfx_cmd_f32[112772] = 12.0;
  gfx_cmd_u8[0] = 65;
  gfx_cmd_u8[1] = 0;
}
",
        )
        .expect("write production source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut frame_i32 = vec![0i32; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut frame_f32 = vec![0.0f32; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut frame_u8 = vec![0u8; ANDROID_RENDER_GFX_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v2(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            540,
            1200,
            1,
            1080,
            2400,
            frame_i32.as_mut_ptr(),
            frame_i32.len(),
            frame_f32.as_mut_ptr(),
            frame_f32.len(),
            frame_u8.as_mut_ptr(),
            frame_u8.len(),
        );
        assert_eq!(status, 0);
        // Current source frames copy into the canonical destination layout.
        assert_eq!(&frame_i32[..5], &[1196967473, 6, 3, 1, 1]);
        assert_eq!(&frame_i32[10..16], &[360, 720, 1080, 2400, 1080, 2400]);
        assert_eq!(&frame_i32[16..20], &[0, 0, 360, 720]);
        assert_eq!(&frame_i32[20..22], &[1, 1]);
        assert_eq!(frame_i32[22], 3);
        assert_eq!(frame_i32[32], 77);
        assert_eq!(frame_i32[33], 11);
        assert_eq!(&frame_i32[12320..12323], &[5, 0, 1]);
        assert_eq!(&frame_i32[18464..18467], &[32768, 16384, 49152]);
        assert_eq!(frame_f32[4], 180.0);
        assert_eq!(frame_f32[5], 360.0);
        assert_eq!(&frame_f32[80004..80008], &[10.25, 20.5, 30.75, 40.125]);
        assert_eq!(&frame_f32[80008..80012], &[0.0, 0.0, 1.0, 1.0]);
        assert_eq!(frame_f32[112772], 12.0);
        assert_eq!(&frame_u8[..2], &[65, 0]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_bridge_reports_resource_load_failure_for_runtime_ui() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_resource_error");
        fs::write(
            root.join("src/main.stasis"),
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;
global host_i32: i32[768];
global host_f32: f32[64];
global gfx_cmd_i32: i32[35120];
global gfx_cmd_f32: f32[126084];
global gfx_cmd_u8: u8[65536];
function main(): void {}
function tick(): void {}
function render(): void { gfx_load_sprite(\"assets/render_missing.svg\", 32, 32); }
",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut i32_values = vec![0; ANDROID_RENDER_GFX_I32_CAPACITY];
        let mut f32_values = vec![0.0; ANDROID_RENDER_GFX_F32_CAPACITY];
        let mut u8_values = vec![0; ANDROID_RENDER_GFX_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v2(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            0,
            0,
            0,
            360,
            640,
            i32_values.as_mut_ptr(),
            i32_values.len(),
            f32_values.as_mut_ptr(),
            f32_values.len(),
            u8_values.as_mut_ptr(),
            u8_values.len(),
        );
        assert_eq!(status, -1);
        let error_ptr = stasis_android_bridge_last_frame_error();
        let error = unsafe { CStr::from_ptr(error_ptr) }
            .to_string_lossy()
            .into_owned();
        stasis_android_bridge_free_string(error_ptr);
        assert!(
            error.contains("render resource error"),
            "unexpected error: {error}"
        );
        assert!(
            error.contains("render_missing.svg"),
            "unexpected error: {error}"
        );
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("render resource envelope");
        assert_eq!(envelope["stage"], "resource");
        assert_eq!(envelope["code"], "stasis.missingResource");
        assert_eq!(envelope["context"]["symbol"], "render");
        assert_eq!(envelope["context"]["resource"], "assets/render_missing.svg");
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn c_bridge_drains_tick_resource_error_before_render() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_tick_resource_error");
        fs::write(
            root.join("src/main.stasis"),
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;\n\
global host_i32: i32[768];\n\
global host_f32: f32[64];\n\
global gfx_cmd_i32: i32[35120];\n\
global gfx_cmd_f32: f32[126084];\n\
global gfx_cmd_u8: u8[65536];\n\
function main(): void {}\n\
function tick(): void { gfx_load_sprite(\"assets/tick_missing.svg\", 32, 32); }\n\
function render(): void { gfx_load_sprite(\"assets/render_missing.svg\", 32, 32); }\n",
        )
        .expect("write source");
        let error =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect_err("tick resource failure must stop before render");
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("tick resource envelope");
        assert_eq!(envelope["stage"], "resource");
        assert_eq!(envelope["code"], "stasis.missingResource");
        assert_eq!(envelope["context"]["symbol"], "tick");
        assert_eq!(envelope["context"]["resource"], "assets/tick_missing.svg");
        assert!(
            !error.contains("render_missing.svg"),
            "render must not mask tick error: {error}"
        );
        fs::remove_dir_all(root).ok();
        clear_runtime_session_for_test();
    }

    #[test]
    fn embedded_resource_errors_own_path_context_for_sprite_and_font_forms() {
        let root = temp_project("embedded_resource_error_context");
        fs::create_dir_all(root.join("assets")).expect("assets directory");
        fs::write(root.join("assets/undeclared.svg"), b"svg").expect("sprite fixture");
        install_embedded_resource_host(&root).expect("install resource host");
        embedded_load_sprite(b"assets/missing.svg", 32, 32);
        let missing = take_embedded_resource_error().expect_err("missing sprite error");
        assert_eq!(missing.resource.as_deref(), Some("assets/missing.svg"));
        embedded_load_sprite(b"assets/undeclared.svg", 32, 32);
        let undeclared = take_embedded_resource_error().expect_err("undeclared sprite error");
        assert_eq!(
            undeclared.resource.as_deref(),
            Some("assets/undeclared.svg")
        );
        embedded_load_font(b"assets/missing.ttf", 12);
        let missing_font = take_embedded_resource_error().expect_err("missing font error");
        assert_eq!(missing_font.resource.as_deref(), Some("assets/missing.ttf"));
        embedded_load_font(b"assets/undeclared.svg", 0);
        let invalid_font = take_embedded_resource_error().expect_err("invalid font error");
        assert_eq!(
            invalid_font.resource.as_deref(),
            Some("assets/undeclared.svg")
        );
        *embedded_resource_catalog().lock().unwrap() = None;
        stasis_dynload::set_embedded_graphics_host(None);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn it031_missing_resource_on_initialized_hot_reload_reports_real_hook_error() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("it031_resource_hot_reload");
        let baseline =
            "extern function gfx_load_sprite(path: string, max_w: i32, max_h: i32): i32;\n\
global host_i32: i32[768];\n\
global host_f32: f32[64];\n\
global gfx_cmd_i32: i32[35120];\n\
global gfx_cmd_f32: f32[126084];\n\
global gfx_cmd_u8: u8[65536];\n\
global GameState { tick_count: i32; }\n\
function main(): void { GameState.tick_count = 7; }\n\
function tick(): void { GameState.tick_count += 1; }\n\
function render(): void {}\n\
function on_code_swap(): void {}\n";
        fs::write(root.join("src/main.stasis"), baseline).expect("baseline source");
        let input = default_tick_input();
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), input)
            .expect("initialize healthy runtime");
        let baseline_state = inspect_android_runtime_state(&root).expect("baseline runtime state");
        let edited = baseline
            .replace(
                "function tick(): void { GameState.tick_count += 1; }",
                "function tick(): void { GameState.tick_count += 100; }",
            )
            .replace(
                "function on_code_swap(): void {}",
                "function on_code_swap(): void { gfx_load_sprite(\"assets/IT031_missing.svg\", 32, 32); }",
            );
        fs::write(root.join("src/main.stasis"), edited).expect("hot reload source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage hot reload candidate");
        let error = run_android_workshop_tick(&root, Path::new("src/main.stasis"), input)
            .expect_err("hook resource must fail after initialized activation");
        let envelope = error
            .split("diagnostic_envelope=")
            .nth(1)
            .map(percent_decode_for_test)
            .and_then(|value| serde_json::from_str::<serde_json::Value>(&value).ok())
            .expect("resource envelope");
        assert_eq!(envelope["stage"], "resource");
        assert_eq!(envelope["context"]["resource"], "assets/IT031_missing.svg");
        assert_eq!(envelope["causes"][0], "resource phase");
        let restored_state = inspect_android_runtime_state(&root).expect("restored runtime state");
        assert_eq!(restored_state["generation"], baseline_state["generation"]);
        assert_eq!(
            restored_state["source_fingerprint"],
            baseline_state["source_fingerprint"]
        );
        assert_eq!(
            restored_state["game_tick_count"],
            baseline_state["game_tick_count"]
        );
        run_android_workshop_tick(&root, Path::new("src/main.stasis"), input)
            .expect("prior runtime remains healthy after rejected hook");
        let post_failure_state =
            inspect_android_runtime_state(&root).expect("post-failure runtime state");
        assert_eq!(
            post_failure_state["game_tick_count"].as_i64(),
            baseline_state["game_tick_count"]
                .as_i64()
                .map(|value| value + 1)
        );
        clear_runtime_session_for_test();
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn embedded_resource_refresh_preserves_loaded_font_handles() {
        let _guard = bridge_runtime_test_guard();
        let root = temp_project("embedded_resource_refresh");
        install_embedded_resource_host(&root).expect("install embedded resource host");
        {
            let mut slot = embedded_resource_catalog()
                .lock()
                .expect("resource catalog lock");
            let catalog = slot.as_mut().expect("installed resource catalog");
            catalog.fonts.push(EmbeddedFont {
                handle: 1,
                path: root.join("assets/font.ttf"),
                size: 18,
            });
            catalog.text_runs.push(EmbeddedTextRun {
                handle: 1,
                font: 1,
                text: "refresh".to_string(),
                measured_width: 75.6,
                measured_height: 18.0,
            });
        }

        assert_eq!(embedded_measure_text_cached(1), 75.6);
        assert_eq!(embedded_measure_text_cached_height(1), 18.0);

        let refreshed = prepare_embedded_resource_catalog(&root, true)
            .expect("prepare refreshed embedded resource catalog");

        assert_eq!(refreshed.fonts.len(), 1);
        assert_eq!(refreshed.fonts[0].handle, 1);
        assert_eq!(refreshed.text_runs.len(), 1);
        assert_eq!(refreshed.text_runs[0].text, "refresh");
        assert_eq!(refreshed.text_runs[0].measured_height, 18.0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_bridge_run_tick_returns_jit_executed_message() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 4; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let ptr =
            stasis_android_bridge_run_tick(root_c.as_ptr(), entry_c.as_ptr(), 72, 144, 1, 360, 640);
        assert!(!ptr.is_null());
        let message = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("message utf8")
            .to_string();
        stasis_android_bridge_free_string(ptr);
        assert!(message.contains("RunTick"));
        assert!(message.contains("mode=JitExecuted"));
        assert!(message.contains("game_tick_count=5"));
        fs::remove_dir_all(&root).ok();
    }
    #[test]
    fn c_bridge_returns_compile_message() {
        let root = temp_project("ffi");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 3; }\n",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let ptr = stasis_android_bridge_compile_project(root_c.as_ptr(), entry_c.as_ptr());
        assert!(!ptr.is_null());
        let message = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("message utf8")
            .to_string();
        stasis_android_bridge_free_string(ptr);
        assert!(message.contains("CompileReady"));
        assert!(message.contains("functions="));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_reference_bridge_returns_compact_compiler_owned_results() {
        let root = temp_project("reference_ffi");
        fs::write(
            root.join("src/main.stasis"),
            "global GameState { score: i32; }\nfunction tick(): void { GameState.score += 1; }\nfunction current(): i32 { return GameState.score; }\n",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let symbol_c = CString::new("GameState.score").expect("symbol cstr");

        let result = ffi_json(stasis_android_bridge_find_references(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            symbol_c.as_ptr(),
            16,
        ));

        assert_eq!(result["schema_version"], 1);
        assert_eq!(result["references"].as_array().map(Vec::len), Some(2));
        assert!(result["references"]
            .as_array()
            .unwrap()
            .iter()
            .all(|reference| reference.get("source_hash").is_none()
                && reference.get("source").is_none()));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_semantic_bridge_returns_versioned_json_for_success_and_errors() {
        let root = temp_project("semantic_ffi");
        let original = "function main(): i32 { return 1; }\n";
        fs::write(root.join("src/main.stasis"), original).expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");

        let null_paths = ffi_json(stasis_android_bridge_source_items(
            std::ptr::null(),
            entry_c.as_ptr(),
        ));
        assert_eq!(null_paths["schema_version"], 1);
        assert_eq!(null_paths["status"], "error");
        assert!(null_paths["error"]
            .as_str()
            .unwrap_or_default()
            .contains("null project root"));

        let items = ffi_json(stasis_android_bridge_source_items(
            root_c.as_ptr(),
            entry_c.as_ptr(),
        ));
        assert_eq!(items["schema_version"], 1);
        assert!(items["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["name"] == "main"));

        let invalid_request = CString::new("not json").expect("invalid request cstr");
        let invalid = ffi_json(stasis_android_bridge_semantic_edit(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            invalid_request.as_ptr(),
            1,
            1,
            0,
        ));
        assert_eq!(invalid["schema_version"], 1);
        assert_eq!(invalid["status"], "error");
        assert!(invalid["error"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid semantic edit request"));

        let request = CString::new(
            serde_json::json!({
                "schema_version": 1,
                "edits": [{
                    "operation": "update",
                    "target": {
                        "kind": "function",
                        "file": "src/main.stasis",
                        "name": "main"
                    },
                    "new_source": "function main(): i32 { return 2; }"
                }]
            })
            .to_string(),
        )
        .expect("request cstr");
        let preview = ffi_json(stasis_android_bridge_semantic_edit(
            root_c.as_ptr(),
            entry_c.as_ptr(),
            request.as_ptr(),
            1,
            1,
            0,
        ));
        assert_eq!(preview["schema_version"], 1);
        assert_eq!(preview["status"], "preview");
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("preview source"),
            original
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn c_bridge_compile_failure_sanitizes_guest_nul_and_delimiter_detail() {
        let root = temp_project("ffi_compile_diagnostic_boundary");
        fs::write(
            root.join("src/main.stasis"),
            b"function @effects(bad|diagnostic_envelope=\0) on_code_swap(): void {}\n",
        )
        .expect("write malformed source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let ptr = stasis_android_bridge_compile_project(root_c.as_ptr(), entry_c.as_ptr());
        let message = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("compile diagnostic utf8")
            .to_string();
        stasis_android_bridge_free_string(ptr);
        assert!(!message.contains('\0'));
        assert!(!message.contains("bad|diagnostic_envelope="));
        assert!(message.contains("diagnostic_envelope="));
        assert!(message.contains("diagnostic_stage=parse"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_and_cli_share_rust_semantic_edit_contract() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("semantic_edit");
        fs::write(
            root.join("src/main.stasis"),
            "import \"old.stasis\";\nfunction main(): i32 { return tick(); }\nfunction tick(): i32 { return old_value(); }\n",
        )
        .expect("write main");
        fs::write(
            root.join("src/old.stasis"),
            "function old_value(): i32 { return 1; }\n",
        )
        .expect("write old");
        fs::write(
            root.join("src/new.stasis"),
            "function new_value(): i32 { return 9; }\n",
        )
        .expect("write new");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "tick".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: None,
                },
                new_source: Some(
                    "function tick(): i32 { import \"new.stasis\"; return new_value(); }"
                        .to_string(),
                ),
                expected_source_hash: None,
            }],
        };
        let preview = execute_android_workshop_semantic_edit(
            &root,
            Path::new("src/main.stasis"),
            &batch,
            true,
            true,
            false,
        )
        .expect("preview");
        assert_eq!(preview["status"], "preview");
        assert!(fs::read_to_string(root.join("src/main.stasis"))
            .expect("preview source")
            .contains("old_value"));

        let applied = execute_android_workshop_semantic_edit(
            &root,
            Path::new("src/main.stasis"),
            &batch,
            false,
            true,
            false,
        )
        .expect("apply");
        assert_eq!(applied["status"], "applied");
        let source = fs::read_to_string(root.join("src/main.stasis")).expect("applied source");
        assert!(source.starts_with("import \"new.stasis\";\n"));
        assert!(!source.contains("old.stasis"));
        assert!(source.contains("return new_value();"));
        assert!(root
            .join(applied["receipt"].as_str().expect("receipt"))
            .is_file());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn android_semantic_items_use_rust_owner_and_signature_identity() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("semantic_identity");
        fs::write(
            root.join("src/main.stasis"),
            "struct Player { value: i32; }\nstruct Enemy { value: i32; }\nfunction main(): i32 { return 0; }\nfunction tick(): void {}\nfunction adjust(self: Player): i32 { return 1; }\nfunction adjust(self: Enemy): i32 { return 2; }\n",
        )
        .expect("write overloads");
        let response = android_workshop_source_items(&root, Path::new("src/main.stasis"))
            .expect("source items");
        let items = response["items"].as_array().expect("items array");
        let tick = items
            .iter()
            .find(|item| item["kind"] == "function" && item["name"] == "tick")
            .expect("tick item");
        assert!(tick.get("owner").is_none());
        let player_adjust = items
            .iter()
            .find(|item| {
                item["kind"] == "function" && item["name"] == "adjust" && item["owner"] == "Player"
            })
            .expect("Player adjust item");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "adjust".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: Some("Player".to_string()),
                    signature: Some(
                        player_adjust["signature"]
                            .as_str()
                            .expect("signature")
                            .to_string(),
                    ),
                },
                new_source: Some("function adjust(self: Player): i32 { return 7; }".to_string()),
                expected_source_hash: Some(
                    player_adjust["source_hash"]
                        .as_str()
                        .expect("source hash")
                        .to_string(),
                ),
            }],
        };
        execute_android_workshop_semantic_edit(
            &root,
            Path::new("src/main.stasis"),
            &batch,
            false,
            true,
            false,
        )
        .expect("update one overload");
        let source = fs::read_to_string(root.join("src/main.stasis")).expect("updated source");
        assert!(source.contains("self: Player): i32 { return 7;"));
        assert!(source.contains("self: Enemy): i32 { return 2;"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn android_receipt_failure_restores_sources_and_live_runtime() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("semantic_receipt_rollback");
        let entry = Path::new("src/main.stasis");
        let original = "global State { value: i32; }\nfunction main(): void { State.value = 0; }\nfunction tick(): void { State.value += 1; }\n";
        fs::write(root.join(entry), original).expect("write source");
        compile_android_workshop_project(&root, entry).expect("initial compile");
        fs::create_dir_all(root.join("build")).expect("create build");
        fs::write(root.join("build/semantic-edits"), "block receipt directory")
            .expect("block receipt directory");
        let items = android_workshop_source_items(&root, entry).expect("source items");
        let tick = items["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["kind"] == "function" && item["name"] == "tick")
            .expect("tick item");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "tick".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: Some(tick["signature"].as_str().expect("signature").to_string()),
                },
                new_source: Some("function tick(): void { State.value += 10; }".to_string()),
                expected_source_hash: Some(
                    tick["source_hash"]
                        .as_str()
                        .expect("source hash")
                        .to_string(),
                ),
            }],
        };
        let error =
            execute_android_workshop_semantic_edit(&root, entry, &batch, false, true, false)
                .expect_err("receipt should fail");
        assert!(error.contains("receipt"));
        assert_eq!(
            fs::read_to_string(root.join(entry)).expect("restored source"),
            original
        );
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("restored tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "State.value").expect("restored value"),
            1
        );
        clear_runtime_session_for_test();
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn android_test_failure_restores_sources_and_live_runtime() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("semantic_test_rollback");
        let entry = Path::new("src/main.stasis");
        let original = "global State { value: i32; }\nfunction main(): void { State.value = 0; }\nfunction tick(): void { State.value += 1; }\n";
        fs::write(root.join(entry), original).expect("write source");
        fs::create_dir_all(root.join("tests")).expect("create tests");
        fs::write(
            root.join("tests/failing.test.stasis"),
            "test `reject edit`(): bool { return false; }\n",
        )
        .expect("write failing test");
        compile_android_workshop_project(&root, entry).expect("initial compile");
        let items = android_workshop_source_items(&root, entry).expect("source items");
        let tick = items["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|item| item["kind"] == "function" && item["name"] == "tick")
            .expect("tick item");
        let batch = WorkshopSemanticEditBatch {
            schema_version: 1,
            edits: vec![WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: WorkshopSymbolSelector {
                    symbol_id: None,
                    name: "tick".to_string(),
                    kind: Some(WorkshopSourceItemKind::Function),
                    file: Some("src/main.stasis".to_string()),
                    owner: None,
                    signature: Some(tick["signature"].as_str().expect("signature").to_string()),
                },
                new_source: Some("function tick(): void { State.value += 10; }".to_string()),
                expected_source_hash: Some(
                    tick["source_hash"]
                        .as_str()
                        .expect("source hash")
                        .to_string(),
                ),
            }],
        };
        let error = execute_android_workshop_semantic_edit(&root, entry, &batch, false, true, true)
            .expect_err("tests should reject edit");
        assert!(error.contains("validation failed"));
        assert_eq!(
            fs::read_to_string(root.join(entry)).expect("restored source"),
            original
        );
        run_android_workshop_tick(&root, entry, default_tick_input()).expect("restored tick");
        assert_eq!(
            get_android_workshop_i32_global(&root, entry, "State.value").expect("restored value"),
            1
        );
        clear_runtime_session_for_test();
        fs::remove_dir_all(root).ok();
    }
}

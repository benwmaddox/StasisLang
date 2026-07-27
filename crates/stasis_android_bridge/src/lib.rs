use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use stasis_assets::{AssetFormat, AssetHandle, AssetLimits, ResolvedAssetManifest};
use stasis_compiler::backend::jit::JitProcess;
#[cfg(test)]
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::backend::state_migration::{
    activate_candidate_transactionally, finalize_runtime_preview, plan_state_migration,
};
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
use stasis_compiler::frontend::workshop::{
    build_workshop_compile_plan, find_workshop_references, load_workshop_edit_workspace,
    load_workshop_project, plan_workshop_semantic_edits, render_workshop_artifacts,
    workshop_reachable_files, workshop_source_items, write_workshop_semantic_plan,
    write_workshop_semantic_receipt, WorkshopCompilePlan, WorkshopReload,
    WorkshopSemanticEditBatch, WorkshopSemanticEditPlan, WorkshopSourceFile,
};
#[cfg(test)]
use stasis_compiler::frontend::workshop::{
    WorkshopSemanticEdit, WorkshopSemanticEditOperation, WorkshopSourceItemKind,
    WorkshopSymbolSelector,
};
use stasis_compiler::IncrementalCompilerHost;

pub const ANDROID_RENDER_COMMAND_CAPACITY: usize = 8;
pub const ANDROID_RENDER_FRAME_HEADER_SIZE: usize = 6;
pub const ANDROID_RENDER_COMMAND_STRIDE: usize = 13;
pub const ANDROID_RENDER_FRAME_I32_CAPACITY: usize = ANDROID_RENDER_FRAME_HEADER_SIZE
    + ANDROID_RENDER_COMMAND_CAPACITY * ANDROID_RENDER_COMMAND_STRIDE;
pub const ANDROID_RENDER_V1_I32_CAPACITY: usize = stasis_dynload::STASIS_RENDER_I32_COUNT;
pub const ANDROID_RENDER_V1_F32_CAPACITY: usize = stasis_dynload::STASIS_RENDER_F32_COUNT;
pub const ANDROID_RENDER_V1_U8_CAPACITY: usize = stasis_dynload::STASIS_RENDER_U8_COUNT;

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
    jit: JitProcess,
    initialized: bool,
    pending_candidate: Option<JitProcess>,
    pending_resource_catalog: Option<EmbeddedResourceCatalog>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidBridgeCompileResult {
    pub status: i32,
    pub reload: WorkshopReload,
    pub manifest_path: PathBuf,
    pub runtime_state_path: PathBuf,
    pub function_artifact_count: usize,
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
    let (encoding, width, height) = match asset.entry.format {
        AssetFormat::Sprite {
            encoding,
            width,
            height,
        } => (format!("{encoding:?}").to_ascii_lowercase(), width, height),
        AssetFormat::Audio { .. } => {
            return Err(format!(
                "asset handle {} identifies audio, not a sprite",
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
        jit.upsert_file(path.to_string_lossy().replace('\\', "/"), rewritten);
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
    build_runtime_session(project_root, &reachable, source_fingerprint)?;
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
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();
    discard_pending_runtime_candidate(project_root);
    let files = load_workshop_project(project_root, entry_file)?;
    let changed_files = files
        .iter()
        .map(|file| project_root.join(&file.path))
        .collect::<Vec<_>>();

    let mut host = IncrementalCompilerHost::new();
    host.set_required_reachability_roots(&["tick", "render", "on_code_swap"]);
    let compile = match host.compile_changed_files(&changed_files) {
        Ok(compile) => compile,
        Err(error) => {
            return Err(host
                .last_source_diagnostic()
                .map(|diagnostic| format_compiler_source_diagnostic(project_root, diagnostic))
                .unwrap_or(error));
        }
    };
    let previous = read_previous_android_plan(project_root)?;
    let mut plan = build_workshop_compile_plan(&files, &compile, previous.as_ref())?;
    // The legacy artifact analyzer does not understand every production extern and can
    // report detector errors for programs the real JIT has compiled successfully. The
    // executable pipeline above is authoritative for Workshop; retain the artifact
    // manifest for tooling, but derive its success/reload state from the source set.
    let executable_project_hash = fingerprint_workshop_sources(&files) as i32;
    let previous_hashes = previous
        .as_ref()
        .map(|previous| (previous.project_hash, previous.layout_hash));
    plan.status = 0;
    plan.errors.clear();
    plan.project_hash = executable_project_hash;
    (plan.reload, plan.reason) = match previous_hashes {
        None => (
            WorkshopReload::InitialCompile,
            "Production JIT initialized the Workshop runtime.".to_string(),
        ),
        Some((project_hash, _)) if project_hash == executable_project_hash => (
            WorkshopReload::NoChange,
            "Production JIT source fingerprint is unchanged.".to_string(),
        ),
        Some((_, layout_hash)) if layout_hash == plan.layout_hash => (
            WorkshopReload::FastReload,
            "Production JIT accepted a layout-compatible source update.".to_string(),
        ),
        Some(_) => (
            WorkshopReload::ResetRequired,
            "Production JIT accepted a layout-changing source update.".to_string(),
        ),
    };
    let artifacts = render_workshop_artifacts(&plan);

    let manifest_path = project_root.join(&artifacts.manifest_path);
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed creating Android manifest directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&manifest_path, artifacts.manifest.as_bytes()).map_err(|error| {
        format!(
            "failed writing Android manifest {}: {error}",
            manifest_path.display()
        )
    })?;

    let runtime_state_path = project_root.join(&artifacts.runtime_state_path);
    if let Some(runtime_state) = artifacts.runtime_state.as_deref() {
        if let Some(parent) = runtime_state_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed creating Android runtime state directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&runtime_state_path, runtime_state.as_bytes()).map_err(|error| {
            format!(
                "failed writing Android runtime state {}: {error}",
                runtime_state_path.display()
            )
        })?;
    }

    for artifact in &artifacts.function_artifacts {
        let path = project_root.join(&artifact.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed creating Android function artifact directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&path, artifact.source.as_bytes()).map_err(|error| {
            format!(
                "failed writing Android function artifact {}: {error}",
                path.display()
            )
        })?;
    }

    warm_or_reload_runtime_session(project_root, &files, fingerprint_workshop_sources(&files))?;

    Ok(AndroidBridgeCompileResult {
        status: plan.status,
        reload: plan.reload,
        manifest_path,
        runtime_state_path,
        function_artifact_count: artifacts.function_artifacts.len(),
    })
}

pub fn run_android_workshop_tick(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
    input: AndroidBridgeTickInput,
) -> Result<AndroidBridgeRunTickResult, String> {
    run_android_workshop_tick_internal(project_root, entry_file, input, true)
}

const MAX_EMBEDDED_FONTS: usize = 64;
const MAX_EMBEDDED_TEXT_RUNS: usize = 4096;

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
}

struct EmbeddedResourceCatalog {
    project_root: PathBuf,
    assets: ResolvedAssetManifest,
    fonts: Vec<EmbeddedFont>,
    text_runs: Vec<EmbeddedTextRun>,
    error: Option<String>,
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
        release_sprite: |_| {},
        load_font: embedded_load_font,
        measure_text: embedded_measure_text,
        cache_text: embedded_cache_text,
        measure_text_cached: embedded_measure_text_cached,
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
            assets: Vec::new(),
        }
    };
    let (fonts, text_runs) = if preserve_loaded_resources {
        let slot = embedded_resource_catalog()
            .lock()
            .map_err(|_| "embedded resource catalog mutex poisoned")?;
        slot.as_ref()
            .filter(|catalog| catalog.project_root == project_root)
            .map(|catalog| (catalog.fonts.clone(), catalog.text_runs.clone()))
            .unwrap_or_else(|| {
                (
                    Vec::with_capacity(MAX_EMBEDDED_FONTS),
                    Vec::with_capacity(MAX_EMBEDDED_TEXT_RUNS),
                )
            })
    } else {
        (
            Vec::with_capacity(MAX_EMBEDDED_FONTS),
            Vec::with_capacity(MAX_EMBEDDED_TEXT_RUNS),
        )
    };
    Ok(EmbeddedResourceCatalog {
        project_root,
        assets,
        fonts,
        text_runs,
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
        catalog.error = Some(message);
    }
}

fn take_embedded_resource_error() -> Result<(), String> {
    let mut slot = embedded_resource_catalog()
        .lock()
        .map_err(|_| "embedded resource catalog mutex poisoned".to_string())?;
    let catalog = slot
        .as_mut()
        .ok_or_else(|| "embedded resource catalog is not initialized".to_string())?;
    match catalog.error.take() {
        Some(error) => Err(format!("render resource error: {error}")),
        None => Ok(()),
    }
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
        set_embedded_resource_error(
            catalog,
            format!("sprite path is invalid or missing: {display_path}"),
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
        set_embedded_resource_error(
            catalog,
            format!("sprite is not declared in the asset manifest: {display_path}"),
        );
    }
    handle
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
        set_embedded_resource_error(
            catalog,
            format!("font size must be positive: {display_path}"),
        );
        return 0;
    }
    let Some(absolute) = embedded_path(catalog, path) else {
        set_embedded_resource_error(
            catalog,
            format!("font path is invalid or missing: {display_path}"),
        );
        return 0;
    };
    if !absolute.is_file() {
        set_embedded_resource_error(catalog, format!("font file is missing: {display_path}"));
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
    read_legacy_render_commands: bool,
) -> Result<AndroidBridgeRunTickResult, String> {
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();

    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        let mut recompiled = false;
        let needs_lazy_build = session_slot
            .as_ref()
            .is_none_or(|session| session.project_root != project_root);
        if needs_lazy_build {
            let files = load_workshop_project(project_root, entry_file)?;
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
            execute_lifecycle_noarg(&session.jit, "main")?;
            take_embedded_resource_error()?;
            session.initialized = true;
            session.previous_input = None;
            session.display_generation = 0;
            session.density_generation = 0;
            true
        };
        let metrics = write_production_host_frame(session, input)?;
        if read_legacy_render_commands {
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
        execute_lifecycle_noarg(&session.jit, "tick")?;
        session.tick_count = session.tick_count.saturating_add(1);
        execute_optional_lifecycle_noarg(&session.jit, "render")?;
        take_embedded_resource_error()?;
        let write_runtime_state = should_write_jit_runtime_state(initialized, recompiled);
        let observed_game_tick_count = if read_legacy_render_commands || write_runtime_state {
            session.jit.read_i32_global_path("GameState.tick_count")
        } else {
            0
        };
        let (render_command_count, render_commands) = if read_legacy_render_commands {
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
    let (previous_x, previous_y) = previous.map_or((touch_x, touch_y), |value| {
        metrics.native_to_logical(value.touch_x, value.touch_y)
    });
    host_i32[0] = stasis_dynload::stasis_get_time_ms();
    host_i32[1] = metrics.logical_w;
    host_i32[2] = metrics.logical_h;
    host_i32[3] = 0;
    host_i32[4] = 0;
    host_i32[5] = metrics.logical_w;
    host_i32[6] = metrics.logical_h;
    host_i32[7] = 1;
    host_i32[8] = 0;
    host_i32[9] = 0;
    host_i32[10] = session.tick_count;
    host_i32[11] = i32::from(resized);
    host_i32[12] = metrics.native_w;
    host_i32[13] = metrics.native_h;
    host_i32[14] = 2;
    host_i32[15] = 0;
    host_i32[16] = 60;
    host_i32[17] = 1;
    host_i32[18] = 0;
    host_i32[19] = stasis_dynload::stasis_get_time_us();
    host_i32[20] = metrics.logical_w;
    host_i32[21] = metrics.logical_h;
    host_i32[22] = metrics.native_w;
    host_i32[23] = metrics.native_h;
    host_i32[24] = metrics.drawable_w;
    host_i32[25] = metrics.drawable_h;
    host_i32[26] = 0;
    host_i32[27] = 0;
    host_i32[28] = metrics.logical_w;
    host_i32[29] = metrics.logical_h;
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
    session.previous_input = Some(input);
    Ok(metrics)
}

fn write_android_display_metadata(out: &mut [i32]) -> Result<(), String> {
    if out.len() < 22 {
        return Err("render header is too small for display metadata".to_string());
    }
    let (metrics, display_generation, density_generation) = RUNTIME_SESSION.with(|slot| {
        let slot = slot.borrow();
        let session = slot
            .as_ref()
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
        Ok::<_, String>((
            session.display_metrics,
            session.display_generation,
            session.density_generation,
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
            let files = load_workshop_project(project_root, entry_file)?;
            let source_fingerprint = fingerprint_workshop_sources(&files);
            *session_slot = Some(build_runtime_session(
                project_root,
                &files,
                source_fingerprint,
            )?);
        }

        let session = session_slot
            .as_mut()
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
        activate_pending_runtime_candidate(session)?;
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
) -> Result<AndroidRuntimeSession, String> {
    install_embedded_resource_host(project_root)?;
    let mut jit = JitProcess::new();
    jit.set_local_runtime_helper_trampolines(true);
    configure_runtime_jit(&mut jit, project_root, files);
    if let Err(error) = jit.compile() {
        return Err(jit
            .last_source_diagnostic()
            .map(|diagnostic| format_compiler_source_diagnostic(project_root, diagnostic))
            .unwrap_or_else(|| format!("Android JIT compile failed: {error:?}")));
    }
    let display_metrics = AndroidDisplayMetrics::new(1, 1, 1, 1);
    Ok(AndroidRuntimeSession {
        project_root: project_root.to_path_buf(),
        source_fingerprint,
        jit,
        initialized: false,
        pending_candidate: None,
        pending_resource_catalog: None,
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
) -> Result<(), String> {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        match session_slot.as_mut() {
            Some(session) if session.project_root == project_root => {
                recompile_runtime_session(session, project_root, files, source_fingerprint)?;
            }
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
) -> Result<(), String> {
    session.pending_candidate = None;
    session.pending_resource_catalog = None;
    let mut candidate = session.jit.staged_candidate();
    configure_runtime_jit(&mut candidate, project_root, files);
    if let Err(error) = candidate.compile_staged() {
        return Err(candidate
            .last_source_diagnostic()
            .map(|diagnostic| format_compiler_source_diagnostic(project_root, diagnostic))
            .unwrap_or_else(|| format!("Android JIT hot reload failed: {error:?}")));
    }
    candidate.validate_on_code_swap_signature()?;
    let resource_catalog = prepare_embedded_resource_catalog(project_root, true)?;
    session.pending_candidate = Some(candidate);
    session.pending_resource_catalog = Some(resource_catalog);
    session.source_fingerprint = source_fingerprint;
    Ok(())
}

fn activate_pending_runtime_candidate(session: &mut AndroidRuntimeSession) -> Result<bool, String> {
    let Some(candidate) = session.pending_candidate.take() else {
        return Ok(false);
    };
    let pending_catalog = session.pending_resource_catalog.take();
    let mut preview = plan_state_migration(
        &session.jit.state_layout(),
        &candidate.state_layout(),
        Vec::new(),
        false,
        None,
    )?;
    finalize_runtime_preview(&candidate, &mut preview);
    let previous_catalog = if let Some(catalog) = pending_catalog {
        let mut slot = embedded_resource_catalog()
            .lock()
            .map_err(|_| "embedded resource catalog mutex poisoned")?;
        Some(slot.replace(catalog))
    } else {
        None
    };
    let run_hook = session.initialized && candidate.has_on_code_swap();
    let activation = activate_candidate_transactionally(
        Some(&session.jit),
        &candidate,
        &preview,
        run_hook,
        || {
            if run_hook {
                candidate.execute_optional_on_code_swap()
            } else {
                Ok(())
            }
        },
        Result::is_ok,
    );
    let activation_error = match activation {
        Ok(Ok(())) => None,
        Ok(Err(error)) | Err(error) => Some(error),
    };
    if let Some(error) = activation_error {
        if let Some(previous) = previous_catalog {
            *embedded_resource_catalog()
                .lock()
                .map_err(|_| "embedded resource catalog mutex poisoned")? = previous;
        }
        return Err(error);
    }
    session.jit = candidate;
    Ok(true)
}

fn discard_pending_runtime_candidate(project_root: &Path) {
    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        if let Some(session) = session_slot
            .as_mut()
            .filter(|session| session.project_root == project_root)
        {
            session.pending_candidate = None;
            session.pending_resource_catalog = None;
        }
    });
}

fn configure_runtime_jit(jit: &mut JitProcess, project_root: &Path, files: &[WorkshopSourceFile]) {
    jit.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    for file in files {
        let disk_path = project_root.join(&file.path);
        let compiler_path = disk_path
            .canonicalize()
            .unwrap_or(disk_path)
            .to_string_lossy()
            .to_string();
        jit.upsert_file(compiler_path, file.source.clone());
    }
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
fn read_previous_android_plan(project_root: &Path) -> Result<Option<WorkshopCompilePlan>, String> {
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
    let Some(project_hash) = parse_manifest_hex_i32(&manifest, "project_hash=") else {
        return Ok(None);
    };
    let Some(layout_hash) = parse_manifest_hex_i32(&manifest, "layout_hash=") else {
        return Ok(None);
    };
    Ok(Some(WorkshopCompilePlan {
        status: 0,
        reload: WorkshopReload::NoChange,
        reason: "Loaded from previous Android manifest.".to_string(),
        project_hash,
        layout_hash,
        entrypoints: Vec::new(),
        functions: Vec::new(),
        errors: Vec::new(),
    }))
}

fn parse_manifest_hex_i32(manifest: &str, key: &str) -> Option<i32> {
    let line = manifest.lines().find(|line| line.starts_with(key))?;
    let value = line[key.len()..].trim();
    u32::from_str_radix(value, 16)
        .ok()
        .map(|value| value as i32)
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
            "CompilePlanned: reload={:?} status={} functions={} manifest={}",
            result.reload,
            result.status,
            result.function_artifact_count,
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
    CString::new(message)
        .unwrap_or_else(|_| CString::new("CompileError: invalid bridge message").unwrap())
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
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let source = fs::read_to_string(path).unwrap_or_default();
    let (line, column) = source_line_column(&source, diagnostic.start);
    let (end_line, end_column) = source_line_column(&source, diagnostic.end);
    let file = path
        .strip_prefix(project_root)
        .or_else(|_| path.strip_prefix(&canonical_root))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    format!(
        "{}: {}|diagnostic_file={}|diagnostic_line={}|diagnostic_column={}|diagnostic_end_line={}|diagnostic_end_column={}|diagnostic_symbol={}|diagnostic_message={}",
        diagnostic.path,
        diagnostic.message,
        percent_encode(&file),
        line,
        column,
        end_line,
        end_column,
        percent_encode(&diagnostic.symbol),
        percent_encode(&diagnostic.message),
    )
}

fn diagnostic_offset(source: &str, error: &str) -> usize {
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
pub extern "C" fn stasis_android_bridge_run_tick_frame_v1(
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
        || out_i32_len < ANDROID_RENDER_V1_I32_CAPACITY
        || out_f32_len < ANDROID_RENDER_V1_F32_CAPACITY
        || out_u8_len < ANDROID_RENDER_V1_U8_CAPACITY
    {
        return -1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        if project_root.is_null() || entry_file.is_null() {
            return Err("null project root or entry file".to_string());
        }
        let project_root = CStr::from_ptr(project_root)
            .to_str()
            .map_err(|error| format!("project root was not UTF-8: {error}"))?;
        let entry_file = CStr::from_ptr(entry_file)
            .to_str()
            .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
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
        stasis_dynload::copy_jit_render_v1_active(i32_values, f32_values, u8_values)?;
        write_android_display_metadata(i32_values)
    }));
    match result {
        Ok(Ok(())) => {
            LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = None);
            0
        }
        Ok(Err(error)) => {
            LAST_FRAME_ERROR.with(|slot| *slot.borrow_mut() = Some(error));
            unsafe {
                *out_i32 = -1;
            }
            -1
        }
        Err(_) => {
            LAST_FRAME_ERROR.with(|slot| {
                *slot.borrow_mut() = Some("panic while running Android preview frame".to_string());
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
    CString::new(message)
        .unwrap_or_else(|_| CString::new("native preview frame failed").unwrap())
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
    use super::*;

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
            format!(r#"{{"schema":"stasis-assets","version":1,"assets":[{{"id":"hero","path":"assets/hero.png","content_sha256":"{hash}","format":{{"kind":"sprite","encoding":"png","width":4,"height":6}},"dependencies":[]}}]}}"#),
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

    fn temp_project(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_android_bridge_{name}_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("create project");
        root
    }

    #[test]
    fn bridge_compiles_project_and_writes_compiler_rendered_artifacts() {
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
        assert!(result.function_artifact_count >= 2);

        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read manifest");
        assert!(manifest.contains("status=CompilePlanned"));
        assert!(manifest.contains("entrypoint=main"));
        assert!(manifest.contains("entrypoint=tick"));
        assert!(manifest.contains("signature=tick(): i32"));

        let state =
            fs::read_to_string(root.join("build/runtime_state.txt")).expect("read runtime state");
        assert!(state.contains("status=RuntimeStateReady"));
        assert!(state.contains("tick_count=0"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_uses_successful_jit_result_when_legacy_scan_misreads_from_field() {
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
    fn artifact_write_failure_does_not_stage_runtime_candidate() {
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

        fs::write(
            &source,
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 100; }\n",
        )
        .expect("write changed source");
        fs::remove_dir_all(root.join("build/functions")).expect("remove artifact directory");
        fs::write(root.join("build/functions"), b"blocks artifact directory")
            .expect("block artifact directory");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("artifact write must fail");
        assert!(error.contains("function artifact directory"), "{error}");
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

        fs::remove_file(root.join("build/functions")).ok();
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
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 1; }\n",
        )
        .expect("write first source");

        let first =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect("first real tick");
        assert_eq!(first.observed_game_tick_count, 11);

        fs::write(
            &source,
            "extern function reject_code_swap(): void;\nglobal GameState { tick_count: i32; added: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 2; }\nfunction on_code_swap(): void { GameState.tick_count = 99; reject_code_swap(); return; }\n",
        )
        .expect("write rejecting hot reload source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("stage rejecting hot reload source");

        let error =
            run_android_workshop_tick(&root, Path::new("src/main.stasis"), default_tick_input())
                .expect_err("hook rejection should abort hot reload");
        assert!(error.contains("hook requested rejection"));

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
            jit: active,
            initialized: true,
            pending_candidate: Some(candidate),
            pending_resource_catalog: None,
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
    fn android_bundled_touch_pong_sample_compile_plan_is_runnable() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile bundled pong sample");
        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read bundled sample manifest");

        assert_eq!(result.status, 0, "{manifest}");
        assert!(result.function_artifact_count >= 5, "{manifest}");
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
    #[ignore = "host AI prompt regression target; run after AI edits the workshop sample"]
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
        assert_eq!(result["passed"], 1, "{result}");
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
        assert!(error.contains("|diagnostic_file=src/systems/broken.stasis"));
        assert!(error.contains("|diagnostic_line=3"));
        assert!(error.contains("|diagnostic_column=17"));
        assert!(error.contains("|diagnostic_symbol=broken"));
        assert!(error.contains("|diagnostic_message="));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn android_backend_failure_reports_imported_function_span() {
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
        .expect("write imported backend failure");

        let error = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect_err("compile should fail");
        assert!(
            error.contains("unknown%20call%20target%20%27missing_target%27"),
            "unexpected error: {error}"
        );
        assert!(error.contains("|diagnostic_file=src/systems/broken.stasis"));
        assert!(error.contains("|diagnostic_line=3"));
        assert!(error.contains("|diagnostic_symbol=on_code_swap"));
        fs::remove_dir_all(root).ok();
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
    fn c_bridge_run_tick_frame_v1_copies_only_production_active_spans() {
        let _guard = bridge_runtime_test_guard();
        clear_runtime_session_for_test();
        let root = temp_project("ffi_production_frame_tick");
        fs::write(
            root.join("src/main.stasis"),
            "global host_i32: i32[768];
global host_f32: f32[64];
global host_req_window_w_px: i32;
global host_req_window_h_px: i32;
global gfx_cmd_i32: i32[34848];
global gfx_cmd_f32: f32[92292];
global gfx_cmd_u8: u8[65536];
function main(): void { host_req_window_w_px = 360; host_req_window_h_px = 720; }
function tick(): void {}
function render(): void {
  gfx_cmd_i32[0] = 1196967473;
  gfx_cmd_i32[1] = 1;
  gfx_cmd_i32[2] = 3;
  gfx_cmd_i32[3] = 1;
  gfx_cmd_i32[4] = 1;
  gfx_cmd_i32[7] = 1;
  gfx_cmd_i32[9] = 2;
  gfx_cmd_f32[0] = 0.1;
  gfx_cmd_f32[4] = host_f32[0];
  gfx_cmd_f32[5] = host_f32[1];
  gfx_cmd_f32[6] = 30.0;
  gfx_cmd_f32[7] = 40.0;
  gfx_cmd_f32[8] = 1.0;
  gfx_cmd_i32[32] = 77;
  gfx_cmd_i32[33] = 11;
  gfx_cmd_i32[28704] = 5;
  gfx_cmd_i32[28705] = 0;
  gfx_cmd_i32[28706] = 1;
  gfx_cmd_f32[80004] = 12.0;
  gfx_cmd_u8[0] = 65;
  gfx_cmd_u8[1] = 0;
}
",
        )
        .expect("write production source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut frame_i32 = vec![0i32; ANDROID_RENDER_V1_I32_CAPACITY];
        let mut frame_f32 = vec![0.0f32; ANDROID_RENDER_V1_F32_CAPACITY];
        let mut frame_u8 = vec![0u8; ANDROID_RENDER_V1_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v1(
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
        assert_eq!(&frame_i32[..5], &[1196967473, 1, 3, 1, 1]);
        assert_eq!(&frame_i32[10..16], &[360, 720, 1080, 2400, 1080, 2400]);
        assert_eq!(&frame_i32[16..20], &[0, 0, 360, 720]);
        assert_eq!(&frame_i32[20..22], &[1, 1]);
        assert_eq!(frame_i32[32], 77);
        assert_eq!(frame_i32[33], 11);
        assert_eq!(&frame_i32[28704..28707], &[5, 0, 1]);
        assert_eq!(frame_f32[4], 180.0);
        assert_eq!(frame_f32[5], 360.0);
        assert_eq!(frame_f32[80004], 12.0);
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
global gfx_cmd_i32: i32[34848];
global gfx_cmd_f32: f32[92292];
global gfx_cmd_u8: u8[65536];
function main(): void { gfx_load_sprite(\"../assets/missing.svg\", 32, 32); }
function tick(): void {}
function render(): void {}
",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let mut i32_values = vec![0; ANDROID_RENDER_V1_I32_CAPACITY];
        let mut f32_values = vec![0.0; ANDROID_RENDER_V1_F32_CAPACITY];
        let mut u8_values = vec![0; ANDROID_RENDER_V1_U8_CAPACITY];
        let status = stasis_android_bridge_run_tick_frame_v1(
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
        assert!(error.contains("missing.svg"), "unexpected error: {error}");
        fs::remove_dir_all(&root).ok();
        clear_runtime_session_for_test();
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
            });
        }

        let refreshed = prepare_embedded_resource_catalog(&root, true)
            .expect("prepare refreshed embedded resource catalog");

        assert_eq!(refreshed.fonts.len(), 1);
        assert_eq!(refreshed.fonts[0].handle, 1);
        assert_eq!(refreshed.text_runs.len(), 1);
        assert_eq!(refreshed.text_runs[0].text, "refresh");
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
        assert!(message.contains("CompilePlanned"));
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

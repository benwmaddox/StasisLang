use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use stasis_assets::{AssetFormat, AssetHandle, AssetLimits, ResolvedAssetManifest};
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::frontend::parser::rewrite_top_level_test_declarations;
use stasis_compiler::frontend::workshop::{
    build_workshop_compile_plan, load_workshop_edit_workspace, load_workshop_project,
    plan_workshop_semantic_edits, render_workshop_artifacts, workshop_reachable_files,
    workshop_source_items, write_workshop_semantic_plan, write_workshop_semantic_receipt,
    WorkshopCompilePlan, WorkshopReload, WorkshopSemanticEditBatch, WorkshopSemanticEditPlan,
    WorkshopSourceFile,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidBridgeTickInput {
    pub touch_x: i32,
    pub touch_y: i32,
    pub touch_active: i32,
    pub screen_w: i32,
    pub screen_h: i32,
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
    pending_code_swap: bool,
    tick_count: i32,
}

thread_local! {
    static RUNTIME_SESSION: RefCell<Option<AndroidRuntimeSession>> = const { RefCell::new(None) };
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
    let plan = build_workshop_compile_plan(&files, &compile, previous.as_ref())?;
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
        let swapped_code = session.pending_code_swap;
        if swapped_code {
            recompiled = true;
        }
        session.pending_code_swap = false;
        let initialized = if session.initialized {
            false
        } else {
            execute_lifecycle_noarg(&session.jit, "main")?;
            session.initialized = true;
            true
        };
        if swapped_code && !initialized {
            execute_optional_lifecycle_noarg(&session.jit, "on_code_swap")?;
        }
        session
            .jit
            .write_i32_global_path("Input.touch_x", input.touch_x);
        session
            .jit
            .write_i32_global_path("Input.touch_y", input.touch_y);
        session
            .jit
            .write_i32_global_path("Input.touch_active", input.touch_active);
        session
            .jit
            .write_i32_global_path("Input.screen_w", input.screen_w);
        session
            .jit
            .write_i32_global_path("Input.screen_h", input.screen_h);
        execute_lifecycle_noarg(&session.jit, "tick")?;
        session.tick_count = session.tick_count.saturating_add(1);
        execute_optional_lifecycle_noarg(&session.jit, "render")?;
        let observed_game_tick_count = session.jit.read_i32_global_path("GameState.tick_count");
        let render_command_count = session.jit.read_i32_global_path("Render.command_count");
        let render_commands = read_render_commands(&session.jit);
        if should_write_jit_runtime_state(session.tick_count, initialized, recompiled) {
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
    let mut jit = JitProcess::new();
    jit.set_local_runtime_helper_trampolines(true);
    configure_runtime_jit(&mut jit, project_root, files);
    if let Err(error) = jit.compile() {
        return Err(jit
            .last_source_diagnostic()
            .map(|diagnostic| format_compiler_source_diagnostic(project_root, diagnostic))
            .unwrap_or_else(|| format!("Android JIT compile failed: {error:?}")));
    }
    Ok(AndroidRuntimeSession {
        project_root: project_root.to_path_buf(),
        source_fingerprint,
        jit,
        initialized: false,
        pending_code_swap: false,
        tick_count: 0,
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
                session.pending_code_swap = session.initialized;
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
    configure_runtime_jit(&mut session.jit, project_root, files);
    if let Err(error) = session.jit.compile() {
        return Err(session
            .jit
            .last_source_diagnostic()
            .map(|diagnostic| format_compiler_source_diagnostic(project_root, diagnostic))
            .unwrap_or_else(|| format!("Android JIT hot reload failed: {error:?}")));
    }
    session.source_fingerprint = source_fingerprint;
    Ok(())
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

fn should_write_jit_runtime_state(tick_count: i32, initialized: bool, recompiled: bool) -> bool {
    initialized || recompiled || tick_count % 60 == 0
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
    let result = unsafe { compile_project_from_c(project_root, entry_file) };
    let message = match result {
        Ok(result) => format!(
            "CompilePlanned: reload={:?} status={} functions={} manifest={}",
            result.reload,
            result.status,
            result.function_artifact_count,
            result.manifest_path.display()
        ),
        Err(error) => format!("CompileError: {error}"),
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
        assert!(should_write_jit_runtime_state(1, true, false));
        assert!(should_write_jit_runtime_state(7, false, true));
        assert!(should_write_jit_runtime_state(60, false, false));
        assert!(!should_write_jit_runtime_state(2, false, false));
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
            "global GameState { tick_count: i32; }\nfunction main(): void { GameState.tick_count = 10; }\nfunction tick(): void { GameState.tick_count += 2; }\nfunction on_code_swap(): void { GameState.tick_count += 100; }\n",
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mobile/android/app/src/main/assets/workshop_sample")
            .canonicalize()
            .expect("bundled sample root");
        let result = run_android_workshop_stasis_tests(&root).expect("run bundled Stasis tests");
        assert_eq!(result["passed"], 1);
        assert_eq!(result["failed"], 0);
        assert_eq!(result["all_passed"], true);
        assert_eq!(
            result["results"][0]["file"],
            "tests/enemy_paddle_speed_schedule.test.stasis"
        );
        assert_eq!(result["results"][0]["line"], 3);
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

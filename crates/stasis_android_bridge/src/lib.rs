use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::frontend::workshop::{
    build_android_workshop_compile_plan, load_workshop_project, render_android_workshop_artifacts,
    AndroidWorkshopCompilePlan, AndroidWorkshopReload, WorkshopSourceFile,
};
use stasis_compiler::IncrementalCompilerHost;

pub const ANDROID_RENDER_COMMAND_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AndroidBridgeTickInput {
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
    tick_count: i32,
}

thread_local! {
    static RUNTIME_SESSION: RefCell<Option<AndroidRuntimeSession>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidBridgeCompileResult {
    pub status: i32,
    pub reload: AndroidWorkshopReload,
    pub manifest_path: PathBuf,
    pub runtime_state_path: PathBuf,
    pub function_artifact_count: usize,
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
    let compile = host.compile_changed_files(&changed_files)?;
    let previous = read_previous_android_plan(project_root)?;
    let plan = build_android_workshop_compile_plan(&files, &compile, previous.as_ref())?;
    let artifacts = render_android_workshop_artifacts(&plan);

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
    let files = load_workshop_project(project_root, entry_file)?;
    let source_fingerprint = fingerprint_workshop_sources(&files);

    RUNTIME_SESSION.with(|session_cell| {
        let mut session_slot = session_cell.borrow_mut();
        let mut recompiled = false;
        let mut swapped_code = false;
        match session_slot.as_mut() {
            Some(session) if session.project_root == project_root => {
                if session.source_fingerprint != source_fingerprint {
                    recompile_runtime_session(session, project_root, &files, source_fingerprint)?;
                    recompiled = true;
                    swapped_code = session.initialized;
                }
            }
            _ => {
                *session_slot = Some(build_runtime_session(
                    project_root,
                    &files,
                    source_fingerprint,
                )?);
                recompiled = true;
            }
        }

        let session = session_slot
            .as_mut()
            .ok_or_else(|| "Android runtime session was not initialized".to_string())?;
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
        write_jit_runtime_state(
            project_root,
            session.tick_count,
            observed_game_tick_count,
            render_command_count,
            &render_commands,
        )?;

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

fn build_runtime_session(
    project_root: &Path,
    files: &[WorkshopSourceFile],
    source_fingerprint: u64,
) -> Result<AndroidRuntimeSession, String> {
    let mut jit = JitProcess::new();
    configure_runtime_jit(&mut jit, project_root, files);
    jit.compile()
        .map_err(|error| format!("Android JIT compile failed: {error:?}"))?;
    Ok(AndroidRuntimeSession {
        project_root: project_root.to_path_buf(),
        source_fingerprint,
        jit,
        initialized: false,
        tick_count: 0,
    })
}

fn recompile_runtime_session(
    session: &mut AndroidRuntimeSession,
    project_root: &Path,
    files: &[WorkshopSourceFile],
    source_fingerprint: u64,
) -> Result<(), String> {
    configure_runtime_jit(&mut session.jit, project_root, files);
    session
        .jit
        .compile()
        .map_err(|error| format!("Android JIT hot reload failed: {error:?}"))?;
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
        jit.upsert_file(
            disk_path.to_string_lossy().replace('\\', "/"),
            file.source.clone(),
        );
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
    for (index, command) in commands.iter_mut().enumerate() {
        command.kind = jit.read_i32_global_path(&format!("Render.command{index}_kind"));
        command.x = jit.read_i32_global_path(&format!("Render.command{index}_x"));
        command.y = jit.read_i32_global_path(&format!("Render.command{index}_y"));
        command.w = jit.read_i32_global_path(&format!("Render.command{index}_w"));
        command.h = jit.read_i32_global_path(&format!("Render.command{index}_h"));
        command.color = jit.read_i32_global_path(&format!("Render.command{index}_color"));
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
            "render{index}_kind={}\nrender{index}_x={}\nrender{index}_y={}\nrender{index}_w={}\nrender{index}_h={}\nrender{index}_color={}\n",
            command.kind, command.x, command.y, command.w, command.h, command.color
        ));
    }
    lines
}

fn render_command_message_fields(
    render_command_count: i32,
    render_commands: &[AndroidBridgeRenderCommand; ANDROID_RENDER_COMMAND_CAPACITY],
) -> String {
    let count = render_command_count.clamp(0, ANDROID_RENDER_COMMAND_CAPACITY as i32) as usize;
    let mut fields = format!("render_command_count={count}");
    for (index, command) in render_commands.iter().enumerate().take(count) {
        fields.push_str(&format!(
            " render{index}_kind={} render{index}_x={} render{index}_y={} render{index}_w={} render{index}_h={} render{index}_color={}",
            command.kind, command.x, command.y, command.w, command.h, command.color
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
) -> Result<Option<AndroidWorkshopCompilePlan>, String> {
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
    Ok(Some(AndroidWorkshopCompilePlan {
        status: 0,
        reload: AndroidWorkshopReload::NoChange,
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

#[no_mangle]
pub extern "C" fn stasis_android_bridge_run_tick(
    project_root: *const c_char,
    entry_file: *const c_char,
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

    fn default_tick_input() -> AndroidBridgeTickInput {
        AndroidBridgeTickInput {
            touch_y: 120,
            touch_active: 1,
            screen_w: 360,
            screen_h: 640,
        }
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
        assert_eq!(result.reload, AndroidWorkshopReload::InitialCompile);
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
        assert_eq!(result.reload, AndroidWorkshopReload::FastReload);
        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read preserved runtime state");
        assert!(state.contains("tick_count=41"));
        fs::remove_dir_all(&root).ok();
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
        assert!(state.contains("tick_count=2"));
        assert!(state.contains("game_tick_count=12"));
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
            "global Input { touch_y: i32; touch_active: i32; screen_w: i32; screen_h: i32; }\nglobal GameState { tick_count: i32; paddle_y: i32; }\nglobal Render { command_count: i32; command0_kind: i32; command0_x: i32; command0_y: i32; command0_w: i32; command0_h: i32; command0_color: i32; }\nfunction main(): void { GameState.paddle_y = 40; }\nfunction tick(): void { GameState.tick_count += 1; if (Input.touch_active != 0) { GameState.paddle_y = Input.touch_y; } }\nfunction render(): void { Render.command_count = 1; Render.command0_kind = 1; Render.command0_x = 12; Render.command0_y = GameState.paddle_y; Render.command0_w = 8; Render.command0_h = 64; Render.command0_color = 65535; }\n",
        )
        .expect("write source");

        let result = run_android_workshop_tick(
            &root,
            Path::new("src/main.stasis"),
            AndroidBridgeTickInput {
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
        assert_eq!(result.render_commands[0].x, 12);
        assert_eq!(result.render_commands[0].y, 222);
        assert_eq!(result.render_commands[0].w, 8);
        assert_eq!(result.render_commands[0].h, 64);
        assert_eq!(result.render_commands[0].color, 65535);

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
        assert_eq!(result.render_commands[3].kind, 1);
        assert!(result.observed_game_tick_count >= 1);
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
            stasis_android_bridge_run_tick(root_c.as_ptr(), entry_c.as_ptr(), 144, 1, 360, 640);
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
}

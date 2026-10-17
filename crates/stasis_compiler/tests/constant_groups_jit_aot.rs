use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess, wasm::WasmProcess};
use stasis_jit::{AotLinkConfig, AotTarget};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAIN_SOURCE: &str = r#"
import "display.stasis";
import "controls.stasis";

global keys: bool[controls.INPUT.KEY_CAPACITY];

function string_length(value: string): i32 {
    return value.length;
}

function main(): i32 {
    keys[controls.SCANCODE.W] = true;
    let half_width: f32 = display.SCREEN.HALF_WIDTH;
    if (keys[controls.SCANCODE.W]
        && half_width == 320.0
        && string_length(display.ASSETS.FONT_PATH) == 16
        && controls.VALUES.ENABLED
        && controls.VALUES.BYTE == 255
        && controls.VALUES.SHORT == 65535
        && controls.VALUES.WORD == 4294967295
        && controls.VALUES.PRECISE > 1.000000000000005
        && controls.VALUES.MODE == controls.Mode.Ready
        && controls.SCREEN.WIDTH == 640) {
        return 0;
    }
    return 1;
}
"#;

const DISPLAY_SOURCE: &str = r#"
const SCREEN {
    WIDTH: f32 = 640.0;
    HALF_WIDTH: f32 = SCREEN.WIDTH / 2.0;
}

const ASSETS {
    FONT_PATH: string = "assets/group.ttf";
}
"#;

const CONTROLS_SOURCE: &str = r#"
enum Mode { Ready = 2, }

const SCANCODE {
    W: i32 = 26;
}

const INPUT {
    KEY_CAPACITY: i32 = 512;
}

const VALUES {
    ENABLED: bool = true;
    BYTE: u8 = 255;
    SHORT: u16 = 65535;
    WORD: u32 = 4294967295;
    PRECISE: f64 = 1.00000000000001;
    MODE: Mode = Mode.Ready;
}

const SCREEN {
    WIDTH: i32 = 640;
}
"#;

const MIGRATED_STDLIB_MAIN_SOURCE: &str = r#"
import "../../src/stdlib/graphics.stasis";
import "../../src/stdlib/external_url.stasis";
import "../../src/stdlib/platform_services.stasis";
import "../../src/stdlib/ui_single_pass.stasis";

global response_fields: i32[PLATFORM_SERVICE_RESPONSE_FIELD.COUNT];
global undersized_fields: i32[PLATFORM_SERVICE_RESPONSE_FIELD.STATUS];
global response_text: utf8[8];
global empty_key: ascii[16];

function main(): i32 {
    if (animation_frame_at(10, 3, 2, ANIMATION_PLAYBACK_MODE.PING_PONG, 4) != 12) { return 1; }
    if (!animation_finished(3, 2, ANIMATION_PLAYBACK_MODE.ONCE, 6)) { return 2; }
    if (animation_finished(3, 2, ANIMATION_PLAYBACK_MODE.LOOP, 6)) { return 3; }

    if (open_external_url("invalid") != EXTERNAL_URL_RESULT.INVALID) { return 4; }
    if (platform_service_submit(0, 1, 1, empty_key) != PLATFORM_SERVICE_SUBMIT_RESULT.INVALID) { return 5; }
    if (platform_service_poll(undersized_fields, response_text) != PLATFORM_SERVICE_SUBMIT_RESULT.INVALID) { return 6; }

    response_fields[PLATFORM_SERVICE_RESPONSE_FIELD.STATUS] = PLATFORM_SERVICE_RESPONSE_STATUS.UNSUPPORTED;
    if (response_fields[3] != PLATFORM_SERVICE_RESPONSE_STATUS.UNSUPPORTED) { return 7; }

    ui_layout_begin(0.0, 0.0, 100.0, 100.0);
    ui_tap(1, 0, 0.0, 0.0, 10.0, 10.0);
    ui_tap(1, 0, 0.0, 0.0, 10.0, 10.0);
    if (ui_status() != UI_STATUS_FLAG.BAD_ARGUMENT) { return 8; }
    return 0;
}
"#;

fn configure_jit(process: &mut JitProcess) {
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file("main.stasis", MAIN_SOURCE);
    process.upsert_file("display.stasis", DISPLAY_SOURCE);
    process.upsert_file("controls.stasis", CONTROLS_SOURCE);
}

fn configure_aot(process: &mut AotProcess) {
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file("main.stasis", MAIN_SOURCE);
    process.upsert_file("display.stasis", DISPLAY_SOURCE);
    process.upsert_file("controls.stasis", CONTROLS_SOURCE);
}

fn configure_wasm(process: &mut WasmProcess) {
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file("main.stasis", MAIN_SOURCE);
    process.upsert_file("display.stasis", DISPLAY_SOURCE);
    process.upsert_file("controls.stasis", CONTROLS_SOURCE);
}

fn configure_migrated_jit(process: &mut JitProcess) {
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(
        "tests/stasis/constant_groups_migration_probe.test.stasis",
        MIGRATED_STDLIB_MAIN_SOURCE,
    );
}

fn configure_migrated_aot(process: &mut AotProcess) {
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set AOT project root");
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(
        "tests/stasis/constant_groups_migration_probe.test.stasis",
        MIGRATED_STDLIB_MAIN_SOURCE,
    );
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn run_linked_aot(aot: &AotProcess) -> i32 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!(
        "constant-groups-aot-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&directory).expect("create linked AOT directory");

    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let (import, runtime) = [&deps, deps.parent().expect("Cargo profile directory")]
        .into_iter()
        .find_map(|candidate| {
            let (import_name, runtime_name) = if cfg!(windows) {
                ("stasis_dynload.dll.lib", "stasis_dynload.dll")
            } else if cfg!(target_os = "macos") {
                ("libstasis_dynload.dylib", "libstasis_dynload.dylib")
            } else {
                ("libstasis_dynload.so", "libstasis_dynload.so")
            };
            let import = candidate.join(import_name);
            let runtime = candidate.join(runtime_name);
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh dynload runtime and import library");
    std::fs::copy(
        &runtime,
        directory.join(runtime.file_name().expect("runtime library name")),
    )
    .expect("copy dynload runtime");

    #[cfg(windows)]
    let linker_path = Some(
        std::env::var_os("STASIS_AOT_LINKER")
            .map(PathBuf::from)
            .or_else(|| {
                cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
                    .map(|tool| tool.path().to_path_buf())
            })
            .expect("MSVC link.exe is required for linked AOT execution"),
    );
    #[cfg(not(windows))]
    let linker_path = std::env::var_os("STASIS_AOT_LINKER").map(PathBuf::from);
    let executable = directory.join(format!(
        "constant_groups_main{}",
        std::env::consts::EXE_SUFFIX
    ));
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path,
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link native AOT executable");
    #[cfg(windows)]
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .status()
        .expect("run linked AOT executable");
    #[cfg(not(windows))]
    let status = {
        let variable = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![directory.clone()];
        if let Some(existing) = std::env::var_os(variable) {
            paths.extend(std::env::split_paths(&existing));
        }
        Command::new(&executable)
            .current_dir(&directory)
            .env(
                variable,
                std::env::join_paths(paths).expect("join AOT runtime search path"),
            )
            .status()
            .expect("run linked AOT executable")
    };
    let _ = std::fs::remove_dir_all(&directory);
    status.code().expect("linked AOT process exit code")
}

#[test]
fn imported_typed_groups_execute_in_jit() {
    let mut jit = JitProcess::new();
    configure_jit(&mut jit);
    jit.compile().expect("compile grouped-constant JIT fixture");
    assert_eq!(
        jit.execute_i32_noarg_by_name("main")
            .expect("execute grouped-constant JIT fixture"),
        0,
        "qualified imported i32 member indexes storage and f32 member evaluates"
    );
}

#[test]
fn imported_typed_groups_execute_in_wasm() {
    let mut wasm = WasmProcess::new();
    configure_wasm(&mut wasm);
    wasm.compile()
        .expect("compile grouped-constant Wasm fixture");
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_constant_groups_{}.wasm",
        std::process::id()
    ));
    std::fs::write(&wasm_path, wasm.module_bytes()).expect("write grouped-constant Wasm module");
    let node = std::env::var_os("STASIS_NODE").unwrap_or_else(|| "node".into());
    let output = Command::new(node)
        .args([
            "-e",
            "const fs=require('node:fs'); WebAssembly.instantiate(fs.readFileSync(process.argv[1]), {}).then(({instance})=>process.stdout.write(String(instance.exports.main()))).catch(error=>{console.error(error);process.exit(1)})",
        ])
        .arg(&wasm_path)
        .output()
        .expect("execute grouped-constant Wasm fixture");
    let _ = std::fs::remove_file(&wasm_path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "0",
        "Wasm observes imported i32, f32, and string group members"
    );
}

#[test]
fn imported_typed_groups_execute_in_linked_native_aot() {
    let mut aot = AotProcess::new();
    configure_aot(&mut aot);
    aot.compile().expect("compile grouped-constant AOT fixture");
    assert_eq!(
        run_linked_aot(&aot),
        0,
        "linked native AOT executable evaluates imported grouped constants"
    );
}

#[test]
fn migrated_stdlib_groups_execute_in_jit_and_linked_native_aot() {
    let mut jit = JitProcess::new();
    configure_migrated_jit(&mut jit);
    jit.compile()
        .expect("compile real-stdlib grouped-constant JIT fixture");
    assert_eq!(
        jit.execute_i32_noarg_by_name("main")
            .expect("execute real-stdlib grouped-constant JIT fixture"),
        0,
        "real animation, external URL, platform service, and UI paths use grouped constants"
    );

    let mut aot = AotProcess::new();
    configure_migrated_aot(&mut aot);
    aot.compile()
        .expect("compile real-stdlib grouped-constant AOT fixture");
    assert_eq!(
        run_linked_aot(&aot),
        0,
        "linked native AOT executes real stdlib grouped-constant behavior"
    );
}

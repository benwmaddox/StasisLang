use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::{wasm_global_hash, WasmProcess};
use stasis_compiler::backend::EngineEntrypoints;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static RUNTIME_GLOBALS: Mutex<()> = Mutex::new(());

const FIXTURE_PATH: &str = "tests/stasis/seams/sealed_display_list_probe.stasis";
const FIXTURE: &str = r#"
import "../../../src/stdlib/graphics.stasis";
enum SpriteRef { Probe = 77, }
global presentation: PresentationList;
global render_mode: i32;
global render_writer: SpriteRunWriter;
function bounds_probe(): i32 {
    presentation.reset_presentation();
    for (let i: i32 = 0; i < 256; i += 1) {
        if (!presentation.append_solid_rect(1.0, 2.0, 3.0, 4.0, 0.1, 0.2, 0.3, 0.4)) { return 10; }
    }
    if (presentation.append_solid_rect(1.0, 2.0, 3.0, 4.0, 0.1, 0.2, 0.3, 0.4)) { return 11; }
    presentation.count = 300;
    begin_frame();
    presentation.replay();
    end_frame();
    return 0;
}
function main(): i32 {
    begin_frame();
    presentation.reset_presentation();
    if (!presentation.append_solid_rect(1.0, 2.0, 3.0, 4.0, 0.1, 0.2, 0.3, 0.4)) { return 1; }
    if (!presentation.append_sprite(SpriteRef.Probe, 5.0, 6.0, 7.0, 8.0, 9, 128)) { return 2; }
    if (!presentation.append_solid_rect(10.0, 11.0, 12.0, 13.0, 0.5, 0.6, 0.7, 0.8)) { return 3; }
    if (!presentation.patch_sprite(1, SpriteRef.Probe, 15.0, 16.0, 17.0, 18.0, 19, 64)) { return 4; }
    presentation.replay();
    end_frame();
    return 0;
}
function set_render_mode(value: i32): i32 {
    render_mode = value;
    return 0;
}
function tick(): i32 { return 0; }
function on_code_swap(): void {}
function render(): i32 {
    if (render_mode == 5) {
        end_frame();
        return 0;
    }
    if (render_mode == 6) {
        fill_rect(2.0, 3.0, 4.0, 5.0, 0.4, 0.5, 0.6, 1.0);
        end_frame();
        return 0;
    }
    clear(0.1, 0.2, 0.3, 1.0);
    fill_rect(2.0, 3.0, 4.0, 5.0, 0.4, 0.5, 0.6, 1.0);
    draw_text(0, "x", 1.0, 2.0, 1.0, 1.0, 1.0, 1.0);
    if (render_mode == 1) {
        render_writer.reserve(2, -1, 0, 0, 0, 0, 0);
        end_frame();
        return 0;
    }
    if (render_mode == 2) {
        return 0;
    }
    if (render_mode == 3) {
        end_frame();
        return 0;
    }
    if (render_mode == 4) {
        end_frame();
        return 9;
    }
    end_frame();
    end_frame();
    clear(0.7, 0.8, 0.9, 1.0);
    return 0;
}
"#;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn configured_jit(path: &str, source: &str) -> JitProcess {
    let mut process = JitProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set project root");
    process.set_required_emit_roots(&[
        "bounds_probe".to_string(),
        "main".to_string(),
        "set_render_mode".to_string(),
    ]);
    process.upsert_file(path, source);
    process
}

fn configured_wasm(path: &str, source: &str) -> WasmProcess {
    let mut process = WasmProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set project root");
    process.set_required_emit_roots(&[
        "bounds_probe".to_string(),
        "main".to_string(),
        "set_render_mode".to_string(),
    ]);
    process.upsert_file(path, source);
    process
}

#[test]
fn typed_presentation_list_preserves_order_in_jit_and_compiles_for_wasm() {
    let _runtime_globals = RUNTIME_GLOBALS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut i32s = vec![0; 67_888];
    let mut f32s = vec![0.0; 146_564];
    let mut u8s = vec![0; 65_536];
    register_global_i32_array(
        global_path_hash("gfx_cmd_i32"),
        0,
        i32s.as_mut_ptr(),
        i32s.len(),
    );
    register_global_f32_array(
        global_path_hash("gfx_cmd_f32"),
        0,
        f32s.as_mut_ptr(),
        f32s.len(),
    );
    register_global_u8_array(
        global_path_hash("gfx_cmd_u8"),
        0,
        u8s.as_mut_ptr(),
        u8s.len(),
    );

    let mut jit = configured_jit(FIXTURE_PATH, FIXTURE);
    jit.compile()
        .expect("compile typed presentation list for JIT");
    assert_eq!(jit.execute_i32_noarg_by_name("bounds_probe"), Ok(0));
    assert_eq!(i32s[24], 256, "corrupted list count clamps to capacity");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
    assert_eq!(i32s[4], 1, "sprite count");
    assert_eq!(i32s[24], 2, "rectangle count");
    assert_eq!(i32s[22], 3, "order count");
    assert_eq!(&i32s[51_232..51_235], &[65_536, 32_768, 65_537]);
    assert_eq!(i32s[32], 77, "opaque reference reaches the ABI unchanged");
    assert_eq!(f32s[80_004], 15.0, "logical patch is applied before replay");

    let mut wasm = configured_wasm(FIXTURE_PATH, FIXTURE);
    wasm.compile()
        .expect("compile typed presentation list for Wasm");
    assert!(wasm.module_bytes().starts_with(b"\0asm"));
}

#[test]
fn negotiated_render_entry_resets_once_and_requires_finished_publication() {
    let _runtime_globals = RUNTIME_GLOBALS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut i32s = vec![0; 67_888];
    let mut f32s = vec![0.0; 146_564];
    let mut u8s = vec![0; 65_536];
    register_global_i32_array(
        global_path_hash("gfx_cmd_i32"),
        0,
        i32s.as_mut_ptr(),
        i32s.len(),
    );
    register_global_f32_array(
        global_path_hash("gfx_cmd_f32"),
        0,
        f32s.as_mut_ptr(),
        f32s.len(),
    );
    register_global_u8_array(
        global_path_hash("gfx_cmd_u8"),
        0,
        u8s.as_mut_ptr(),
        u8s.len(),
    );

    let mut jit = configured_jit(FIXTURE_PATH, FIXTURE);
    jit.compile()
        .expect("compile negotiated construction fixture");
    let package = jit
        .build_engine_package(&EngineEntrypoints::runtime_default())
        .expect("build negotiated engine package");
    assert_eq!(package.render_construction_lifecycle_version, 1);
    assert!(package.render_construction_reset_code_ptr.is_some());
    assert!(package.render_construction_finish_code_ptr.is_some());
    stasis_dynload::begin_jit_host_entry_session(
        package.host_entry_targets(1).expect("host targets"),
    )
    .expect("publish host targets");

    i32s[10] = 321;
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(i32s[2], 3, "clear plus idempotent publication");
    assert_eq!(i32s[24], 1, "one rectangle in the fresh construction");
    assert_eq!(i32s[7], 1, "one text command in the fresh construction");
    assert_eq!(i32s[22], 2, "rectangle and text preserve source order");
    assert_eq!(&i32s[51_232..51_234], &[65_536, 49_152]);
    assert_eq!(i32s[10], 321, "host display metadata survives reset");
    assert_eq!(&f32s[..4], &[0.7, 0.8, 0.9, 1.0], "last clear wins");

    jit.execute_i32_onearg_by_name("set_render_mode", 1)
        .expect("select unfinished writer");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(i32s[2], 0, "unfinished writer aborts publication");
    assert_eq!(i32s[24], 0, "aborted geometry becomes unreachable");
    assert_eq!(i32s[7], 0, "aborted text becomes unreachable");

    jit.execute_i32_onearg_by_name("set_render_mode", 2)
        .expect("select early return");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(i32s[2], 0, "unpublished construction is discarded");
    assert_eq!(i32s[24], 0, "unpublished geometry is discarded");

    jit.execute_i32_onearg_by_name("set_render_mode", 3)
        .expect("select published early return");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(
        i32s[2], 3,
        "publication before a successful return is retained"
    );
    assert_eq!(i32s[24], 1);

    jit.execute_i32_onearg_by_name("set_render_mode", 4)
        .expect("select failed published return");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(9)
    );
    assert_eq!(i32s[2], 0, "nonzero render result discards publication");
    assert_eq!(i32s[24], 0);

    jit.execute_i32_onearg_by_name("set_render_mode", 5)
        .expect("select empty no-clear frame");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(i32s[2], 2, "empty no-clear frame publishes explicitly");
    assert_eq!(i32s[24], 0);

    jit.execute_i32_onearg_by_name("set_render_mode", 6)
        .expect("select geometry no-clear frame");
    assert_eq!(
        stasis_dynload::invoke_noarg_i32(stasis_dynload::jit_host_render_trampoline_ptr()),
        Ok(0)
    );
    assert_eq!(i32s[2], 2, "no-clear does not imply background replacement");
    assert_eq!(i32s[24], 1);

    let mut wasm = configured_wasm(FIXTURE_PATH, FIXTURE);
    wasm.compile()
        .expect("compile negotiated construction fixture for Wasm");
    let i32_offset = wasm.memory_layout()["gfx_cmd_i32"].offset;
    let render_mode_hash = wasm_global_hash("render_mode");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let wasm_path = std::env::temp_dir().join(format!(
        "stasis_render_construction_{}_{}.wasm",
        std::process::id(),
        stamp
    ));
    fs::write(&wasm_path, wasm.module_bytes()).expect("write negotiated Wasm fixture");
    let script = r#"
const fs = require('node:fs');
const module = new WebAssembly.Module(fs.readFileSync(process.argv[1]));
const env = {};
for (const imported of WebAssembly.Module.imports(module)) {
  if (imported.module === 'env' && imported.kind === 'function') env[imported.name] = () => 0;
}
WebAssembly.instantiate(module, {env}).then(instance => {
  const e = instance.exports;
  const i32 = new Int32Array(e.memory.buffer, Number(process.argv[2]), 67888);
  const run = mode => {
    e.__stasis_global_set_i32(Number(process.argv[3]), mode);
    e.gfx_cmd_construction_reset();
    const result = e.gfx_cmd_construction_finish(e.render());
    return [result, i32[2], i32[24], i32[10]];
  };
  i32[10] = 321;
  process.stdout.write([run(0), run(1), run(2), run(3), run(4), run(5), run(6)].flat().join(','));
}).catch(error => { console.error(error); process.exit(1); });
"#;
    let output = Command::new("node")
        .args(["-e", script])
        .arg(&wasm_path)
        .arg(i32_offset.to_string())
        .arg(render_mode_hash.to_string())
        .output()
        .expect("execute negotiated Wasm fixture");
    let _ = fs::remove_file(&wasm_path);
    assert!(
        output.status.success(),
        "Node failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "0,3,1,321,0,0,0,321,0,0,0,321,0,3,1,321,9,0,0,321,0,2,0,321,0,2,1,321"
    );
}

#[test]
fn integer_sprite_reference_forgery_is_rejected_by_jit_and_wasm() {
    let _runtime_globals = RUNTIME_GLOBALS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for source in [
        "import \"src/stdlib/graphics.stasis\"; global hero: Sprite; function main(): i32 { hero.sprite_ref = 7; return 0; }",
        "import \"src/stdlib/graphics.stasis\"; function main(): i32 { draw_sprite(7, 0.0, 0.0, 1.0, 1.0, 0, 255); return 0; }",
    ] {
        let mut jit = configured_jit("main.stasis", source);
        jit.compile().expect_err("JIT must reject forged SpriteRef");

        let mut wasm = configured_wasm("main.stasis", source);
        wasm.compile().expect_err("Wasm must reject forged SpriteRef");
    }
}

#[test]
fn privileged_graphics_extern_alias_is_rejected_by_jit_and_wasm() {
    let _runtime_globals = RUNTIME_GLOBALS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for source in [
        "function @extern(\"stasis_jit_gfx_release_sprite\") release_alias(value: i32): void; function main(): i32 { return 0; }",
        "extern function gfx_release_sprite(value: i32): void; function main(): i32 { return 0; }",
    ] {
        let mut jit = configured_jit("main.stasis", source);
        assert!(jit.compile().is_err());
        let mut wasm = configured_wasm("main.stasis", source);
        assert!(wasm.compile().is_err());
    }
}

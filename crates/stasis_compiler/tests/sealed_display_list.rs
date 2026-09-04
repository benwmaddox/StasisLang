use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array,
};
use std::path::{Path, PathBuf};

const FIXTURE_PATH: &str = "tests/stasis/seams/sealed_display_list_probe.stasis";
const FIXTURE: &str = r#"
import "../../../src/stdlib/graphics.stasis";
enum SpriteRef { Probe = 77, }
global presentation: PresentationList;
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
    process.set_required_emit_roots(&["bounds_probe".to_string(), "main".to_string()]);
    process.upsert_file(path, source);
    process
}

fn configured_wasm(path: &str, source: &str) -> WasmProcess {
    let mut process = WasmProcess::new();
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set project root");
    process.set_required_emit_roots(&["bounds_probe".to_string(), "main".to_string()]);
    process.upsert_file(path, source);
    process
}

#[test]
fn typed_presentation_list_preserves_order_in_jit_and_compiles_for_wasm() {
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
fn integer_sprite_reference_forgery_is_rejected_by_jit_and_wasm() {
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

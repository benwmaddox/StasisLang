#![cfg(windows)]

use image::RgbaImage;
use serde_json::json;
use stasis_assets::{load_project_asset_manifest, AssetLimits};
use stasis_compiler::backend::jit::JitProcess;
use stasis_dynload::{
    global_path_hash, register_global_f32_array, register_global_i32_array,
    register_global_u8_array, runtime_library_candidate_paths, Library, StasisGraphicsApi,
    STASIS_RENDER_F32_COUNT, STASIS_RENDER_I32_COUNT, STASIS_RENDER_U8_COUNT,
};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

const FIXTURE: &str =
    include_str!("../../../tests/stasis/seams/desktop_manifest_assets_probe.stasis");
const LOGICAL_SIZE: [u32; 2] = [320, 180];
const PNG_SHA256: &str = "98d61197c8db539121336207a1cc722093a0d3e0acd5ef5196c1eda3e9b92d72";
const FONT_SHA256: &str = "17ec668bd0cd62e934f97563287ed72a4a8599ae716d20c1a93c82f1876dde47";
const PNG_MANIFEST_HANDLE: i32 = 1_221_991_035;
const FONT_MANIFEST_HANDLE: i32 = 623_275_877;

type SetAssetRoot = extern "system" fn(*const std::ffi::c_char) -> i32;
type ScheduleScreenshot = extern "system" fn(*const std::ffi::c_char) -> i32;

struct NativeAssetHarness {
    _library: Library,
    set_asset_root: SetAssetRoot,
    schedule_screenshot: ScheduleScreenshot,
}

impl NativeAssetHarness {
    fn load(path: &Path) -> Self {
        let library = Library::load(path).expect("load graphics runtime for asset seam");
        let set_asset_root = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_set_asset_root")
                    .expect("resolve native asset-root setter"),
            )
        };
        let schedule_screenshot = unsafe {
            std::mem::transmute(
                library
                    .symbol_address("stasis_host_schedule_screenshot")
                    .expect("resolve native screenshot scheduler"),
            )
        };
        Self {
            _library: library,
            set_asset_root,
            schedule_screenshot,
        }
    }

    fn asset_root(&self, path: &Path) {
        let path = CString::new(path.to_string_lossy().as_bytes()).expect("asset root CString");
        assert_eq!((self.set_asset_root)(path.as_ptr()), 1, "set asset root");
    }

    fn screenshot(&self, path: &Path) {
        let path = CString::new(path.to_string_lossy().as_bytes()).expect("screenshot CString");
        assert_eq!(
            (self.schedule_screenshot)(path.as_ptr()),
            1,
            "schedule native screenshot"
        );
    }
}

struct WorkingDirectoryGuard(PathBuf);

impl Drop for WorkingDirectoryGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.0).expect("restore test working directory");
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"))
        .join("seam-tests")
}

fn scalar_i32(path: &str) -> i32 {
    stasis_dynload::stasis_jit_global_i32_load(global_path_hash(path))
}

fn scalar_f32(path: &str) -> f32 {
    stasis_dynload::stasis_jit_global_f32_load(global_path_hash(path))
}

fn colored_pixels(
    image: &RgbaImage,
    logical_region: [u32; 4],
    predicate: impl Fn([u8; 4]) -> bool,
) -> usize {
    let x0 = logical_region[0] * image.width() / LOGICAL_SIZE[0];
    let y0 = logical_region[1] * image.height() / LOGICAL_SIZE[1];
    let x1 = (logical_region[0] + logical_region[2]) * image.width() / LOGICAL_SIZE[0];
    let y1 = (logical_region[1] + logical_region[3]) * image.height() / LOGICAL_SIZE[1];
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| image.get_pixel(x, y).0))
        .filter(|pixel| predicate(*pixel))
        .count()
}

#[test]
fn manifest_assets_survive_wrong_cwd_and_render_sprite_direct_and_cached_text() {
    let repository = repository_root();
    let project = repository.join("samples/windows_launch_smoke");
    let manifest = load_project_asset_manifest(&project, AssetLimits::default())
        .expect("load checked desktop asset manifest");
    let png = manifest.by_id("smoke_png").expect("manifest PNG identity");
    let font = manifest
        .by_id("smoke_font")
        .expect("manifest font identity");
    assert_eq!(png.entry.path, "assets/smoke.png");
    assert_eq!(png.entry.content_sha256, PNG_SHA256);
    assert_eq!(png.handle.as_i32(), PNG_MANIFEST_HANDLE);
    assert_eq!(
        png.absolute_path,
        project
            .join("assets/smoke.png")
            .canonicalize()
            .expect("canonical PNG path")
    );
    assert_eq!(font.entry.path, "assets/smoke.ttf");
    assert_eq!(font.entry.content_sha256, FONT_SHA256);
    assert_eq!(font.handle.as_i32(), FONT_MANIFEST_HANDLE);
    assert_eq!(
        font.absolute_path,
        project
            .join("assets/smoke.ttf")
            .canonicalize()
            .expect("canonical font path")
    );

    let evidence_root = evidence_root();
    let wrong_cwd = evidence_root.join("it-008-wrong-working-directory");
    fs::create_dir_all(&wrong_cwd).expect("create isolated wrong working directory");
    let original_cwd = std::env::current_dir().expect("read original working directory");
    let _cwd_guard = WorkingDirectoryGuard(original_cwd);
    std::env::set_current_dir(&wrong_cwd).expect("enter wrong working directory");

    let runtime_path = std::env::var_os("STASIS_RUNTIME_LIBRARY_PATH")
        .or_else(|| std::env::var_os("STASIS_RUNTIME_DLL_PATH"))
        .map(PathBuf::from)
        .expect(
        "STASIS_RUNTIME_LIBRARY_PATH (or legacy STASIS_RUNTIME_DLL_PATH) must name the CI-built SDL runtime",
    );
    let selected_runtime = runtime_library_candidate_paths()
        .into_iter()
        .find(|candidate| candidate.is_file())
        .expect("select configured graphics runtime");
    assert_eq!(
        selected_runtime
            .canonicalize()
            .expect("canonical selected runtime"),
        runtime_path
            .canonicalize()
            .expect("canonical configured runtime"),
        "explicit runtime must outrank repository development fallbacks"
    );
    std::env::set_var("STASIS_GFX_LOG_SPRITES", "1");
    let gfx = StasisGraphicsApi::load(&runtime_path).expect("load graphics runtime");
    assert!(gfx
        .init_window(320, 180, "Stasis IT-008 manifest assets seam")
        .expect("initialize native window"));
    let native = NativeAssetHarness::load(&runtime_path);

    let mut gfx_i32 = vec![0; STASIS_RENDER_I32_COUNT];
    let mut gfx_f32 = vec![0.0; STASIS_RENDER_F32_COUNT];
    let mut gfx_u8 = vec![0; STASIS_RENDER_U8_COUNT];
    register_global_i32_array(
        global_path_hash("gfx_cmd_i32"),
        0,
        gfx_i32.as_mut_ptr(),
        gfx_i32.len(),
    );
    register_global_f32_array(
        global_path_hash("gfx_cmd_f32"),
        0,
        gfx_f32.as_mut_ptr(),
        gfx_f32.len(),
    );
    register_global_u8_array(
        global_path_hash("gfx_cmd_u8"),
        0,
        gfx_u8.as_mut_ptr(),
        gfx_u8.len(),
    );

    let mut jit = JitProcess::new();
    jit.set_project_root(repository.to_string_lossy())
        .expect("set fixture project root");
    jit.upsert_file(
        "tests/stasis/seams/desktop_manifest_assets_probe.stasis",
        FIXTURE,
    );
    jit.compile().expect("compile manifest asset fixture");

    let wrong_cwd_result = jit
        .execute_i32_noarg_by_name("main")
        .expect("execute wrong-working-directory probe");
    assert_eq!(
        wrong_cwd_result, 21,
        "relative sprite lookup must fail before the host supplies its asset root"
    );

    native.asset_root(&project);
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));
    let sprite_handle = scalar_i32("seam_sprite_handle");
    let font_handle = scalar_i32("seam_font_handle");
    let cached_handle = scalar_i32("seam_cached_handle");
    let direct_width = scalar_f32("seam_direct_width");
    let cached_width = scalar_f32("seam_cached_width");
    assert_eq!(sprite_handle, 1, "first native sprite handle");
    assert_eq!(font_handle, 1, "first native font handle");
    assert_eq!(cached_handle, 1, "first native cached-text handle");
    assert!(direct_width > 0.0 && cached_width > 0.0);

    assert_eq!(jit.execute_i32_noarg_by_name("tick"), Ok(0));
    assert_eq!(jit.execute_i32_noarg_by_name("render"), Ok(0));
    assert_eq!(gfx_i32[4], 1, "one sprite command");
    assert_eq!(gfx_i32[7], 2, "direct and cached text commands");
    assert_eq!(gfx_i32[22], 3, "ordered sprite and text commands");
    assert_eq!(&gfx_u8[0..7], b"DIRECT\0", "direct UTF-8 command bytes");
    assert_eq!(gfx_i32[12321], 0, "direct text byte offset");
    assert_eq!(gfx_i32[12322], 6, "direct text byte length");
    assert_eq!(gfx_i32[12324], -cached_handle, "cached text handle tag");
    let trace = unsafe {
        stasis_dynload::stasis_jit_render_trace(
            global_path_hash("gfx_cmd_i32"),
            gfx_i32.len() as i32,
            global_path_hash("gfx_cmd_f32"),
            gfx_f32.len() as i32,
            global_path_hash("gfx_cmd_u8"),
            gfx_u8.len() as i32,
        )
    };
    assert_ne!(
        trace, 0,
        "canonical manifest asset frame must produce a trace"
    );

    let screenshot = evidence_root.join("it-008-desktop-manifest-assets.png");
    native.screenshot(&screenshot);
    gfx.gfx_submit_u8(&mut gfx_i32, &gfx_f32, &gfx_u8)
        .expect("submit manifest asset frame");
    let image = image::open(&screenshot)
        .expect("open native asset screenshot")
        .to_rgba8();
    let regions = [
        (
            "sprite_magenta",
            colored_pixels(&image, [52, 28, 64, 64], |p| {
                p[0] > 170 && p[2] > 170 && p[1] < 150
            }),
        ),
        (
            "direct_text_yellow",
            colored_pixels(&image, [24, 106, 125, 38], |p| {
                p[0] > 140 && p[1] > 110 && p[2] < 130
            }),
        ),
        (
            "cached_text_cyan",
            colored_pixels(&image, [169, 106, 125, 38], |p| {
                p[0] < 150 && p[1] > 130 && p[2] > 150
            }),
        ),
    ];
    for (name, count) in regions {
        assert!(
            count >= 8,
            "named pixel region {name} contained only {count} matching pixels"
        );
    }

    let evidence = json!({
        "schema": "stasis.seam_test.v1",
        "test_id": "IT-008",
        "status": "passed",
        "target": "windows-sdl-jit",
        "wrong_working_directory_main_result": wrong_cwd_result,
        "manifest": [
            {"id": png.entry.id, "path": png.entry.path, "resolved_path": png.absolute_path, "sha256": png.entry.content_sha256, "manifest_handle": png.handle.as_i32(), "runtime_handle": sprite_handle},
            {"id": font.entry.id, "path": font.entry.path, "resolved_path": font.absolute_path, "sha256": font.entry.content_sha256, "manifest_handle": font.handle.as_i32(), "runtime_handle": font_handle}
        ],
        "text": {"direct_width": direct_width, "cached_width": cached_width, "cached_handle": cached_handle},
        "render": {"trace": trace, "screenshot": screenshot, "regions": regions.into_iter().collect::<std::collections::BTreeMap<_, _>>()}
    });
    let evidence_path = evidence_root.join("it-008-desktop-manifest-assets.json");
    fs::write(
        &evidence_path,
        serde_json::to_vec_pretty(&evidence).expect("serialize seam evidence"),
    )
    .expect("write seam evidence");
    eprintln!("IT-008 evidence: {evidence}");
}

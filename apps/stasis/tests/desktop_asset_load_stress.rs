#![cfg(windows)]

use serde_json::json;
use stasis_assets::{load_project_asset_manifest, AssetFormat, AssetLimits};
use stasis_dynload::Library;
use std::collections::HashSet;
use std::ffi::{c_char, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const FONT_COUNT: usize = 10;
const SPRITE_COUNT: usize = 536;
const PHRASE_COUNT: usize = 600;

type InitWindow = extern "system" fn(i32, i32, *const c_char) -> i32;
type Shutdown = extern "system" fn();
type SetAssetRoot = extern "system" fn(*const c_char) -> i32;
type LoadSprite = extern "system" fn(*const c_char, i32, i32) -> i32;
type LoadFont = extern "system" fn(*const c_char, i32) -> i32;
type MeasureText = extern "system" fn(i32, *const c_char) -> f32;
type CacheText = extern "system" fn(i32, *const c_char) -> i32;

struct ShutdownGuard(Option<Shutdown>);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(shutdown) = self.0.take() {
            shutdown();
        }
    }
}

struct FixtureGuard {
    path: PathBuf,
    parent: PathBuf,
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        debug_assert!(self.path.starts_with(&self.parent));
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn fixture_root(repository: &Path) -> FixtureGuard {
    let parent = std::env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("read system clock")
        .as_nanos();
    let root = (0..100)
        .map(|attempt| {
            parent.join(format!(
                "stasis_asset_load_stress_{}_{}_{}",
                std::process::id(),
                timestamp,
                attempt
            ))
        })
        .find(|candidate| fs::create_dir(candidate).is_ok())
        .expect("create unique asset-load fixture directory");
    assert!(root.starts_with(&parent));
    let guard = FixtureGuard { path: root, parent };
    let generator = repository.join("tools/ci/generate_asset_load_fixture.py");
    let output = Command::new("python")
        .arg(generator)
        .arg(&guard.path)
        .output()
        .expect("run deterministic asset fixture generator");
    assert!(
        output.status.success(),
        "fixture generator failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    guard
}

fn evidence_path(repository: &Path) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("target"));
    target.join("seam-tests/it-asset-load-stress.json")
}

unsafe fn symbol<T>(library: &Library, name: &str) -> T {
    let address = library
        .symbol_address(name)
        .unwrap_or_else(|error| panic!("resolve native symbol {name}: {error}"));
    assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<usize>(),
        "native symbol pointer size"
    );
    std::mem::transmute_copy(&address)
}

#[test]
fn desktop_asset_load_stress_loads_bounded_fixture() {
    std::env::set_var("STASIS_USE_SDL", "1");
    let repository = repository_root();
    let fixture = fixture_root(&repository);
    let manifest = load_project_asset_manifest(&fixture.path, AssetLimits::default())
        .expect("validate generated v2 asset manifest");
    assert_eq!(manifest.assets.len(), 546);
    let sprites = manifest
        .assets
        .iter()
        .filter(|asset| matches!(asset.entry.format, AssetFormat::Sprite { .. }))
        .collect::<Vec<_>>();
    let fonts = manifest
        .assets
        .iter()
        .filter(|asset| matches!(asset.entry.format, AssetFormat::Font { .. }))
        .collect::<Vec<_>>();
    assert_eq!(sprites.len(), SPRITE_COUNT);
    assert_eq!(fonts.len(), FONT_COUNT);

    let runtime_path = PathBuf::from(
        std::env::var_os("STASIS_RUNTIME_DLL_PATH")
            .expect("STASIS_RUNTIME_DLL_PATH must name the CI-built SDL runtime"),
    );
    let library = Library::load(&runtime_path).expect("load SDL graphics runtime");
    let init_window: InitWindow = unsafe { symbol(&library, "stasis_init_window") };
    let shutdown: Shutdown = unsafe { symbol(&library, "stasis_shutdown") };
    let set_asset_root: SetAssetRoot = unsafe { symbol(&library, "stasis_set_asset_root") };
    let load_sprite: LoadSprite = unsafe { symbol(&library, "stasis_gfx_load_sprite") };
    let load_font: LoadFont = unsafe { symbol(&library, "stasis_load_font") };
    let measure_text: MeasureText = unsafe { symbol(&library, "stasis_measure_text") };
    let cache_text: CacheText = unsafe { symbol(&library, "stasis_gfx_cache_text") };
    let title = CString::new("Stasis asset load stress").unwrap();
    assert_ne!(
        init_window(320, 180, title.as_ptr()),
        0,
        "initialize SDL runtime"
    );
    let _shutdown = ShutdownGuard(Some(shutdown));
    let root = CString::new(fixture.path.to_string_lossy().as_bytes()).unwrap();
    assert_eq!(set_asset_root(root.as_ptr()), 1, "set generated asset root");

    let started = Instant::now();
    let mut sprite_handles = Vec::with_capacity(sprites.len());
    let mut unique_sprite_handles = HashSet::with_capacity(sprites.len());
    for asset in &sprites {
        let (width, height) = match asset.entry.format {
            AssetFormat::Sprite { width, height, .. } => (width as i32, height as i32),
            _ => unreachable!(),
        };
        let path = CString::new(asset.entry.path.as_bytes()).unwrap();
        let handle = load_sprite(path.as_ptr(), width, height);
        assert!(handle > 0, "sprite failed to load: {}", asset.entry.id);
        assert!(
            unique_sprite_handles.insert(handle),
            "duplicate sprite handle"
        );
        sprite_handles.push(handle);
    }

    let mut font_handles = Vec::with_capacity(fonts.len());
    for asset in &fonts {
        let path = CString::new(asset.entry.path.as_bytes()).unwrap();
        let handle = load_font(path.as_ptr(), 18);
        assert!(handle > 0, "font failed to load: {}", asset.entry.id);
        assert!(!font_handles.contains(&handle), "duplicate font handle");
        font_handles.push(handle);
    }
    assert_eq!(font_handles.len(), FONT_COUNT);
    let repeated_font = CString::new(fonts[0].entry.path.as_bytes()).unwrap();
    assert_eq!(
        load_font(repeated_font.as_ptr(), 18),
        font_handles[0],
        "duplicate font load reuses the cache handle"
    );

    let phrases: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.path.join("phrases.json")).expect("read generated phrases"),
    )
    .expect("parse generated phrases");
    let phrases = phrases["phrases"].as_array().expect("phrase array");
    assert_eq!(phrases.len(), PHRASE_COUNT);
    let mut cached_handles = Vec::with_capacity(phrases.len());
    for phrase in phrases {
        let text = CString::new(phrase.as_str().expect("phrase text")).unwrap();
        let width = measure_text(font_handles[0], text.as_ptr());
        assert!(width > 0.0, "phrase measured zero width");
        let cached = cache_text(font_handles[0], text.as_ptr());
        assert!(cached > 0, "phrase cache returned zero");
        cached_handles.push(cached);
    }
    assert_eq!(
        cached_handles.iter().collect::<HashSet<_>>().len(),
        PHRASE_COUNT,
        "distinct phrases receive distinct cache handles"
    );
    let repeated = CString::new("deterministic asset-load phrase 000").unwrap();
    assert_eq!(
        cache_text(font_handles[0], repeated.as_ptr()),
        cached_handles[0],
        "cached phrase handle is stable"
    );

    let evidence = evidence_path(&repository);
    if let Some(parent) = evidence.parent() {
        fs::create_dir_all(parent).expect("create persistent asset-load evidence directory");
    }
    fs::write(
        evidence,
        serde_json::to_vec_pretty(&json!({
            "schema": "stasis-asset-load-stress",
            "test_id": "desktop_asset_load_stress",
            "status": "passed",
            "sprites": sprite_handles.len(),
            "fonts": font_handles.len(),
            "phrases": cached_handles.len(),
            "elapsed_ms": started.elapsed().as_millis(),
        }))
        .unwrap(),
    )
    .expect("write asset load evidence");
    eprintln!(
        "desktop asset load stress: sprites={} fonts={} phrases={} elapsed_ms={}",
        sprite_handles.len(),
        font_handles.len(),
        cached_handles.len(),
        started.elapsed().as_millis()
    );
}

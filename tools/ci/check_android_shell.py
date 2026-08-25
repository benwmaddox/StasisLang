from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_FILES = [
    "mobile/android/settings.gradle",
    "mobile/android/build.gradle",
    "mobile/android/build_rust_bridge.ps1",
    "mobile/android/rust_bridge_provenance.ps1",
    "mobile/android/build_release.ps1",
    "mobile/android/validate_device.ps1",
    "mobile/android/app/build.gradle",
    "mobile/android/app/src/main/AndroidManifest.xml",
    "mobile/android/app/src/workshop/AndroidManifest.xml",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopJniFrameAbiAcceptance.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTouchAcceptance.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTextureProvider.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSecretStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidEditRecoveryStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidDraftStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectRegistry.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTemplateCatalog.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopFrameBudget.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopPongAssetManifestMigration.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectFormatPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectArchive.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopOnboardingPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopOnboardingStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopImageAssets.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAccessibilityPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAdaptiveLayout.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopPaintView.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAudioAssets.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetManifest.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetIdentity.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopMoney.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiPricing.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiObservationMemory.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiToolLoopPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiWorkingNotes.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiOverlayPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiInitialContextPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopConnectivity.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopBackgroundWorkPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopGitHubSyncPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopGitHubApi.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopLongWorkCoordinator.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopLongWorkService.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSupportBundle.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidCrashStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopRestartLoopPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiQueue.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AiQueuePolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiQueueRunPolicy.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiSessionCheckpointStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiResumePolicy.java",
    "mobile/android/app/src/main/java/com/stasislang/workshop/StasisPreviewRenderer.java",
    "mobile/android/app/src/main/cpp/CMakeLists.txt",
    "mobile/android/app/src/main/cpp/stasis_android_sprite.c",
    "mobile/android/app/src/main/cpp/stasis_mobile_smoke.c",
    "mobile/android/codex_native/src/lib.rs",
    "mobile/android/app/src/main/res/values/styles.xml",
    "mobile/android/app/src/main/assets/workshop_sample/src/main.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/preview_adapter.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/root.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/game_state.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/player.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/enemy.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/input.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/assets.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/AGENTS.md",
    "mobile/android/app/src/main/assets/workshop_sample/CLAUDE.md",
    "mobile/android/app/src/main/assets/workshop_sample/stasis.json",
    "mobile/android/app/src/main/assets/workshop_sample/assets/ball.svg",
    "mobile/android/app/src/main/assets/workshop_sample/assets/paddle.svg",
    "mobile/android/app/src/main/assets/workshop_sample/assets/center_line.svg",
    "mobile/android/app/src/main/assets/workshop_sample/assets/manifest.json",
    "mobile/android/app/src/main/assets/exploration_sample/src/main.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/host.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/host_aot.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/host_game.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/host_runtime.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/config.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/components.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/world_data.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/input.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/assets.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/movement.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/collection.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/inventory.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/camera.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/tutorial.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/audio.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/render_extract.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/src/systems/schedule.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/tests/exploration_gameplay.test.stasis",
    "mobile/android/app/src/main/assets/exploration_sample/AGENTS.md",
    "mobile/android/app/src/main/assets/exploration_sample/CLAUDE.md",
    "mobile/android/app/src/main/assets/exploration_sample/assets/manifest.json",
    "mobile/android/app/src/main/assets/exploration_sample/assets/player.svg",
    "mobile/android/app/src/main/assets/exploration_sample/assets/sun_keepsake.svg",
    "mobile/android/app/src/main/assets/exploration_sample/assets/moon_keepsake.svg",
    "mobile/android/app/src/main/assets/exploration_sample/assets/destination.svg",
    "mobile/android/app/src/main/assets/exploration_sample/stasis.json",
    "mobile/android/app/src/main/assets/exploration_sample/qa/first_keepsake.json",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopExplorationLessonPolicy.java",
    "mobile/android/app/src/test/java/com/stasislang/workshop/WorkshopExplorationLessonPolicyTest.java",
    "mobile/android/app/src/test/java/com/stasislang/workshop/WorkshopTemplateCatalogTest.java",
    "mobile/android/README.md",
    "tools/android_ai_agent_host.py",
    "mobile/shells/android/app/src/main/java/com/stasislang/game/MainActivity.java",
    "mobile/shells/android/app/src/main/java/com/stasislang/shell/StasisAssetCache.java",
    "mobile/shells/android/app/src/main/cpp/stasis_android_assets.c",
    "tools/ci/test_android_asset_cache.py",
    "tools/ci/java/com/stasislang/shell/StasisAssetCacheTest.java",
    "tools/ci/check_android_release_package.py",
    "tests/android/AiQueuePolicyTest.java",
    "tests/android/WorkshopProjectFormatPolicyTest.java",
]

STASIS_SAMPLE_FILES = [
    "mobile/android/app/src/main/assets/workshop_sample/src/main.stasis",
]

def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8-sig")


def main() -> int:
    missing = [path for path in REQUIRED_FILES if not (ROOT / path).is_file()]
    if missing:
        raise AssertionError(f"missing Android shell files: {missing}")

    workspace = read("Cargo.toml")
    bridge_toml = read("crates/stasis_android_bridge/Cargo.toml")
    bridge = read("crates/stasis_android_bridge/src/lib.rs")
    assert "crates/stasis_android_bridge" in workspace
    assert "stasis_android_bridge" in bridge_toml
    assert "stasis_compiler" in bridge_toml
    assert "crate-type = [\"rlib\", \"cdylib\"]" in bridge_toml
    assert "compile_android_workshop_project" in bridge
    assert "stasis_android_bridge_compile_project" in bridge
    assert "load_android_workshop_asset_manifest" in bridge
    assert "stasis_assets::load_project_asset_manifest" in bridge
    assert "render_android_jit_manifest" in bridge
    assert "artifact_kind=executable-memory" in bridge

    rust_bridge_script = read("mobile/android/build_rust_bridge.ps1")
    rust_bridge_provenance = read("mobile/android/rust_bridge_provenance.ps1")
    debug_script = read("mobile/android/build_debug.ps1")
    emulator_script = read("mobile/android/start_emulator.ps1")
    emulator_test_script = read("mobile/android/test_emulator.ps1")
    assert "Workshop Android Gradle build failed with exit code" in debug_script
    assert "Rust Android bridge build failed with exit code" in rust_bridge_script
    assert "rustup target discovery failed with exit code" in rust_bridge_script
    release_script = read("mobile/android/build_release.ps1")
    device_script = read("mobile/android/validate_device.ps1")
    android_gitignore = read("mobile/android/.gitignore")
    assert 'linkerVariable = "CARGO_TARGET_' in rust_bridge_script
    assert "$useRelease = -not $Debug" in rust_bridge_script
    assert "rust_bridge_provenance.ps1" in rust_bridge_script
    assert "requires both ABIs" in rust_bridge_script
    assert "Get-BridgeInputFingerprint" in rust_bridge_provenance
    assert "stasis-rust-bridge.json" in rust_bridge_provenance
    assert "inputFingerprint = Get-BridgeInputFingerprint" in rust_bridge_provenance
    assert "Get-FileHash -LiteralPath $bridge -Algorithm SHA256" in rust_bridge_provenance
    assert "must contain exactly one entry for each required ABI" in rust_bridge_provenance
    assert "stale for the current Rust/Cargo inputs" in rust_bridge_provenance
    assert "aarch64-linux-android" in rust_bridge_script
    assert "x86_64-linux-android" in rust_bridge_script
    assert "linux-x86_64" in rust_bridge_script
    assert "[System.IO.Path]::IsPathRooted" in rust_bridge_script
    assert "libstasis_android_bridge.so" in rust_bridge_script
    assert 'Join-Path (Join-Path (Join-Path (Join-Path (Join-Path $scriptRoot "app") "src") "workshop") "jniLibs") $abi' in rust_bridge_script
    assert "cargo_cache.py" in rust_bridge_script
    assert 'build_rust_bridge.ps1") -Release' in debug_script
    assert ":app:assembleWorkshopDebug" in debug_script
    assert "package-mobile" in release_script
    assert ":app:bundleRelease" in release_script
    assert ":app:installDebug" in release_script
    assert "check_android_release_package.py" in release_script
    assert "build_rust_bridge.ps1" not in release_script
    assert "app/src/*/jniLibs/" in android_gitignore
    assert "RequireDevice" in device_script
    assert "android_device_acceptance" in device_script
    assert "ro.product.cpu.abilist" in device_script
    assert "arm64-v8a" in device_script
    assert "am\", \"start\", \"-W" in device_script
    assert "pidof" in device_script
    assert "$appPid" in device_script
    assert "$pid =" not in device_script
    assert "Serial" in device_script
    assert "Stasis_API_35" in emulator_script
    assert "sys.boot_completed" in emulator_script
    assert "build_debug.ps1" in emulator_test_script
    assert "validate_device.ps1" in emulator_test_script

    app_gradle = read("mobile/android/app/build.gradle")
    assert "flavorDimensions 'mode'" in app_gradle
    assert "verifyWorkshopRustBridge" in app_gradle
    assert "stasis.allowDebugRustBridge" in app_gradle
    assert "rust_bridge_provenance.ps1" in app_gradle
    assert "workshop {" in app_gradle
    assert "applicationId 'com.stasislang.workshop'" in app_gradle
    assert "abiFilters 'arm64-v8a', 'x86_64'" in app_gradle
    assert "externalNativeBuild" in app_gradle
    assert "STASIS_ANDROID_SMOKE_ONLY=ON" in app_gradle
    assert "prepareWorkshopAssets" in app_gradle
    assert "workshop_sample/build/**" in app_gradle
    assert "exploration_sample/build/**" in app_gradle

    manifest = read("mobile/android/app/src/main/AndroidManifest.xml")
    styles = read("mobile/android/app/src/main/res/values/styles.xml")
    workshop_manifest = read("mobile/android/app/src/workshop/AndroidManifest.xml")
    assert "android.permission.INTERNET" not in manifest
    assert "android.permission.RECORD_AUDIO" in workshop_manifest
    assert "android.permission.INTERNET" in workshop_manifest
    assert "android.permission.ACCESS_NETWORK_STATE" in workshop_manifest
    assert "android.permission.FOREGROUND_SERVICE" in workshop_manifest
    assert "android.permission.FOREGROUND_SERVICE_DATA_SYNC" in workshop_manifest
    assert 'android:foregroundServiceType="dataSync"' in workshop_manifest
    assert "${appLabel}" in manifest
    assert "android.intent.action.MAIN" in manifest
    assert "android.intent.category.LAUNCHER" in manifest
    assert 'android:exported="true"' in manifest
    assert 'android:resizeableActivity="true"' in manifest
    assert 'android:windowSoftInputMode="adjustResize"' in manifest
    assert '<item name="android:windowLightStatusBar">false</item>' in styles
    assert '<item name="android:textColorPrimary">#161B22</item>' in styles

    activity = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java")
    preview_renderer = read("mobile/android/app/src/main/java/com/stasislang/workshop/StasisPreviewRenderer.java")
    workshop_textures = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTextureProvider.java")
    onboarding_policy = read(
        "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopOnboardingPolicy.java"
    )
    onboarding_store = read(
        "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopOnboardingStore.java"
    )
    secret_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSecretStore.java")
    github_sync_policy = read(
        "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopGitHubSyncPolicy.java"
    )
    github_api = read(
        "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopGitHubApi.java"
    )
    recovery_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidEditRecoveryStore.java")
    draft_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidDraftStore.java")
    project_registry = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectRegistry.java")
    template_catalog = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTemplateCatalog.java")
    project_format_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectFormatPolicy.java")
    project_archive = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectArchive.java")
    image_assets = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopImageAssets.java")
    accessibility_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAccessibilityPolicy.java")
    adaptive_layout = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAdaptiveLayout.java")
    paint_view = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopPaintView.java")
    audio_assets = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAudioAssets.java")
    asset_manifest = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetManifest.java")
    asset_identity = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetIdentity.java")
    money = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopMoney.java")
    ai_pricing = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiPricing.java")
    assert "MediaExtractor" in audio_assets
    assert "MediaFormat.KEY_SAMPLE_RATE" in audio_assets
    assert "MediaFormat.KEY_CHANNEL_COUNT" in audio_assets
    assert "audio stream metadata exceeds supported bounds" in audio_assets
    assert "asset.sampleRate" in activity
    assert "asset.channels" in activity
    assert 'RELATIVE_PATH = "assets/manifest.json"' in asset_manifest
    assert 'put("schema", "stasis-assets")' in asset_manifest
    assert 'put("version", 1)' in asset_manifest
    assert 'MessageDigest.getInstance("SHA-256")' in asset_manifest
    assert "StandardCopyOption.ATOMIC_MOVE" in asset_manifest
    assert "validateStableHandles" in asset_manifest
    assert "seedMissing" in asset_manifest
    assert "WorkshopAssetIdentity.stableHandle" in asset_manifest
    assert "0x811c9dc5" in asset_identity
    assert "0x01000193" in asset_identity
    assert "WorkshopMoney.formatUsd(costUsd)" in activity
    assert "setScale(2, RoundingMode.HALF_UP)" in money
    assert "Math.max(0.0, value)" in money
    assert "WorkshopAssetManifest.putSprite" in image_assets
    assert "WorkshopAssetManifest.putAudio" in audio_assets
    assert "WorkshopAssetManifest.readForSync" in activity
    assert "files.put(WorkshopAssetManifest.RELATIVE_PATH" in activity
    assert "WorkshopAssetManifest.remove" in image_assets
    assert "WorkshopAssetManifest.remove" in audio_assets
    assert "target.renameTo(asset.file)" in image_assets
    assert "target.renameTo(asset.file)" in audio_assets
    assert "target.renameTo(latest)" in image_assets
    assert "target.renameTo(latest)" in audio_assets
    support_bundle = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSupportBundle.java")
    crash_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidCrashStore.java")
    ai_queue = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiQueue.java")
    ai_transaction = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiTransactionStore.java")
    ai_checkpoint = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiSessionCheckpointStore.java")
    ai_resume_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiResumePolicy.java")
    verification_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiVerificationPolicy.java")
    verification_runner = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiVerificationRunner.java")
    temporary_verification = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiTemporaryVerification.java")
    project_transaction = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiProjectTransaction.java")
    ai_queue_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AiQueuePolicy.java")
    ai_queue_run_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAiQueueRunPolicy.java")
    host_agent = read("tools/android_ai_agent_host.py")
    host_comparison = read("tools/run_android_ai_model_comparison.py")
    host_compile = read("crates/stasis_android_bridge/src/android_workshop_compile.rs")
    assert "System.loadLibrary(\"stasis_mobile_smoke\")" in activity
    assert "protected void onPause()" in activity
    assert "protected void onSaveInstanceState(Bundle outState)" in activity
    assert 'outState.putString("ai_prompt"' in activity
    assert 'outState.putString("selected_file"' in activity
    assert 'outState.putStringArrayList("selected_image_paths"' in activity
    assert 'outState.putInt("editor_scroll_y"' in activity
    assert "restoreWorkshopUiState(savedInstanceState)" in activity
    assert 'state.getBoolean("editor_open"' in activity
    assert 'state.getBoolean("privacy_open"' in activity
    assert 'state.getStringArrayList("selected_image_paths")' in activity
    assert "clearPendingPreviewCapture();" in activity
    assert "allowAiImageGeneration.setChecked(false)" in activity
    assert "persistPendingDraft();" in activity
    assert "restorePendingDraft();" in activity
    assert "Recovered unsaved source draft after app interruption" in activity
    assert "source changed; recovery will not overwrite newer code" in activity
    assert "App stopped before AI completion" in activity
    assert '"interrupted"' in activity
    assert 'ROOT = "workshop_drafts"' in draft_store
    assert "MAX_DRAFT_BYTES = 2 * 1024 * 1024" in draft_store
    assert 'put("base_sha256", sha256(baseSource))' in draft_store
    assert "StandardCopyOption.ATOMIC_MOVE" in draft_store
    assert "StandardCopyOption.REPLACE_EXISTING" in draft_store
    assert "clearIfMatches" in draft_store
    assert "draft path escaped root" in draft_store
    assert "Intent.ACTION_OPEN_DOCUMENT" in activity
    assert 'intent.setType("image/*")' in activity
    assert "WorkshopImageAssets.importImage" in activity
    assert "WorkshopImageAssets.decodePreview" in activity
    assert "WorkshopImageAssets.readForSync" in activity
    assert "WorkshopImageAssets.rename" in activity
    assert "WorkshopImageAssets.moveToTrash" in activity
    assert "WorkshopImageAssets.restoreLatest" in activity
    assert "Rename blocked: image is referenced by" in activity
    assert "Delete blocked: image is referenced by" in activity
    assert "selectedImageAssetProjectId" in activity
    assert "Map<String, byte[]> githubBackupFiles()" in activity
    assert "MAX_GITHUB_BACKUP_BYTES = 32 * 1024 * 1024" in activity
    assert "project exceeds the 32 MiB direct backup limit" in activity
    assert "Image Assets" in activity
    assert "MAX_IMPORT_BYTES = 8 * 1024 * 1024" in image_assets
    assert "MAX_DIMENSION = 4096" in image_assets
    assert "MAX_PIXELS = 16_000_000L" in image_assets
    assert "exceeds the image sync limit" in image_assets
    assert "MAX_TRASH_FILES = 20" in image_assets
    assert 'TRASH_DIRECTORY = ".stasis-trash/images"' in image_assets
    assert "output.getFD().sync()" in image_assets
    assert "image path escapes the active project" in image_assets
    assert '"image/png"' in image_assets
    assert '"image/jpeg"' in image_assets
    assert '"image/webp"' in image_assets
    assert "New Painted Image" in activity
    assert "Paint as Copy" in activity
    assert "Paint cancelled; project assets unchanged" in activity
    assert "Resize / Crop Canvas" in activity
    assert "WorkshopImageAssets.savePainted" in activity
    assert "MAX_CANVAS_DIMENSION = 1024" in paint_view
    assert "MAX_HISTORY = 8" in paint_view
    assert "PorterDuff.Mode.CLEAR" in paint_view
    assert "void undo()" in paint_view
    assert "void redo()" in paint_view
    assert "void resizeCanvas" in paint_view
    assert "Bitmap snapshot()" in paint_view
    assert "private static native String nativeStatus()" in activity
    assert "private static native String nativeCompileProject(String projectRoot)" in activity
    assert "compile_android_workshop_project" in host_compile
    assert "run_compile_check(project" in host_agent
    assert "build_followup_request(shared_context" in host_agent
    assert "source_file_path(project" in host_agent
    assert "MAX_TOOL_CALLS_PER_BATCH = 50" in host_agent
    assert 'DEFAULT_MODELS = ("gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna")' in host_comparison
    assert "private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight)" in activity
    assert "static native int nativeRunFrameInto(String projectRoot" in activity
    assert "static native String nativeFrameAbiDescriptor()" in activity
    assert "ByteBuffer frameI32" in activity
    assert "ByteBuffer frameF32" in activity
    assert "ByteBuffer frameU8" in activity
    assert 'assetRoot = "workshop_sample/"' not in template_catalog
    assert '"workshop_sample/"' in template_catalog
    assert "activeWorkshopTemplate" in activity
    assert "createWorkshopView" in activity
    assert "installGameStatusOverlay(root, true)" in activity
    assert "BuildConfig.STASIS_PUBLISHED_BUILD" not in activity
    assert "toggleBenchmarkHudFromPreview" in activity
    assert "MotionEvent.ACTION_POINTER_DOWN" in activity
    assert "event.getPointerCount() >= 3" in activity
    assert "startGameLoop();" in activity
    assert "GamePreviewView" in activity
    assert "GLSurfaceView" in activity
    assert "new StasisPreviewRenderer(" in activity
    assert "onDrawFrame" in preview_renderer
    assert "drawFrame" in preview_renderer
    draw_loop = preview_renderer.split("private void drawFrame()", 1)[1].split(
        "\n    }\n", 1
    )[0]
    assert "new " not in draw_loop
    assert "allocate" not in draw_loop
    assert "drawLines" in draw_loop
    assert "drawSprites" in draw_loop
    assert "drawText" in draw_loop
    assert "GLES20.glBlendFunc(GLES20.GL_SRC_ALPHA, GLES20.GL_ONE_MINUS_SRC_ALPHA)" in preview_renderer
    assert "GLES20.glDisable(GLES20.GL_SCISSOR_TEST)" in preview_renderer
    assert "WorkshopTextureProvider" in activity
    assert "implements StasisPreviewRenderer.TextureProvider" in workshop_textures
    assert "SparseArray<SpriteTexture>" in workshop_textures
    assert "projectChanged(projectRootPath, currentProjectRoot)" in workshop_textures
    assert "clearTextures();" in workshop_textures
    assert "private final int[] deletedTexture = new int[1]" in workshop_textures
    assert "extractIntField" in activity
    assert "private final int[] nativeFrameValues = new int[RENDER_FRAME_HEADER_SIZE]" in activity
    assert "WorkshopFrameBudget.percent" in activity
    assert "budget tick=--% render=--% sync=--% total=--%" in activity
    assert "debugColorForBudget" in activity
    assert "appendExplorationProgress(debugTextBuilder)" in activity
    assert '"GameState.collected_count"' in activity
    assert '"keepsakes="' in activity
    assert "garden complete" in activity
    assert "String projectRootPath" in activity
    assert "nativeCompileProject(projectRootPath())" in activity
    assert "String.format" not in activity
    assert "gamePreview.runNativeFrame(" in activity
    assert "RenderFrame.fromNativeFrame" not in activity
    assert "new RenderCommand" not in activity
    assert "ProjectSnapshot.from" in activity
    assert "createAiControls" in activity
    assert "createAiProgressPill" in activity
    assert "postAiProgress" in activity
    assert "actions " in activity
    assert "WorkshopAiRunPhase.EDITING.wireValue()" in activity
    assert "time 0.0s" in activity
    assert "hot swap=FastReload" in activity
    assert "aiReloadPhase" in activity
    assert "WorkshopAiCompletionStatus.afterEdits(aiReloadPhase(compileResult))" in activity
    assert "WorkshopAiCompletionStatus.canFinalizeTestedWrites" in activity
    assert "auto_finalize_tested_writes" in activity
    assert 'followup.put("original_request", new JSONObject(initialRequestJson))' in activity
    assert 'appendAiTrace("prompt_cache_context"' in activity
    assert 'put("approx_cacheable_tokens", (cacheableChars + 3) / 4)' in activity
    assert 'response.optBoolean("applied_tool_writes", false)' in activity
    assert "Context & Images" in activity
    assert 'aiPatch.setText("Run")' in activity
    assert 'aiCancelButton.setText("Stop")' in activity
    assert "AI Work Queue" in activity
    assert 'runAiPatch("voice", null)' in activity
    assert "cancelPendingAiItem" in activity
    assert "startNextQueuedAiIfIdle" in activity
    assert 'ROOT = "workshop_ai_queue"' in ai_queue
    assert 'static final String PENDING = "pending"' in ai_queue
    assert 'static final String IN_PROGRESS = "in_progress"' in ai_queue
    assert "writeSyncedAtomic" not in ai_queue or "getFD().sync()" in ai_queue
    assert "StandardCopyOption.ATOMIC_MOVE" in ai_queue
    assert "recoverInterrupted" in ai_queue
    assert "restored AI cancellation could not be recorded" in activity
    assert "clearTerminalAiRecoveryArtifacts" in activity
    assert "retryTerminal" in ai_queue
    assert 'put("phase", phase)' in ai_queue
    assert "AndroidAiQueue.updatePhase" in activity
    assert "AndroidAiTransactionStore.save" in activity
    assert "restoreInterruptedAiTransactions" in activity
    assert "WorkshopAiProjectTransaction.restore" in activity
    assert "independent_test_author" in activity
    assert "verifierCallCount >= 2" in activity
    assert "verification_repair_requested" in activity
    assert "generated_test_audit" in activity
    assert "WorkshopAiObservationCompactor.compactSuccessfulWrite" in activity
    assert 'ROOT = "workshop_ai_transactions"' in ai_transaction
    assert "StandardCopyOption.ATOMIC_MOVE" in ai_transaction
    assert "requiresIndependentReview" in verification_policy
    assert "independent verification is required" in verification_runner
    assert "temporary verification test cleanup failed" in temporary_verification
    assert "failed to remove provisional file" in project_transaction
    assert "cancelPending" in ai_queue
    assert "AiQueuePolicy.canTransition" in ai_queue
    assert "loadPreview" in ai_queue
    assert 'MAX_PREVIEW_BYTES = 12 * 1024 * 1024' in ai_queue
    assert 'MessageDigest.getInstance("SHA-256")' in ai_queue
    assert "writeSyncedAtomic(savedPreview, previewPng)" in ai_queue
    assert "deletePreview(filesDir, entry)" in ai_queue
    assert "pruneOrphanPreviews" in ai_queue
    assert "removeOldestTerminal" in ai_queue
    assert "AiQueuePolicy.terminal(state)" in ai_queue
    assert 'retry.setText("Fresh Retry")' in activity
    assert "retryTerminalAiItem" in activity
    assert "AndroidAiSessionCheckpointStore.save" in activity
    assert "WorkshopAiResumePolicy.PROVIDER_IN_FLIGHT" in activity
    assert "if (checkpoint == null && snapshot != null)" not in activity
    assert "if (snapshot != null) {\n                WorkshopAiProjectTransaction.restore(projectRoot(), snapshot);" in activity
    assert 'put("usage", usage.checkpointJson())' in activity
    assert 'ROOT = "workshop_ai_sessions"' in ai_checkpoint
    assert "MAX_BYTES = 8 * 1024 * 1024" in ai_checkpoint
    assert "StandardCopyOption.ATOMIC_MOVE" in ai_checkpoint
    assert "paid provider call may have completed" in ai_resume_policy
    assert "will not be replayed" in ai_resume_policy
    assert "encodeBitmapPng" in activity
    assert "queuedEntry.previewFile" in activity
    assert "nextPendingIndex" in ai_queue_policy
    assert "WorkshopAiQueueRunPolicy.decide" in activity
    assert "WAIT_FOR_NETWORK" in ai_queue_run_policy
    assert "CLAIM_NEXT" in ai_queue_run_policy
    assert "AI Settings" in activity
    assert "aiSettingsBody.setVisibility(View.GONE)" in activity
    assert "AI run started: preparing workspace and command context" in activity
    assert "AI run needs both a request and an API key" in activity
    assert "Recent Commands" in activity
    assert "Clear Commands + Outcomes" in activity
    assert "MAX_COMMAND_HISTORY = 20" in activity
    assert "recordCommandHistory(prompt)" in activity
    assert "Command and AI outcome history cleared for this project" in activity
    assert "AI_PREF_OUTCOME_HISTORY_PREFIX" in activity
    assert "AI outcomes:" in activity
    assert "Retry Last AI" in activity
    assert "recordAiOutcome" in activity
    assert 'put("trace_path", aiTraceLogPath())' in activity
    assert "updated.length() < MAX_COMMAND_HISTORY" in activity
    assert '"started".equals(prior.optString("status", ""))' in activity
    assert "Retrying last AI request as a new budget-checked run" in activity
    assert '"Cancelled by user; project restored"' in activity
    assert 'finalTransactionRestoreError.isEmpty() ? "cancelled" : "rollback_failed"' in activity
    assert 'recordAiOutcome(activeAiPrompt, "applied"' in activity
    assert 'recordAiOutcome(activeAiPrompt, "rolled_back"' in activity
    assert "Device monthly AI limit USD" in activity
    assert "AI budget:" in activity
    assert "AI run blocked by the device monthly spending limit" in activity
    assert "Device monthly AI spending limit reached before agent turn" in activity
    assert "recordMonthlyAiSpend" in activity
    assert "maxOutputTokensForBudget" in activity
    assert "MAX_AI_IMAGE_ATTACHMENTS = 4" in activity
    assert "MAX_AI_IMAGE_ATTACHMENT_BYTES = 12 * 1024 * 1024" in activity
    assert "Review AI Image Attachments" in activity
    assert "Only these app-private project images" in activity
    assert 'put("type", "input_image")' in activity
    assert 'put("detail", "original")' in activity
    assert '"data:" + attachment.mimeType + ";base64,"' in activity
    assert "buildAiOpenAiInput(requestJson, false, pricing.explicitCacheBreakpoints)" in activity
    assert "buildAiOpenAiInput(requestJson, true, pricing.explicitCacheBreakpoints)" in activity
    assert "selected_images_are_explicit_project_assets_only" in activity
    assert "activeAiImageAttachments = Collections.emptyList()" in activity
    assert "Capture Preview for AI" in activity
    assert "Review Preview Capture" in activity
    assert "Attach these rendered pixels" in activity
    assert "Attach logical render/runtime/input snapshot" in activity
    assert "Nothing is sent until selected here and Queue AI Change is pressed" in activity
    assert "GLES20.glReadPixels" in preview_renderer
    assert "capturedFrame = capture == null ? null : captureLogicalFrame()" in preview_renderer
    assert "lastDrawnFrame" not in preview_renderer
    assert "MAX_CAPTURE_PIXELS = 8_000_000" in preview_renderer
    assert "preview framebuffer exceeds the 8 megapixel capture limit" in preview_renderer
    assert "Bitmap.createScaledBitmap" in preview_renderer
    assert "selected_preview_logical_snapshot" in activity
    assert '"captured-preview.png"' in activity
    assert "clearPendingPreviewCapture();" in activity
    assert "GPT_IMAGE_2_LOW_1024_USD = 0.006" in activity
    assert "Allow one low-quality 1024x1024 AI image" in activity
    assert 'put("type", "image_generation")' in activity
    assert 'put("quality", "low")' in activity
    assert 'put("size", "1024x1024")' in activity
    assert 'put("output_format", "png")' in activity
    assert "allowImageGeneration && turn == 0" in activity
    assert "extractAiGeneratedImages" in activity
    assert "AI generated image is not a bounded PNG" in activity
    assert "Review AI Image" in activity
    assert "Accept as New Asset" in activity
    assert "AI image rejected; project assets unchanged" in activity
    assert "WorkshopImageAssets.saveGeneratedPng" in activity
    assert "image_generation_cost_usd" in activity
    assert "active project changed before image acceptance" in activity
    assert 'File.createTempFile(".ai-review-"' in image_assets
    assert "could not publish accepted AI image" in image_assets
    assert 'intent.setType("audio/*")' in activity
    assert "WorkshopAudioAssets.importAudio" in activity
    assert "Audio Assets" in activity
    assert "Stop Audio Preview" in activity
    assert "MediaPlayer player = new MediaPlayer()" in activity
    assert "Audio rename blocked: referenced by" in activity
    assert "Audio delete blocked: referenced by" in activity
    assert "WorkshopAudioAssets.readForSync" in activity
    assert "MAX_AUDIO_BYTES = 16 * 1024 * 1024" in audio_assets
    assert "MAX_DURATION_MS = 5L * 60L * 1000L" in audio_assets
    assert "AUDIO_RECORD_PERMISSION_REQUEST = 42" in activity
    assert "Recording name (saved as M4A)" in activity
    assert "Record Audio" in activity
    assert "Stop & Save" in activity
    assert "Cancel Recording" in activity
    assert "MediaRecorder.AudioSource.MIC" in activity
    assert "MediaRecorder.OutputFormat.MPEG_4" in activity
    assert "MediaRecorder.AudioEncoder.AAC" in activity
    assert "setMaxDuration((int)WorkshopAudioAssets.MAX_DURATION_MS)" in activity
    assert "setMaxFileSize(WorkshopAudioAssets.MAX_AUDIO_BYTES)" in activity
    assert "WorkshopAudioAssets.publishRecording" in activity
    assert "cancelAudioRecording(false)" in activity
    assert "Finish or cancel audio recording before running AI" in activity
    assert "createRecordingFile" in audio_assets
    assert "publishRecording" in audio_assets
    assert "discardRecording" in audio_assets
    assert "could not publish recorded audio" in audio_assets
    assert "MediaMetadataRetriever" in audio_assets
    assert "audio path escapes the active project" in audio_assets
    assert "output.getFD().sync()" in audio_assets
    assert 'TRASH_DIRECTORY = ".stasis-trash/audio"' in audio_assets
    assert "Privacy & Data" in activity
    assert "On-device by default: project code, assets, drafts, recovery, and traces" in activity
    assert "only media explicitly selected in review" in activity
    assert "Revoke OpenAI API Key" in activity
    assert "Revoke GitHub Token" in activity
    assert "writeSecretPreference(preferences, AI_PREF_API_KEY, \"\")" in activity
    assert "writeSecretPreference(preferences, GITHUB_PREF_TOKEN, \"\")" in activity
    assert "Clear Pending Media Consent" in activity
    assert "selectedImageAssets.clear()" in activity
    assert "Erase AI Histories + Trace" in activity
    assert "preferences.getAll().keySet()" in activity
    assert "aiTraceLogFile()" in activity
    assert "Code and assets remain" in activity
    assert "Delete Active Non-Bundled Project" in activity
    assert "Bundled Workshop cannot be deleted" in activity
    assert "confirmation name did not match exactly" in activity
    assert "Export a project archive first" in activity
    assert "WorkshopProjectRegistry.deleteProject" in activity
    assert "AndroidDraftStore.clear(this, target.id)" in activity
    assert "AndroidEditRecoveryStore.clearProject(this, target.id)" in activity
    assert "clearDeletedProjectPreferences" in activity
    assert "Bundled Workshop is active" in activity
    assert "bundled project cannot be deleted" in project_registry
    assert "switch away from a project before deleting it" in project_registry
    assert "project directory deletion did not complete" in project_registry
    assert "clearProject" in recovery_store
    assert 'ONBOARDING_PREFS = "onboarding_settings"' in activity
    assert 'VERSION = "manual_tutorial_version"' in onboarding_store
    assert 'COMPLETED_STEPS = "manual_tutorial_completed_steps"' in onboarding_store
    assert 'PROJECT_ID = "manual_tutorial_project_id"' in onboarding_store
    assert 'CHANGE_HASH = "manual_tutorial_change_hash"' in onboarding_store
    assert "WorkshopOnboardingStore.load" in activity
    assert "WorkshopOnboardingStore.save" in activity
    assert "showFirstRunAiSetup" not in activity
    assert "showOnboardingGuide(true)" in activity
    assert "recordOnboardingProjectOpened" in activity
    assert "recordOnboardingProjectStep(WorkshopOnboardingPolicy.Step.PROJECT_RAN)" in activity
    assert "recordOnboardingChangeApplied(refreshedSymbol, refreshedSymbol.source)" in activity
    assert 'result.optBoolean("all_runnable_tests_passed", false)' in activity
    assert "WorkshopOnboardingPolicy.Step.TESTS_PASSED, loadBundledProject()" in activity
    assert "recordOnboardingTrackedChangeStep" in activity
    assert "recordOnboardingRevert" in activity
    assert ".putInt(COMPLETED_STEPS, progress.completedSteps)" in onboarding_store
    assert ".commit();" in onboarding_store
    assert "CURRENT_VERSION = 2" in onboarding_policy
    assert "PROJECT_OPENED" in onboarding_policy
    assert "PROJECT_RAN" in onboarding_policy
    assert "CHANGE_APPLIED" in onboarding_policy
    assert "TESTS_PASSED" in onboarding_policy
    assert "CHANGES_REVIEWED" in onboarding_policy
    assert "CHANGE_REVERTED" in onboarding_policy
    assert "Welcome to Stasis Workshop" in activity
    assert "Remind Me Later" in activity
    assert "Help & Onboarding" in activity
    assert "Resume Zero-AI Manual Tutorial" in activity
    assert "Restart Manual Tutorial" in activity
    assert "previous project/change context cleared" in activity
    assert "no API key is required" in activity
    assert "permissions are requested only when you start" in onboarding_policy
    assert "toggleEditorPanel();" in activity
    assert "Interactive Stasis game preview" in activity
    assert "Open Workshop menu" in activity
    assert "Close Workshop menu" in activity
    assert "Start voice command recording" in activity
    assert "Stasis source editor for the selected symbol" in activity
    assert "Queue the requested AI change with current reviewed attachments" in activity
    assert "Cancel the active AI run after its current atomic operation" in activity
    assert "ACCESSIBILITY_LIVE_REGION_POLITE" in activity
    assert "ACCESSIBILITY_LIVE_REGION_ASSERTIVE" in activity
    assert "setAccessibilityHeading(true)" in activity
    assert "chainAccessibilityTraversal" in activity
    assert "setAccessibilityTraversalAfter" in activity
    assert "setNextFocusForwardId" in activity
    assert "setAccessibilityPaneTitle" in activity
    assert "SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR" in activity
    assert 'announceForAccessibility("Workshop menu opened")' in activity
    assert "createFocusableControlBackground" in activity
    assert "setPreviewCovered(coverPreview)" in activity
    assert "gamePreview.setFocusable(false)" in activity
    assert "gamePreview.setFocusable(previewFocusableWhenUncovered)" in activity
    assert "voiceActionRow, layout" in activity
    assert "configuration.fontScale" in activity
    assert "MEDIUM_WIDTH_DP = 600" in adaptive_layout
    assert "EXPANDED_WIDTH_DP = 840" in adaptive_layout
    assert "LARGE_TEXT_SCALE = 1.3f" in adaptive_layout
    assert "contrastRatio" in accessibility_policy
    assert "Selected image asset" in activity
    assert "Audio asset " in activity
    assert "use arrow keys to move the paint cursor" in paint_view
    assert "R.id.paint_at_cursor" in paint_view
    assert "onKeyDown" in paint_view
    assert "performAccessibilityAction" in paint_view
    assert "setFocusableInTouchMode(true)" in paint_view
    assert "isAccessibilityFocused()" in paint_view
    assert "RetainedPaintSession" in activity
    assert "onRetainNonConfigurationInstance" in activity
    assert "Brush size " in activity
    assert "paint color selected" in activity
    assert "Canvas width in pixels" in activity
    assert "Canvas height in pixels" in activity
    assert 'outState.putBoolean("diagnostics_open"' in activity
    assert 'outState.putBoolean("context_open"' in activity
    assert 'outState.putBoolean("more_tools_open"' in activity
    assert "Export Redacted Support Bundle" in activity
    assert "Excludes credentials, source, prompts" in activity
    assert 'intent.setType("application/json")' in activity
    assert "AndroidSupportBundle.build" in activity
    assert "Redacted support bundle exported without credentials, source, prompts, or media" in activity
    assert '"stasis-android-redacted-support-v1"' in support_bundle
    assert '"credentials_excluded"' in support_bundle
    assert '"source_and_prompts_excluded"' in support_bundle
    assert '"media_bytes_and_names_excluded"' in support_bundle
    assert '"absolute_paths_excluded"' in support_bundle
    assert "MAX_TRACE_READ_BYTES = 512 * 1024" in support_bundle
    assert "MAX_TRACE_EVENTS = 50" in support_bundle
    assert "redacted support bundle exceeds 1 MiB" in support_bundle
    assert 'outcome.optString("status"' in support_bundle
    assert 'entry.optString("event"' in support_bundle
    assert "draft_source" not in support_bundle
    assert "before_source" not in support_bundle
    assert "api_key" not in support_bundle
    assert "AndroidCrashStore.install(this)" in activity
    assert "Previous crash detected" in activity
    assert "Clear Local Crash Record" in activity
    assert "AndroidCrashStore.clear" in activity
    assert "Restart loop detected" in activity
    assert "restartLoopRecoveryActive" in activity
    assert "AndroidCrashStore.markLaunchStable" in activity
    assert 'FILE_NAME = "android_crash_redacted.json"' in crash_store
    assert "MAX_FRAMES = 30" in crash_store
    assert "MAX_RECORD_BYTES = 64 * 1024" in crash_store
    assert "Thread.setDefaultUncaughtExceptionHandler" in crash_store
    assert "prior.uncaughtException(thread, error)" in crash_store
    assert '"message_excluded"' in crash_store
    assert '"paths_and_source_excluded"' in crash_store
    assert "getClassName()" in crash_store
    assert "getMethodName()" in crash_store
    assert "getMessage()" not in crash_store
    assert "output.getFD().sync()" in crash_store
    assert '"previous_crash"' in support_bundle
    assert "max_output_tokens" in activity
    assert "Device monthly AI spending limit reached" in activity
    assert 'aiCancelButton.setText("Stop")' in activity
    assert "aiRunActive" in activity
    assert "aiCancelRequested" in activity
    assert "AI request queued behind the active item" in activity
    assert "registerDefaultNetworkCallback" in activity
    assert "unregisterNetworkCallback" in activity
    assert "AI work is waiting for an internet connection" in activity
    assert "WorkshopBackgroundWorkPolicy.decide" in activity
    assert "WorkshopLongWorkCoordinator.beginAi" in activity
    assert "WorkshopLongWorkCoordinator.finishAi" in activity
    assert "WorkshopLongWorkCoordinator.isAiActive" in activity
    assert "WorkshopLongWorkCoordinator.beginGitHub" in activity
    assert "WorkshopLongWorkCoordinator.finishGitHub" in activity
    assert "WorkshopLongWorkCoordinator.beginProjectIo" in activity
    assert "WorkshopLongWorkCoordinator.finishProjectIo" in activity
    assert "throwIfAiCancelled()" in activity
    assert "if (!batchHasWrites) throwIfAiCancelled()" in activity
    assert "finishing any active call or atomic write batch" in activity
    assert "activeAiConnection" in activity
    assert "nativeCodexBeginResponse()" in activity
    assert "nativeCodexCancelResponse()" in activity
    assert "AI_CONNECT_TIMEOUT_MS" in activity
    assert "AI_READ_TIMEOUT_MS" in activity
    assert "completed calls remain in usage totals" in activity
    assert "installVoiceChangeControls(root)" in activity
    assert "VOICE_TOP_MARGIN_DP = 64" in activity
    assert "VOICE_ACTION_TOP_MARGIN_DP = 120" in activity
    assert "voiceParams.setMargins(0, dp(VOICE_TOP_MARGIN_DP), dp(TOP_CONTROL_END_MARGIN_DP), 0)" in activity
    assert "toggleParams.setMargins(0, dp(8), dp(TOP_CONTROL_END_MARGIN_DP), 0)" in activity
    assert "SpeechRecognizer.createSpeechRecognizer(this)" in activity
    assert "VOICE_RECORD_PERMISSION_REQUEST" in activity
    assert "voiceCancel.setText(\"Cancel\")" in activity
    assert "voiceRunButton.setText(\"Run\")" in activity
    assert "Voice change confirmed: adding it to the AI queue" in activity
    assert "Voice change cancelled" in activity
    assert "GITHUB_PREF_TOKEN" in activity
    assert "GitHub Sync Settings" in activity
    assert "githubSettingsBody.setVisibility(View.GONE)" in activity
    assert "GitHub sync: not configured" in activity
    assert "GitHub sync: ready for " in activity
    assert "Save GitHub Sync Settings" in activity
    assert "Sync GitHub Now" in activity
    assert "queueGitHubSync" in activity
    assert "applyFileChange" in github_api
    assert "WorkshopGitHubSyncPolicy.backupPlan" in activity
    assert 'writeJson("DELETE"' in github_api
    assert "changedTextFiles" in github_sync_policy
    assert "priorRemoteState" in github_sync_policy
    assert "Automatically back up validated project changes" in activity
    assert "automaticSchedule" in activity
    assert "automatic backup deferred by battery saver" in activity
    assert "automatic backup waiting for a usable network" in activity
    assert "validateTarget" in github_api
    assert "GITHUB_PREF_VALIDATED_TARGET" in activity
    assert "githubTargetValidated" in activity
    assert "authenticated target ready" in activity
    assert "changed remotely since the last backup" in github_sync_policy
    assert "GitHub sync: queued" in activity
    assert "postGitHubOperationFailure" in activity
    assert "Review GitHub Changes" in activity
    assert "Create / Update Pull Request" in activity
    assert "reviewedGitHubChangeFingerprint" in activity
    assert 'MessageDigest.getInstance("SHA-256")' in github_sync_policy
    assert "NETWORK_TIMEOUT_MS" in github_api
    assert "GitHub pull request: review current changes first" in activity
    assert "ensureReviewBranch" in github_api
    assert "stasis-workshop-" in activity
    assert "createOrFindPullRequest" in github_api
    assert 'apiUrl("/git/refs")' in github_api
    assert 'apiUrl("/pulls"' in github_api
    assert "GitHub pull request: ready " in activity
    assert "AndroidSecretStore.readAndMigrate" in activity
    assert "AndroidSecretStore.write" in activity
    assert "TYPE_TEXT_VARIATION_PASSWORD" in activity
    assert "getString(GITHUB_PREF_TOKEN" not in activity
    assert "putString(GITHUB_PREF_TOKEN" not in activity
    assert "getString(AI_PREF_API_KEY" not in activity
    assert "putString(AI_PREF_API_KEY" not in activity
    assert 'KeyStore.getInstance("AndroidKeyStore")' in secret_store
    assert 'Cipher.getInstance("AES/GCM/NoPadding")' in secret_store
    assert "KeyGenParameterSpec.Builder" in secret_store
    assert "editor.putString(ENCRYPTED_PREFIX + key" in secret_store
    assert "preferences.getString(key" in secret_store
    assert "preferences.edit().remove(key)" in secret_store
    assert "if (!editor.commit())" in secret_store
    assert "Executors.newSingleThreadExecutor()" in activity
    assert "githubSyncExecutor.submit" in activity
    assert "githubSyncExecutor.shutdownNow()" in activity
    assert "isGitHubOperationActive" in activity
    assert "beginGitHubOperation" in activity
    assert "another operation is already queued or running" in activity
    assert "GITHUB_PREF_OPERATION_STATE" in activity
    assert "GITHUB_PREF_OPERATION_DETAIL" in activity
    assert "GITHUB_PREF_OPERATION_AUTOMATIC" in activity
    assert "NetworkResumeDecision.RECHECK_AUTOMATIC_SYNC" in activity
    assert 'queueGitHubSync(!detail.contains("automatic"))' not in activity
    assert "persistGitHubSyncOrigin" not in activity
    assert activity.count("persistGitHubSyncOperationState(") == 6
    assert "GITHUB_PREF_OPERATION_AUTOMATIC), false" in activity
    assert 'automatic backup waiting for a usable network", true' in activity
    assert 'automatic backup deferred by battery saver", true' in activity
    assert "GITHUB_PREF_REVIEW_FINGERPRINT" in activity
    assert "Retry GitHub Operation" in activity
    assert "persistGitHubOperationState" in activity
    assert '"interrupted", "app stopped before completion"' in activity
    assert "continues in background" in activity
    assert '"waiting_network".equals(state)' in activity
    assert "resumeGitHubAfterNetworkChange" in activity
    assert "registerPowerMonitoring" in activity
    assert "ACTION_POWER_SAVE_MODE_CHANGED" in activity
    assert '"sync".equals(operation)' in activity
    assert '"pull_request".equals(operation)' in activity
    assert "WorkshopProjectRegistry.initialize(this," in activity
    assert "New Project From Selected Template" in activity
    assert "WorkshopTemplateCatalog.list()" in activity
    assert "templateSelector.getSelectedItem()" in activity
    assert "createFromTemplate" in activity
    assert "Switch Project" in activity
    assert "projectSettingsBody.setVisibility(View.GONE)" in activity
    assert "Project switch blocked while AI, GitHub, or project I/O is active" in activity
    assert "Project creation blocked while AI, GitHub, or project I/O is active" in activity
    assert "WorkshopProjectRegistry.setActive(this, project)" in activity
    assert "nativeCompileProject(projectRootPath())" in activity
    assert "githubProjectPreferenceKey" in activity
    assert "readGitHubProjectPreference" in activity
    assert "hasPendingSourceEdit()" in activity
    assert "Apply or Reset the pending source edit before switching projects" in activity
    assert 'return "stasis-workshop-" + identity' in activity
    assert "WorkshopProjectRegistry.METADATA_FILE.equals(file.getName())" in activity
    assert "FORMAT_VERSION = WorkshopProjectFormatPolicy.CURRENT_VERSION" in project_registry
    assert "CURRENT_VERSION = 3" in project_format_policy
    assert 'return ".stasis-workshop.json.v1.bak"' in project_format_policy
    assert 'return ".stasis-workshop.json.v2.bak"' in project_format_policy
    assert "migrateLegacyMetadata" in project_registry
    assert "WorkshopProjectFormatPolicy.supported(version)" in project_registry
    assert "WorkshopProjectFormatPolicy.templateId" in project_registry
    assert "WorkshopProjectFormatPolicy.backupFileName" in project_registry
    assert 'put("schema", "stasis-workshop-project")' in project_registry
    assert 'put("migrated_from_version", migratedFromVersion)' in project_registry
    assert "update the Workshop before opening this project" in project_registry
    assert "project metadata schema is invalid" in project_registry
    assert '" migration failed; the fsynced v"' in project_registry
    assert "migrated metadata verification failed" in project_registry
    assert "StandardCopyOption.ATOMIC_MOVE" in project_registry
    assert "StandardCopyOption.REPLACE_EXISTING" in project_registry
    assert 'LEGACY_PROJECT_DIR = "workshop_project"' in project_registry
    assert 'METADATA_FILE = ".stasis-workshop.json"' in project_registry
    assert "UUID.randomUUID().toString()" in project_registry
    assert "project root must stay in app-private storage" in project_registry
    assert "project root is outside the registry" in project_registry
    assert "output.getFD().sync()" in project_registry
    assert "active project preference commit failed" in project_registry
    assert "unsupported project format version" in project_registry
    assert '.put("origin", project.origin)' in project_registry
    assert '.put("template_id", project.templateId)' in project_registry
    assert "WorkshopTemplateCatalog.LEGACY_TEMPLATE_ID" in project_format_policy
    assert "WorkshopTemplateCatalog.isKnown(templateId)" in project_registry
    assert 'DEFAULT_TEMPLATE_ID = "exploration"' in template_catalog
    assert 'LEGACY_TEMPLATE_ID = "pong"' in template_catalog
    assert '"exploration_sample/"' in template_catalog
    assert '"Exploration Garden"' in template_catalog
    assert template_catalog.count('"AGENTS.md"') == 2
    assert template_catalog.count('"CLAUDE.md"') == 2
    assert "for (String file : template.auxiliaryFiles)" in activity
    pong_agents = read("mobile/android/app/src/main/assets/workshop_sample/AGENTS.md")
    exploration_agents = read("mobile/android/app/src/main/assets/exploration_sample/AGENTS.md")
    assert pong_agents == exploration_agents
    assert "## Theory-Building Practice" in exploration_agents
    assert "Mapping:" in exploration_agents
    assert "Rationale:" in exploration_agents
    assert "Extension:" in exploration_agents
    assert "Theory gained:" in exploration_agents
    pong_claude = read("mobile/android/app/src/main/assets/workshop_sample/CLAUDE.md")
    exploration_claude = read("mobile/android/app/src/main/assets/exploration_sample/CLAUDE.md")
    assert pong_claude == exploration_claude
    assert exploration_claude.strip() == "# CLAUDE.md\n\n@AGENTS.md"
    exploration_main = read("mobile/android/app/src/main/assets/exploration_sample/src/main.stasis")
    exploration_config = read("mobile/android/app/src/main/assets/exploration_sample/src/config.stasis")
    exploration_components = read("mobile/android/app/src/main/assets/exploration_sample/src/components.stasis")
    exploration_schedule = read("mobile/android/app/src/main/assets/exploration_sample/src/systems/schedule.stasis")
    exploration_inventory = read("mobile/android/app/src/main/assets/exploration_sample/src/systems/inventory.stasis")
    exploration_render = read("mobile/android/app/src/main/assets/exploration_sample/src/systems/render_extract.stasis")
    exploration_assets = read("mobile/android/app/src/main/assets/exploration_sample/src/assets.stasis")
    exploration_manifest = read("mobile/android/app/src/main/assets/exploration_sample/assets/manifest.json")
    exploration_host = read("mobile/android/app/src/main/assets/exploration_sample/src/host_game.stasis")
    exploration_tests = read("mobile/android/app/src/main/assets/exploration_sample/tests/exploration_gameplay.test.stasis")
    assert 'import "systems/schedule.stasis";' in exploration_main
    assert "exploration_input_target_system();" in exploration_schedule
    assert "exploration_movement_system();" in exploration_schedule
    assert "exploration_collection_system();" in exploration_schedule
    assert "exploration_inventory_system();" in exploration_schedule
    assert "exploration_camera_follow_system();" in exploration_schedule
    assert "WORLD_WIDTH: i32 = 720" in exploration_config
    assert "test `camera follow is deterministic and bounded`(): bool" in exploration_tests
    assert "test `spawn capacity rejects player occupied and out of range slots`(): bool" in exploration_tests
    assert "test `overlapping collectibles resolve in ascending entity order`(): bool" in exploration_tests
    assert "last_collected_entity_id" in exploration_components
    assert "entity_alive: i32[8]" in exploration_components
    assert "exploration_audio_collect(kind);" in exploration_inventory
    assert "EXPLORATION_PLAYER_ASSET" in exploration_assets
    assert "World.sprite_handle[0]" in exploration_render
    assert '"id": "player"' in exploration_manifest
    assert "exploration_host_sync_input();" in exploration_host
    assert "test `new touch edge sets one clamped destination`(): bool" in exploration_tests
    assert "assert_runtime" not in exploration_tests
    assert '"sample".equals(origin)' in project_registry
    assert '"import".equals(origin)' in project_registry
    assert "project metadata id is invalid" in project_registry
    assert "originMissing" in project_registry
    assert "StandardCopyOption.ATOMIC_MOVE" in project_registry
    assert "StandardCopyOption.REPLACE_EXISTING" in project_registry
    assert "PROJECT_BASELINES_DIR" in activity
    assert "ensureActiveProjectBaseline" in activity
    assert "loadProjectBaselineSnapshot" in activity
    assert "restoreImportedProjectSourceBaseline" in activity
    assert '"import".equals(activeProject.origin)' in activity
    assert '"sample".equals(activeProject.origin)' in activity
    assert "collectProjectStasisFiles(projectRoot, files, seen)" in activity
    assert "files = githubBackupFiles()" in activity
    assert "sourcesByFile(loadBundledProject()).entrySet()" in activity
    assert "Reverted saved symbol to project baseline" in activity
    assert "Export Project Archive" in activity
    assert "Intent.ACTION_CREATE_DOCUMENT" in activity
    assert 'intent.setType("application/zip")' in activity
    assert "FLAG_GRANT_WRITE_URI_PERMISSION" in activity
    assert "projectIoExecutor.submit" in activity
    assert "projectIoExecutor.shutdownNow()" in activity
    assert "WorkshopProjectArchive.exportProject" in activity
    assert "Apply or Reset the pending source edit before export" in activity
    assert "MAX_FILES = 512" in project_archive
    assert "MAX_ENTRY_BYTES = 32L * 1024L * 1024L" in project_archive
    assert "MAX_TOTAL_BYTES = 128L * 1024L * 1024L" in project_archive
    assert '"build".equals(current.getName())' in project_archive
    assert "entry.setTime(0L)" in project_archive
    assert "project file escaped project root" in project_archive
    assert "Import Project Archive" in activity
    assert "Intent.ACTION_OPEN_DOCUMENT" in activity
    assert "FLAG_GRANT_READ_URI_PERMISSION" in activity
    assert "WorkshopProjectArchive.importProject" in activity
    assert "WorkshopProjectRegistry.createForImport" in activity
    assert "WorkshopProjectRegistry.deleteFailedImport" in activity
    assert "Project import failed and was discarded" in activity
    assert "Apply or Reset the pending source edit before import" in activity
    assert "validateArchivePath" in project_archive
    assert "project archive contains duplicate path" in project_archive
    assert "project archive metadata format is unsupported" in project_archive
    assert '?:1|2|3' in project_archive
    assert "project archive needs src/main.stasis" in project_archive
    assert "output.getFD().sync()" in project_archive
    assert "legacy project cannot be deleted as a failed import" in project_registry
    assert "Manual Symbols and Source" in activity
    assert "Go to Diagnostic" in activity
    assert "captureFirstTestFailureDiagnostic" in activity
    assert "WorkshopSourceDiagnostic.sourceOffset" in activity
    assert 'applySourceDiagnostic(diagnostic, "Test failure")' in activity
    assert 'result.optInt("line", 0)' in activity
    assert "sourceEditor.setSelection(symbolOffset, symbolEnd)" in activity
    assert "Undo Failed Apply" in activity
    assert "Recovery History" in activity
    assert "Failed Apply History" in activity
    assert "Recovery history selection" in activity
    assert "AndroidEditRecoveryStore.record" in activity
    assert "AndroidEditRecoveryStore.latest" in activity
    assert "AndroidEditRecoveryStore.list" in activity
    assert "selectedRecoveryEntry" in activity
    assert "Undo blocked: source changed after the failed apply" in activity
    assert "Failed manual apply restored safely" in activity
    assert "Recoverable failed apply" in activity
    assert "MAX_ENTRIES = 10" in recovery_store
    assert "static Entry[] list" in recovery_store
    assert "MAX_SOURCE_BYTES = 2 * 1024 * 1024" in recovery_store
    assert "writeSyncedAtomic" in recovery_store
    assert "recovery entry publish failed" in recovery_store
    assert "output.getFD().sync()" in recovery_store
    assert "recovery project id is invalid" in recovery_store
    assert "recovery path escaped root" in recovery_store
    assert "manualEditBody.setVisibility(View.GONE)" in activity
    assert "selectedSourcePanel.addView(sourceEditor" in activity
    assert "sameSymbolIdentity(symbol, selectedSymbol)" in activity
    assert "compile.setText(\"Compile\")" not in activity
    assert "https://api.openai.com/v1/responses" in activity
    assert "payload.put(\"text\", buildAiResponseTextFormat())" in activity
    assert "private static final int MAX_AI_AGENT_TURNS = 15" in activity
    assert '.put("response_model", apiResponse.model)' in activity
    assert '.put("elapsed_ms", SystemClock.elapsedRealtime() - llmStartedMs)' in activity
    assert '.put("estimated_cost_usd", !useCodex && usage.lastCallCostAvailable' in activity
    assert 'harmless != null && harmless.length() == 0' in activity
    assert '.put("successful_writes", session.successfulWriteCount)' in activity
    assert "AI_PREF_LAST_USAGE" in activity
    assert "AI_TRACE_LOG" in activity
    assert "appendAiTrace" in activity
    assert "llm_request" in activity
    assert "tool_observations" in activity
    assert "response_contract" in activity
    assert "Return exactly one JSON object matching the stable request response_contract" in activity
    assert "Use tool_calls instead." in activity
    assert "requires imports array, or source/import_source as a compatibility alias" not in activity
    assert "call.optString(\"new_source\", call.optString(\"source\", \"\"))" not in activity
    assert "AI read_symbol target ambiguous: " in activity
    assert "AI read_symbol target not found: " in activity
    assert "AI read_symbol target ambiguous or not found" not in activity
    assert "SOL = gpt56(5.00, 0.50, 6.25, 30.00)" in ai_pricing
    assert "private AiApiResponse callOpenAiResponsesApi" in activity
    assert "extractAiUsage(response)" in activity
    assert "saveLastAiUsage(aiResult.usageJson)" in activity
    assert "usageTokenCount(usage, \"input_tokens\", \"prompt_tokens\")" in activity
    assert "cachedInputTokenCount" in activity
    assert "cacheWriteInputTokenCount" in activity
    assert "estimated_cost_usd" in activity
    assert "estimated cost=" in activity
    assert "aiResult.usageSummary" in activity
    assert "runAiAgentLoop" in activity
    assert "executeAiToolCalls" in activity
    assert "validateAiToolCall" in activity
    assert "read_imports" in activity
    assert "write_imports" in activity
    assert "aiToolReadImports" in activity
    assert "aiToolWriteImports" in activity
    assert "backing_struct_source" in activity
    assert "project_globals" in activity
    assert "backing_struct_type" in activity
    assert "parseGlobal" in activity
    assert "sections.put(\"Globals\"" in activity
    assert "validation_error" in activity
    assert "accepted_shape" in activity
    assert "required_args" in activity
    assert "Tool errors, validation_error observations, and test_observation failures are not final" in activity
    assert "recordAiToolResult" in activity
    assert "agent_turn_limit_after_successful_tested_writes" in activity
    assert "repeated_tool_calls" in activity
    assert "repeated tools" in activity
    assert "successful_writes" in activity
    assert "private final class AiAgentSession" in activity
    assert "ProjectSnapshot cachedProject" in activity
    assert "session.project()" in activity
    assert "session.invalidateProject()" in activity
    assert "available_tools" in activity
    assert "tool_specs" in activity
    assert "Use tool_specs in the request for required_args, optional_args, and examples" in activity
    assert "aiToolSpecs" in activity
    assert "list_tests" in activity
    assert "read_test_file" in activity
    assert "write_test_file" in activity
    assert "run_tests" in activity
    assert "test_observation" in activity
    assert "runAiTestsAfterBatch" in activity
    assert "new_passing_tests" in activity
    assert "nativeRunTests(projectRootPath())" in activity
    assert "runTests.setText(\"Run Tests\")" in activity
    assert "runNativeTests();" in activity
    assert "rawDiffs.setText(\"Raw Diffs\")" in activity
    assert "showRawDiffReview();" in activity
    assert "formatRawFileDiffs" in activity
    assert "appendUnifiedFileDiff" in activity
    assert "splitSourceLines" in activity
    assert "template.testFiles" in activity
    assert "parseTest" in activity
    assert "TestUpdated: run tests to validate" in activity
    assert "sections.put(\"Tests\"" in activity
    assert "readAsset(assets, template.assetRoot + file)" in activity
    assert "newTest.setText(\"New Test\")" in activity
    assert "createManualTest();" in activity
    assert "Created failing test template; edit it, then Run Tests" in activity
    assert "findSymbolByIdentity" in activity
    assert "deleteTest.setText(\"Delete Test\")" in activity
    assert "deleteSelectedManualTest();" in activity
    assert "baseline tests can be reverted, not deleted" in activity
    assert "Deleted user-created test" in activity
    assert "newHelper.setText(\"New Helper\")" in activity
    assert "createManualHelper();" in activity
    assert "function \" + name + \"(): void" in activity
    assert "Created root helper" in activity
    assert "deleteHelper.setText(\"Delete Helper\")" in activity
    assert "deleteSelectedManualHelper();" in activity
    assert "baseline helpers can be reverted, not deleted" in activity
    assert "Deleted user-created root helper" in activity
    assert "revertSaved.setText(\"Revert Saved\")" in activity
    assert "revertSelectedToBundled();" in activity
    assert "Reverted saved symbol to project baseline" in activity
    assert "Revert unavailable: selected symbol is not in the project baseline" in activity
    assert "lastPassingTestKeys" in activity
    assert "list_symbols" in activity
    assert "list_owner_symbols" in activity
    assert "preferredFunctionCall" not in activity
    assert "preferred_call" not in activity
    assert 'request.put("stasis_basics", aiStasisBasics())' in activity
    assert '"kind": "auto_finalize_tested_writes"' in host_agent
    assert 'buildAiOpenAiInput(requestJson, true, false)' in activity
    assert 'AI_PREF_CODEX_FAST_MODE = "codex_fast_mode"' in activity
    assert 'WorkshopCodexServiceTier.requestTier' in activity
    assert 'payload.put("service_tier", serviceTier)' in activity
    assert 'appendAiTrace("codex_request_tier"' in activity
    assert 'payload.put("model", requestedModel.isEmpty() ? DEFAULT_AI_MODEL : requestedModel)' in activity
    assert "installAiGameProgressOverlay(root)" in activity
    assert "WorkshopAiOverlayPolicy.shouldShow" in activity
    assert 'setContentDescription("AI work status; tap to open Workshop")' in activity
    codex_method = activity[activity.index("private AiApiResponse callCodexResponses"):
                            activity.index("private boolean migrateBundledPongBallSpeed")]
    assert 'payload.put("prompt_cache_options"' not in codex_method
    assert "global instance_name: StructType" in activity
    assert "function name(arg_name: Type, other: Type): ReturnType" in activity
    assert "struct TypeName { field_name: Type; ... }" in activity
    assert "bounded text ascii[N] or utf8[N]" in activity
    assert "Gameplay progression is tick-based rather than dt-based" in activity
    assert "read_symbol" in activity
    assert "read_file" in activity
    assert "write_symbol" in activity
    assert "get_diagnostics" in activity
    assert "set_input_state" in activity
    assert "nativeSetRuntimeI32" in activity
    assert "nativeGetRuntimeI32" in activity
    assert "aiToolSetRuntimeI32" in activity
    assert "runtimeI32ResultToJson" in activity
    assert "run_frame" in activity
    assert "inspect_runtime_state" in activity
    assert "aiToolSetInputState" in activity
    assert "nativeRunTests" in activity
    assert "test `name`(): bool" in activity
    assert "read-only inspection batches do not rerun tests" in activity
    assert "Use on_code_swap() only for post-hot-swap migration" in activity
    assert "MAX_AI_TOOL_CALLS_PER_BATCH = 50" in activity
    assert "MAX_AI_READ_ONLY_BATCHES = 2" in activity
    assert "retainedToolObservations" in activity
    assert "read_only_batch_not_executed" in activity
    assert "Never reread a target already present" in activity
    assert 'responseProperties.put("working_notes"' in activity
    assert 'put("maxLength", WorkshopAiWorkingNotes.MAX_CHARS)' in activity
    assert 'request.put("project_symbol_index", aiProjectSymbolIndex(project))' in activity
    assert "WorkshopAiInitialContextPolicy.canAppend" in activity
    assert "project_symbol_index_count" in activity
    assert 'setStatusText("AI working notes: " + display)' in activity
    assert 'appendAiTrace("working_notes"' in activity
    assert "Report decisions and evidence, not private chain-of-thought" in activity
    assert "runtimeStateJson" in activity
    assert "frameValuesToJson" in activity
    assert "aiToolGetDiagnostics" in activity
    assert "compileResultToJson" in activity
    assert "lastCompileResult = compileResult" in activity
    assert "take_screenshot" in activity
    assert "aiToolWriteSymbol" in activity
    assert "writeSymbolTransaction" in activity
    assert "status\", \"rolled_back" in activity
    assert "restored_diagnostics" in activity
    assert "write_symbol creates or replaces a symbol" in activity
    assert "Before writing, inspect the current target" in activity
    assert "game_design_rules" in activity
    assert "prefer_lifecycle_local_state" in activity
    assert "avoid_global_tick_for_per_entity_progression" in activity
    assert "architecture_recommendations" in activity
    assert "Use command/event-style functions" in activity
    assert "durable gameplay concepts" in activity
    assert "spawn_actor" in activity
    assert "Follow architecture_recommendations" in activity
    assert "rendered rectangles as one contract" in activity
    assert "just-inside, exact-boundary, and just-outside" in activity
    assert "Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits" in activity
    assert "Tool errors, validation_error observations, and test_observation failures are not final" in activity
    assert "mode=done" in activity
    assert 'appliedToolWrites ? "applied" : "complete"' in activity
    assert 'appliedToolWrites ? "tested tool writes" : "no actions"' in activity
    assert "rolls back the whole batch and returns diagnostics" in activity
    assert "AI edit apply failed and rolled back" in activity
    assert "appendAiFunction" in activity
    assert "\"created\"" in activity
    assert "logical_render_snapshot" in activity
    assert "take_screenshot returns a compact logical render snapshot" in activity
    assert "json_schema" in activity
    assert "toolArgsSchema" in activity
    assert "toolProperties.put(\"args\", toolArgsSchema)" in activity
    assert "observation.put(\"args\", args)" in activity
    assert "format.put(\"strict\", false)" in activity
    assert "stasis_ai_code_response" in activity
    assert "response.has(\"edits\")" in activity
    assert "part.optString(\"output_text\", \"\")" in activity
    assert "validateAiReplacementSource(kind, target.name, newSource)" in activity
    assert "AI edit must use Stasis syntax, not Rust syntax" in activity
    assert "must contain exactly one top-level" in activity
    assert "body must not contain nested function, struct, or global declarations" in activity
    assert "extractDeclarationName(newSource, \"function\")" in activity
    assert "replace_function" in activity
    assert "replace_struct" in activity
    assert "SharedPreferences" in activity
    assert "AI_PREF_API_KEY" in activity
    assert "readSecretPreference(aiPrefs, AI_PREF_API_KEY)" in activity
    assert "aiPrefs.getString(AI_PREF_MODEL" in activity
    assert 'DEFAULT_AI_MODEL = "gpt-5.6-sol"' in activity
    assert 'reasoningSummary.setText("Reasoning: medium")' in activity
    assert 'put("effort", "medium")' in activity
    assert 'GPT-5.6 Sol defaults to medium reasoning' in activity
    assert '"gpt-5.6-terra".equals(configuredModel)' in activity
    assert "AI_PREF_MODEL_DEFAULT_VERSION" in activity
    assert 'DEFAULT_MODEL = "gpt-5.6-sol"' in host_agent
    assert 'DEFAULT_REASONING_EFFORT = "medium"' in host_agent
    assert "MAX_WORKING_NOTES_CHARS = 2_000" in host_agent
    assert '"required": ["mode", "working_notes"]' in host_agent
    assert '"kind": "working_notes"' in host_agent
    assert '"cache_write": 6.25' in host_agent
    assert "prompt_cache_key" in activity
    assert "prompt_cache_breakpoint" in activity
    assert 'content.put("prompt_cache_breakpoint", new JSONObject().put("mode", "explicit"))' in activity
    assert 'payload.put("prompt_cache_options", new JSONObject().put("mode", "explicit").put("ttl", "30m"))' in activity
    assert 'payload.put("reasoning", new JSONObject().put("effort", pricing.reasoningEffort))' in activity
    assert 'put("type", "prompt_cache_breakpoint")' not in activity
    assert 'payload.put("prompt_cache_retention"' not in activity
    assert '"prompt_cache_options": {"mode": "explicit", "ttl": "30m"}' in host_agent
    assert 'parser.add_argument("--service-tier", choices=("standard", "priority")' in host_agent
    assert 'payload["service_tier"] = "priority"' in host_agent
    assert '"reasoning": {"effort": DEFAULT_REASONING_EFFORT}' in host_agent
    assert 'payload.get("reasoning") != {"effort": "medium"}' in host_agent
    assert '"prompt_cache_breakpoint": {"mode": "explicit"}' in host_agent
    assert '"type": "prompt_cache_breakpoint"' not in host_agent
    assert '"prompt_cache_retention"' not in host_agent
    assert 'DEFAULT_TRACE_DIR = ROOT / "artifacts/android_ai_runs"' in host_agent
    assert 'parser.add_argument("--preflight"' in host_agent
    assert 'trace_file = args.trace_file or default_trace_file()' in host_agent
    assert '"kind": "api_error"' in host_agent
    assert '"kind": "openai_request"' in host_agent
    assert 'def summarize_openai_payload' in host_agent
    assert "saveAiSettings(apiKey, model)" in activity
    assert "private LinearLayout symbolList" in activity
    assert "rebuildSymbolList(refreshedProject)" in activity
    assert "findMatchingSymbol(refreshedProject, editedSymbol)" in activity
    assert "formatChangeSummary" in activity
    assert "Changed symbols:" in activity
    assert "Changed files:" in activity
    assert "parseSymbols" in activity
    assert "groupSymbols" in activity
    assert "createSymbolRow" in activity
    assert "EditText" in activity
    assert "createEditControls" in activity
    assert 'compile.setText("Compile")' not in activity
    assert 'runTick.setText("Run Tick")' in activity
    assert "runNativeCompile" in activity
    assert "runNativeTick" in activity
    assert "nativeRunFrameInto(" in activity
    assert "gamePreview.touchX()" in activity
    assert "gamePreview.touchY()" in activity
    assert "gamePreview.touchActive()" in activity
    assert "MotionEvent" in activity
    assert "LINE_F32_STRIDE = 8" in preview_renderer
    assert "SPRITE_I32_STRIDE = 3" in preview_renderer
    assert "SPRITE_F32_STRIDE = 4" in preview_renderer
    assert "TEXT_I32_STRIDE = 3" in preview_renderer
    assert "drawColorBatch" in preview_renderer
    assert "drawPreparedTextureBatch" in preview_renderer
    assert "frameI32" in preview_renderer
    assert "frameF32" in preview_renderer
    assert "frameU8Bytes" in preview_renderer
    assert "GLES20.glDrawArrays" in preview_renderer
    assert "GL_TRIANGLES" in preview_renderer
    assert "glUniform4f" not in preview_renderer
    assert "attribute vec4 aColor" in preview_renderer
    assert "TEXTURE_FRAGMENT_SHADER" in preview_renderer
    assert "nativeResolveSpriteAsset" in workshop_textures
    assert "nativeDecodeSvgSprite" in workshop_textures
    assert "createFallbackTexture" in workshop_textures
    assert "decoded sprite dimensions do not match the manifest" in workshop_textures
    assert "glTexImage2D" in workshop_textures
    assert "applySelectedEdit" in activity
    assert "persistSelectedEdit" in activity
    assert "getFilesDir()" in activity
    assert "PROJECT_DIR = WorkshopProjectRegistry.LEGACY_PROJECT_DIR" in activity
    assert "ensureProjectFile" in activity
    assert "writeTextFile" in activity
    assert "resetProject.setText(\"Reset Project\")" in activity
    assert "if (diskFile.isFile())" in activity
    assert "Saved to .stasis file" in activity
    assert "nativeCompileProject(projectRootPath())" in activity
    assert "resetSelectedEdit" in activity
    assert "classifySelectedReload" in activity
    assert "FrameLayout root = new FrameLayout(this)" in activity
    assert "window.setStatusBarColor(Color.BLACK)" in activity
    assert "window.setNavigationBarColor(Color.BLACK)" in activity
    assert "installSystemInsetGuard(root)" in activity
    assert "setOnApplyWindowInsetsListener" in activity
    assert "getSystemWindowInsetTop" in activity
    assert "getSystemWindowInsetBottom" in activity
    assert "getDisplayCutout" in activity
    assert "getSafeInsetTop" in activity
    assert "getSafeInsetBottom" in activity
    assert "view.setPadding(left, top, right, bottom)" in activity
    assert "FrameLayout.LayoutParams.MATCH_PARENT" in activity
    assert "editorPanel.setFillViewport(false)" in activity
    assert "editorPanel.setVisibility(View.GONE)" in activity
    assert "editorToggle.setText(\"\\u2630\")" in activity
    assert "editorToggle.setText(opening ? \"\\u00D7\" : \"\\u2630\")" in activity
    assert "toggleEditorPanel" in activity
    assert "startGameLoop" in activity
    assert "private static final long DEFAULT_TICK_INTERVAL_MS = 16L" in activity
    assert "gameLoopHandler.postDelayed(this, DEFAULT_TICK_INTERVAL_MS)" in activity
    assert "compileReady = isRunnableCompile(compileResult)" in activity
    assert "compileAttempted = true" in activity
    assert 'String frameError = "RunError: " + nativeLastFrameError()' in activity
    assert "!compileReady && !compileAttempted" in activity
    assert "compileResult.contains(\"status=0\")" in activity
    assert "if (resetProject)" in activity
    assert "deleteProjectDirectory(projectRoot)" in activity
    assert "setStatusText" in activity
    assert "setOnFocusChangeListener" in activity
    assert "scrollEditorIntoView" in activity
    assert "keyboardSpacer" in activity
    assert "dp(360)" in activity
    assert "smoothScrollTo(0, sourceEditor.getBottom())" in activity
    assert "FastReload: function signature unchanged" in activity
    assert "ResetRequired: function signature changed" in activity
    assert "ResetRequired: struct or layout source changed" in activity
    assert "setContentView(status)" not in activity

    release_activity = read("mobile/shells/android/app/src/main/java/com/stasislang/game/MainActivity.java")
    release_cache = read("mobile/shells/android/app/src/main/java/com/stasislang/shell/StasisAssetCache.java")
    release_bridge = read("mobile/shells/android/app/src/main/cpp/stasis_android_assets.c")
    assert "System.loadLibrary(\"SDL3\")" not in release_activity
    assert "System.loadLibrary(\"SDL3_image\")" not in release_activity
    assert "System.loadLibrary(\"main\")" in release_activity
    assert 'return new String[] {"main"}' in release_activity
    assert "https://api.openai.com" not in release_activity
    assert "SharedPreferences" not in release_activity
    assert "event.getPointerCount() >= 3" in release_activity
    assert "nativeReadPerformanceMetrics" in release_activity
    assert "nativeReadRuntimeError" in release_activity
    assert 'appendPhase(hudText, "guest render"' in release_activity
    assert 'appendPhase(hudText, "host replay"' in release_activity
    assert 'appendPhase(hudText, "frame work"' in release_activity
    assert 'appendWorkload(hudText, "commands"' in release_activity
    assert "percentile(" not in release_activity
    assert "performanceHud.setSingleLine(false)" in release_activity
    assert "setOnApplyWindowInsetsListener" in release_activity
    assert "getDisplayCutout" in release_activity
    assert "StasisAssetCache.Result" in release_activity
    assert "new AndroidAssetSource(getAssets())" in release_activity
    assert "packageInfo.lastUpdateTime" in release_activity
    assert "INVALID_ASSET_ROOT" in release_activity
    assert "nativeSetAssetRoot(invalidAssetRoot.getAbsolutePath())" in release_activity
    assert "Asset cache mode=" in release_activity
    assert "packaged_read_bytes=" in release_activity
    assert "CACHE_SCHEMA = \"stasis.android.asset-cache\"" in release_cache
    assert "MAX_MARKER_BYTES" in release_cache
    assert "inventoryMatches" in release_cache
    assert "publicationInterceptor.beforeInstall" in release_cache
    assert "rename(backup, root)" in release_cache
    assert "BACKUP_ALT_NAME" in release_cache
    assert "MAX_COPIED_TREE_BYTES" in release_cache
    assert 'child.equals(".")' in release_cache
    assert "stasis asset cache JVM scenarios" in read("tools/ci/java/com/stasislang/shell/StasisAssetCacheTest.java")
    assert "stasis_host_get_latest_performance_metrics" in release_bridge
    assert "stasis_host_get_latest_performance_metrics_v1" in release_bridge
    assert "stasis_performance_metrics.h" in release_bridge
    assert "stasis_host_copy_runtime_error" in release_bridge
    assert "drawSprites" in preview_renderer
    assert '"com.stasislang.pong"' in device_script
    workshop = read("crates/stasis_compiler/src/frontend/workshop.rs")
    assert "WorkshopReload" in workshop
    assert "WorkshopCompilePlan" not in workshop
    assert "render_workshop_artifacts" not in workshop
    assert "CompiledStub" not in workshop

    native = read("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
    codex_native = read("mobile/android/codex_native/src/lib.rs")
    assert "stasis_codex_android_begin_response" in native
    assert "stasis_codex_android_cancel_response" in native
    assert "cancel_on_generation_change" in codex_native
    assert "Codex request cancelled" in codex_native
    assert "select_codex_model" in codex_native
    assert "Codex model is unavailable" in codex_native
    assert "Java_com_stasislang_workshop_MainActivity_nativeStatus" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeCompileProject" in native
    compile_native_start = native.index(
        "Java_com_stasislang_workshop_MainActivity_nativeCompileProject")
    compile_native_end = native.index(
        "Java_com_stasislang_workshop_MainActivity_nativeSourceItems", compile_native_start)
    compile_native = native[compile_native_start:compile_native_end]
    assert "char message[" not in compile_native
    assert 'bridge->compile_project(root, "src/main.stasis")' in compile_native
    assert "jstring result = (*env)->NewStringUTF(env, message);" in compile_native
    assert "bridge->free_string(message);" in compile_native
    assert "required Rust Android compiler bridge is unavailable" in native
    assert "dlopen(\"libstasis_android_bridge.so\"" in native
    assert "stasis_android_bridge_compile_project" in native
    assert "stasis_android_bridge_version" in native
    assert "stasis_android_bridge_set_i32_global" in native
    assert "stasis_android_bridge_get_i32_global" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeSetRuntimeI32" in native
    assert "stasis_android_bridge_free_string" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunTick" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunFrameInto" in native
    assert "STASIS_RENDER_BUFFER_DESCRIPTORS" in native
    assert "validate_stasis_jni_frame_buffers" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeFrameAbiDescriptor" in native
    assert "stasis_jni_order_mutex" in native
    assert "stasis_jni_order_ready" in native
    assert "memory_order_acquire" in native
    assert "memory_order_release" in native
    assert "pthread_mutex_lock" in native
    assert "ExceptionCheck" in native
    assert "NewGlobalRef" in native
    assert "clear_stasis_jni_frame_error" in native
    assert 'dlsym(rust_bridge_api.handle, "stasis_android_bridge_run_tick_frame_v2")' in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunFrame(JNIEnv" not in native
    assert "scan_stasis_files" not in native
    assert "analyze_stasis_file" not in native
    assert "validate_braces" not in native
    assert "write_compile_manifest" not in native
    assert "write_function_manifest_entries" not in native
    assert "STASIS_FUNCTION_ARTIFACT_DIR" not in native
    assert "STASIS_RUNTIME_STATE_RELATIVE_PATH" in native
    assert "CompiledStub" not in native
    assert "write_function_artifact" not in native
    assert "RuntimeStateReady" in native
    assert "read_runtime_tick_count" in native
    assert "write_runtime_tick_count" in native
    assert "RunTick: tick_count=%d" in native
    assert "RunError: compile project before running tick" in native
    assert "state=%s" in native
    assert "touch_y" in bridge
    assert "render_command_count" in bridge
    assert "Render.command0_kind" in bridge
    assert "Render.command{index}_asset" in bridge
    assert "PreviousManifest" not in native
    assert "read_previous_compile_manifest" not in native
    assert "classify_reload" not in native
    assert "WorkshopReload::InitialCompile" in bridge
    assert "WorkshopReload::NoChange" in bridge
    assert "WorkshopReload::FastReload" in bridge
    assert "WorkshopReload::ResetRequired" in bridge
    assert "CompileReady: backend=cranelift-jit" in bridge
    assert "Stasis Workshop IT-025" in native
    render_main = read("samples/render_parity/main.stasis")
    render_frame = read("samples/render_parity/frame.stasis")
    assert "seam_touch_checksum" in render_main
    assert "host_f32[4] * 1000.0" in render_main
    assert "append_parity_touch_marker" in render_frame
    assert "marker_active" in render_frame
    assert "PARITY_GFX_F_RECT_REVERSE_BASE - 8" in render_frame
    acceptance = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopJniFrameAbiAcceptance.java")
    assert "Stasis Workshop IT-026" in acceptance
    assert "all_invalid_unchanged" in acceptance
    assert "isDescriptorEnvelope" in acceptance
    assert "descriptor.optString(\"schema\")" in acceptance
    diagnostic_model = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopNativeDiagnostic.java")
    diagnostic_acceptance = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopDiagnosticSeamAcceptance.java")
    assert 'SCHEMA = "stasis.native_diagnostic.v1"' in diagnostic_model
    assert "fromNative" in diagnostic_model and "causes" in diagnostic_model
    assert "Stasis Workshop IT-031" in diagnostic_acceptance
    assert "MISSING_EXTERN" in diagnostic_acceptance
    assert "runIt031Frame" in activity
    assert "WorkshopDiagnosticSeamAcceptance.run" in activity
    touch_acceptance = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopTouchAcceptance.java")
    assert "Stasis Workshop IT-027" in touch_acceptance
    assert "ACTION_DOWN" in touch_acceptance and "ACTION_MOVE" in touch_acceptance
    assert "ACTION_UP" in touch_acceptance
    assert "java_only" in touch_acceptance
    assert "nativeValidateFrameAbi" not in acceptance
    assert "STASIS_RENDER_ACCEPTANCE" in native
    assert "STASIS_RENDER_I_FRAME_TOKEN" in native
    assert "IT-025 GLES" in preview_renderer
    assert "IT-027 GLES" in preview_renderer
    assert "rect_count" in preview_renderer
    assert "I_FRAME_TOKEN" in preview_renderer
    assert "awaitPresentedFrameToken" in preview_renderer
    assert "ACCEPTANCE_RENDER_PUMP_SLICE_MILLIS" in activity
    assert "long deadline = System.nanoTime() + timeoutMillis * 1_000_000L" in activity
    assert "long remainingNanos = deadline - System.nanoTime();" in activity
    assert activity.count("remainingNanos = deadline - System.nanoTime();") >= 2
    assert "long waitMillis = Math.min(remainingMillis" in activity
    assert "return System.nanoTime() <= deadline;" in activity
    assert "requestRender();" in activity
    assert "lastAcceptanceGlesEvidenceToken" in preview_renderer
    assert "frameToken != lastAcceptanceGlesEvidenceToken" in preview_renderer
    assert "nativeFrameTrace" in native
    assert "Stasis Android native smoke loaded" in native
    assert "stasis_android_audio_set_paused" in native
    assert "stasis_android_audio_set_focus" in native
    assert "stasis_android_bridge_install_audio_api" in native
    android_audio = read("mobile/android/app/src/main/cpp/stasis_android_audio.c")
    assert "AAudioStreamBuilder_openStream" in android_audio
    assert "AAudioStreamBuilder_setFormat" in android_audio
    assert "AAudioStreamBuilder_setUsage" in android_audio
    assert "AAudioStreamBuilder_setContentType" in android_audio
    assert "AAUDIO_USAGE_GAME" in android_audio
    assert "AAUDIO_CONTENT_TYPE_MUSIC" in android_audio
    assert "stasis_audio_get_queued_frames" in android_audio
    assert "stasis_audio_get_underruns" in android_audio
    assert "stasis_audio_push_f32_interleaved" in android_audio
    assert "stasis_audio_ring" in read("runtime/stasis_audio_ring.c")

    cmake = read("mobile/android/app/src/main/cpp/CMakeLists.txt")
    assert "add_library(stasis_mobile_smoke SHARED" in cmake
    assert "stasis_mobile_smoke.c" in cmake
    assert "stasis_android_sprite.c" in cmake
    assert "stasis_android_audio.c" in cmake
    assert "stasis_audio_ring.c" in cmake
    assert "../../../../../../runtime" in cmake
    assert "find_library(math_lib m)" in cmake
    assert "STASIS_ANDROID_PUBLISHED_AOT" not in cmake
    assert "published_aot_objects.cmake" not in cmake
    assert "find_library(dl_lib dl)" in cmake
    assert "STASIS_RENDER_ACCEPTANCE" in cmake
    assert "${dl_lib}" in cmake

    for sample in STASIS_SAMPLE_FILES:
        source = read(sample)
        assert "function " in source
        assert "fn " not in source
        assert "&mut" not in source

    player = read("mobile/android/app/src/main/assets/workshop_sample/src/player.stasis")
    assert "struct PlayerPaddle" in player

    sample_main = read("mobile/android/app/src/main/assets/workshop_sample/src/main.stasis")
    assert "command3_asset" in sample_main
    assert "Render.command3_kind = 2" in sample_main
    assert "Render.command3_asset = -1520461853" in sample_main
    assert "Render.command_schema_version = 3" in sample_main
    assert "Render.command3_rotation_degrees" in sample_main
    assert "Render.command3_alpha = 255" in sample_main
    assert "Render.command3_clip_w = GameState.screen_w" in sample_main
    pong_manifest = read("mobile/android/app/src/main/assets/workshop_sample/assets/manifest.json")
    assert '"id": "ball"' in pong_manifest
    assert '"id": "paddle"' in pong_manifest
    assert '"id": "center_line"' in pong_manifest
    pong_project = read("mobile/android/app/src/main/assets/workshop_sample/stasis.json")
    assert '"application_id": "com.stasislang.pong"' in pong_project
    assert '"label": "Stasis Pong"' in pong_project
    assert '"orientation": "sensorLandscape"' in pong_project
    preview_adapter = read("mobile/android/app/src/main/assets/workshop_sample/src/preview_adapter.stasis")
    assert "function on_code_swap(): void { pong_game_on_code_swap(); pong_host_render(); }" in preview_adapter
    assert '"encoding": "svg"' in pong_manifest

    collision = read("mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis")
    assert "Collision logic lives" in collision

    audio_fixture = read("mobile/android/app/src/main/assets/audio_sink_sample/stasis.json")
    assert '"name": "brickout_audio"' in audio_fixture
    assert '"application_id": "com.stasislang.brickoutaudio"' in audio_fixture
    assert '"entry": "src/main.stasis"' in audio_fixture
    audio_source = read("mobile/android/app/src/main/assets/audio_sink_sample/src/main.stasis")
    assert "audio_push_f32_interleaved" in audio_source

    print("android shell structure ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

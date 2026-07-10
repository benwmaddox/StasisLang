from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_FILES = [
    "mobile/android/settings.gradle",
    "mobile/android/build.gradle",
    "mobile/android/build_rust_bridge.ps1",
    "mobile/android/build_published.ps1",
    "mobile/android/validate_device.ps1",
    "mobile/android/app/build.gradle",
    "mobile/android/app/src/main/AndroidManifest.xml",
    "mobile/android/app/src/workshop/AndroidManifest.xml",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSecretStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidEditRecoveryStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidDraftStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectRegistry.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectArchive.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopImageAssets.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopPaintView.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAudioAssets.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetManifest.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetIdentity.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopMoney.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSupportBundle.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidCrashStore.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidAiQueue.java",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/AiQueuePolicy.java",
    "mobile/android/app/src/published/java/com/stasislang/workshop/MainActivity.java",
    "mobile/android/app/src/main/cpp/CMakeLists.txt",
    "mobile/android/app/src/main/cpp/stasis_mobile_smoke.c",
    "mobile/android/app/src/main/res/values/styles.xml",
    "mobile/android/app/src/main/assets/workshop_sample/src/main.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/root.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/game_state.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/player.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/enemy.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/input.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/assets.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis",
    "mobile/android/README.md",
    "tools/android_ai_agent_host.py",
    "tools/ci/check_android_published_apk.py",
    "tests/android/AiQueuePolicyTest.java",
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
    assert "build_workshop_compile_plan" in bridge
    assert "render_workshop_artifacts" in bridge

    rust_bridge_script = read("mobile/android/build_rust_bridge.ps1")
    debug_script = read("mobile/android/build_debug.ps1")
    assert "Workshop Android Gradle build failed with exit code" in debug_script
    assert "Rust Android bridge build failed with exit code" in rust_bridge_script
    assert "rustup target discovery failed with exit code" in rust_bridge_script
    published_script = read("mobile/android/build_published.ps1")
    device_script = read("mobile/android/validate_device.ps1")
    android_gitignore = read("mobile/android/.gitignore")
    assert "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" in rust_bridge_script
    assert "aarch64-linux-android" in rust_bridge_script
    assert "libstasis_android_bridge.so" in rust_bridge_script
    assert "app\\src\\workshop\\jniLibs\\arm64-v8a" in rust_bridge_script
    assert "build_rust_bridge.ps1" in debug_script
    assert ":app:assembleWorkshopDebug" in debug_script
    assert ":app:assemblePublishedRelease" in published_script
    assert ":app:installPublishedDebug" in published_script
    assert "ValidateAot" in published_script
    assert "aot_engine_bundle_writes_manifest_and_required_entrypoints" in published_script
    assert "check_android_published_apk.py" in published_script
    assert "build_rust_bridge.ps1" not in published_script
    assert "app/src/*/jniLibs/" in android_gitignore
    assert "RequireDevice" in device_script
    assert "android_device_acceptance" in device_script
    assert "ro.product.cpu.abilist" in device_script
    assert "arm64-v8a" in device_script
    assert "am\", \"start\", \"-W" in device_script
    assert "pidof" in device_script

    app_gradle = read("mobile/android/app/build.gradle")
    assert "flavorDimensions 'mode'" in app_gradle
    assert "workshop {" in app_gradle
    assert "published {" in app_gradle
    assert "applicationId 'com.stasislang.workshop'" in app_gradle
    assert "applicationId 'com.stasislang.pong'" in app_gradle
    assert "manifestPlaceholders = [appLabel: 'Stasis Pong']" in app_gradle
    assert "buildConfigField 'String', 'STASIS_GAME_ID', '\"pong\"'" in app_gradle
    assert "pongReleaseProjectDir" in app_gradle
    assert "STASIS_PUBLISHED_BUILD" in app_gradle
    assert "abiFilters 'arm64-v8a'" in app_gradle
    assert "externalNativeBuild" in app_gradle
    assert "STASIS_ANDROID_SMOKE_ONLY=ON" in app_gradle
    assert "generatePublishedAotBundle" in app_gradle
    assert "STASIS_ANDROID_PUBLISHED_AOT=ON" in app_gradle
    assert "prepareWorkshopAssets" in app_gradle
    assert "workshop_sample/build/**" in app_gradle
    assert "published.assets.setSrcDirs([])" in app_gradle

    manifest = read("mobile/android/app/src/main/AndroidManifest.xml")
    workshop_manifest = read("mobile/android/app/src/workshop/AndroidManifest.xml")
    assert "android.permission.INTERNET" not in manifest
    assert "android.permission.RECORD_AUDIO" in workshop_manifest
    assert "android.permission.INTERNET" in workshop_manifest
    assert "${appLabel}" in manifest
    assert "android.intent.action.MAIN" in manifest
    assert "android.intent.category.LAUNCHER" in manifest
    assert 'android:exported="true"' in manifest
    assert 'android:windowSoftInputMode="adjustResize"' in manifest

    activity = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java")
    secret_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidSecretStore.java")
    recovery_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidEditRecoveryStore.java")
    draft_store = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AndroidDraftStore.java")
    project_registry = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectRegistry.java")
    project_archive = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectArchive.java")
    image_assets = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopImageAssets.java")
    paint_view = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopPaintView.java")
    audio_assets = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAudioAssets.java")
    asset_manifest = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetManifest.java")
    asset_identity = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopAssetIdentity.java")
    money = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopMoney.java")
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
    ai_queue_policy = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/AiQueuePolicy.java")
    host_agent = read("tools/android_ai_agent_host.py")
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
    assert "private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight)" in activity
    assert "private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight, int[] frameValues)" in activity
    assert "workshop_sample/" in activity
    assert "createWorkshopView" in activity
    assert "BuildConfig.STASIS_PUBLISHED_BUILD" in activity
    assert "installGameStatusOverlay(root, false)" in activity
    assert "installGameStatusOverlay(root, true)" in activity
    assert "toggleBenchmarkHudFromPreview" in activity
    assert "MotionEvent.ACTION_POINTER_DOWN" in activity
    assert "event.getPointerCount() >= 3" in activity
    assert "startGameLoop();" in activity
    assert "GamePreviewView" in activity
    assert "GLSurfaceView" in activity
    assert "onDrawFrame" in activity
    assert "extractIntField" in activity
    assert "private final int[] nativeFrameValues = new int[RENDER_FRAME_I32_CAPACITY]" in activity
    assert "FRAME_BUDGET_MILLIS = 1000.0 / 60.0" in activity
    assert "budget=--%" in activity
    assert "debugColorForBudget" in activity
    assert "private String projectRootPath" in activity
    assert "nativeCompileProject(projectRootPath())" in activity
    assert "String.format" not in activity
    assert "gamePreview.setRenderFrameValues(nativeFrameValues)" in activity
    assert "RenderFrame.fromNativeFrame" not in activity
    assert "new RenderCommand" not in activity
    assert "ProjectSnapshot.from" in activity
    assert "createAiControls" in activity
    assert "createAiProgressPill" in activity
    assert "postAiProgress" in activity
    assert "actions " in activity
    assert "calling AI" in activity
    assert "time 0.0s" in activity
    assert "hot swap=FastReload" in activity
    assert "aiReloadPhase" in activity
    assert "Chat and Commands" in activity
    assert "Queue AI Change" in activity
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
    assert "cancelPending" in ai_queue
    assert "AiQueuePolicy.canTransition" in ai_queue
    assert "nextPendingIndex" in ai_queue_policy
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
    assert 'recordAiOutcome(activeAiPrompt, "cancelled"' in activity
    assert 'recordAiOutcome(activeAiPrompt, "applied"' in activity
    assert 'recordAiOutcome(activeAiPrompt, "rolled_back"' in activity
    assert "Maximum USD per AI run" in activity
    assert "Monthly AI limit USD" in activity
    assert "AI budget:" in activity
    assert "AI run blocked by configured spending limit" in activity
    assert "AI spending limit reached before agent turn" in activity
    assert "recordMonthlyAiSpend" in activity
    assert "maxOutputTokensForBudget" in activity
    assert "MAX_AI_IMAGE_ATTACHMENTS = 4" in activity
    assert "MAX_AI_IMAGE_ATTACHMENT_BYTES = 12 * 1024 * 1024" in activity
    assert "Review AI Image Attachments" in activity
    assert "Only these app-private project images" in activity
    assert 'put("type", "input_image")' in activity
    assert 'put("detail", "original")' in activity
    assert '"data:" + attachment.mimeType + ";base64,"' in activity
    assert "buildAiOpenAiInput(requestJson, false)" in activity
    assert "buildAiOpenAiInput(requestJson, true)" in activity
    assert "selected_images_are_explicit_project_assets_only" in activity
    assert "activeAiImageAttachments = Collections.emptyList()" in activity
    assert "Capture Preview for AI" in activity
    assert "Review Preview Capture" in activity
    assert "Attach these rendered pixels" in activity
    assert "Attach logical render/runtime/input snapshot" in activity
    assert "Nothing is sent until selected here and Queue AI Change is pressed" in activity
    assert "GLES20.glReadPixels" in activity
    assert "lastDrawnFrame" in activity
    assert "MAX_PREVIEW_CAPTURE_PIXELS = 8_000_000L" in activity
    assert "preview framebuffer exceeds the 8 megapixel capture limit" in activity
    assert "Bitmap.createScaledBitmap" in activity
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
    assert 'ONBOARDING_COMPLETE = "manual_tutorial_seen_v1"' in activity
    assert "Welcome to Stasis Workshop" in activity
    assert "You can build and test a game entirely on-device without AI" in activity
    assert "Remind Me Later" in activity
    assert "Help & Onboarding" in activity
    assert "Start Zero-AI Manual Tutorial" in activity
    assert "no API key is required" in activity
    assert "Voice or audio recording asks for microphone permission only when started" in activity
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
    assert "screenWidthDp < 480" in activity
    assert "Selected image asset" in activity
    assert "Audio asset " in activity
    assert "Touch paint canvas" in paint_view
    assert "setFocusable(true)" in paint_view
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
    assert "AI spending limit leaves insufficient budget" in activity
    assert "Cancel AI" in activity
    assert "aiRunActive" in activity
    assert "aiCancelRequested" in activity
    assert "AI request queued behind the active item" in activity
    assert "throwIfAiCancelled()" in activity
    assert "if (!batchHasWrites) throwIfAiCancelled()" in activity
    assert "finishing any active call or atomic write batch" in activity
    assert "activeAiConnection" in activity
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
    assert "uploadGitHubFile" in activity
    assert "GitHub sync: queued" in activity
    assert "GitHub sync error:" in activity
    assert "Review GitHub Changes" in activity
    assert "Create / Update Pull Request" in activity
    assert "reviewedGitHubChangeFingerprint" in activity
    assert 'MessageDigest.getInstance("SHA-256")' in activity
    assert "GITHUB_NETWORK_TIMEOUT_MS" in activity
    assert "GitHub pull request: review current changes first" in activity
    assert "ensureGitHubReviewBranch" in activity
    assert "stasis-workshop-" in activity
    assert "createOrFindGitHubPullRequest" in activity
    assert 'githubApiUrl(repository, "/git/refs")' in activity
    assert 'githubApiUrl(repository, "/pulls")' in activity
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
    assert "githubOperationActive" in activity
    assert "beginGitHubOperation" in activity
    assert "another operation is already queued or running" in activity
    assert "GITHUB_PREF_OPERATION_STATE" in activity
    assert "GITHUB_PREF_OPERATION_DETAIL" in activity
    assert "GITHUB_PREF_REVIEW_FINGERPRINT" in activity
    assert "Retry GitHub Operation" in activity
    assert "persistGitHubOperationState" in activity
    assert '"interrupted", "app stopped before completion"' in activity
    assert '"sync".equals(operation)' in activity
    assert '"pull_request".equals(operation)' in activity
    assert "WorkshopProjectRegistry.initialize(this)" in activity
    assert "New Project From Sample" in activity
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
    assert "static final int FORMAT_VERSION = 2" in project_registry
    assert 'V1_BACKUP_FILE = ".stasis-workshop.json.v1.bak"' in project_registry
    assert "migrateV1Metadata" in project_registry
    assert 'put("schema", "stasis-workshop-project")' in project_registry
    assert 'put("migrated_from_version", migratedFromVersion)' in project_registry
    assert "update the Workshop before opening this project" in project_registry
    assert "project format 2 metadata schema is invalid" in project_registry
    assert "the fsynced v1 backup was preserved" in project_registry
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
    assert '?:1|2' in project_archive
    assert "project archive needs src/main.stasis" in project_archive
    assert "output.getFD().sync()" in project_archive
    assert "legacy project cannot be deleted as a failed import" in project_registry
    assert "Manual Symbols and Source" in activity
    assert "Go to Diagnostic" in activity
    assert "captureFirstTestFailureDiagnostic" in activity
    assert "sourceOffsetForLine" in activity
    assert 'diagnosticStatus.setText("Test failure' in activity
    assert 'result.optInt("line", 0)' in activity
    assert "sourceEditor.setSelection(symbolOffset)" in activity
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
    assert "private static final double GPT_5_6_SOL_INPUT_USD_PER_MILLION = 5.00" in activity
    assert "private static final double GPT_5_6_SOL_CACHED_INPUT_USD_PER_MILLION = 0.50" in activity
    assert "private static final double GPT_5_6_SOL_CACHE_WRITE_USD_PER_MILLION = 6.25" in activity
    assert "private static final double GPT_5_6_SOL_OUTPUT_USD_PER_MILLION = 30.00" in activity
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
    assert "tool_call_limit_after_successful_tested_writes" in activity
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
    assert "SAMPLE_TEST_FILES" in activity
    assert "parseTest" in activity
    assert "TestUpdated: run tests to validate" in activity
    assert "sections.put(\"Tests\"" in activity
    assert "readAsset(assets, ASSET_ROOT + file)" in activity
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
    assert "preferredFunctionCall" in activity
    assert "preferred_call" in activity
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
    assert "The app compiles once after each tool-call batch that contains writes and runs tests after each tool-call batch" in activity
    assert "Use on_code_swap() for post-hot-swap migration" in activity
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
    assert "Apply code changes with write_symbol, delete_symbol, write_imports, write_test_file, or delete_test_file before final edits" in activity
    assert "Tool errors, validation_error observations, and test_observation failures are not final" in activity
    assert "mode=done" in activity
    assert "AI edit complete" in activity
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
    assert 'DEFAULT_AI_REASONING_EFFORT = "medium"' in activity
    assert 'reasoningSummary.setText("Reasoning: medium")' in activity
    assert 'GPT-5.6 Sol defaults to medium reasoning' in activity
    assert '"gpt-5.6-terra".equals(configuredModel)' in activity
    assert "AI_PREF_MODEL_DEFAULT_VERSION" in activity
    assert 'DEFAULT_MODEL = "gpt-5.6-sol"' in host_agent
    assert 'DEFAULT_REASONING_EFFORT = "medium"' in host_agent
    assert '"cache_write": 6.25' in host_agent
    assert "prompt_cache_key" in activity
    assert "prompt_cache_breakpoint" in activity
    assert 'content.put("prompt_cache_breakpoint", new JSONObject().put("mode", "explicit"))' in activity
    assert 'payload.put("prompt_cache_options", new JSONObject().put("mode", "explicit").put("ttl", "30m"))' in activity
    assert 'payload.put("reasoning", new JSONObject().put("effort", DEFAULT_AI_REASONING_EFFORT))' in activity
    assert 'put("type", "prompt_cache_breakpoint")' not in activity
    assert 'payload.put("prompt_cache_retention"' not in activity
    assert '"prompt_cache_options": {"mode": "explicit", "ttl": "30m"}' in host_agent
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
    assert "RENDER_COMMAND_STRIDE = 7" in activity
    assert "frameValues[5]" in activity
    assert "frameValues[base + 6]" in activity
    assert "GLES20.glDrawArrays" in activity
    assert "GL_TRIANGLES" in activity
    assert "glUniform4f" not in activity
    assert "attribute vec4 aColor" in activity
    assert "drawBatch(vertexCount)" in activity
    assert "TEXTURE_FRAGMENT_SHADER" in activity
    assert "drawSpriteBatch(spriteVertexCount)" in activity
    assert "glTexImage2D" in activity
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
    assert "RunError: native frame tick failed" in activity
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

    published_activity = read("mobile/android/app/src/published/java/com/stasislang/workshop/MainActivity.java")
    assert "System.loadLibrary(\"stasis_mobile_smoke\")" in published_activity
    assert "nativeCompileProject(String projectRoot)" in published_activity
    assert "nativeRunFrameInto(String projectRoot" in published_activity
    assert "https://api.openai.com" not in published_activity
    assert "SharedPreferences" not in published_activity
    assert "createAiControls" not in published_activity
    assert "Manual Symbols and Source" not in published_activity
    assert "GameSurfaceView" in published_activity
    assert "GLSurfaceView" in published_activity
    assert "onDrawFrame" in published_activity
    assert "FRAME_BUDGET_MILLIS = 1000.0 / 60.0" in published_activity
    assert "event.getPointerCount() >= 3" in published_activity
    assert 'PUBLISHED_RUNTIME_ID = BuildConfig.STASIS_GAME_ID + "_aot"' in published_activity
    assert '"com.stasislang.pong"' in device_script
    assert "ensureBundledProject" not in published_activity
    assert "AssetManager" not in published_activity
    workshop = read("crates/stasis_compiler/src/frontend/workshop.rs")
    assert "build_workshop_compile_plan" in workshop
    assert "WorkshopCompilePlan" in workshop
    assert "IncrementalCompileOutput" in workshop
    assert "WorkshopReload" in workshop
    assert "workshop_compile_plan_tests" in workshop
    assert "render_workshop_artifacts" in workshop
    assert "WorkshopArtifactSet" in workshop
    assert "status=RuntimeStateReady" in workshop
    assert "status=CompiledStub" in workshop

    native = read("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
    assert "Java_com_stasislang_workshop_MainActivity_nativeStatus" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeCompileProject" in native
    assert "try_rust_bridge_compile" in native
    assert "dlopen(\"libstasis_android_bridge.so\"" in native
    assert "stasis_android_bridge_compile_project" in native
    assert "stasis_android_bridge_set_i32_global" in native
    assert "stasis_android_bridge_get_i32_global" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeSetRuntimeI32" in native
    assert "stasis_android_bridge_free_string" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunTick" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunFrameInto" in native
    assert "const int frame_len = 62" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunFrame(JNIEnv" not in native
    assert "scan_stasis_files" in native
    assert "analyze_stasis_file" in native
    assert "validate_braces" in native
    assert "CompilePlanned: reload=%s files=" in native
    assert "STASIS_COMPILE_MANIFEST_RELATIVE_PATH" in native
    assert "write_compile_manifest" in native
    assert "write_function_manifest_entries" in native
    assert "append_function_entries_for_project" in native
    assert "body_hash=%016llx" in native
    assert "STASIS_FUNCTION_ARTIFACT_DIR" in native
    assert "STASIS_RUNTIME_STATE_RELATIVE_PATH" in native
    assert "CompiledStub" in native
    assert "write_function_artifact" in native
    assert "artifact=%s/%016llx.stub" in native
    assert "signature_hash=%016llx" in native
    assert "project_hash=%016llx" in native
    assert "status=CompilePlanned" in native
    assert "RuntimeStateReady" in native
    assert "read_runtime_tick_count" in native
    assert "write_runtime_tick_count" in native
    assert "RunTick: tick_count=%d" in native
    assert "RunError: compile project before running tick" in native
    assert "write_runtime_state" in native
    assert "runtime_state=%s" in native
    assert "entrypoint=main" in native
    assert "entrypoint=tick" in native
    assert "state=%s" in native
    assert "touch_y" in bridge
    assert "render_command_count" in bridge
    assert "Render.command0_kind" in bridge
    assert "Render.command{index}_asset" in bridge
    assert "PreviousManifest" in native
    assert "read_previous_compile_manifest" in native
    assert "classify_reload" in native
    assert '"GameState.ball_age_ticks", &published_game_ball_age_ticks' in native
    assert '"GameState.enemy_paddle_speed_x100", &published_game_enemy_paddle_speed_x100' in native
    assert "reload=%s" in native
    assert "InitialCompile" in native
    assert "NoChange" in native
    assert "FastReload" in native
    assert "ResetRequired" in native
    assert "CompileError: missing lifecycle root" in native
    assert "Stasis Android native smoke loaded" in native

    cmake = read("mobile/android/app/src/main/cpp/CMakeLists.txt")
    assert "add_library(stasis_mobile_smoke SHARED stasis_mobile_smoke.c)" in cmake
    assert "STASIS_ANDROID_PUBLISHED_AOT" in cmake
    assert "published_aot_objects.cmake" in cmake
    assert "find_library(dl_lib dl)" in cmake
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
    assert "Render.command3_asset = 1" in sample_main

    collision = read("mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis")
    assert "Collision logic lives" in collision

    print("android shell structure ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_FILES = [
    "mobile/android/settings.gradle",
    "mobile/android/build.gradle",
    "mobile/android/build_rust_bridge.ps1",
    "mobile/android/app/build.gradle",
    "mobile/android/app/src/main/AndroidManifest.xml",
    "mobile/android/app/src/main/java/com/stasislang/workshop/MainActivity.java",
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
    assert "build_android_workshop_compile_plan" in bridge
    assert "render_android_workshop_artifacts" in bridge

    rust_bridge_script = read("mobile/android/build_rust_bridge.ps1")
    debug_script = read("mobile/android/build_debug.ps1")
    android_gitignore = read("mobile/android/.gitignore")
    assert "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" in rust_bridge_script
    assert "aarch64-linux-android" in rust_bridge_script
    assert "libstasis_android_bridge.so" in rust_bridge_script
    assert "app\\src\\main\\jniLibs\\arm64-v8a" in rust_bridge_script
    assert "build_rust_bridge.ps1" in debug_script
    assert "app/src/main/jniLibs/" in android_gitignore

    app_gradle = read("mobile/android/app/build.gradle")
    assert "applicationId 'com.stasislang.workshop'" in app_gradle
    assert "abiFilters 'arm64-v8a'" in app_gradle
    assert "externalNativeBuild" in app_gradle
    assert "STASIS_ANDROID_SMOKE_ONLY=ON" in app_gradle

    manifest = read("mobile/android/app/src/main/AndroidManifest.xml")
    assert "android.intent.action.MAIN" in manifest
    assert "android.intent.category.LAUNCHER" in manifest
    assert 'android:exported="true"' in manifest
    assert 'android:windowSoftInputMode="adjustResize"' in manifest

    activity = read("mobile/android/app/src/main/java/com/stasislang/workshop/MainActivity.java")
    assert "System.loadLibrary(\"stasis_mobile_smoke\")" in activity
    assert "private static native String nativeStatus()" in activity
    assert "private static native String nativeCompileProject(String projectRoot)" in activity
    assert "private static native String nativeRunTick(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight)" in activity
    assert "private static native int nativeRunFrameInto(String projectRoot, int touchX, int touchY, int touchActive, int screenWidth, int screenHeight, int[] frameValues)" in activity
    assert "workshop_sample/" in activity
    assert "createWorkshopView" in activity
    assert "GamePreviewView" in activity
    assert "GLSurfaceView" in activity
    assert "onDrawFrame" in activity
    assert "extractIntField" in activity
    assert "private final int[] nativeFrameValues = new int[RENDER_FRAME_I32_CAPACITY]" in activity
    assert "gamePreview.setRenderFrameValues(nativeFrameValues)" in activity
    assert "RenderFrame.fromNativeFrame" not in activity
    assert "new RenderCommand" not in activity
    assert "ProjectSnapshot.from" in activity
    assert "parseSymbols" in activity
    assert "groupSymbols" in activity
    assert "createSymbolRow" in activity
    assert "EditText" in activity
    assert "createEditControls" in activity
    assert 'compile.setText("Compile")' in activity
    assert 'runTick.setText("Run Tick")' in activity
    assert "runNativeCompile" in activity
    assert "runNativeTick" in activity
    assert "nativeRunFrameInto(" in activity
    assert "gamePreview.touchX()" in activity
    assert "gamePreview.touchY()" in activity
    assert "gamePreview.touchActive()" in activity
    assert "MotionEvent" in activity
    assert "frameValues[5]" in activity
    assert "GLES20.glDrawArrays" in activity
    assert "GL_TRIANGLES" in activity
    assert "glUniform4f" not in activity
    assert "attribute vec4 aColor" in activity
    assert "drawBatch(vertexCount)" in activity
    assert "applySelectedEdit" in activity
    assert "persistSelectedEdit" in activity
    assert "getFilesDir()" in activity
    assert 'PROJECT_DIR = "workshop_project"' in activity
    assert "ensureProjectFile" in activity
    assert "writeTextFile" in activity
    assert "Saved to .stasis file" in activity
    assert "nativeCompileProject(projectRoot().getAbsolutePath())" in activity
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

    workshop = read("crates/stasis_compiler/src/frontend/workshop.rs")
    assert "build_android_workshop_compile_plan" in workshop
    assert "AndroidWorkshopCompilePlan" in workshop
    assert "IncrementalCompileOutput" in workshop
    assert "AndroidWorkshopReload" in workshop
    assert "android_compile_plan_tests" in workshop
    assert "render_android_workshop_artifacts" in workshop
    assert "AndroidWorkshopArtifactSet" in workshop
    assert "status=RuntimeStateReady" in workshop
    assert "status=CompiledStub" in workshop

    native = read("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
    assert "Java_com_stasislang_workshop_MainActivity_nativeStatus" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeCompileProject" in native
    assert "try_rust_bridge_compile" in native
    assert "dlopen(\"libstasis_android_bridge.so\"" in native
    assert "stasis_android_bridge_compile_project" in native
    assert "stasis_android_bridge_free_string" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunTick" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeRunFrameInto" in native
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
    assert "PreviousManifest" in native
    assert "read_previous_compile_manifest" in native
    assert "classify_reload" in native
    assert "reload=%s" in native
    assert "InitialCompile" in native
    assert "NoChange" in native
    assert "FastReload" in native
    assert "ResetRequired" in native
    assert "CompileError: missing lifecycle root" in native
    assert "Stasis Android native smoke loaded" in native

    cmake = read("mobile/android/app/src/main/cpp/CMakeLists.txt")
    assert "add_library(stasis_mobile_smoke SHARED stasis_mobile_smoke.c)" in cmake
    assert "find_library(dl_lib dl)" in cmake
    assert "${dl_lib}" in cmake

    for sample in STASIS_SAMPLE_FILES:
        source = read(sample)
        assert "function " in source
        assert "fn " not in source
        assert "&mut" not in source

    player = read("mobile/android/app/src/main/assets/workshop_sample/src/player.stasis")
    assert "struct PlayerPaddle" in player

    collision = read("mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis")
    assert "Collision logic lives" in collision

    print("android shell structure ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

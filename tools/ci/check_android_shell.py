from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_FILES = [
    "mobile/android/settings.gradle",
    "mobile/android/build.gradle",
    "mobile/android/build_rust_bridge.ps1",
    "mobile/android/build_published.ps1",
    "mobile/android/app/build.gradle",
    "mobile/android/app/src/main/AndroidManifest.xml",
    "mobile/android/app/src/workshop/AndroidManifest.xml",
    "mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java",
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
    published_script = read("mobile/android/build_published.ps1")
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
    assert "app/src/*/jniLibs/" in android_gitignore

    app_gradle = read("mobile/android/app/build.gradle")
    assert "flavorDimensions 'mode'" in app_gradle
    assert "workshop {" in app_gradle
    assert "published {" in app_gradle
    assert "applicationId 'com.stasislang.workshop'" in app_gradle
    assert "applicationId 'com.stasislang.workshop.published'" in app_gradle
    assert "STASIS_PUBLISHED_BUILD" in app_gradle
    assert "abiFilters 'arm64-v8a'" in app_gradle
    assert "externalNativeBuild" in app_gradle
    assert "STASIS_ANDROID_SMOKE_ONLY=ON" in app_gradle
    assert "generatePublishedAotBundle" in app_gradle
    assert "STASIS_ANDROID_PUBLISHED_AOT=ON" in app_gradle

    manifest = read("mobile/android/app/src/main/AndroidManifest.xml")
    workshop_manifest = read("mobile/android/app/src/workshop/AndroidManifest.xml")
    assert "android.permission.INTERNET" not in manifest
    assert "android.permission.INTERNET" in workshop_manifest
    assert "${appLabel}" in manifest
    assert "android.intent.action.MAIN" in manifest
    assert "android.intent.category.LAUNCHER" in manifest
    assert 'android:exported="true"' in manifest
    assert 'android:windowSoftInputMode="adjustResize"' in manifest

    activity = read("mobile/android/app/src/workshop/java/com/stasislang/workshop/MainActivity.java")
    host_agent = read("tools/android_ai_agent_host.py")
    assert "System.loadLibrary(\"stasis_mobile_smoke\")" in activity
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
    assert "AI Edit Workspace" in activity
    assert "Manual Symbols and Source" in activity
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
    assert "private static final double GPT_5_6_TERRA_INPUT_USD_PER_MILLION = 2.50" in activity
    assert "private static final double GPT_5_6_TERRA_CACHE_WRITE_USD_PER_MILLION = 3.125" in activity
    assert "private static AiApiResponse callOpenAiResponsesApi" in activity
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
    assert "aiPrefs.getString(AI_PREF_API_KEY" in activity
    assert "aiPrefs.getString(AI_PREF_MODEL" in activity
    assert "gpt-5.6-terra" in activity
    assert 'DEFAULT_MODEL = "gpt-5.6-terra"' in host_agent
    assert '"cache_write": 3.125' in host_agent
    assert "prompt_cache_key" in activity
    assert "prompt_cache_breakpoint" in activity
    assert 'content.put("prompt_cache_breakpoint", new JSONObject().put("mode", "explicit"))' in activity
    assert 'payload.put("prompt_cache_options", new JSONObject().put("mode", "explicit").put("ttl", "30m"))' in activity
    assert 'put("type", "prompt_cache_breakpoint")' not in activity
    assert 'payload.put("prompt_cache_retention"' not in activity
    assert '"prompt_cache_options": {"mode": "explicit", "ttl": "30m"}' in host_agent
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
    assert 'PROJECT_DIR = "workshop_project"' in activity
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
    assert "PROJECT_DIR = \"published_project\"" in published_activity
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

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED_FILES = [
    "mobile/android/settings.gradle",
    "mobile/android/build.gradle",
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
    "mobile/android/app/src/main/assets/workshop_sample/src/player.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/enemy.stasis",
    "mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis",
]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8-sig")


def main() -> int:
    missing = [path for path in REQUIRED_FILES if not (ROOT / path).is_file()]
    if missing:
        raise AssertionError(f"missing Android shell files: {missing}")

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
    assert "workshop_sample/" in activity
    assert "createWorkshopView" in activity
    assert "ProjectSnapshot.from" in activity
    assert "parseSymbols" in activity
    assert "groupSymbols" in activity
    assert "createSymbolRow" in activity
    assert "EditText" in activity
    assert "createEditControls" in activity
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
    assert "setFillViewport(true)" in activity
    assert "setOnFocusChangeListener" in activity
    assert "scrollEditorIntoView" in activity
    assert "keyboardSpacer" in activity
    assert "dp(360)" in activity
    assert "smoothScrollTo(0, sourceEditor.getBottom())" in activity
    assert "FastReload: function signature unchanged" in activity
    assert "ResetRequired: function signature changed" in activity
    assert "ResetRequired: struct or layout source changed" in activity
    assert "setContentView(status)" not in activity

    native = read("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
    assert "Java_com_stasislang_workshop_MainActivity_nativeStatus" in native
    assert "Java_com_stasislang_workshop_MainActivity_nativeCompileProject" in native
    assert "scan_stasis_files" in native
    assert "analyze_stasis_file" in native
    assert "validate_braces" in native
    assert "CompilePlanned: files=" in native
    assert "STASIS_COMPILE_MANIFEST_RELATIVE_PATH" in native
    assert "write_compile_manifest" in native
    assert "project_hash=%016llx" in native
    assert "status=CompilePlanned" in native
    assert "CompileError: missing lifecycle root" in native
    assert "Stasis Android native smoke loaded" in native

    cmake = read("mobile/android/app/src/main/cpp/CMakeLists.txt")
    assert "add_library(stasis_mobile_smoke SHARED stasis_mobile_smoke.c)" in cmake

    for sample in STASIS_SAMPLE_FILES:
        source = read(sample)
        assert "function " in source
        assert "fn " not in source
        assert "&mut" not in source

    player = read("mobile/android/app/src/main/assets/workshop_sample/src/player.stasis")
    assert "function update(self: Player, input: InputState): void" in player
    assert "self.jump();" in player

    collision = read("mobile/android/app/src/main/assets/workshop_sample/src/systems/collision.stasis")
    assert "enemy.damage(1);" in collision

    print("android shell structure ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

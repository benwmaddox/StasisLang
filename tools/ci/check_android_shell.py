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
    "mobile/android/README.md",
]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


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
    assert "android:exported=\"true\"" in manifest

    activity = read("mobile/android/app/src/main/java/com/stasislang/workshop/MainActivity.java")
    assert "System.loadLibrary(\"stasis_mobile_smoke\")" in activity
    assert "private static native String nativeStatus()" in activity
    assert "setContentView(status)" in activity

    native = read("mobile/android/app/src/main/cpp/stasis_mobile_smoke.c")
    assert "Java_com_stasislang_workshop_MainActivity_nativeStatus" in native
    assert "Stasis Android native smoke loaded" in native

    cmake = read("mobile/android/app/src/main/cpp/CMakeLists.txt")
    assert "add_library(stasis_mobile_smoke SHARED stasis_mobile_smoke.c)" in cmake

    print("android shell structure ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
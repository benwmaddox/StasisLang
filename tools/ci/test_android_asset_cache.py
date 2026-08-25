"""Compile and run the platform-neutral Android release asset cache scenarios."""

from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
CACHE = ROOT / "mobile/shells/android/app/src/main/java/com/stasislang/shell/StasisAssetCache.java"
TEST = ROOT / "tools/ci/java/com/stasislang/shell/StasisAssetCacheTest.java"


def main() -> int:
    javac = shutil.which("javac")
    java = shutil.which("java")
    if not javac or not java:
        raise RuntimeError("JDK 8+ is required for the direct Android asset-cache runner")
    with tempfile.TemporaryDirectory(prefix="stasis-android-asset-cache-") as directory:
        output = Path(directory) / "classes"
        output.mkdir()
        subprocess.run([javac, "-encoding", "UTF-8", "-d", str(output), str(CACHE), str(TEST)],
                       cwd=ROOT, check=True)
        subprocess.run([java, "-cp", str(output), "com.stasislang.shell.StasisAssetCacheTest"],
                       cwd=ROOT, check=True)
    print("android asset cache JVM runner ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

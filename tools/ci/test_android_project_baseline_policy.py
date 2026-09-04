"""Compile and run the platform-neutral Workshop project-baseline policy scenarios."""

from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / "mobile/android/app/src/workshop/java/com/stasislang/workshop/WorkshopProjectBaselinePolicy.java"
CHECK = ROOT / "tools/ci/java/com/stasislang/workshop/WorkshopProjectBaselinePolicyCheck.java"


def main() -> int:
    javac = shutil.which("javac")
    java = shutil.which("java")
    if not javac or not java:
        raise RuntimeError("JDK 8+ is required for the direct Android project-baseline runner")
    with tempfile.TemporaryDirectory(prefix="stasis-android-project-baseline-") as directory:
        output = Path(directory) / "classes"
        output.mkdir()
        subprocess.run(
            [javac, "-encoding", "UTF-8", "-d", str(output), str(POLICY), str(CHECK)],
            cwd=ROOT,
            check=True,
        )
        subprocess.run(
            [java, "-cp", str(output), "com.stasislang.workshop.WorkshopProjectBaselinePolicyCheck"],
            cwd=ROOT,
            check=True,
        )
    print("android project baseline JVM runner ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

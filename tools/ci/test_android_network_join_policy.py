"""Run the Android native-client provisioning component policy scenarios."""

from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[2]


def main():
    output_root = ROOT / "target" / "network-join-policy"
    output_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output_root) as directory:
        subprocess.run([
            "javac", "-encoding", "UTF-8", "-d", directory,
            ROOT / "mobile/shells/android/app/src/main/java/com/stasislang/shell/NetworkJoinPolicy.java",
            ROOT / "tools/ci/java/com/stasislang/shell/NetworkJoinPolicyTest.java",
        ], cwd=ROOT, check=True, timeout=60)
        subprocess.run([
            "java", "-cp", directory, "com.stasislang.shell.NetworkJoinPolicyTest",
        ], cwd=ROOT, check=True, timeout=60)


if __name__ == "__main__":
    main()

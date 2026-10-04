from __future__ import annotations

import base64
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from tools.ci.materialize_android_test_signing import INIT_SCRIPT, KEYSTORE_NAME, materialize


ROOT = Path(__file__).resolve().parents[2]


class MaterializeAndroidTestSigningTests(unittest.TestCase):
    def test_materializes_only_under_runner_temp_and_exports_nonsecret_paths(self) -> None:
        key_payload = b"opaque-existing-vault-keystore-bytes"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runner_temp = root / "runner-temp"
            github_env = root / "github-env"
            github_env.write_text("", encoding="utf-8")
            result = materialize(
                base64.b64encode(key_payload).decode("ascii"),
                runner_temp=runner_temp,
                workspace=ROOT,
                github_env=github_env,
            )
            self.assertEqual(result, runner_temp.resolve() / KEYSTORE_NAME)
            self.assertEqual(result.read_bytes(), key_payload)
            self.assertIn("STASIS_ANDROID_TEST_KEYSTORE_PATH=", github_env.read_text(encoding="utf-8"))
            self.assertIn(f"STASIS_GRADLE_INIT_SCRIPT={ROOT / INIT_SCRIPT}", github_env.read_text(encoding="utf-8"))
            self.assertNotIn(base64.b64encode(key_payload).decode("ascii"), github_env.read_text(encoding="utf-8"))

    def test_rejects_invalid_or_repeated_materialization(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            github_env = root / "github-env"
            github_env.write_text("", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "valid base64"):
                materialize("not-base64", runner_temp=root / "temp", workspace=ROOT, github_env=github_env)
            materialize(
                base64.b64encode(b"opaque-existing-vault-keystore-bytes").decode("ascii"),
                runner_temp=root / "temp",
                workspace=ROOT,
                github_env=github_env,
            )
            with self.assertRaisesRegex(ValueError, "already exists"):
                materialize(
                    base64.b64encode(b"opaque-existing-vault-keystore-bytes").decode("ascii"),
                    runner_temp=root / "temp",
                    workspace=ROOT,
                    github_env=github_env,
                )

    def test_cli_never_prints_keystore_bytes(self) -> None:
        key_payload = b"opaque-existing-vault-keystore-bytes"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "github-env").write_text("", encoding="utf-8")
            environment = os.environ.copy()
            environment["STASIS_ANDROID_TEST_KEYSTORE_BASE64"] = base64.b64encode(key_payload).decode("ascii")
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "tools/ci/materialize_android_test_signing.py"),
                    "--runner-temp",
                    str(root / "temp"),
                    "--workspace",
                    str(ROOT),
                    "--github-env",
                    str(root / "github-env"),
                ],
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertNotIn(environment["STASIS_ANDROID_TEST_KEYSTORE_BASE64"], result.stdout + result.stderr)
            self.assertNotIn(key_payload.decode("ascii"), result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

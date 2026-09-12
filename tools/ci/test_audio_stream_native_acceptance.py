"""Evidence must describe the bundled runtime selected by the native loader."""

import array
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import run_audio_stream_native_acceptance as acceptance


class NativeAudioRuntimeSelectionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.compiler = self.root / "stasis.exe"
        self.runtime = self.root / "stasis_graphics.dll"
        self.compiler.write_bytes(b"compiler")
        self.runtime.write_bytes(b"sibling runtime")
        self.platform = patch.object(acceptance.sys, "platform", "win32")
        self.platform.start()
        self.addCleanup(self.platform.stop)

    def identity(self):
        return {"ok": True, "result": {
            "executable": {"path": str(self.compiler), "sha256": acceptance.sha256(self.compiler)},
            "graphics_runtime": {"path": str(self.runtime), "sha256": acceptance.sha256(self.runtime)},
        }}

    def test_conflicting_sibling_rejected_before_recording_or_receipt(self):
        selected = self.root / "selected.dll"
        selected.write_bytes(b"requested runtime")
        output = self.root / "evidence"
        with patch.object(acceptance.sys, "argv", ["acceptance", "--stasis", str(self.compiler),
                          "--runtime", str(selected), "--output", str(output)]), \
                patch.object(acceptance.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "stage the requested compiler/runtime pair"):
                acceptance.main()
            run.assert_not_called()
        self.assertFalse(output.exists())

    def test_missing_sibling_cannot_fall_back_to_environment(self):
        selected = self.root / "selected.dll"
        self.runtime.rename(selected)
        with patch.object(acceptance.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "bundled sibling"):
                acceptance.verify_runtime_pair(self.compiler, selected, {})
            run.assert_not_called()

    def test_matching_pair_is_loaded_and_verified_before_use(self):
        with patch.object(acceptance.subprocess, "run", return_value=subprocess.CompletedProcess(
                [], 0, json.dumps(self.identity()))) as run:
            hashes = acceptance.verify_runtime_pair(self.compiler, self.runtime, {})
        self.assertEqual(hashes["runtime"], acceptance.sha256(self.runtime))
        self.assertEqual(run.call_args.args[0], [str(self.compiler), "editor-info", "--json"])
        self.assertTrue(run.call_args.kwargs["check"])

    def test_identity_for_another_runtime_or_hash_is_rejected(self):
        other = self.root / "other.dll"
        other.write_bytes(self.runtime.read_bytes())
        for field, value in [("path", str(other)), ("sha256", "incorrect")]:
            with self.subTest(field=field):
                identity = self.identity()
                identity["result"]["graphics_runtime"][field] = value
                with patch.object(acceptance.subprocess, "run", return_value=subprocess.CompletedProcess(
                        [], 0, json.dumps(identity))):
                    with self.assertRaisesRegex(ValueError, "does not match"):
                        acceptance.verify_runtime_pair(self.compiler, self.runtime, {})

    def test_binary_changed_during_capture_cannot_receive_receipt(self):
        output = self.root / "evidence"
        identity = self.identity()

        def run(command, **kwargs):
            if "editor-info" in command:
                return subprocess.CompletedProcess(command, 0, json.dumps(identity))
            if "record" in command:
                (output / "audio.mp3").write_bytes(b"test recording")
                self.runtime.write_bytes(b"replacement runtime")
            else:
                # Simulated decoded fixture, solely to reach receipt validation.
                pcm = array.array("f")
                for frame in range(96000):
                    value = 0.5 if frame % 100 < 50 else -0.5
                    pcm.extend([value, value * 0.5])
                if acceptance.sys.byteorder != "little":
                    pcm.byteswap()
                (output / "audio.f32le").write_bytes(pcm.tobytes())
            return subprocess.CompletedProcess(command, 0)

        with patch.object(acceptance.sys, "argv", ["acceptance", "--stasis", str(self.compiler),
                          "--runtime", str(self.runtime), "--output", str(output)]), \
                patch.object(acceptance.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(ValueError, "changed during recording"):
                acceptance.main()
        self.assertFalse((output / "receipt.json").exists())

    def test_unloadable_sibling_does_not_allow_recording_fallback(self):
        with patch.object(acceptance.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "editor-info")):
            with self.assertRaises(subprocess.CalledProcessError):
                acceptance.verify_runtime_pair(self.compiler, self.runtime, {})


if __name__ == "__main__":
    unittest.main()

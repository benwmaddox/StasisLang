from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

try:
    import verify_ios_presentation_baseline as verifier
except ImportError:
    from . import verify_ios_presentation_baseline as verifier


def receipt(phase: str) -> dict:
    return {
        "schema": "stasis.ios.presentation_baseline.v1",
        "phase": phase,
        "frame": 1,
        "logical": [640, 360],
        "poison_target": "physical-window",
        "poison_state_restored": True,
        "baseline_state_restored": True,
        "poison_selected_target_state": list(range(20)) * 2,
        "baseline_selected_target_state": list(range(20)) * 2,
        "render": {
            "accepted": 1,
            "rejected": 0,
            "presented": 1,
            "validation": 0,
            "command_trace": 123,
        },
        "resource": {
            "state": 1,
            "surface_generation": 1,
            "renderer_generation": 1,
            "restore_failures": 0,
        },
        "drawable_viewport": [0, 0, 640, 360],
        "safe_drawable": [0, 0, 640, 360],
    }


def canonical_frame() -> bytes:
    width, height = 640, 360
    rgba = bytearray(bytes((0, 0, 0, 255)) * width * height)
    for y in range(45, 135):
        for x in range(80, 240):
            offset = (y * width + x) * 4
            rgba[offset : offset + 4] = bytes((230, 38, 20, 255))
    return bytes(rgba)


class IosPresentationBaselineVerificationTests(unittest.TestCase):
    def test_accepts_landscape_and_transposed_simulator_encodings(self) -> None:
        rgba = canonical_frame()
        portrait_width, portrait_height, portrait = verifier.rotate_rgba(
            640, 360, rgba, True
        )
        with tempfile.TemporaryDirectory() as directory:
            frame = Path(directory) / "frame.png"
            frame.write_bytes(b"capture-hash-fixture")
            for encoded, expected in (
                ((640, 360, rgba), "app-landscape"),
                (
                    (portrait_width, portrait_height, portrait),
                    "hardware-native-portrait-counterclockwise",
                ),
            ):
                with self.subTest(expected=expected), mock.patch.object(
                    verifier, "read_capture", return_value=encoded
                ):
                    evidence = verifier.verify_frame(frame, receipt("initial"))
                    self.assertEqual(evidence["encoding"], expected)
                    self.assertEqual(evidence["poisoned_magenta_pixels"], 0)
                    self.assertEqual(evidence["normalized_size"], [640, 360])

    def test_rejects_surviving_physical_poison(self) -> None:
        rgba = bytearray(canonical_frame())
        rgba[0:4] = bytes((255, 0, 255, 255))
        with tempfile.TemporaryDirectory() as directory:
            frame = Path(directory) / "frame.png"
            frame.write_bytes(b"capture-hash-fixture")
            with mock.patch.object(
                verifier, "read_capture", return_value=(640, 360, bytes(rgba))
            ), self.assertRaisesRegex(ValueError, "magenta"):
                verifier.verify_frame(frame, receipt("initial"))

    def test_rejects_receipt_without_accepted_present(self) -> None:
        value = receipt("initial")
        value["render"]["presented"] = 0
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            import json

            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "accepted and presented"):
                verifier.load_receipt(path, "initial")

    def test_rejects_raw_state_mismatch_even_when_boolean_claims_restored(self) -> None:
        value = receipt("initial")
        value["poison_selected_target_state"][39] += 1
        self.assertIs(value["poison_state_restored"], True)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            import json

            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exact restored state"):
                verifier.load_receipt(path, "initial")

    def test_rejects_non_i32_raw_state_words(self) -> None:
        for invalid_word in (True, 1 << 31, -(1 << 31) - 1):
            with self.subTest(invalid_word=invalid_word):
                value = receipt("initial")
                value["baseline_selected_target_state"][0] = invalid_word
                value["baseline_selected_target_state"][20] = invalid_word
                with tempfile.TemporaryDirectory() as directory:
                    path = Path(directory) / "receipt.json"
                    import json

                    path.write_text(json.dumps(value), encoding="utf-8")
                    with self.assertRaisesRegex(ValueError, "non-i32 state word"):
                        verifier.load_receipt(path, "initial")


if __name__ == "__main__":
    unittest.main()

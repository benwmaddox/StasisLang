from __future__ import annotations

import unittest

try:
    from verify_presentation_baseline import (
        verify_alpha_probe,
        verify_capture,
        verify_reset_placeholder,
    )
except ImportError:
    from .verify_presentation_baseline import (
        verify_alpha_probe,
        verify_capture,
        verify_reset_placeholder,
    )


class PresentFrameVerificationTests(unittest.TestCase):
    def test_accepts_black_canvas_with_red_scene_and_no_magenta_poison(self) -> None:
        width, height = 640, 360
        rgba = bytearray(bytes((0, 0, 0, 255)) * (width * height))
        for y in range(45, 135):
            for x in range(80, 240):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((230, 38, 20, 255))

        evidence = verify_capture(
            width, height, bytes(rgba), (0, 0, width, height), (0, 0, width, height)
        )

        self.assertEqual(evidence["poisoned_magenta_pixels"], 0)
        self.assertEqual(evidence["logical_background_black_samples"][0],
                         evidence["logical_background_black_samples"][1])

    def test_rejects_any_magenta_pixel_even_outside_sampling_grid(self) -> None:
        width, height = 640, 360
        rgba = bytearray(bytes((0, 0, 0, 255)) * (width * height))
        for y in range(45, 135):
            for x in range(80, 240):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((230, 38, 20, 255))
        offset = (1 * width + 1) * 4
        rgba[offset : offset + 4] = bytes((255, 0, 255, 255))

        with self.assertRaisesRegex(ValueError, "poisoned magenta pixels: 1"):
            verify_capture(
                width, height, bytes(rgba), (0, 0, width, height),
                (0, 0, width, height),
            )

    def test_alpha_probe_requires_blend_over_the_accepted_red_frame(self) -> None:
        width, height = 640, 360
        rgba = bytearray(bytes((0, 0, 0, 255)) * (width * height))
        for y in range(45, 135):
            for x in range(80, 240):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((230, 38, 20, 255))
        for y in range(55, 75):
            for x in range(90, 110):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((115, 147, 10, 255))

        evidence = verify_alpha_probe(
            width, height, bytes(rgba), (0, 0, width, height), (0, 0, width, height)
        )

        self.assertEqual(evidence["schema"], "stasis.workshop_present_only_alpha_capture.v1")
        self.assertEqual(evidence["alpha_probe_rgba"], [115, 147, 10, 255])

    def test_alpha_probe_rejects_disabled_blending_after_snapshot_replay(self) -> None:
        width, height = 640, 360
        rgba = bytearray(bytes((0, 0, 0, 255)) * (width * height))
        for y in range(45, 135):
            for x in range(80, 240):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((230, 38, 20, 255))
        for y in range(55, 75):
            for x in range(90, 110):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((0, 128, 0, 255))

        with self.assertRaisesRegex(ValueError, "did not blend over"):
            verify_alpha_probe(
                width, height, bytes(rgba), (0, 0, width, height),
                (0, 0, width, height),
            )


class ResetPlaceholderVerificationTests(unittest.TestCase):
    def test_accepts_initialized_dark_surface_with_blue_restore_label(self) -> None:
        width = height = 40
        rgba = bytearray(bytes((15, 20, 28, 255)) * (width * height))
        for y in range(17, 22):
            for x in range(17, 22):
                offset = (y * width + x) * 4
                rgba[offset : offset + 4] = bytes((66, 153, 225, 255))

        evidence = verify_reset_placeholder(width, height, bytes(rgba), (0, 0, width, height))

        self.assertEqual(evidence["schema"], "stasis.workshop_present_only_reset_capture.v1")
        self.assertEqual(evidence["restore_label_pixels"], 25)
        self.assertEqual(evidence["poisoned_magenta_pixels"], 0)

    def test_rejects_a_poisoned_or_uninitialized_reset_surface(self) -> None:
        width = height = 40
        rgba = bytearray(bytes((15, 20, 28, 255)) * (width * height))
        offset = (10 * width + 10) * 4
        rgba[offset : offset + 4] = bytes((255, 0, 255, 255))

        with self.assertRaisesRegex(ValueError, "poisoned magenta"):
            verify_reset_placeholder(width, height, bytes(rgba), (0, 0, width, height))


if __name__ == "__main__":
    unittest.main()

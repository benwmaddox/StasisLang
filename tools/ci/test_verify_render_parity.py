import json
import struct
import tempfile
import unittest
from pathlib import Path

from tools.ci.verify_render_parity import (
    ATLAS_SPRITE_HANDLES,
    DEFAULT_MANIFEST,
    _atlas_sprite_handles,
    read_capture,
    validate_fixture,
    verify_capture,
    verify_runtime_evidence,
    write_stage_evidence,
)


def write_bmp(path: Path, width: int, height: int, rgba: bytes) -> None:
    row_bytes = width * 4
    payload = bytearray()
    for y in range(height - 1, -1, -1):
        for x in range(width):
            offset = (y * width + x) * 4
            r, g, b, a = rgba[offset : offset + 4]
            payload.extend((b, g, r, a))
    header = bytearray(54)
    header[:2] = b"BM"
    struct.pack_into("<I", header, 2, 54 + len(payload))
    struct.pack_into("<I", header, 10, 54)
    struct.pack_into("<IiiHH", header, 14, 40, width, height, 1, 32)
    struct.pack_into("<I", header, 34, row_bytes * height)
    path.write_bytes(header + payload)


class RenderParityGateTest(unittest.TestCase):
    def test_checked_in_fixture_is_complete(self):
        manifest = validate_fixture(DEFAULT_MANIFEST)
        self.assertEqual(manifest["logical_size"], [640, 360])
        self.assertEqual(len(manifest["stages"]), 4)
        self.assertIn("android_emulator", manifest["capture_profiles"])

    def test_fixture_uses_only_canonical_resolved_atlas_resources(self):
        frame = (DEFAULT_MANIFEST.parent / "frame.stasis").read_text(encoding="utf-8")
        self.assertEqual(_atlas_sprite_handles(frame), ATLAS_SPRITE_HANDLES)
        mutated = frame.replace("canvas_sprite, 0, 96", "missing_sprite, 0, 96", 1)
        self.assertNotEqual(_atlas_sprite_handles(mutated), ATLAS_SPRITE_HANDLES)

    def test_bmp_reader_and_exact_capture_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "frame.bmp"
            rgba = bytes((10, 20, 30, 255, 40, 50, 60, 255))
            write_bmp(capture, 2, 1, rgba)
            self.assertEqual(read_capture(capture), (2, 1, rgba))
            manifest = {
                "logical_size": [2, 1],
                "capture_profiles": {
                    "exact": {
                        "comparison": "exact",
                        "sha256_rgba": __import__("hashlib").sha256(rgba).hexdigest(),
                    }
                },
            }
            self.assertEqual(
                verify_capture(manifest, capture, "exact"),
                (manifest["capture_profiles"]["exact"]["sha256_rgba"], None),
            )

    def test_region_failure_names_the_stage_region(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "frame.bmp"
            write_bmp(capture, 2, 1, bytes((0, 0, 0, 255)) * 2)
            manifest = {
                "logical_size": [2, 1],
                "capture_profiles": {
                    "portable": {
                        "comparison": "regions",
                        "regions": [{
                            "name": "sprite_upload",
                            "rect": [0, 0, 2, 1],
                            "rgba": [255, 0, 255, 255],
                            "max_channel_delta": 0,
                            "min_coverage": 1.0,
                        }],
                    }
                },
            }
            with self.assertRaisesRegex(ValueError, "sprite_upload"):
                verify_capture(manifest, capture, "portable")

    def test_atlas_canvas_region_requires_blue_gradient_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "frame.bmp"
            blue_gradient = bytes((31, 45, 60, 255))
            write_bmp(capture, 2, 1, blue_gradient * 2)
            manifest = {
                "logical_size": [2, 1],
                "capture_profiles": {
                    "portable": {
                        "comparison": "regions",
                        "regions": [{
                            "name": "atlas_canvas_sprite",
                            "rect": [0, 0, 2, 1],
                            "predicate": "atlas_canvas",
                            "min_coverage": 1.0,
                        }],
                    }
                },
            }
            verify_capture(manifest, capture, "portable")

    def test_letterboxed_capture_uses_explicit_viewport(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "device.bmp"
            black = bytes((0, 0, 0, 255))
            green = bytes((20, 200, 80, 255))
            write_bmp(capture, 4, 4, black * 8 + green * 8)
            manifest = {
                "logical_size": [2, 2],
                "capture_profiles": {
                    "portable": {
                        "comparison": "regions",
                        "regions": [{
                            "name": "scene",
                            "rect": [0, 0, 2, 2],
                            "rgba": [20, 200, 80, 255],
                            "max_channel_delta": 0,
                            "min_coverage": 1.0,
                        }],
                    }
                },
            }
            _, selected = verify_capture(
                manifest, capture, "portable", [0, 2, 4, 2], 2
            )
            self.assertEqual(selected, [0, 2, 4, 2])

    def test_bounded_vertical_search_recovers_offset_viewport(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "device.bmp"
            black = bytes((0, 0, 0, 255))
            green = bytes((20, 200, 80, 255))
            write_bmp(capture, 4, 6, black * 12 + green * 8 + black * 4)
            manifest = {
                "logical_size": [2, 2],
                "capture_profiles": {
                    "portable": {
                        "comparison": "regions",
                        "regions": [{
                            "name": "scene",
                            "rect": [0, 0, 2, 2],
                            "rgba": [20, 200, 80, 255],
                            "max_channel_delta": 0,
                            "min_coverage": 1.0,
                        }],
                    }
                },
            }
            _, selected = verify_capture(
                manifest, capture, "portable", [0, 1, 4, 2], 2
            )
            self.assertEqual(selected, [0, 3, 4, 2])

    def test_bounded_vertical_search_rejects_out_of_radius_offset(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "device.bmp"
            black = bytes((0, 0, 0, 255))
            green = bytes((20, 200, 80, 255))
            write_bmp(capture, 4, 6, black * 12 + green * 8 + black * 4)
            manifest = {
                "logical_size": [2, 2],
                "capture_profiles": {
                    "portable": {
                        "comparison": "regions",
                        "regions": [{
                            "name": "scene",
                            "rect": [0, 0, 2, 2],
                            "rgba": [20, 200, 80, 255],
                            "max_channel_delta": 0,
                            "min_coverage": 1.0,
                        }],
                    }
                },
            }
            with self.assertRaisesRegex(ValueError, "no viewport matched"):
                verify_capture(manifest, capture, "portable", [0, 0, 4, 2], 1)

    def test_bounded_vertical_search_rejects_invalid_radius_and_base(self):
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "device.bmp"
            write_bmp(capture, 2, 2, bytes((20, 200, 80, 255)) * 4)
            manifest = {
                "logical_size": [2, 2],
                "capture_profiles": {
                    "portable": {"comparison": "regions", "regions": []}
                },
            }
            with self.assertRaisesRegex(ValueError, "nonnegative"):
                verify_capture(manifest, capture, "portable", [0, 0, 2, 2], -1)
            with self.assertRaisesRegex(ValueError, "out of bounds"):
                verify_capture(manifest, capture, "portable", [0, 1, 2, 2], 1)

    def test_bad_stage_matrix_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = json.loads(DEFAULT_MANIFEST.read_text(encoding="utf-8"))
            manifest["stages"] = ["initial_launch"]
            temporary = Path(directory) / "manifest.json"
            temporary.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "matrix"):
                validate_fixture(temporary)

    def test_runtime_evidence_rejects_zero_trace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capture = root / "capture.png"
            log = root / "runtime.log"
            evidence = root / "evidence.json"
            capture.write_bytes(b"captured frame")
            log.write_text(
                "Stasis render contract v7 trace=0 flags=3 lines=2 rects=1 sprites=5 text=2\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "zero command trace"):
                write_stage_evidence(capture, log, "initial_launch", evidence)

    def test_runtime_evidence_requires_generation_advance_and_foreground_restore(self):
        manifest = {}
        log = """Stasis display metrics: logical=1280x720 native=2400x1080 drawable=2400x1080 scale=1.50
gfx_load_sprite: /fixture/assets/opaque.svg (96x72) -> handle=1 raster=144x108 backend=sdl
gfx_load_sprite: /fixture/assets/translucent.svg (96x72) -> handle=2 raster=144x108 backend=sdl
gfx_load_sprite: /fixture/assets/full_canvas.svg (640x360) -> handle=3 raster=960x540 backend=sdl
stasis_load_font: loaded /fixture/assets/parity.ttf logical_size=24 raster_size=36 scale=1.50 handle=1
Stasis render contract v7 trace=1853793133 flags=3 lines=2 rects=1 sprites=5 text=2
Stasis renderer resources restored: backend=sdl surface_generation=3 renderer_generation=1 reason=surface_changed sprites=3
Stasis renderer resources restored: backend=sdl surface_generation=4 renderer_generation=1 reason=surface_changed sprites=3
Stasis renderer resources restored: backend=sdl surface_generation=5 renderer_generation=2 reason=foreground sprites=3
"""
        with tempfile.TemporaryDirectory() as directory:
            runtime_log = Path(directory) / "runtime.log"
            capture = Path(directory) / "capture.png"
            evidence = Path(directory) / "evidence.json"
            capture.write_bytes(b"captured frame")
            runtime_log.write_text(
                log + f"Stasis parity capture: stage=resize_or_density_change path={capture} frame=2 backend=sdl surface_generation=4 renderer_generation=1\n",
                encoding="utf-8",
            )
            write_stage_evidence(capture, runtime_log, "resize_or_density_change", evidence)
            verify_runtime_evidence(
                manifest, runtime_log, capture, evidence, "resize_or_density_change", True
            )
            runtime_log.write_text(
                log + f"Stasis parity capture: stage=resource_restore path={capture} frame=2 backend=sdl surface_generation=5 renderer_generation=2\n",
                encoding="utf-8",
            )
            write_stage_evidence(capture, runtime_log, "resource_restore", evidence)
            verify_runtime_evidence(
                manifest, runtime_log, capture, evidence, "resource_restore"
            )
            capture.write_bytes(b"different frame")
            with self.assertRaisesRegex(ValueError, "bound"):
                verify_runtime_evidence(
                    manifest, runtime_log, capture, evidence, "resource_restore"
                )

    def test_launch_stages_accept_initial_sdl3_resource_generation(self):
        manifest = {}
        log = """Stasis display metrics: logical=800x600 native=800x600 drawable=800x600 scale=1.00
gfx_load_sprite: /fixture/assets/opaque.svg (96x72) -> handle=1 raster=96x72 backend=sdl
gfx_load_sprite: /fixture/assets/translucent.svg (96x72) -> handle=2 raster=96x72 backend=sdl
gfx_load_sprite: /fixture/assets/full_canvas.svg (640x360) -> handle=3 raster=640x360 backend=sdl
stasis_load_font: loaded /fixture/assets/parity.ttf logical_size=24 raster_size=24 scale=1.00 handle=1
Stasis render contract v7 trace=123456789 flags=3 lines=2 rects=1 sprites=5 text=2
"""
        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / "capture.png"
            runtime_log = Path(directory) / "runtime.log"
            evidence = Path(directory) / "evidence.json"
            capture.write_bytes(b"captured frame")
            for stage, frame in (("initial_launch", 1), ("second_frame", 2)):
                with self.subTest(stage=stage):
                    runtime_log.write_text(
                        log
                        + f"Stasis parity capture: stage={stage} path={capture} frame={frame} "
                        "backend=sdl surface_generation=1 renderer_generation=1\n",
                        encoding="utf-8",
                    )
                    write_stage_evidence(capture, runtime_log, stage, evidence)
                    verify_runtime_evidence(
                        manifest, runtime_log, capture, evidence, stage, True
                    )
            runtime_log.write_text(
                log
                + f"Stasis parity capture: stage=initial_launch path={capture} frame=2 "
                "backend=sdl surface_generation=1 renderer_generation=1\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "wrong frame"):
                write_stage_evidence(capture, runtime_log, "initial_launch", evidence)
            runtime_log.write_text(
                log
                + "Stasis renderer resources restored: backend=sdl surface_generation=2 "
                "renderer_generation=2 reason=foreground sprites=3\n"
                + f"Stasis parity capture: stage=second_frame path={capture} frame=2 "
                "backend=sdl surface_generation=1 renderer_generation=1\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "unknown generations"):
                write_stage_evidence(capture, runtime_log, "second_frame", evidence)


if __name__ == "__main__":
    unittest.main()

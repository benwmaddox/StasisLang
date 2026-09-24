import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VERIFY = ROOT / "tools" / "ci" / "verify_ios_aspect_fit.py"


def receipt(stage: str, injected_x: float, injected_w: float, generation: int) -> dict:
    safe_x = injected_x * 3.0
    safe_w = injected_w * 3.0
    safe_h = 380.0 * 3.0
    viewport_h = float(int(safe_w * 720.0 / 1600.0))
    pointer = {
        "id": 1 if stage == "pointer" else 0,
        "down": 1 if stage == "pointer" else 0,
        "went_down": 1 if stage == "pointer" else 0,
        "went_up": 0,
        "x": 160.0 if stage == "pointer" else 0.0,
        "y": 72.0 if stage == "pointer" else 0.0,
        "x_normalized": 0.1 if stage == "pointer" else 0.0,
        "y_normalized": 0.1 if stage == "pointer" else 0.0,
    }
    return {
        "schema": "stasis.ios.aspect_fit.v1",
        "stage": stage,
        "logical": [1600, 720],
        "native": [874, 402],
        "drawable": [2622, 1206],
        "safe_logical": [0, 0, 1600, 720],
        "native_viewport": [injected_x, 11.0, injected_w, 365.0],
        "drawable_viewport": [safe_x, (safe_h - viewport_h) / 2.0, safe_w, viewport_h],
        "safe_drawable": [safe_x, 0.0, safe_w, safe_h],
        "content_scale": safe_w / 1600.0,
        "raster_scale": 2.0,
        "display_generation": generation,
        "density_generation": 2,
        "injected_safe_native": [injected_x, 0, injected_w, 380],
        "pointer": pointer,
    }


def write_png_header(path: Path, width: int, height: int, marker: bytes) -> None:
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + struct.pack(">I", 13)
        + b"IHDR"
        + struct.pack(">II", width, height)
        + marker
    )


class VerifyIosAspectFitTests(unittest.TestCase):
    def run_fixture(
        self, mutate=None, mutate_screenshots=None
    ) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            values = {
                "actual": receipt("actual", 0.0, 874.0, 1),
                "left": receipt("landscape-left", 62.0, 812.0, 2),
                "pointer": receipt("pointer", 62.0, 812.0, 2),
                "right": receipt("landscape-right", 0.0, 812.0, 3),
            }
            values["actual"].update(
                {
                    "native_viewport": [0.0, 4.0, 874.0, 393.0],
                    "drawable_viewport": [0.0, 13.0, 2622.0, 1180.0],
                    "safe_drawable": [0.0, 0.0, 2622.0, 1206.0],
                    "injected_safe_native": [0, 0, 0, 0],
                }
            )
            if mutate:
                mutate(values)
            paths = {}
            for name, value in values.items():
                path = root / f"{name}.json"
                path.write_text(json.dumps(value), encoding="utf-8")
                paths[name] = path
            left_png = root / "left.png"
            right_png = root / "right.png"
            write_png_header(left_png, 1206, 2622, b"left")
            write_png_header(right_png, 1206, 2622, b"right")
            if mutate_screenshots:
                mutate_screenshots(left_png, right_png)
            output = root / "evidence.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(VERIFY),
                    "--actual",
                    str(paths["actual"]),
                    "--left",
                    str(paths["left"]),
                    "--pointer",
                    str(paths["pointer"]),
                    "--right",
                    str(paths["right"]),
                    "--left-screenshot",
                    str(left_png),
                    "--right-screenshot",
                    str(right_png),
                    "--output",
                    str(output),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            if result.returncode == 0:
                evidence = json.loads(output.read_text(encoding="utf-8"))
                self.assertFalse(evidence["physical_device_qualified"])
                self.assertTrue(evidence["injected_safe_area_qualified"])
                self.assertFalse(evidence["actual_simulator_safe_area_inset_observed"])
                self.assertEqual(
                    evidence["screenshots"]["landscape_left"]["encoding"],
                    "hardware-native-portrait",
                )
            return result

    def test_accepts_fitted_cutout_and_pointer_receipts(self) -> None:
        result = self.run_fixture()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_pointer_drift(self) -> None:
        result = self.run_fixture(
            lambda values: values["pointer"]["pointer"].update({"x": 159.0})
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pointer x=", result.stderr)

    def test_rejects_injected_cutout_that_does_not_reach_presentation(self) -> None:
        result = self.run_fixture(
            lambda values: values["left"].update(
                {"safe_drawable": [0.0, 0.0, 2622.0, 1206.0]}
            )
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("injected safe drawable", result.stderr)

    def test_rejects_byte_identical_stage_screenshots(self) -> None:
        result = self.run_fixture(
            mutate_screenshots=lambda left, right: right.write_bytes(
                left.read_bytes()
            )
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("byte-identical", result.stderr)

    def test_slow_ci_owns_arm64_simulator_evidence(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "pr-ci.yml").read_text(
            encoding="utf-8"
        )
        script = (ROOT / "tools" / "ci" / "build_ios_package.sh").read_text(
            encoding="utf-8"
        )
        project = (
            ROOT
            / "mobile"
            / "shells"
            / "ios"
            / "StasisMobile.xcodeproj"
            / "project.pbxproj"
        ).read_text(encoding="utf-8")
        self.assertIn("ios-package-link:", workflow)
        self.assertIn("runs-on: macos-15", workflow)
        self.assertIn("simulator-evidence.json", workflow)
        self.assertIn("ios-arm64_x86_64-simulator", script)
        self.assertNotIn("-sdk iphonesimulator -arch arm64", script)
        self.assertNotIn("-destination 'generic/platform=iOS Simulator'", script)
        self.assertIn('lipo "${simulator_executable}" -verify_arch arm64', script)
        self.assertIn("ARCHS = arm64", project)
        self.assertIn("ios_aspect_fit_bindings.c", script)
        self.assertIn("Intentionally empty simulator replacement object", script)
        self.assertNotIn("stasis_ios_simulator_placeholder(void)", script)
        self.assertIn("physical_device_qualified=false", script)
        fixture = (ROOT / "tools" / "ci" / "ios_aspect_fit_bindings.c").read_text(
            encoding="utf-8"
        )
        runtime = (ROOT / "runtime" / "stasis_graphics.c").read_text(encoding="utf-8")
        self.assertIn(
            "#if defined(SDL_PLATFORM_IOS) || defined(__IPHONEOS__)", runtime
        )
        self.assertIn("#define STASIS_PLATFORM_IOS 1", runtime)
        self.assertIn(
            "#if defined(__ANDROID__) || defined(STASIS_PLATFORM_IOS)", runtime
        )
        self.assertIn("STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT", fixture)
        self.assertIn("hash_path(\"gfx_cmd_i32\")", fixture)
        self.assertIn("frame == 91 && !write_receipt(\"pointer\")", fixture)
        self.assertIn("frame == 600", fixture)
        self.assertIn("frame == 630 && !write_receipt(\"landscape-right\")", fixture)
        self.assertIn("const int right_stage = frame >= 600;", fixture)


if __name__ == "__main__":
    unittest.main()

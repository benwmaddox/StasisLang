import json
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VERIFY = ROOT / "tools" / "ci" / "verify_ios_aspect_fit.py"


def receipt(stage: str, safe_x: float, safe_w: float, generation: int) -> dict:
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
        "native": [844, 390],
        "drawable": [2532, 1170],
        "safe_logical": [0, 0, 1600, 720],
        "native_viewport": [59, 15.5, 785, 353.25],
        "drawable_viewport": [safe_x, 25.0, safe_w, safe_w * 720.0 / 1600.0],
        "safe_drawable": [safe_x, 0.0, safe_w, 1107.0],
        "content_scale": safe_w / 1600.0,
        "raster_scale": 2.0,
        "display_generation": generation,
        "density_generation": 2,
        "injected_safe_native": [0, 0, 785, 369],
        "pointer": pointer,
    }


def write_png_header(path: Path, width: int, height: int) -> None:
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + struct.pack(">I", 13)
        + b"IHDR"
        + struct.pack(">II", width, height)
    )


class VerifyIosAspectFitTests(unittest.TestCase):
    def run_fixture(self, mutate=None) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            values = {
                "actual": receipt("actual", 177.0, 2178.0, 1),
                "left": receipt("landscape-left", 177.0, 2355.0, 2),
                "pointer": receipt("pointer", 177.0, 2355.0, 2),
                "right": receipt("landscape-right", 0.0, 2355.0, 3),
            }
            if mutate:
                mutate(values)
            paths = {}
            for name, value in values.items():
                path = root / f"{name}.json"
                path.write_text(json.dumps(value), encoding="utf-8")
                paths[name] = path
            left_png = root / "left.png"
            right_png = root / "right.png"
            write_png_header(left_png, 2532, 1170)
            write_png_header(right_png, 2532, 1170)
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

    def test_slow_ci_owns_arm64_simulator_evidence(self) -> None:
        workflow = (ROOT / ".github" / "workflows" / "pr-ci.yml").read_text(
            encoding="utf-8"
        )
        script = (ROOT / "tools" / "ci" / "build_ios_package.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn("ios-package-link:", workflow)
        self.assertIn("runs-on: macos-15", workflow)
        self.assertIn("simulator-evidence.json", workflow)
        self.assertIn("ios-arm64_x86_64-simulator", script)
        self.assertIn("ios_aspect_fit_bindings.c", script)
        self.assertIn("Intentionally empty simulator replacement object", script)
        self.assertNotIn("stasis_ios_simulator_placeholder(void)", script)
        self.assertIn("physical_device_qualified=false", script)
        fixture = (ROOT / "tools" / "ci" / "ios_aspect_fit_bindings.c").read_text(
            encoding="utf-8"
        )
        self.assertIn("STASIS_RENDER_FLAG_CLEAR | STASIS_RENDER_FLAG_PRESENT", fixture)
        self.assertIn("hash_path(\"gfx_cmd_i32\")", fixture)
        self.assertIn("frame == 91 && !write_receipt(\"pointer\")", fixture)


if __name__ == "__main__":
    unittest.main()

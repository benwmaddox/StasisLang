import tempfile
import copy
import json
import subprocess
import sys
import unittest
from pathlib import Path

import numpy as np

from evaluate import audit, compare, validate_acceptance


class Gates(unittest.TestCase):
    def test_recorded_acceptance_matrix_passes(self):
        report = json.loads((Path(__file__).parent / "evidence" / "renders.json").read_text())
        validate_acceptance(report)

    def test_matrix_rejects_inconsistent_aggregate(self):
        report = json.loads((Path(__file__).parent / "evidence" / "renders.json").read_text())
        report["assets"]["character"]["targets"]["256x256"]["palette"]["pass"] = False
        with self.assertRaisesRegex(ValueError, "Inconsistent render result"):
            validate_acceptance(report)

    def test_matrix_deviation_exits_nonzero_with_python_optimization(self):
        command = (
            "import json; from pathlib import Path; from evaluate import validate_acceptance; "
            "r=json.loads(Path('evidence/renders.json').read_text()); "
            "c=r['assets']['character']['targets']['256x256']; "
            "c['precision1']=c['baseline']; validate_acceptance(r)"
        )
        result = subprocess.run([sys.executable, "-O", "-c", command],
                                cwd=Path(__file__).parent, capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Render acceptance matrix changed", result.stderr)

    def test_acceptance_matrix_rejects_changed_outcomes_and_coverage(self):
        recorded = json.loads((Path(__file__).parent / "evidence" / "renders.json").read_text())
        for candidate, target, background, pixels in (
            ("palette", "256x256", "transparent", 1),
            ("palette", "256x256", "black", 1),
            ("palette", "256x256", "white", 1),
            ("precision1", "256x256", "transparent", 0),
            ("numeric", "512x512", "black", 0),
        ):
            with self.subTest(candidate=candidate, background=background):
                report = copy.deepcopy(recorded)
                result = report["assets"]["character"]["targets"][target][candidate]
                result["comparison"][background]["changed_pixels"] = pixels
                result["pass"] = all(v["changed_pixels"] == 0 for v in result["comparison"].values())
                with self.assertRaisesRegex(ValueError, "Render acceptance matrix changed"):
                    validate_acceptance(report)
        for level in ("asset", "target", "candidate", "background", "extra", "negative_control"):
            with self.subTest(level=level):
                report = copy.deepcopy(recorded)
                candidates = report["assets"]["character"]["targets"]["256x256"]
                if level == "asset":
                    del report["assets"]["screen"]
                elif level == "target":
                    del report["assets"]["character"]["targets"]["512x512"]
                elif level == "candidate":
                    del candidates["palette"]
                elif level == "background":
                    del candidates["palette"]["comparison"]["white"]
                elif level == "extra":
                    candidates["unexpected"] = copy.deepcopy(candidates["palette"])
                else:
                    candidates["precision1"] = copy.deepcopy(candidates["baseline"])
                with self.assertRaisesRegex(ValueError, "Render acceptance matrix changed"):
                    validate_acceptance(report)

    def test_alpha_only_change_fails_transparent_and_white(self):
        baseline = np.array([[[0, 0, 0, 0]]], dtype=np.uint8)
        changed = np.array([[[0, 0, 0, 1]]], dtype=np.uint8)
        result = compare(baseline, changed)
        self.assertEqual(result["transparent"]["changed_pixels"], 1)
        self.assertEqual(result["white"]["changed_pixels"], 1)
        self.assertEqual(result["black"]["changed_pixels"], 0)

    def test_equal_pixels_pass_all_backgrounds(self):
        pixels = np.array([[[10, 20, 30, 40], [255, 255, 255, 255]]], dtype=np.uint8)
        self.assertTrue(all(v["changed_pixels"] == 0 for v in compare(pixels, pixels).values()))

    def test_audit_rejects_external_use_and_raster(self):
        for content in ('<use href="https://example.invalid/a.svg"/>', '<image/>', '<script/>',
                        '<path fill="url(#gradient)"/>', '<style>@import "external.css";</style>'):
            with self.subTest(content=content), tempfile.TemporaryDirectory() as directory:
                file = Path(directory) / "bad.svg"
                file.write_text(f'<svg xmlns="http://www.w3.org/2000/svg">{content}</svg>')
                with self.assertRaises(ValueError):
                    audit(file)

    def test_corpus_path_counts_and_intrinsic_dimensions(self):
        for name, count, height in (("character", 300, "128"), ("screen", 1500, "277")):
            result = audit(Path(__file__).parent / "corpus" / f"{name}.svg")
            self.assertEqual(result["tags"]["path"], count)
            self.assertEqual((result["width"], result["height"]), ("128", height))


if __name__ == "__main__":
    unittest.main()

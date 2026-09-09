import tempfile
import unittest
from pathlib import Path

import numpy as np

from evaluate import audit, compare


class Gates(unittest.TestCase):
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

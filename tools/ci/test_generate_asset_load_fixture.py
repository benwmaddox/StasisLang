import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tools.ci.generate_asset_load_fixture import (
    FONT_COUNT,
    LARGE_PNG_COUNT,
    MANIFEST_ENTRY_COUNT,
    PHRASE_COUNT,
    SMALL_PNG_COUNT,
    SVG_COUNT,
    generate_fixture,
)


class AssetLoadFixtureTests(unittest.TestCase):
    def test_counts_hashes_and_repeatability(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "first"
            second = root / "second"
            generate_fixture(first)
            generate_fixture(second)
            manifest = json.loads((first / "assets/manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["schema"], "stasis-assets")
            self.assertEqual(manifest["version"], 2)
            self.assertEqual(len(manifest["assets"]), MANIFEST_ENTRY_COUNT)
            self.assertEqual(len(list((first / "assets/fonts").glob("*.ttf"))), FONT_COUNT)
            self.assertEqual(len(list((first / "assets/sprites").glob("small_*.png"))), SMALL_PNG_COUNT)
            self.assertEqual(len(list((first / "assets/sprites").glob("vector_*.svg"))), SVG_COUNT)
            self.assertEqual(len(list((first / "assets/sprites").glob("large_*.png"))), LARGE_PNG_COUNT)
            phrases = json.loads((first / "phrases.json").read_text(encoding="utf-8"))["phrases"]
            self.assertEqual(len(phrases), PHRASE_COUNT)
            for entry in manifest["assets"]:
                path = first / entry["path"]
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), entry["content_sha256"])
                kind = entry["format"]["kind"]
                if kind == "font":
                    self.assertEqual(entry["format"]["encoding"], "ttf")
                else:
                    self.assertEqual(kind, "sprite")
                    self.assertIn(entry["format"]["encoding"], {"png", "svg"})
                    self.assertGreater(entry["format"]["width"], 0)
                    self.assertGreater(entry["format"]["height"], 0)
            first_files = sorted(path.relative_to(first) for path in first.rglob("*"))
            second_files = sorted(path.relative_to(second) for path in second.rglob("*"))
            self.assertEqual(first_files, second_files)
            for relative in first_files:
                if (first / relative).is_file():
                    self.assertEqual((first / relative).read_bytes(), (second / relative).read_bytes())

    def test_requires_empty_output_and_isolated_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "fixture"
            generate_fixture(root)
            with self.assertRaises(ValueError):
                generate_fixture(root)
            self.assertFalse((root / "assets/manifest.json").is_symlink())
        self.assertFalse(root.exists())


if __name__ == "__main__":
    unittest.main()

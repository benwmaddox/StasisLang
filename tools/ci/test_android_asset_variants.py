import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tools.ci.mutate_android_asset_variant import VARIANTS, mutate_asset_tree


class AndroidAssetVariantTests(unittest.TestCase):
    def make_tree(self) -> Path:
        temporary = Path(tempfile.mkdtemp(prefix="stasis-it022-"))
        root = temporary / "stasis_game" / "assets"
        root.mkdir(parents=True)
        payload = b"known-good-payload"
        second_payload = b"second-payload"
        (root / "token.bin").write_bytes(payload)
        (root / "second.bin").write_bytes(second_payload)
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "schema": "stasis-assets",
                    "version": 1,
                    "assets": [
                        {
                            "id": "token",
                            "path": "assets/token.bin",
                            "content_sha256": hashlib.sha256(payload).hexdigest(),
                        },
                        {
                            "id": "second",
                            "path": "assets/second.bin",
                            "content_sha256": hashlib.sha256(second_payload).hexdigest(),
                        },
                    ],
                }
            ),
            encoding="utf-8",
        )
        return temporary

    def test_variants_are_deterministic_and_path_bearing(self):
        expected = {
            "missing": ("missing_asset", "assets/token.bin"),
            "tampered": ("tampered_asset", "assets/token.bin"),
            "traversal": ("traversal_path", "assets/../it022-escape.bin"),
            "duplicate": ("duplicate_asset", "assets/token.bin"),
            "oversized": ("oversized_asset", "assets/token.bin"),
            "malformed-manifest": ("malformed_manifest", "assets/manifest.json"),
        }
        for variant in VARIANTS:
            with self.subTest(variant=variant):
                temporary = self.make_tree()
                try:
                    result = mutate_asset_tree(temporary, variant)
                    self.assertEqual((result["code"], result["path"]), expected[variant])
                    if variant == "missing":
                        self.assertFalse((temporary / "stasis_game/assets/token.bin").exists())
                    elif variant == "oversized":
                        self.assertGreater(
                            (temporary / "stasis_game/assets/token.bin").stat().st_size, 1
                        )
                        self.assertEqual(
                            b"x", (temporary / "stasis_game/assets/second.bin").read_bytes()
                        )
                        manifest = json.loads(
                            (temporary / "stasis_game/assets/manifest.json").read_text()
                        )
                        second = next(item for item in manifest["assets"] if item["id"] == "second")
                        self.assertEqual(
                            hashlib.sha256(b"x").hexdigest(), second["content_sha256"]
                        )
                    elif variant == "duplicate":
                        manifest = json.loads(
                            (temporary / "stasis_game/assets/manifest.json").read_text()
                        )
                        self.assertEqual(3, len(manifest["assets"]))
                finally:
                    for child in sorted(temporary.rglob("*"), reverse=True):
                        if child.is_file():
                            child.unlink()
                        elif child.is_dir():
                            child.rmdir()
                    temporary.rmdir()


if __name__ == "__main__":
    unittest.main()

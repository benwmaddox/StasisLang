import tempfile
import unittest
from pathlib import Path

from tools.copy_release_samples import (
    OMITTED_SAMPLE,
    RELEASE_README_NAME,
    copy_release_samples,
)


class CopyReleaseSamplesTests(unittest.TestCase):
    def test_omits_only_root_heavy_sample_and_preserves_other_sample_bytes(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source = root / "samples"
            destination = root / "release" / "samples"

            omitted_file = source / OMITTED_SAMPLE / "main.stasis"
            omitted_file.parent.mkdir(parents=True)
            omitted_file.write_bytes(b"large sample source\n")

            retained_files = {
                "brickout_revenge/main.stasis": b"fn main(): void {}\n",
                "brickout_revenge/assets/hero.svg": b"<svg>caf\xc3\xa9</svg>\n",
                "asset_contract/brickout_defense/fixture.stasis": b"nested sample\n",
                "windows_launch_smoke/main.stasis": b"fn launch_smoke(): void {}\n",
                "bucket_catcher.stasis": b"fn bucket_catcher(): void {}\n",
            }
            for relative_path, content in retained_files.items():
                path = source / relative_path
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)

            copy_release_samples(source, destination)

            self.assertFalse((destination / OMITTED_SAMPLE).exists())
            for relative_path, content in retained_files.items():
                self.assertEqual((destination / relative_path).read_bytes(), content)
            self.assertEqual(omitted_file.read_bytes(), b"large sample source\n")

            release_readme = (destination / RELEASE_README_NAME).read_text(encoding="utf-8")
            self.assertIn("brickout_defense", release_readme)
            self.assertIn(
                "https://github.com/benwmaddox/StasisLang/tree/main/samples/brickout_defense",
                release_readme,
            )

    def test_existing_destination_is_rejected_without_changing_its_bytes(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source = root / "samples"
            source.mkdir()
            (source / "brickout_revenge").mkdir()
            (source / "brickout_revenge" / "main.stasis").write_bytes(b"source\n")

            destination = root / "release" / "samples"
            sentinel = destination / "existing.bin"
            sentinel.parent.mkdir(parents=True)
            sentinel.write_bytes(b"preserve these bytes\x00\xff")

            with self.assertRaisesRegex(FileExistsError, "destination already exists"):
                copy_release_samples(source, destination)

            self.assertEqual(sentinel.read_bytes(), b"preserve these bytes\x00\xff")
            self.assertEqual(sorted(path.name for path in destination.iterdir()), ["existing.bin"])

    def test_source_must_be_a_real_directory(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            source_file = root / "samples.txt"
            source_file.write_text("not a directory", encoding="utf-8")
            destination = root / "release" / "samples"

            with self.assertRaisesRegex(ValueError, "source must be a real directory"):
                copy_release_samples(source_file, destination)

            self.assertFalse(destination.exists())

    def test_destination_inside_source_is_rejected_without_mutating_source(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            source = Path(temporary_directory) / "samples"
            source.mkdir()
            retained = source / "brickout_revenge" / "main.stasis"
            retained.parent.mkdir()
            retained.write_bytes(b"keep source intact\n")
            destination = source / "release" / "samples"

            with self.assertRaisesRegex(ValueError, "destination must be outside"):
                copy_release_samples(source, destination)

            self.assertEqual(retained.read_bytes(), b"keep source intact\n")
            self.assertFalse((source / "release").exists())


if __name__ == "__main__":
    unittest.main()

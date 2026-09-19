import json
import pathlib
import tempfile
import unittest
from unittest import mock

from tools.ci import windows_signing_manifest as manifest


class WindowsSigningManifestTests(unittest.TestCase):
    def _fixture(self, root: pathlib.Path) -> None:
        for relative in manifest.REQUIRED_STASIS_FILES:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(relative.encode("ascii"))
        nested = root / "nested" / "stasis_helper.dll"
        nested.parent.mkdir()
        nested.write_bytes(b"nested")
        (root / "SDL3.dll").write_bytes(b"third-party")
        (root / "clang-cl.exe").write_bytes(b"toolchain")
        (root / "nested" / "SDL3.dll").write_bytes(b"owned-name-collision")

    def test_nested_native_files_are_included_and_exclusions_are_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            receipt = root / "stasis_windows_signing.json"
            manifest.create(root, receipt, "a" * 40)
            record = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertIn("nested/stasis_helper.dll", {item["path"] for item in record["files"]})
            self.assertIn("nested/SDL3.dll", {item["path"] for item in record["files"]})
            self.assertEqual(
                {"SDL3.dll", "clang-cl.exe"},
                {item["path"] for item in record["excluded_files"]},
            )

    def test_unreceipted_nested_native_file_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            receipt = root / "stasis_windows_signing.json"
            manifest.create(root, receipt, "b" * 40)
            (root / "nested" / "late.dll").write_bytes(b"late")
            with self.assertRaisesRegex(manifest.ReceiptError, "complete native file set"):
                manifest.list_paths(root, receipt)

    def test_unsigned_receipt_rejects_same_size_mutation(self):
        cases = (
            ("nested/stasis_helper.dll", "unsigned file hash mismatch"),
            ("SDL3.dll", "excluded third-party file hash mismatch"),
        )
        for relative, message in cases:
            with self.subTest(relative=relative), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self._fixture(root)
                receipt = root / "stasis_windows_signing.json"
                manifest.create(root, receipt, "d" * 40)
                target = root / relative
                target.write_bytes(b"x" * target.stat().st_size)
                with self.assertRaisesRegex(manifest.ReceiptError, message):
                    manifest.list_paths(root, receipt)

    def test_final_receipt_binds_signed_and_excluded_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            receipt = root / "stasis_windows_signing.json"
            identity = root / "identity.json"
            source_commit = "c" * 40
            thumbprint = "D" * 40
            manifest.create(root, receipt, source_commit)
            signed, _ = manifest._inventory(root)
            for path in signed:
                path.write_bytes(path.read_bytes() + b"-signed")
            identity.write_text(
                json.dumps({"thumbprint": thumbprint, "subject": "CN=Stasis", "self_signed": True}),
                encoding="utf-8",
            )
            manifest.finalize(root, receipt, identity, source_commit)
            manifest.verify_files(root, receipt, source_commit, thumbprint)
            tampered = root / "nested" / "stasis_helper.dll"
            tampered.write_bytes(b"x" * tampered.stat().st_size)
            with self.assertRaisesRegex(manifest.ReceiptError, "signed file hash mismatch"):
                manifest.verify_files(root, receipt, source_commit, thumbprint)

    def test_native_file_count_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            with mock.patch.object(manifest, "MAX_NATIVE_FILES", 1):
                with self.assertRaisesRegex(manifest.ReceiptError, "native file count"):
                    manifest.create(root, root / "receipt.json", "e" * 40)

    def test_native_file_bytes_are_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            with mock.patch.object(manifest, "MAX_NATIVE_BYTES", 1):
                with self.assertRaisesRegex(manifest.ReceiptError, "native file bytes"):
                    manifest.create(root, root / "receipt.json", "f" * 40)


if __name__ == "__main__":
    unittest.main()

import json
import pathlib
import tempfile
import unittest

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

    def test_nested_native_files_are_included_and_exclusions_are_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self._fixture(root)
            receipt = root / "stasis_windows_signing.json"
            manifest.create(root, receipt, "a" * 40)
            record = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertIn("nested/stasis_helper.dll", {item["path"] for item in record["files"]})
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
            (root / "nested" / "stasis_helper.dll").write_bytes(b"tampered")
            with self.assertRaisesRegex(manifest.ReceiptError, "signed file hash mismatch"):
                manifest.verify_files(root, receipt, source_commit, thumbprint)


if __name__ == "__main__":
    unittest.main()

from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from tools.ci.check_unsafe_boundaries import unexpected_unsafe_files


class UnsafeBoundaryTests(unittest.TestCase):
    def test_rejects_unsafe_rust_in_orchestration_crates(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps" / "stasis" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("fn bad() { unsafe { raw(); } }", encoding="utf-8")
            self.assertEqual(
                unexpected_unsafe_files(root),
                ["apps/stasis/src/lib.rs"],
            )

    def test_rejects_unsafe_traits_in_orchestration_crates(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "stasis_runner" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("unsafe trait Bad {}", encoding="utf-8")
            self.assertEqual(
                unexpected_unsafe_files(root),
                ["crates/stasis_runner/src/lib.rs"],
            )

    def test_allows_unsafe_rust_in_audited_boundary(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "stasis_dynload" / "src" / "ffi.rs"
            source.parent.mkdir(parents=True)
            source.write_text("unsafe extern \"C\" fn boundary() {}", encoding="utf-8")
            self.assertEqual(unexpected_unsafe_files(root), [])

    def test_allows_exact_audited_platform_seam_file(self) -> None:
        with TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates" / "stasis_ai" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("fn boundary() { unsafe { raw(); } }", encoding="utf-8")
            self.assertEqual(unexpected_unsafe_files(root), [])


if __name__ == "__main__":
    unittest.main()

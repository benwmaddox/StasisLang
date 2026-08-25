import pathlib
import tempfile
import unittest

from tools.ci.check_windows_vs_generator import CALLERS, HELPER, ROOT, validate


class WindowsVisualStudioGeneratorTests(unittest.TestCase):
    def test_repository_uses_installed_visual_studio_instances(self):
        self.assertEqual(validate(ROOT), [])

    def test_cmake_advertisement_detection_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative in (HELPER, *CALLERS):
                source = ROOT / relative
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")
            workflow = root / ".github/workflows/pr-ci.yml"
            workflow.write_text(
                workflow.read_text(encoding="utf-8") + "\n# cmake --help\n",
                encoding="utf-8",
            )
            self.assertTrue(any("advertised" in error for error in validate(root)))


if __name__ == "__main__":
    unittest.main()

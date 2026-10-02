import pathlib
import re
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
            workflow = root / ".github/workflows/nightly-validation.yml"
            workflow.write_text(
                workflow.read_text(encoding="utf-8") + "\n# cmake --help\n",
                encoding="utf-8",
            )
            self.assertTrue(any("advertised" in error for error in validate(root)))

    def test_nightly_uses_vs2022_for_production_and_latest_for_consumers(self):
        workflow = (ROOT / ".github/workflows/nightly-release.yml").read_text(
            encoding="utf-8"
        )

        def job(name: str) -> str:
            match = re.search(
                rf"(?ms)^  {re.escape(name)}:\n(?P<body>.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
                workflow,
            )
            self.assertIsNotNone(match, f"missing nightly workflow job {name}")
            return match.group("body")

        def matrix_rows(job_body: str) -> list[dict[str, str]]:
            match = re.search(
                r"(?ms)^      matrix:\n        include:\n(?P<rows>.*?)(?=^    [A-Za-z0-9_-]+:|\Z)",
                job_body,
            )
            self.assertIsNotNone(match, "job is missing its include matrix")
            rows: list[dict[str, str]] = []
            current: dict[str, str] | None = None
            for line in match.group("rows").splitlines():
                if line.startswith("          - "):
                    if current is not None:
                        rows.append(current)
                    key, value = line.strip().removeprefix("- ").split(":", 1)
                    current = {key: value.strip()}
                elif current is not None and line.startswith("            "):
                    field = line.strip()
                    if ":" in field:
                        key, value = field.split(":", 1)
                        current[key] = value.strip()
            if current is not None:
                rows.append(current)
            return rows

        build = job("build")
        windows_producers = [
            row for row in matrix_rows(build) if row.get("bundle_platform") == "windows"
        ]
        self.assertEqual(len(windows_producers), 1)
        self.assertEqual(windows_producers[0]["os"], "windows-2022")
        self.assertRegex(build, r"(?m)^    runs-on: \$\{\{ matrix\.os \}\}$")
        self.assertRegex(
            build,
            r"(?m)^          name: \$\{\{ matrix\.bundle_platform == 'windows' && 'stasis-nightly-win-x64-unsigned' \|\| matrix\.archive \}\}$",
            "unsigned artifact naming must follow the bundle platform, not producer runner label",
        )
        self.assertRegex(job("windows_signing"), r"(?m)^    runs-on: windows-latest$")

        editor_windows = [
            row
            for row in matrix_rows(job("vscode_extension"))
            if row.get("vsce_target") == "win32-x64"
        ]
        self.assertEqual(len(editor_windows), 1)
        self.assertEqual(editor_windows[0]["os"], "windows-latest")


if __name__ == "__main__":
    unittest.main()

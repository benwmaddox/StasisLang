import unittest

from tools.ci.check_action_versions import PINS, ROOT, validate


class ActionVersionTests(unittest.TestCase):
    def test_all_workflows(self):
        for path in (ROOT / ".github/workflows").glob("*.y*ml"):
            with self.subTest(path=path.name):
                self.assertEqual(validate(path.read_text()), [])

    def test_provenance_and_runtime_exception_are_bounded(self):
        exceptions = []
        for action, pin in PINS.items():
            self.assertRegex(pin["sha"], r"^[0-9a-f]{40}$")
            self.assertIn(pin["runtime"], ("node24", "composite", "node20"))
            if pin["runtime"] == "node20":
                self.assertTrue(pin.get("exception"))
                exceptions.append(action)
        self.assertEqual(exceptions, ["ilammy/msvc-dev-cmd"])

    def test_rejects_version_regression_and_unknown_action(self):
        for ref in ("actions/setup-node@v4", "other/action@main"):
            self.assertTrue(validate("      - uses: " + ref))

    def test_runtime_cache_and_archive_regressions(self):
        source = (ROOT / ".github/workflows/pr-ci.yml").read_text()
        for before, after in (('node-version: "24"', 'node-version: "20"'),
                              ('package-manager-cache: false', 'package-manager-cache: true'),
                              ('archive: true', 'archive: false')):
            with self.subTest(before=before):
                self.assertTrue(validate(source.replace(before, after)))
        self.assertTrue(validate(source + '\nNODE_NO_WARNINGS: 1\n'))

    def test_local_reusable_workflow_is_allowed(self):
        self.assertEqual(validate("    uses: ./.github/workflows/pr-ci.yml"), [])


if __name__ == "__main__":
    unittest.main()

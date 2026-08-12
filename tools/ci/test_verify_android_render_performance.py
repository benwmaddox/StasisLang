import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.ci.verify_android_render_performance import build_evidence, main, parse_report


REPORT = (
    "08-12 StasisRenderer: RenderPerformance: schema=1 warmup=60 samples=180 "
    "total_p50_us=410 total_p95_us=620 resource_p50_us=120 resource_p95_us=180 "
    "draw_p50_us=270 draw_p95_us=410 draw_calls_min=4 draw_calls_max=4 "
    "lines=2 rects=1 sprites=3 text=2 order=8"
)


class VerifyAndroidRenderPerformanceTest(unittest.TestCase):
    def test_parses_bounded_stage_metrics(self):
        metrics = parse_report(REPORT)
        self.assertEqual(410, metrics["total_p50_us"])
        self.assertEqual(180, metrics["samples"])
        self.assertEqual(4, metrics["draw_calls_max"])

    def test_requires_exactly_one_complete_report(self):
        with self.assertRaisesRegex(ValueError, "expected one"):
            parse_report("")
        with self.assertRaisesRegex(ValueError, "expected one"):
            parse_report(REPORT + "\n" + REPORT)
        with self.assertRaisesRegex(ValueError, "missing"):
            parse_report("RenderPerformance: schema=1 warmup=60 samples=180")
        with self.assertRaisesRegex(ValueError, "duplicate.*total_p50_us"):
            parse_report(REPORT.replace(
                "total_p50_us=410", "total_p50_us=9999 total_p50_us=410"))
        with self.assertRaisesRegex(ValueError, "unexpected.*invented"):
            parse_report(REPORT + " invented=1")

    def test_evidence_requires_device_and_build_identity(self):
        metadata = {
            "scene": "render_parity",
            "git_revision": "abc123",
            "source_dirty": False,
            "apk_sha256": "f" * 64,
            "package_version": "0.1.0 (1)",
            "device_model": "emulator",
            "device_fingerprint": "stasis/test/device",
            "serial": "emulator-5554",
            "avd": "Stasis_API_35",
            "android_sdk": 35,
        }
        evidence = build_evidence(REPORT, metadata)
        self.assertEqual("android_workshop_preview_render", evidence["benchmark"])
        self.assertEqual(620, evidence["metrics"]["total_p95_us"])
        del metadata["apk_sha256"]
        with self.assertRaisesRegex(ValueError, "apk_sha256"):
            build_evidence(REPORT, metadata)

    def test_cli_accepts_powershell_utf8_bom_metadata(self):
        metadata = {
            "scene": "render_parity",
            "git_revision": "abc123",
            "source_dirty": False,
            "apk_sha256": "f" * 64,
            "package_version": "0.1.0 (1)",
            "device_model": "emulator",
            "device_fingerprint": "stasis/test/device",
            "serial": "emulator-5554",
            "avd": "Stasis_API_35",
            "android_sdk": 35,
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root / "log.txt"
            source = root / "metadata.json"
            evidence = root / "evidence.json"
            log.write_text(REPORT, encoding="utf-8")
            source.write_text(json.dumps(metadata), encoding="utf-8-sig")
            with mock.patch(
                "sys.argv",
                ["verify", "--log", str(log), "--metadata", str(source),
                 "--evidence", str(evidence)],
            ):
                self.assertEqual(0, main())
            self.assertEqual(410, json.loads(evidence.read_text())["metrics"]["total_p50_us"])


if __name__ == "__main__":
    unittest.main()

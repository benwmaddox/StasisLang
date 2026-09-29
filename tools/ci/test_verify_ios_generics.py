import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from tools.ci.verify_ios_generics import EvidenceError, build_evidence, main


WIDTH = 1179
HEIGHT = 2556
BACKGROUND = (10, 20, 41, 255)
TEAL = (41, 184, 133, 255)
RED = (220, 30, 40, 255)
TRANSPARENT_TEAL = (41, 184, 133, 0)


def _png_chunk(chunk_type: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + chunk_type
        + payload
        + struct.pack(">I", zlib.crc32(chunk_type + payload) & 0xFFFFFFFF)
    )


def write_png(
    path: Path,
    accent: tuple[int, int, int, int],
    failure_pixels: int = 0,
) -> None:
    rows = bytearray()
    accent_rows = 32
    for y in range(HEIGHT):
        rows.append(0)
        if y < accent_rows:
            rows.extend(bytes(accent) * WIDTH)
            continue
        red_count = min(WIDTH, failure_pixels)
        rows.extend(bytes(RED) * red_count)
        rows.extend(bytes(BACKGROUND) * (WIDTH - red_count))
        failure_pixels -= red_count
    ihdr = struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", zlib.compress(bytes(rows)))
        + _png_chunk(b"IEND", b"")
    )


def write_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")


class IosGenericsVerifierTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.receipt = self.root / "receipt.json"
        self.frame = self.root / "frame.png"
        self.log = self.root / "simulator.log"
        self.bounds_low = self.root / "bounds-low.json"
        self.bounds_high = self.root / "bounds-high.json"
        self.crash_low = self.root / "bounds-low.ips"
        self.crash_high = self.root / "bounds-high.ips"
        self.output = self.root / "evidence.json"
        write_json(
            self.receipt,
            {
                "schema": "stasis.ios.generics.v1",
                "main_result": 0,
                "tick_result": 0,
                "render_result": 0,
                "digest": 507,
                "frame": 1,
            },
        )
        write_png(self.frame, TEAL)
        self.log.write_text(
            "Stasis provenance: generics tag=v0 commit=abc renderer=gfx_cmd schema=7\n"
            "Stasis iOS generics acceptance digest=507 frame=1 receipt=/tmp/result.json\n",
            encoding="utf-8",
        )
        self.crash_low.write_text(
            "Process: StasisMobile [4101]\nException Type: EXC_BREAKPOINT (SIGTRAP)\n",
            encoding="utf-8",
        )
        self.crash_high.write_text(
            "Process: StasisMobile [4102]\nException Type: EXC_BREAKPOINT (SIGTRAP)\n",
            encoding="utf-8",
        )
        write_json(
            self.bounds_low,
            {
                "schema": "stasis.ios.generics.bounds.v1",
                "label": "low",
                "index": -1,
                "process": "StasisMobile",
                "pid": 4101,
                "fatal": True,
                "signal": "SIGTRAP",
                "exception": "EXC_BREAKPOINT",
                "crash_report": str(self.crash_low),
            },
        )
        write_json(
            self.bounds_high,
            {
                "schema": "stasis.ios.generics.bounds.v1",
                "label": "high",
                "index": 2,
                "process": "StasisMobile",
                "pid": 4102,
                "fatal": True,
                "signal": "SIGTRAP",
                "exception": "EXC_BREAKPOINT",
                "crash_report": str(self.crash_high),
            },
        )

    def evidence(self) -> dict:
        return build_evidence(
            self.receipt,
            self.frame,
            [self.log],
            self.bounds_low,
            self.bounds_high,
        )

    def test_success_writes_aggregate_evidence(self) -> None:
        result = main([
            "--receipt", str(self.receipt),
            "--frame", str(self.frame),
            "--log", str(self.log),
            "--bounds-low", str(self.bounds_low),
            "--bounds-high", str(self.bounds_high),
            "--output", str(self.output),
        ])
        self.assertEqual(result, 0)
        evidence = json.loads(self.output.read_text(encoding="utf-8"))
        self.assertEqual(evidence["schema"], "stasis.ios.generics.evidence.v1")
        self.assertEqual(evidence["receipt"]["digest"], 507)
        self.assertGreater(evidence["frame"]["teal_pixels"], WIDTH * HEIGHT // 100)
        self.assertGreater(evidence["frame"]["background_pixels"], WIDTH * HEIGHT // 2)
        self.assertLess(evidence["frame"]["failure_pixels"], 100)
        self.assertIn("renderer=gfx_cmd", evidence["log_markers"]["provenance"])
        self.assertEqual(evidence["bounds"]["low"]["index"], -1)
        self.assertEqual(evidence["bounds"]["high"]["index"], 2)
        self.assertEqual(
            len(evidence["bounds"]["low"]["crash_report_evidence"]["sha256"]),
            64,
        )

    def test_wrong_digest_is_rejected(self) -> None:
        value = json.loads(self.receipt.read_text(encoding="utf-8"))
        value["digest"] = 506
        write_json(self.receipt, value)
        with self.assertRaisesRegex(EvidenceError, "expected digest=507"):
            self.evidence()

    def test_red_frame_is_rejected(self) -> None:
        write_png(self.frame, TEAL, failure_pixels=100)
        with self.assertRaisesRegex(EvidenceError, "digest-failure red rectangle"):
            self.evidence()

    def test_error_log_is_rejected(self) -> None:
        self.log.write_text(
            "dyld: Library not loaded: @rpath/libstasis.dylib\n",
            encoding="utf-8",
        )
        with self.assertRaisesRegex(EvidenceError, "link evidence"):
            self.evidence()

    def test_nonfatal_bounds_evidence_is_rejected(self) -> None:
        value = json.loads(self.bounds_high.read_text(encoding="utf-8"))
        value["fatal"] = False
        write_json(self.bounds_high, value)
        with self.assertRaisesRegex(EvidenceError, "bounds evidence is not fatal"):
            self.evidence()

    def test_arm64_bad_instruction_bounds_trap_is_accepted(self) -> None:
        for path, report, pid in (
            (self.bounds_low, self.crash_low, 4101),
            (self.bounds_high, self.crash_high, 4102),
        ):
            value = json.loads(path.read_text(encoding="utf-8"))
            value["exception"] = "EXC_BAD_INSTRUCTION"
            value["signal"] = "SIGILL"
            write_json(path, value)
            report.write_text(
                f'{{"procName":"StasisMobile","pid":{pid}}}\n'
                '"exception":{"type":"EXC_BAD_INSTRUCTION","signal":"SIGILL"}\n',
                encoding="utf-8",
            )
        self.evidence()

    def test_mismatched_bounds_trap_pair_is_rejected(self) -> None:
        value = json.loads(self.bounds_high.read_text(encoding="utf-8"))
        value["exception"] = "EXC_BAD_INSTRUCTION"
        write_json(self.bounds_high, value)
        with self.assertRaisesRegex(EvidenceError, "unrecognized fatal trap pair"):
            self.evidence()

    def test_transparent_teal_is_rejected(self) -> None:
        write_png(self.frame, TRANSPARENT_TEAL)
        with self.assertRaisesRegex(EvidenceError, "digest-success teal rectangle"):
            self.evidence()

    def test_receipt_remains_authoritative_when_log_marker_is_delayed(self) -> None:
        self.log.write_text(
            "Stasis provenance: generics tag=v0 commit=abc renderer=gfx_cmd schema=7\n",
            encoding="utf-8",
        )
        evidence = self.evidence()
        self.assertEqual(
            evidence["log_markers"]["generics_acceptance"],
            "verified receipt digest=507 frame=1",
        )

    def test_bounds_report_must_match_its_launch_pid(self) -> None:
        value = json.loads(self.bounds_high.read_text(encoding="utf-8"))
        value["pid"] = 4999
        write_json(self.bounds_high, value)
        with self.assertRaisesRegex(EvidenceError, "does not match launch pid 4999"):
            self.evidence()

    def test_bounds_launches_must_have_distinct_pids(self) -> None:
        value = json.loads(self.bounds_high.read_text(encoding="utf-8"))
        value["pid"] = 4101
        self.crash_high.write_text(
            "Process: StasisMobile [4101]\nException Type: EXC_BREAKPOINT (SIGTRAP)\n",
            encoding="utf-8",
        )
        write_json(self.bounds_high, value)
        with self.assertRaisesRegex(EvidenceError, "reused one launch pid"):
            self.evidence()

    def test_bounds_launches_must_have_distinct_crash_reports(self) -> None:
        reused = (
            '{"procName":"StasisMobile","pid":4101}\n'
            '{"procName":"StasisMobile","pid":4102}\n'
            "Exception Type: EXC_BREAKPOINT (SIGTRAP)\n"
        )
        self.crash_low.write_text(reused, encoding="utf-8")
        self.crash_high.write_text(reused, encoding="utf-8")
        with self.assertRaisesRegex(EvidenceError, "reused one crash report"):
            self.evidence()


class IosGenericsManualToolContractTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.repo = Path(__file__).resolve().parents[2]
        cls.script = (cls.repo / "tools/ci/build_ios_package.sh").read_text(
            encoding="utf-8"
        )
        cls.workflow = (cls.repo / ".github/workflows/pr-ci.yml").read_text(
            encoding="utf-8"
        )

    def test_bounds_reports_are_selected_by_launch_pid(self) -> None:
        self.assertIn('launch_pid="$(printf', self.script)
        self.assertIn('${launch_pid}([^0-9]|$)|Process:', self.script)
        self.assertIn('"pid": int(sys.argv[4])', self.script)

    def test_simulator_build_preserves_required_preprocessor_definitions(self) -> None:
        self.assertIn(
            "GCC_PREPROCESSOR_DEFINITIONS=$(inherited) TVG_STATIC=1 NOMINMAX=1 STASIS_ENABLE_SEAM_TESTS=1",
            self.script,
        )

    def test_green_launch_waits_for_unified_log_provenance_marker(self) -> None:
        self.assertIn("for _ in $(seq 1 40); do", self.script)
        self.assertIn(
            "Stasis provenance: .* renderer=gfx_cmd schema=7", self.script
        )
        self.assertIn(
            "simulator unified log did not publish the package provenance marker",
            self.script,
        )

    def test_symbol_gate_uses_manifest_lifecycle_and_workload_symbols(self) -> None:
        self.assertIn("verify_ios_generics_symbols()", self.script)
        self.assertIn('for name in ("main", "tick", "render"):', self.script)
        self.assertIn(
            '"stasis_state_scalar__generics_collections_digest_value"',
            self.script,
        )
        self.assertIn("len(linked_aot_functions) < 16", self.script)

    def test_generics_helper_remains_available_without_a_hosted_ios_lane(self) -> None:
        self.assertNotIn("ios-generics-simulator:", self.workflow)
        self.assertNotIn("ios-package-link:", self.workflow)
        self.assertNotIn("runs-on: macos-", self.workflow)
        self.assertTrue((self.repo / "tools/ci/build_ios_package.sh").is_file())


if __name__ == "__main__":
    unittest.main()

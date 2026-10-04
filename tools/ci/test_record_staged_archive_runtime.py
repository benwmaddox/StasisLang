from __future__ import annotations

import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from tools.ci import record_staged_archive_runtime as recorder


RELEASE_ID = "nightly-20261002-379"
SOURCE_COMMIT = "99a5b3943f1759b7abbdbf0e37634965749c1337"
CLI_SHA256 = "1" * 64
RUNTIME_SOURCES = {"runtime/stasis_graphics.c": "2" * 64}


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class RecordRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def make_receipt(self, target: str) -> Path:
        path = self.root / "receipt.json"
        write_json(
            path,
            {
                "schema": "stasis.staged_archive_acceptance.v1",
                "target": target,
                "release_id": RELEASE_ID,
                "source_commit": SOURCE_COMMIT,
                "toolchain": {
                    "cli_sha256": CLI_SHA256,
                    "build_fingerprint": "3" * 64,
                    "runtime_sources": RUNTIME_SOURCES,
                },
                "consumers": {"bundled": {}, "generated": {}},
            },
        )
        return path

    def make_mobile_package(self, root: Path, target: str, *, development: bool) -> None:
        package = root / "dist" / target
        write_json(
            package / "stasis_mobile_package.json",
            {"target": target, "development_build": development},
        )
        write_json(
            package / "stasis_provenance.json",
            {
                "build_class": "development" if development else "local_release",
                "release_tag": None if development else RELEASE_ID,
                "source_commit": None if development else SOURCE_COMMIT,
                "dirty_state": development,
                "development_build": development,
                "compiler": {"sha256": CLI_SHA256},
                "runtime_sources": RUNTIME_SOURCES,
            },
        )

    def add_android_shipping_evidence(self, root: Path) -> None:
        shipping = root / "shipping"
        shipping.mkdir(parents=True, exist_ok=True)
        (shipping / "app-debug.apk").write_bytes(b"arm64 apk")
        (shipping / "android-apk-audit.log").write_text("arm64-v8a present\n", encoding="utf-8")
        (shipping / "gradle-link.log").write_text("linked arm64 runtime\n", encoding="utf-8")
        (shipping / "package-provenance.log").write_text("official archive inputs\n", encoding="utf-8")

    def android_bounds_probe_fixtures(self) -> list[dict[str, object]]:
        return [
            {
                "index": -1,
                "signal": "SIGILL",
                "pid": 101,
                "process_exited": True,
                "log_line_count": 2222,
                "log_byte_count": 339464,
            },
            {
                "index": 2,
                "signal": "SIGILL",
                "pid": 102,
                "process_exited": True,
                "log_line_count": 2280,
                "log_byte_count": 314171,
            },
        ]

    def add_android_runtime_evidence(
        self,
        root: Path,
        bounds_probes: list[dict[str, object]] | None = None,
    ) -> Path:
        evidence = root / "artifacts" / "evidence.json"
        frame = root / "artifacts" / "frame.png"
        frame.parent.mkdir(parents=True, exist_ok=True)
        frame.write_bytes(b"captured android frame")
        write_json(
            evidence,
            {
                "status": "passed",
                "test_id": "ANDROID-GENERICS",
                "android_generics": {
                    "digest_receipt": {"digest": 507, "event": "oracle"},
                    "bounds_probes": bounds_probes if bounds_probes is not None else self.android_bounds_probe_fixtures(),
                    "frame_capture": str(frame),
                },
            },
        )
        return evidence

    def add_ios_shipping_evidence(self, root: Path) -> None:
        build_root = root / "target" / "ios-archive-acceptance"
        executable = build_root / "StasisMobile.app" / "StasisMobile"
        executable.parent.mkdir(parents=True, exist_ok=True)
        executable.write_bytes(b"linked arm64 executable")
        (build_root / "device-hashes.txt").write_text(
            f"{sha256(executable)}  {executable.as_posix()}\n", encoding="utf-8"
        )
        (build_root / "evidence.txt").write_text(
            "physical_device_qualified=false\ncompiler_payloads=0\n", encoding="utf-8"
        )
        (build_root / "xcodebuild.log").write_text("built iphoneos arm64\n", encoding="utf-8")
        (build_root / "device-platform.txt").write_text("platform IOS\n", encoding="utf-8")
        (build_root / "linked-libraries.txt").write_text(
            "@rpath/SDL3.framework/SDL3\n@rpath/SDL3_image.framework/SDL3_image\n",
            encoding="utf-8",
        )
        (build_root / "device-symbols.txt").write_text(
            "stasis_state_scalar__generics_collections_digest_value\n"
            "stasis_state_scalar__web_bounds_probe_index\n"
            "stasis_state_array__gfx_cmd_i32\n",
            encoding="utf-8",
        )

    def test_records_android_archive_identity_and_runtime_for_both_consumers(self) -> None:
        receipt_path = self.make_receipt("android")
        roots = {label: self.root / label for label in ("bundled", "generated")}
        for label, root in roots.items():
            self.make_mobile_package(root, "android-x86_64", development=True)
            self.make_mobile_package(root, "android-arm64", development=False)
            self.add_android_shipping_evidence(root)
            self.add_android_runtime_evidence(root)
        result = recorder.record_runtime(
            receipt_path,
            "android",
            roots["bundled"],
            roots["generated"],
            expected_release=RELEASE_ID,
            expected_source=SOURCE_COMMIT,
        )
        self.assertEqual(result["result"], "passed")
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        self.assertEqual(set(receipt["runtime_acceptance"]["consumers"]), {"bundled", "generated"})
        for label in ("bundled", "generated"):
            mobile = receipt["consumers"][label]["mobile"]
            self.assertEqual(mobile["package_targets"], ["android-arm64", "android-x86_64"])
            self.assertTrue(mobile["test_package"]["development_build"])
            self.assertFalse(mobile["shipping_package"]["development_build"])
            self.assertEqual(mobile["shipping_package"]["linked_artifact_sha256"], sha256(roots[label] / "shipping/app-debug.apk"))
            self.assertEqual(receipt["consumers"][label]["mobile_runtime"]["frame_sha256"], sha256(roots[label] / "artifacts/frame.png"))

    def test_rejects_invalid_android_bounds_probe_receipts(self) -> None:
        consumer = self.root / "consumer"
        evidence_path = self.add_android_runtime_evidence(consumer)

        def probes() -> list[dict[str, object]]:
            return self.android_bounds_probe_fixtures()

        invalid_cases: list[tuple[str, object]] = []

        wrong_signal = probes()
        wrong_signal[0]["signal"] = "SIGSEGV"
        invalid_cases.append(("wrong signal", wrong_signal))

        missing_signal = probes()
        del missing_signal[0]["signal"]
        invalid_cases.append(("missing signal", missing_signal))

        still_alive = probes()
        still_alive[0]["process_exited"] = False
        invalid_cases.append(("process is alive", still_alive))

        missing_exit = probes()
        del missing_exit[0]["process_exited"]
        invalid_cases.append(("missing process exit", missing_exit))

        invalid_cases.append(("missing probe", probes()[1:]))
        invalid_cases.append(("malformed probe", [None, *probes()[1:]]))
        invalid_cases.append(("malformed bounds container", "not-a-list"))

        missing_pid = probes()
        del missing_pid[0]["pid"]
        invalid_cases.append(("missing PID", missing_pid))

        boolean_pid = probes()
        boolean_pid[0]["pid"] = True
        invalid_cases.append(("boolean PID", boolean_pid))

        nonpositive_pid = probes()
        nonpositive_pid[0]["pid"] = 0
        invalid_cases.append(("nonpositive PID", nonpositive_pid))

        duplicate_pid = probes()
        duplicate_pid[1]["pid"] = duplicate_pid[0]["pid"]
        invalid_cases.append(("duplicate PID", duplicate_pid))

        boolean_index = probes()
        boolean_index[0]["index"] = False
        invalid_cases.append(("boolean index", boolean_index))

        float_index = probes()
        float_index[0]["index"] = -1.0
        invalid_cases.append(("float index", float_index))

        string_index = probes()
        string_index[0]["index"] = "-1"
        invalid_cases.append(("string index", string_index))

        duplicate_index = probes()
        duplicate_index[1]["index"] = -1
        invalid_cases.append(("duplicate index", duplicate_index))

        for name, bounds in invalid_cases:
            with self.subTest(name=name):
                evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
                evidence["android_generics"]["bounds_probes"] = bounds
                write_json(evidence_path, evidence)
                with self.assertRaisesRegex(ValueError, "bounds"):
                    recorder.find_runtime_evidence(consumer, "android")

    def test_rejects_mobile_package_source_mismatch(self) -> None:
        receipt_path = self.make_receipt("ios")
        roots = {label: self.root / label for label in ("bundled", "generated")}
        for root in roots.values():
            self.make_mobile_package(root, "ios-arm64", development=False)
            self.make_mobile_package(root, "ios-simulator-arm64", development=True)
            self.add_ios_shipping_evidence(root)
            evidence_root = root / "target" / "ios-archive-acceptance"
            (evidence_root / "simulator-frame.png").write_bytes(b"frame")
            write_json(
                evidence_root / "simulator-evidence.json",
                {
                    "status": "passed",
                    "schema": "stasis.ios.generics.evidence.v1",
                    "receipt": {"digest": 507},
                    "bounds": {"low": {"index": -1}, "high": {"index": 2}},
                },
            )
            write_json(
                evidence_root / "bounds-low.json",
                {"index": -1, "fatal": True},
            )
            write_json(
                evidence_root / "bounds-high.json",
                {"index": 2, "fatal": True},
            )
        provenance = roots["generated"] / "dist" / "ios-arm64" / "stasis_provenance.json"
        value = json.loads(provenance.read_text(encoding="utf-8"))
        value["source_commit"] = "0" * 40
        write_json(provenance, value)
        with self.assertRaisesRegex(ValueError, "shipping ios-arm64 package provenance differs"):
            recorder.record_runtime(
                receipt_path,
                "ios",
                roots["bundled"],
                roots["generated"],
                expected_release=RELEASE_ID,
                expected_source=SOURCE_COMMIT,
            )


if __name__ == "__main__":
    unittest.main()

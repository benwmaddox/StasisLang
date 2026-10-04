#!/usr/bin/env python3
"""Bind successful mobile runtime evidence to its staged archive receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
import re
from typing import Any


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def load_package_identity(
    consumer_root: Path,
    target: str,
    receipt: dict[str, Any],
    *,
    expected_release: str,
    expected_source: str,
) -> dict[str, Any]:
    manifests = sorted(consumer_root.rglob("stasis_mobile_package.json"))
    shipping_target, test_target = (
        ("android-arm64", "android-x86_64")
        if target == "android"
        else ("ios-arm64", "ios-simulator-arm64")
    )
    expected_targets = sorted((shipping_target, test_target))
    entries: list[dict[str, Any]] = []
    toolchain = receipt.get("toolchain", {})
    for manifest_path in manifests:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        provenance_path = manifest_path.parent / "stasis_provenance.json"
        if not provenance_path.is_file():
            raise ValueError(f"mobile package provenance is missing: {provenance_path}")
        provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
        if manifest.get("target") not in expected_targets:
            continue
        package_target = manifest["target"]
        development_target = package_target == test_target
        expected_development = development_target
        if manifest.get("development_build") is not expected_development:
            raise ValueError(
                f"{package_target} package development flag is not {expected_development}: {manifest_path}"
            )
        compiler = provenance.get("compiler")
        if not isinstance(compiler, dict) or compiler.get("sha256") != toolchain.get("cli_sha256"):
            raise ValueError(f"mobile package used another archive compiler: {provenance_path}")
        if provenance.get("runtime_sources") != toolchain.get("runtime_sources"):
            raise ValueError(f"mobile package runtime sources differ from accepted archive: {provenance_path}")
        if development_target:
            if (
                provenance.get("build_class") != "development"
                or provenance.get("development_build") is not True
                or provenance.get("dirty_state") is not True
                or provenance.get("release_tag") is not None
                or provenance.get("source_commit") not in (None, "unknown", expected_source)
            ):
                raise ValueError(
                    f"test-only {package_target} package falsely claims official release provenance: {provenance_path}"
                )
        elif (
            provenance.get("release_tag") != expected_release
            or provenance.get("source_commit") != expected_source
            or provenance.get("dirty_state") is not False
            or provenance.get("development_build") is not False
        ):
            raise ValueError(
                f"shipping {package_target} package provenance differs from accepted archive: {provenance_path}"
            )
        entries.append(
            {
                "target": package_target,
                "development_build": expected_development,
                "release_tag": provenance.get("release_tag"),
                "source_commit": provenance.get("source_commit"),
                "dirty_state": provenance.get("dirty_state"),
                "compiler_sha256": compiler["sha256"],
                "runtime_sources": provenance["runtime_sources"],
                "provenance_sha256": sha256(provenance_path),
                "manifest_sha256": sha256(manifest_path),
            }
        )
    if sorted(entry["target"] for entry in entries) != expected_targets:
        raise ValueError(
            f"{target} consumer packages are {sorted(entry['target'] for entry in entries)!r}, "
            f"expected {expected_targets!r} under {consumer_root}"
        )
    return {
        "result": "passed",
        "release_id": expected_release,
        "source_commit": expected_source,
        "package_targets": expected_targets,
        "shipping_target": shipping_target,
        "test_target": test_target,
        "compiler_sha256": toolchain.get("cli_sha256"),
        "runtime_fingerprint": toolchain.get("build_fingerprint"),
        "shipping_package": {
            **next(entry for entry in entries if entry["target"] == shipping_target),
            "result": "passed",
            "release_id": expected_release,
            "source_commit": expected_source,
            "runtime_fingerprint": toolchain.get("build_fingerprint"),
        },
        "test_package": {
            **next(entry for entry in entries if entry["target"] == test_target),
            "result": "passed",
            "runtime_fingerprint": toolchain.get("build_fingerprint"),
        },
        "packages": entries,
    }


def find_shipping_link_evidence(consumer_root: Path, target: str) -> tuple[Path, list[Path]]:
    if target == "android":
        shipping_root = consumer_root / "shipping"
        artifact = shipping_root / "app-debug.apk"
        files = [
            artifact,
            shipping_root / "android-apk-audit.log",
            shipping_root / "gradle-link.log",
            shipping_root / "package-provenance.log",
        ]
        if any(not path.is_file() for path in files):
            raise ValueError(f"Android arm64 shipping package/link evidence is incomplete under {shipping_root}")
        if artifact.stat().st_size == 0:
            raise ValueError(f"Android arm64 APK is empty: {artifact}")
        return artifact, files

    candidates = sorted(consumer_root.rglob("device-hashes.txt"))
    if len(candidates) != 1:
        raise ValueError(f"iOS consumer must have one device-hashes.txt, found {len(candidates)} under {consumer_root}")
    hashes_path = candidates[0]
    build_root = hashes_path.parent
    required = [
        hashes_path,
        build_root / "evidence.txt",
        build_root / "xcodebuild.log",
        build_root / "device-platform.txt",
        build_root / "linked-libraries.txt",
        build_root / "device-symbols.txt",
    ]
    if any(not path.is_file() for path in required):
        raise ValueError(f"iOS arm64 shipping package/link evidence is incomplete under {build_root}")
    evidence = (build_root / "evidence.txt").read_text(encoding="utf-8")
    if "physical_device_qualified=false" not in evidence or "compiler_payloads=0" not in evidence:
        raise ValueError("iOS arm64 shipping receipt falsely claims a device run or compiler payload")
    platform = (build_root / "device-platform.txt").read_text(encoding="utf-8")
    libraries = (build_root / "linked-libraries.txt").read_text(encoding="utf-8")
    symbols = (build_root / "device-symbols.txt").read_text(encoding="utf-8")
    if "platform IOS" not in platform or "@rpath/SDL3.framework/SDL3" not in libraries or "@rpath/SDL3_image.framework/SDL3_image" not in libraries:
        raise ValueError("iOS arm64 shipping executable did not link the expected platform SDL frameworks")
    for symbol in (
        "stasis_state_scalar__generics_collections_digest_value",
        "stasis_state_scalar__math_oracle_raw_digest_value",
        "stasis_state_scalar__web_bounds_probe_index",
        "stasis_state_array__gfx_cmd_i32",
    ):
        if symbol not in symbols:
            raise ValueError(f"iOS arm64 shipping executable omitted required AOT state {symbol}")
    executable_lines = [
        line for line in hashes_path.read_text(encoding="utf-8").splitlines()
        if "StasisMobile.app/StasisMobile" in line
    ]
    if len(executable_lines) != 1:
        raise ValueError("iOS arm64 device hash receipt must name exactly one linked app executable")
    executable_fields = executable_lines[0].split(None, 1)
    if len(executable_fields) != 2 or not re.fullmatch(r"[0-9a-f]{64}", executable_fields[0]):
        raise ValueError("iOS arm64 device executable hash receipt is malformed")
    executable = Path(executable_fields[1].strip())
    if not executable.is_file() or sha256(executable) != executable_fields[0]:
        raise ValueError("iOS arm64 linked executable hash differs from device-hashes.txt")
    return executable, required


def find_runtime_evidence(consumer_root: Path, target: str) -> tuple[Path, Path, list[Path]]:
    if target == "android":
        candidates = sorted(consumer_root.rglob("evidence.json"))
        if len(candidates) != 1:
            raise ValueError(f"Android consumer must have one evidence.json, found {len(candidates)} under {consumer_root}")
        evidence_path = candidates[0]
        value = json.loads(evidence_path.read_text(encoding="utf-8"))
        if value.get("status") != "passed" or value.get("test_id") != "ANDROID-GENERICS":
            raise ValueError(f"Android generics runtime did not pass: {evidence_path}")
        generics = value.get("android_generics", {})
        if not isinstance(generics, dict):
            raise ValueError(f"Android generics runtime omitted its nested evidence: {evidence_path}")
        digest = generics.get("digest_receipt", {})
        if not isinstance(digest, dict):
            raise ValueError(f"Android generics runtime omitted its v2 digest receipt: {evidence_path}")
        if (
            digest.get("schema") != "stasis.android.generics.v2"
            or digest.get("test_id") != "ANDROID-GENERICS"
            or type(digest.get("frame")) is not int
            or digest.get("frame") != 1
            or type(digest.get("digest")) is not int
            or digest.get("digest") != 507
            or type(digest.get("math_raw_digest")) is not int
            or digest.get("math_raw_digest") != -1430176193
            or digest.get("event") != "oracle"
        ):
            raise ValueError(
                "Android generics runtime digest differs from the accepted raw-bit oracle: "
                f"{evidence_path}"
            )
        bounds = generics.get("bounds_probes")
        if not isinstance(bounds, list) or len(bounds) != 2:
            raise ValueError(f"Android generics runtime has malformed bounds-probe evidence: {evidence_path}")
        bounds_by_index: dict[int, dict[str, Any]] = {}
        bound_pids: list[int] = []
        for probe in bounds:
            if not isinstance(probe, dict):
                raise ValueError(f"Android generics runtime has malformed bounds-probe evidence: {evidence_path}")
            index = probe.get("index")
            if isinstance(index, bool) or not isinstance(index, int) or index not in (-1, 2) or index in bounds_by_index:
                raise ValueError(f"Android generics runtime has invalid bounds-probe indices: {evidence_path}")
            pid = probe.get("pid")
            if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
                raise ValueError(f"Android generics runtime has invalid bounds-probe PID: {evidence_path}")
            if probe.get("signal") != "SIGILL" or probe.get("process_exited") is not True:
                raise ValueError(f"Android generics runtime has invalid fatal bounds-probe result: {evidence_path}")
            bounds_by_index[index] = probe
            bound_pids.append(pid)
        if set(bounds_by_index) != {-1, 2}:
            raise ValueError(f"Android generics runtime omitted a low or high bounds probe: {evidence_path}")
        if len(set(bound_pids)) != 2:
            raise ValueError(f"Android generics runtime reused a bounds-probe PID: {evidence_path}")
        frame_path = Path(generics.get("frame_capture", ""))
        if not frame_path.is_absolute():
            frame_path = evidence_path.parent / frame_path
        if not frame_path.is_file():
            raise ValueError(f"Android generics screenshot is missing: {frame_path}")
    else:
        candidates = sorted(consumer_root.rglob("simulator-evidence.json"))
        if len(candidates) != 1:
            raise ValueError(f"iOS consumer must have one simulator-evidence.json, found {len(candidates)} under {consumer_root}")
        evidence_path = candidates[0]
        value = json.loads(evidence_path.read_text(encoding="utf-8"))
        receipt = value.get("receipt")
        if not isinstance(receipt, dict):
            raise ValueError(f"iOS generics simulator evidence omitted its nested receipt: {evidence_path}")
        frame = receipt.get("frame")
        if (
            value.get("status") != "passed"
            or value.get("schema") != "stasis.ios.generics.evidence.v2"
            or receipt.get("schema") != "stasis.ios.generics.v2"
            or any(type(receipt.get(field)) is not int or receipt.get(field) != 0
                   for field in ("main_result", "tick_result", "render_result"))
            or type(frame) is not int
            or frame < 1
            or type(receipt.get("digest")) is not int
            or receipt.get("digest") != 507
            or type(receipt.get("math_raw_digest")) is not int
            or receipt.get("math_raw_digest") != -1430176193
            or value.get("bounds", {}).get("low", {}).get("index") != -1
            or value.get("bounds", {}).get("high", {}).get("index") != 2
        ):
            raise ValueError(
                f"iOS generics simulator evidence failed receipt, digest, or bounds checks: {evidence_path}"
            )
        frame_path = evidence_path.parent / "simulator-frame.png"
        if not frame_path.is_file():
            raise ValueError(f"iOS simulator frame is missing: {frame_path}")
        for name, expected_index in (("bounds-low.json", -1), ("bounds-high.json", 2)):
            bounds_path = evidence_path.parent / name
            if not bounds_path.is_file():
                raise ValueError(f"iOS simulator bounds evidence is missing: {bounds_path}")
            bounds = json.loads(bounds_path.read_text(encoding="utf-8"))
            if bounds.get("index") != expected_index or bounds.get("fatal") is not True:
                raise ValueError(f"iOS simulator bounds evidence is invalid: {bounds_path}")
    evidence_files = sorted(
        path for path in evidence_path.parent.rglob("*")
        if path.is_file() and path.suffix.lower() in {".json", ".png", ".log", ".txt"}
    )
    resolved_evidence_files = {path.resolve(strict=True) for path in evidence_files}
    if (
        evidence_path.resolve(strict=True) not in resolved_evidence_files
        or frame_path.resolve(strict=True) not in resolved_evidence_files
    ):
        raise ValueError(f"{target} runtime evidence does not contain its receipt and frame")
    return evidence_path, frame_path, evidence_files


def record_runtime(
    receipt_path: Path,
    target: str,
    bundled_root: Path,
    generated_root: Path,
    *,
    expected_release: str,
    expected_source: str,
) -> dict[str, Any]:
    if target not in {"android", "ios"}:
        raise ValueError(f"unsupported mobile target {target!r}")
    if not receipt_path.is_file():
        raise ValueError(f"staged archive receipt is missing: {receipt_path}")
    for root in (bundled_root, generated_root):
        if not root.is_dir():
            raise ValueError(f"mobile consumer evidence root is missing: {root}")
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if receipt.get("schema") != "stasis.staged_archive_acceptance.v3":
        raise ValueError("staged archive receipt has an unsupported schema")
    if receipt.get("target") != target:
        raise ValueError("mobile evidence target differs from staged archive receipt")

    if receipt.get("release_id") != expected_release or receipt.get("source_commit") != expected_source:
        raise ValueError("mobile evidence identity differs from its staged archive receipt")
    consumers = receipt.get("consumers")
    if not isinstance(consumers, dict) or set(consumers) != {"bundled", "generated"}:
        raise ValueError("staged archive receipt omitted bundled/generated consumers")

    execution_receipts: dict[str, dict[str, Any]] = {}
    for label, consumer_root in (("bundled", bundled_root), ("generated", generated_root)):
        if label not in consumers:
            raise ValueError(f"staged archive receipt omitted {label} consumer")
        package_identity = load_package_identity(
            consumer_root,
            target,
            receipt,
            expected_release=expected_release,
            expected_source=expected_source,
        )
        shipping_artifact, link_files = find_shipping_link_evidence(consumer_root, target)
        package_identity["shipping_package"]["linked_artifact_sha256"] = sha256(shipping_artifact)
        package_identity["shipping_package"]["link_evidence"] = [
            {"path": path.relative_to(consumer_root).as_posix(), "sha256": sha256(path)}
            for path in link_files
        ]
        evidence_path, frame_path, files = find_runtime_evidence(consumer_root, target)
        execution_target = package_identity["test_target"]
        entry = {
            "result": "passed",
            "platform": target,
            "target": execution_target,
            "development_build": True,
            "purpose": "test_only_emulator_or_simulator_execution",
            "frame_sha256": sha256(frame_path),
            "evidence": [
                {"path": path.relative_to(consumer_root).as_posix(), "sha256": sha256(path)}
                for path in files
            ],
        }
        consumers[label]["mobile"] = package_identity
        consumers[label]["mobile_runtime"] = entry
        execution_receipts[label] = entry
    runtime = {"platform": target, "result": "passed", "consumers": execution_receipts}
    runtime["execution_target"] = "android-x86_64" if target == "android" else "ios-simulator-arm64"
    runtime["development_build"] = True
    runtime["purpose"] = "test_only_emulator_or_simulator_execution"
    receipt["runtime_acceptance"] = runtime
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(runtime, indent=2, sort_keys=True))
    return runtime


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--target", choices=("android", "ios"), required=True)
    parser.add_argument("--bundled-root", type=Path, required=True)
    parser.add_argument("--generated-root", type=Path, required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--source-commit", required=True)
    args = parser.parse_args()
    if re.fullmatch(r"nightly-\d{8}-\d+", args.release_id) is None:
        raise ValueError("release ID must have the exact nightly-YYYYMMDD-NNN form")
    if re.fullmatch(r"[0-9a-f]{40}", args.source_commit) is None:
        raise ValueError("source commit must be a lowercase 40-character Git object ID")
    record_runtime(
        args.receipt,
        args.target,
        args.bundled_root,
        args.generated_root,
        expected_release=args.release_id,
        expected_source=args.source_commit,
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"mobile archive evidence recording failed: {error}", file=sys.stderr)
        raise SystemExit(1)

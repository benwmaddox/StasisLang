#!/usr/bin/env python3
"""Require complete, same-run staged archive receipts before nightly publication."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any


TARGET_FILES = {
    "windows": ("stasis-nightly-win-x64", "stasis-nightly-win-x64.zip"),
    "linux": ("stasis-nightly-linux-x64", "stasis-nightly-linux-x64.tar.gz"),
    "macos": ("stasis-nightly-osx-arm64", "stasis-nightly-osx-arm64.tar.gz"),
    "android": ("stasis-nightly-linux-x64", "stasis-nightly-linux-x64.tar.gz"),
    "ios": ("stasis-nightly-osx-arm64", "stasis-nightly-osx-arm64.tar.gz"),
}
ALLOWED_ARTIFACT_NAMES = {
    **{target: {artifact} for target, (artifact, _) in TARGET_FILES.items()},
    "windows": {"stasis-nightly-win-x64", "stasis-nightly-win-x64-unsigned"},
}
CONSUMER_COMMANDS = {"fmt-check", "check", "test", "jit-headless"}
HASH_PATTERN = re.compile(r"^[0-9a-f]{64}$")
SOURCE_PATTERN = re.compile(r"^[0-9a-f]{40}$")
RELEASE_PATTERN = re.compile(r"^nightly-\d{8}-\d+$")
TOOLCHAIN_LAYOUT = {
    "windows": ("x86_64-pc-windows-msvc", "stasis.exe", "stasis_graphics.dll"),
    "linux": ("x86_64-unknown-linux-gnu", "bin/stasis", "bin/libstasis_graphics.so"),
    "macos": ("aarch64-apple-darwin", "bin/stasis", "bin/libstasis_graphics.dylib"),
    "android": ("x86_64-unknown-linux-gnu", "bin/stasis", "bin/libstasis_graphics.so"),
    "ios": ("aarch64-apple-darwin", "bin/stasis", "bin/libstasis_graphics.dylib"),
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def require_sha256(value: Any, label: str) -> None:
    require(
        isinstance(value, str) and HASH_PATTERN.fullmatch(value) is not None,
        f"{label} is not a lowercase SHA-256 digest",
    )


def verify_consumer_commands(consumer: dict[str, Any], label: str, target: str) -> None:
    require(
        set(consumer.get("commands", {})) == CONSUMER_COMMANDS
        and all(result == "passed" for result in consumer["commands"].values()),
        f"{target} {label} consumer did not pass format/check/test/JIT",
    )
    release = consumer.get("post_update_release", consumer.get("release"))
    require(release, f"{target} {label} consumer omitted its installed vendor release")
    require(
        HASH_PATTERN.fullmatch(consumer.get("post_update_sha256", consumer.get("sha256", "")))
        is not None,
        f"{target} {label} consumer omitted its installed vendor hash",
    )


def verify_native(native: Any, label: str, fingerprint: str, target: str) -> None:
    require(isinstance(native, dict), f"{target} {label} consumer omitted native package execution")
    require(native.get("result") == "passed", f"{target} {label} native execution failed")
    project_name = native.get("project_name")
    expected_name = project_name == "generics_collections" if label == "bundled" else (
        isinstance(project_name, str) and project_name.startswith("ArchiveFresh")
    )
    require(expected_name, f"{target} {label} native package name did not come from the expected consumer")
    require(native.get("build_fingerprint") == fingerprint, f"{target} {label} native package uses another fingerprint")
    require_sha256(native.get("package_provenance_sha256"), f"{target} {label} package provenance hash")
    require_sha256(native.get("executable_sha256"), f"{target} {label} executable hash")
    require_sha256(native.get("frame_sha256"), f"{target} {label} frame hash")
    frame = native.get("frame", {})
    pixels = frame.get("width", 0) * frame.get("height", 0)
    require(
        frame.get("width") == 640
        and frame.get("height") == 360
        and frame.get("teal", 0) > pixels // 100
        and frame.get("red", 100) < 100
        and frame.get("blue", 0) > pixels // 2,
        f"{target} {label} native frame failed the digest/background oracle",
    )


def verify_web(web: Any, label: str, target: str, fingerprint: str) -> None:
    require(isinstance(web, dict), f"{target} {label} consumer omitted packaged Web execution")
    require(web.get("result") == "passed", f"{target} {label} Web execution failed")
    require(bool(web.get("project_name")), f"{target} {label} Web receipt omitted its manifest name")
    require(web.get("build_fingerprint") == fingerprint, f"{target} {label} Web uses another runtime fingerprint")
    require_sha256(web.get("build_fingerprint"), f"{target} {label} Web build fingerprint")
    for field in ("package_provenance_sha256", "frame_sha256", "result_sha256"):
        require_sha256(web.get(field), f"{target} {label} Web {field}")
    require(web.get("state_digest") == 507, f"{target} {label} Web digest differs from the oracle")
    require(web.get("bounds") == {"low": True, "high": True}, f"{target} {label} Web bounds traps are incomplete")


def verify_mobile(
    mobile: Any,
    label: str,
    target: str,
    release_id: str,
    source_commit: str,
    toolchain: dict[str, Any],
) -> None:
    require(isinstance(mobile, dict), f"{target} {label} consumer omitted its shipped mobile package")
    require(mobile.get("result") == "passed", f"{target} {label} mobile package identity failed")
    require(mobile.get("release_id") == release_id, f"{target} {label} mobile package release mismatch")
    require(mobile.get("source_commit") == source_commit, f"{target} {label} mobile package source mismatch")
    shipping_target, test_target = (
        ("android-arm64", "android-x86_64")
        if target == "android"
        else ("ios-arm64", "ios-simulator-arm64")
    )
    expected_targets = sorted((shipping_target, test_target))
    require(mobile.get("package_targets") == expected_targets,
            f"{target} {label} mobile package targets are {mobile.get('package_targets')!r}, expected {expected_targets!r}")
    require(mobile.get("shipping_target") == shipping_target,
            f"{target} {label} shipping target is not {shipping_target}")
    require(mobile.get("test_target") == test_target,
            f"{target} {label} test target is not {test_target}")
    require_sha256(mobile.get("compiler_sha256"), f"{target} {label} mobile compiler hash")
    require(mobile.get("compiler_sha256") == toolchain["cli_sha256"],
            f"{target} {label} mobile package used another archive compiler")
    require_sha256(mobile.get("runtime_fingerprint"), f"{target} {label} mobile runtime fingerprint")
    require(mobile.get("runtime_fingerprint") == toolchain["build_fingerprint"],
            f"{target} {label} mobile package used another runtime fingerprint")
    packages = mobile.get("packages")
    require(isinstance(packages, list) and len(packages) == 2,
            f"{target} {label} shipping/test package evidence is missing")
    by_target = {package.get("target"): package for package in packages if isinstance(package, dict)}
    require(set(by_target) == {shipping_target, test_target},
            f"{target} {label} package targets are duplicated or incomplete")
    for package in packages:
        require(isinstance(package, dict), f"{target} {label} mobile package digest entry is malformed")
        package_target = package.get("target")
        is_test_target = package_target == test_target
        require(package.get("development_build") is is_test_target,
                f"{target} {label} {package_target} development flag does not match its test-only status")
        require(package.get("compiler_sha256") == toolchain["cli_sha256"],
                f"{target} {label} {package_target} was not built by the accepted archive CLI")
        require(package.get("runtime_sources") == toolchain["runtime_sources"],
                f"{target} {label} {package_target} runtime source hashes differ from the accepted archive")
        if is_test_target:
            require(package.get("release_tag") is None and package.get("dirty_state") is True,
                    f"{target} {label} test-only package falsely claims official provenance")
            require(package.get("source_commit") in (None, "unknown", source_commit),
                    f"{target} {label} test-only package names an unrelated source commit")
        else:
            require(package.get("release_tag") == release_id
                    and package.get("source_commit") == source_commit
                    and package.get("dirty_state") is False,
                    f"{target} {label} shipping package provenance differs from the accepted archive")
        require_sha256(package.get("provenance_sha256"), f"{target} {label} mobile provenance hash")
        require_sha256(package.get("manifest_sha256"), f"{target} {label} mobile manifest hash")
    shipping = mobile.get("shipping_package")
    require(isinstance(shipping, dict) and shipping.get("result") == "passed",
            f"{target} {label} shipping package/link audit did not pass")
    require(shipping.get("target") == shipping_target and shipping.get("development_build") is False,
            f"{target} {label} shipping package is not the official {shipping_target} target")
    require(shipping.get("release_id") == release_id and shipping.get("source_commit") == source_commit,
            f"{target} {label} shipping package identity differs from the accepted archive")
    require(shipping.get("compiler_sha256") == toolchain["cli_sha256"]
            and shipping.get("runtime_sources") == toolchain["runtime_sources"],
            f"{target} {label} shipping package does not use the accepted compiler and runtime sources")
    require(shipping.get("runtime_fingerprint") == toolchain["build_fingerprint"],
            f"{target} {label} shipping package is not bound to the archive runtime fingerprint")
    require_sha256(shipping.get("provenance_sha256"), f"{target} {label} shipping provenance hash")
    require_sha256(shipping.get("manifest_sha256"), f"{target} {label} shipping manifest hash")
    require_sha256(shipping.get("linked_artifact_sha256"), f"{target} {label} shipping linked artifact hash")
    link_evidence = shipping.get("link_evidence")
    require(isinstance(link_evidence, list) and bool(link_evidence),
            f"{target} {label} shipping package omitted link evidence")
    for entry in link_evidence:
        require(isinstance(entry, dict) and bool(entry.get("path")),
                f"{target} {label} shipping link evidence path is malformed")
        require_sha256(entry.get("sha256"), f"{target} {label} shipping link evidence hash")
    test_package = mobile.get("test_package")
    require(isinstance(test_package, dict) and test_package.get("result") == "passed",
            f"{target} {label} test-only package identity is missing")
    require(test_package.get("target") == test_target and test_package.get("development_build") is True,
            f"{target} {label} test-only package is not the supported {test_target} target")
    require(test_package.get("release_tag") is None and test_package.get("dirty_state") is True,
            f"{target} {label} test-only package falsely claims official provenance")
    require(test_package.get("compiler_sha256") == toolchain["cli_sha256"]
            and test_package.get("runtime_sources") == toolchain["runtime_sources"],
            f"{target} {label} test-only package does not bind to the accepted archive inputs")
    require(test_package.get("runtime_fingerprint") == toolchain["build_fingerprint"],
            f"{target} {label} test-only execution receipt omits the archive fingerprint")


def verify_mobile_execution(execution: Any, label: str, target: str) -> None:
    require(isinstance(execution, dict), f"{target} {label} consumer omitted shipped mobile execution")
    require(execution.get("result") == "passed", f"{target} {label} mobile runtime failed")
    require(execution.get("platform") == target, f"{target} {label} mobile runtime platform mismatch")
    expected_test_target = "android-x86_64" if target == "android" else "ios-simulator-arm64"
    require(execution.get("target") == expected_test_target
            and execution.get("development_build") is True
            and execution.get("purpose") == "test_only_emulator_or_simulator_execution",
            f"{target} {label} execution is not clearly labeled as test-only")
    require_sha256(execution.get("frame_sha256"), f"{target} {label} mobile frame hash")
    files = execution.get("evidence")
    require(isinstance(files, list) and bool(files), f"{target} {label} mobile runtime evidence is missing")
    for entry in files:
        require(isinstance(entry, dict) and bool(entry.get("path")), f"{target} {label} runtime evidence path is malformed")
        require_sha256(entry.get("sha256"), f"{target} {label} runtime evidence hash")


def load_receipts(receipt_root: Path) -> dict[str, dict[str, Any]]:
    receipts: dict[str, dict[str, Any]] = {}
    paths = sorted(receipt_root.rglob("staged-archive-receipt.json"))
    require(bool(paths), f"no staged archive receipts were found under {receipt_root}")
    for path in paths:
        value = json.loads(path.read_text(encoding="utf-8"))
        require(
            value.get("schema") == "stasis.staged_archive_acceptance.v1",
            f"unsupported staged archive receipt schema in {path}",
        )
        target = value.get("target")
        require(target in TARGET_FILES, f"unknown target {target!r} in {path}")
        require(target not in receipts, f"duplicate staged archive receipt for {target}")
        receipts[target] = value
    require(
        set(receipts) == set(TARGET_FILES),
        f"staged archive targets are {sorted(receipts)}, expected {sorted(TARGET_FILES)}",
    )
    return receipts


def verify_receipts(
    receipt_root: Path,
    archive_root: Path,
    *,
    expected_repository: str,
    expected_run_id: str,
    expected_release_id: str,
    expected_source_commit: str,
    require_signed_windows: bool = False,
) -> dict[str, str]:
    receipts = load_receipts(receipt_root)
    checked: dict[str, str] = {}
    shared_toolchains: dict[str, dict[str, Any]] = {}
    for target, receipt in receipts.items():
        expected_artifact, expected_filename = TARGET_FILES[target]
        require(
            receipt.get("repository") == expected_repository,
            f"{target} receipt belongs to another repository",
        )
        require(
            str(receipt.get("workflow_run_id")) == str(expected_run_id),
            f"{target} receipt belongs to another workflow run",
        )
        require(
            receipt.get("release_id") == expected_release_id,
            f"{target} receipt names a different release",
        )
        require(
            receipt.get("source_commit") == expected_source_commit,
            f"{target} receipt names a different source commit",
        )
        require(RELEASE_PATTERN.fullmatch(expected_release_id) is not None,
                "expected release ID must have the exact nightly-YYYYMMDD-NNN form")
        require(SOURCE_PATTERN.fullmatch(expected_source_commit) is not None,
                "expected source commit must be a lowercase 40-character Git object ID")
        artifact_name = receipt.get("artifact_name")
        require(
            artifact_name in ALLOWED_ARTIFACT_NAMES[target],
            f"{target} receipt names unexpected artifact {artifact_name!r}",
        )
        if target == "windows" and require_signed_windows:
            require(
                artifact_name == expected_artifact,
                "publisher requires the signed Windows archive, not the unsigned build artifact",
            )
        require(
            receipt.get("archive_file") == expected_filename,
            f"{target} receipt names archive {receipt.get('archive_file')!r}, expected {expected_filename!r}",
        )
        archive = archive_root / expected_filename
        require(archive.is_file(), f"publisher did not download {archive}")
        archive_hash = sha256_file(archive)
        require(
            receipt.get("archive_sha256") == archive_hash,
            f"{target} acceptance did not test the bytes selected for publication",
        )
        toolchain = receipt.get("toolchain")
        require(isinstance(toolchain, dict), f"{target} receipt omitted toolchain identity")
        expected_target, expected_cli, expected_runtime = TOOLCHAIN_LAYOUT[target]
        require(
            toolchain.get("release_id") == expected_release_id
            and toolchain.get("source_commit") == expected_source_commit
            and toolchain.get("target") == expected_target
            and toolchain.get("executable_path") == expected_cli
            and toolchain.get("runtime_path") == expected_runtime,
            f"{target} receipt has incomplete staged toolchain identity",
        )
        for field in ("build_fingerprint", "runtime_build_fingerprint", "cli_sha256", "runtime_sha256"):
            require_sha256(toolchain.get(field), f"{target} staged toolchain {field}")
        require(
            toolchain["build_fingerprint"] == toolchain["runtime_build_fingerprint"],
            f"{target} staged compiler and runtime fingerprints differ",
        )
        runtime_sources = toolchain.get("runtime_sources")
        require(isinstance(runtime_sources, dict) and bool(runtime_sources),
                f"{target} receipt omitted archived runtime source fingerprints")
        for path, digest in runtime_sources.items():
            require(isinstance(path, str) and path.startswith("runtime/"),
                    f"{target} runtime source path is malformed")
            require_sha256(digest, f"{target} runtime source {path}")
        shared_toolchains[target] = toolchain

        consumers = receipt.get("consumers")
        require(isinstance(consumers, dict) and set(consumers) == {"bundled", "generated"},
                f"{target} receipt omitted a required consumer")
        for label, consumer in consumers.items():
            verify_consumer_commands(consumer, label, target)
            require(
                consumer.get("post_update_release", consumer.get("release")) == expected_release_id,
                f"{target} {label} consumer uses a different vendor release",
            )
        rollback = receipt.get("vendor_failure_rollback", {})
        require(
            rollback.get("result") == "rejected_tampered_archive_without_consumer_changes"
            and rollback.get("consumer_tree_sha256")
            and rollback.get("consumer_manifest_sha256"),
            f"{target} receipt omitted vendor update failure rollback evidence",
        )

        for label, consumer in consumers.items():
            if target in {"windows", "linux", "macos"}:
                verify_native(consumer.get("native"), label, toolchain["build_fingerprint"], target)
            if target == "windows":
                verify_web(consumer.get("web"), label, target, toolchain["build_fingerprint"])
            if target in {"android", "ios"}:
                verify_mobile(
                    consumer.get("mobile"),
                    label,
                    target,
                    expected_release_id,
                    expected_source_commit,
                    toolchain,
                )
                runtime_acceptance = receipt.get("runtime_acceptance")
                require(isinstance(runtime_acceptance, dict) and runtime_acceptance.get("result") == "passed",
                        f"{target} receipt omitted shipped mobile runtime execution evidence")
                require(runtime_acceptance.get("platform") == target,
                        f"{target} runtime evidence records another platform")
                expected_test_target = "android-x86_64" if target == "android" else "ios-simulator-arm64"
                require(runtime_acceptance.get("execution_target") == expected_test_target
                        and runtime_acceptance.get("development_build") is True
                        and runtime_acceptance.get("purpose") == "test_only_emulator_or_simulator_execution",
                        f"{target} runtime evidence does not distinguish test-only execution from shipping package evidence")
                execution_consumers = runtime_acceptance.get("consumers")
                require(isinstance(execution_consumers, dict) and set(execution_consumers) == {"bundled", "generated"},
                        f"{target} runtime evidence omitted a required consumer")
                verify_mobile_execution(execution_consumers.get(label), label, target)
                require(consumer.get("mobile_runtime") == execution_consumers[label],
                        f"{target} {label} mobile runtime identity differs from its consumer record")
        checked[target] = archive_hash
    require(
        checked["android"] == checked["linux"]
        and shared_toolchains["android"]["cli_sha256"] == shared_toolchains["linux"]["cli_sha256"]
        and shared_toolchains["android"]["runtime_sha256"] == shared_toolchains["linux"]["runtime_sha256"]
        and shared_toolchains["android"]["build_fingerprint"] == shared_toolchains["linux"]["build_fingerprint"],
        "Android lane did not qualify the exact Linux archive/compiler/runtime bytes",
    )
    require(
        checked["ios"] == checked["macos"]
        and shared_toolchains["ios"]["cli_sha256"] == shared_toolchains["macos"]["cli_sha256"]
        and shared_toolchains["ios"]["runtime_sha256"] == shared_toolchains["macos"]["runtime_sha256"]
        and shared_toolchains["ios"]["build_fingerprint"] == shared_toolchains["macos"]["build_fingerprint"],
        "iOS lane did not qualify the exact macOS archive/compiler/runtime bytes",
    )
    return checked


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipts", type=Path, required=True)
    parser.add_argument("--archive-dir", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--require-signed-windows", action="store_true")
    args = parser.parse_args()
    checked = verify_receipts(
        args.receipts,
        args.archive_dir,
        expected_repository=args.repository,
        expected_run_id=args.run_id,
        expected_release_id=args.release_id,
        expected_source_commit=args.source_commit,
        require_signed_windows=args.require_signed_windows,
    )
    print(json.dumps({"result": "passed", "archives": checked}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"staged archive receipt verification failed: {error}", file=sys.stderr)
        raise SystemExit(1)

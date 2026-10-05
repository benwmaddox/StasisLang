#!/usr/bin/env python3
"""Collect reviewable staged-archive evidence without build or package trees."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys
from typing import Any


MAX_FILE_BYTES = 20 * 1024 * 1024
DIRECT_EVIDENCE_SUFFIXES = {".json", ".log", ".txt", ".png", ".crash", ".ips"}
MANIFEST_NAMES = {
    "engine_bundle_manifest.json",
    "manifest.json",
    "mobile_aot_bundle_manifest.json",
    "stasis.json",
    "stasis_mobile_package.json",
    "stasis_provenance.json",
    "stasis_release_provenance.json",
}
SKIPPED_DIRECTORIES = {
    ".git",
    ".gradle",
    "build",
    "derived-data",
    "downloads",
    "frameworks",
    "node_modules",
    "simulator-derived-data",
    "target",
}
SHARED_QUALIFICATION_FILES = (
    "bundle-audit.json",
    "bundle-audit.log",
    "editor-info.log",
    "vendor-rollback-probe.log",
    "bundled-vendor-before.log",
    "bundled-vendor-update.log",
    "bundled-vendor-status.json.log",
    "historical-stale-vendor-before.log",
    "historical-stale-vendor-update.log",
    "historical-stale-vendor-status.json.log",
    "generated-project-new.log",
    "generated-vendor-status.json.log",
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class EvidenceCollector:
    def __init__(self, output: Path) -> None:
        self.output = output.resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        if any(self.output.iterdir()):
            raise ValueError(f"evidence output directory is not empty: {self.output}")
        self.files: list[dict[str, Any]] = []
        self.missing: list[str] = []
        self.destinations: set[Path] = set()
        self.copied_sources: set[Path] = set()
        self.total_bytes = 0

    def add(self, source: Path, destination: str, label: str, *, required: bool = False) -> bool:
        relative = Path(destination)
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"evidence destination escapes its output: {destination}")
        if not source.is_file() or source.is_symlink():
            if required:
                self.missing.append(label)
            return False
        size = source.stat().st_size
        if size > MAX_FILE_BYTES:
            if required:
                self.missing.append(f"{label} (exceeds {MAX_FILE_BYTES} byte evidence limit)")
            return False
        if self.total_bytes + size > 100 * 1024 * 1024:
            if required:
                self.missing.append(f"{label} (exceeds 100 MiB evidence bundle limit)")
            return False
        if relative in self.destinations:
            raise ValueError(f"duplicate evidence destination: {destination}")
        self.destinations.add(relative)
        target = self.output / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        self.copied_sources.add(source.resolve())
        self.total_bytes += size
        self.files.append(
            {
                "source": label.replace("\\", "/"),
                "path": relative.as_posix(),
                "size_bytes": size,
                "sha256": sha256(target),
            }
        )
        return True

    def require(self, source: Path, label: str) -> None:
        normalized = label.replace("\\", "/")
        if not source.is_file() or source.is_symlink():
            self.missing.append(normalized)
        elif source.resolve() not in self.copied_sources:
            self.missing.append(f"{normalized} (not retained in the review evidence)")

    def add_direct(
        self,
        source: Path,
        destination: str,
        label: str,
        *,
        suffixes: set[str] = DIRECT_EVIDENCE_SUFFIXES,
        exclude_names: set[str] | None = None,
    ) -> None:
        if not source.is_dir() or source.is_symlink():
            return
        excluded = exclude_names or set()
        for path in sorted(source.iterdir()):
            if path.name in excluded or path.suffix.lower() not in suffixes:
                continue
            self.add(path, f"{destination}/{path.name}", f"{label}/{path.name}")

    def add_package_manifests(
        self,
        package: Path,
        destination: str,
        label: str,
        *,
        require_provenance: bool = False,
    ) -> None:
        if not package.is_dir():
            if require_provenance:
                self.missing.append(f"{label}/stasis_provenance.json")
            return
        found_provenance = False
        for parent, directories, names in os.walk(package, followlinks=False):
            directories[:] = sorted(
                name
                for name in directories
                if name not in SKIPPED_DIRECTORIES
                and not (Path(parent) / name).is_symlink()
            )
            for name in sorted(set(names) & MANIFEST_NAMES):
                source = Path(parent) / name
                relative = source.relative_to(package)
                copied = self.add(
                    source,
                    f"{destination}/{relative.as_posix()}",
                    f"{label}/{relative.as_posix()}",
                    required=require_provenance and name == "stasis_provenance.json",
                )
                found_provenance = found_provenance or (copied and name == "stasis_provenance.json")
        if require_provenance and not found_provenance:
            marker = f"{label}/stasis_provenance.json"
            if marker not in self.missing:
                self.missing.append(marker)


def _receipt_summary(
    collector: EvidenceCollector,
    staged_root: Path,
    lane_passed: bool,
    expected_target: str,
) -> dict[str, Any] | None:
    receipt_path = staged_root / "staged-archive-receipt.json"
    if not receipt_path.is_file() or receipt_path.is_symlink():
        if lane_passed:
            collector.missing.append("staged/staged-archive-receipt.json")
        return None
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        if lane_passed:
            collector.missing.append(f"staged/staged-archive-receipt.json ({error})")
        return {"sha256": sha256(receipt_path), "parse_error": str(error)}
    if not isinstance(receipt, dict):
        if lane_passed:
            collector.missing.append("staged/staged-archive-receipt.json (root is not an object)")
        return {"sha256": sha256(receipt_path), "parse_error": "receipt root is not an object"}
    receipt_target = receipt.get("target")
    if lane_passed and receipt_target != expected_target:
        collector.missing.append(
            "staged/staged-archive-receipt.json "
            f"(target {receipt_target!r} does not match expected {expected_target!r})"
        )
    toolchain = receipt.get("toolchain", {})
    if not isinstance(toolchain, dict):
        toolchain = {}
        if lane_passed:
            collector.missing.append("staged/staged-archive-receipt.json (toolchain is not an object)")
    consumers = receipt.get("consumers", {})
    if not isinstance(consumers, dict) and lane_passed:
        collector.missing.append("staged/staged-archive-receipt.json (consumers is not an object)")
        consumers = {}
    return {
        "sha256": sha256(receipt_path),
        "target": receipt.get("target"),
        "workflow_run_id": receipt.get("workflow_run_id"),
        "artifact_name": receipt.get("artifact_name"),
        "archive_sha256": receipt.get("archive_sha256"),
        "release_id": receipt.get("release_id"),
        "source_commit": receipt.get("source_commit"),
        "toolchain": {
            key: toolchain.get(key)
            for key in (
                "target",
                "build_fingerprint",
                "runtime_build_fingerprint",
                "cli_sha256",
                "runtime_sha256",
            )
        },
        "consumer_labels": sorted(consumers),
    }


def _collect_shared_qualification(
    collector: EvidenceCollector,
    staged_root: Path,
    archive_root: Path,
    lane_passed: bool,
    expected_target: str,
) -> dict[str, Any] | None:
    collector.add_direct(
        staged_root,
        "qualification",
        "staged",
        suffixes={".json", ".log", ".txt"},
        exclude_names={"staged-archive-receipt.json"},
    )
    archive_provenance = archive_root / "stasis_release_provenance.json"
    collector.add(
        archive_provenance,
        "provenance/stasis_release_provenance.json",
        "archive/stasis_release_provenance.json",
        required=lane_passed,
    )
    for consumer in ("bundled-generics", "generated-generics"):
        project = staged_root / "consumers" / consumer
        for relative in (Path("stasis.json"), Path("vendor/stasis/stasis.json")):
            collector.add(
                project / relative,
                f"manifests/{consumer}/{relative.as_posix()}",
                f"staged/consumers/{consumer}/{relative.as_posix()}",
            )
    return _receipt_summary(collector, staged_root, lane_passed, expected_target)


def collect_desktop(
    collector: EvidenceCollector,
    target: str,
    staged_root: Path,
    archive_root: Path,
    lane_passed: bool,
) -> dict[str, Any] | None:
    receipt = _collect_shared_qualification(collector, staged_root, archive_root, lane_passed, target)
    if lane_passed:
        collector.require(staged_root / "hosted-chrome-version.txt", "staged/hosted-chrome-version.txt")
    for name in SHARED_QUALIFICATION_FILES:
        collector.require(staged_root / name, f"staged/{name}") if lane_passed else None
    for label, consumer_name in (("bundled", "bundled-generics"), ("generated", "generated-generics")):
        desktop_evidence = staged_root / "desktop" / label
        collector.add_direct(desktop_evidence, f"native/{label}", f"staged/desktop/{label}")
        for name in ("desktop-package.log", "desktop-provenance-audit.log", "desktop-runtime.log", "desktop-frame.png"):
            collector.require(desktop_evidence / name, f"staged/desktop/{label}/{name}") if lane_passed else None
        consumer = staged_root / "consumers" / consumer_name
        package = consumer / "dist" / f"staged-desktop-{target}"
        collector.add_package_manifests(
            package,
            f"native/{label}/package-manifests",
            f"staged/consumers/{consumer_name}/dist/staged-desktop-{target}",
            require_provenance=lane_passed,
        )
        if lane_passed:
            for name in ("fmt-check", "check", "test", "jit-headless"):
                collector.require(staged_root / f"{label}-{name}.log", f"staged/{label}-{name}.log")
        web = staged_root / "web" / label
        collector.add_direct(web, f"web/{label}", f"staged/web/{label}")
        collector.add_direct(web / "browser", f"web/{label}/browser", f"staged/web/{label}/browser")
        web_package = consumer / "build" / "staged-web"
        collector.add_package_manifests(
            web_package,
            f"web/{label}/package-manifests",
            f"staged/consumers/{consumer_name}/build/staged-web",
            require_provenance=lane_passed,
        )
        if lane_passed:
            for name in ("web-package.log", "web-browser.log"):
                collector.require(web / name, f"staged/web/{label}/{name}")
            for name in ("receipt.json", "browser.png"):
                collector.require(web / "browser" / name, f"staged/web/{label}/browser/{name}")
    return receipt


def collect_android(
    collector: EvidenceCollector,
    staged_root: Path,
    archive_root: Path,
    runtime_root: Path,
    lane_passed: bool,
) -> dict[str, Any] | None:
    receipt = _collect_shared_qualification(collector, staged_root, archive_root, lane_passed, "android")
    for name in SHARED_QUALIFICATION_FILES:
        collector.require(staged_root / name, f"staged/{name}") if lane_passed else None
    for label, consumer_name in (("bundled", "bundled-generics"), ("generated", "generated-generics")):
        consumer_root = runtime_root / label
        consumer_test_root = consumer_root / "test" / "android_generics_collections"
        runtime_evidence = consumer_test_root / "e"
        runtime_evidence_label = f"android-runtime/{label}/test/android_generics_collections/e"
        collector.add_direct(runtime_evidence, f"android/{label}/runtime", runtime_evidence_label)
        if lane_passed:
            for name in (
                "evidence.json",
                "stable-frame.png",
                "android-logcat.txt",
                "bounds-low-logcat.txt",
                "bounds-high-logcat.txt",
                "android-test-signer.json",
            ):
                collector.require(runtime_evidence / name, f"{runtime_evidence_label}/{name}")
        test_package = consumer_test_root / "w" / "d"
        collector.add_package_manifests(
            test_package,
            f"android/{label}/test-package-manifests",
            f"android-runtime/{label}/test/android_generics_collections/w/d",
            require_provenance=lane_passed,
        )
        shipping = consumer_root / "shipping"
        collector.add_direct(shipping, f"android/{label}/shipping", f"android-runtime/{label}/shipping", suffixes={".log", ".json", ".txt"})
        for name in ("android-apk-audit.log", "gradle-link.log", "package-provenance.log", "apksigner.log"):
            collector.require(shipping / name, f"android-runtime/{label}/shipping/{name}") if lane_passed else None
        shipping_package = shipping / "package"
        collector.add_package_manifests(
            shipping_package,
            f"android/{label}/shipping-package-manifests",
            f"android-runtime/{label}/shipping/package",
            require_provenance=lane_passed,
        )
        if lane_passed:
            for name in ("stasis_mobile_package.json", "stasis_provenance.json"):
                collector.require(shipping_package / name, f"android-runtime/{label}/shipping/package/{name}")
            for name in ("mobile_aot_bundle_manifest.json", "engine_bundle_manifest.json"):
                collector.require(shipping_package / "aot" / name, f"android-runtime/{label}/shipping/package/aot/{name}")
        if lane_passed:
            sample_manifest = staged_root / "consumers" / consumer_name / "stasis.json"
            collector.require(sample_manifest, f"staged/consumers/{consumer_name}/stasis.json")
    return receipt


def collect_ios(
    collector: EvidenceCollector,
    staged_root: Path,
    archive_root: Path,
    lane_passed: bool,
) -> dict[str, Any] | None:
    receipt = _collect_shared_qualification(collector, staged_root, archive_root, lane_passed, "ios")
    for name in SHARED_QUALIFICATION_FILES:
        collector.require(staged_root / name, f"staged/{name}") if lane_passed else None
    for label, consumer_name in (("bundled", "bundled-generics"), ("generated", "generated-generics")):
        consumer = staged_root / "consumers" / consumer_name
        build_root = consumer / "target" / "ios-archive-acceptance"
        collector.add_direct(build_root, f"ios/{label}/acceptance", f"staged/consumers/{consumer_name}/target/ios-archive-acceptance")
        if lane_passed:
            for name in (
                "evidence.txt",
                "xcode-version.txt",
                "xcodebuild.log",
                "device-platform.txt",
                "linked-libraries.txt",
                "device-symbols.txt",
                "device-hashes.txt",
                "simulator-platform.txt",
                "simulator-linked-libraries.txt",
                "simulator-symbols.txt",
                "simulator-hashes.txt",
                "simulator-xcodebuild.log",
                "simulator-launch.txt",
                "simulator-result.json",
                "simulator-frame.png",
                "simulator.log",
                "simulator-evidence.json",
                "simulator-evidence.txt",
                "bounds-low.json",
                "bounds-high.json",
                "bounds-low-launch.txt",
                "bounds-high-launch.txt",
            ):
                required_source = build_root / name
                collector.require(required_source, f"staged/consumers/{consumer_name}/target/ios-archive-acceptance/{name}")
            for bounds_label in ("low", "high"):
                crash_reports = sorted(build_root.glob(f"bounds-{bounds_label}-crash.*"))
                if len(crash_reports) != 1:
                    collector.missing.append(
                        f"staged/consumers/{consumer_name}/target/ios-archive-acceptance/bounds-{bounds_label}-crash.*"
                    )
                elif crash_reports[0].resolve() not in collector.copied_sources:
                    collector.missing.append(
                        f"staged/consumers/{consumer_name}/target/ios-archive-acceptance/{crash_reports[0].name} (not retained in the review evidence)"
                    )
        for package_name in (f"staged-ios-{consumer_name}", f"staged-ios-{consumer_name}-simulator"):
            package = consumer / "dist" / package_name
            collector.add_package_manifests(
                package,
                f"ios/{label}/{package_name}/manifests",
                f"staged/consumers/{consumer_name}/dist/{package_name}",
                require_provenance=lane_passed,
            )
            if lane_passed:
                for relative in (
                    "stasis_mobile_package.json",
                    "stasis_provenance.json",
                    "aot/mobile_aot_bundle_manifest.json",
                    "aot/engine_bundle_manifest.json",
                ):
                    collector.require(package / relative, f"staged/consumers/{consumer_name}/dist/{package_name}/{relative}")
    return receipt


def collect_evidence(
    *,
    target: str,
    staged_root: Path,
    archive_root: Path,
    output: Path,
    runtime_root: Path | None = None,
    lane_passed: bool,
) -> dict[str, Any]:
    collector = EvidenceCollector(output)
    staged_root = staged_root.resolve()
    archive_root = archive_root.resolve()
    if target in {"windows", "linux", "macos"}:
        receipt = collect_desktop(collector, target, staged_root, archive_root, lane_passed)
    elif target == "android":
        if runtime_root is None:
            raise ValueError("Android evidence collection requires --runtime-root")
        receipt = collect_android(collector, staged_root, archive_root, runtime_root.resolve(), lane_passed)
    elif target == "ios":
        receipt = collect_ios(collector, staged_root, archive_root, lane_passed)
    else:
        raise ValueError(f"unsupported staged archive target: {target}")

    manifest = {
        "schema": "stasis.staged_release_review_evidence.v1",
        "target": target,
        "lane_passed": lane_passed,
        "status": "complete" if lane_passed and not collector.missing else "diagnostic_partial" if not lane_passed else "incomplete",
        "receipt": receipt,
        "files": sorted(collector.files, key=lambda item: item["path"]),
        "missing_required": sorted(set(collector.missing)),
        "total_bytes": collector.total_bytes,
    }
    (collector.output / "evidence-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=("windows", "linux", "macos", "android", "ios"), required=True)
    parser.add_argument("--staged-root", type=Path, required=True)
    parser.add_argument("--archive-root", type=Path, required=True)
    parser.add_argument("--runtime-root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    lane_passed = os.environ.get("STASIS_STAGED_LANE_PASSED", "false").lower() == "true"
    manifest = collect_evidence(
        target=args.target,
        staged_root=args.staged_root,
        archive_root=args.archive_root,
        runtime_root=args.runtime_root,
        output=args.output,
        lane_passed=lane_passed,
    )
    print(json.dumps({key: manifest[key] for key in ("target", "status", "total_bytes", "missing_required")}, sort_keys=True))
    return 1 if lane_passed and manifest["missing_required"] else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"staged release evidence collection failed: {error}", file=sys.stderr)
        raise SystemExit(1)

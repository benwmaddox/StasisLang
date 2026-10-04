#!/usr/bin/env python3
"""Exercise clean project consumers using one immutable Stasis release archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any


ARCHIVE_LAYOUT = {
    "windows": {
        "target": "x86_64-pc-windows-msvc",
        "cli": "stasis.exe",
        "runtime": "stasis_graphics.dll",
    },
    "linux": {
        "target": "x86_64-unknown-linux-gnu",
        "cli": "bin/stasis",
        "runtime": "bin/libstasis_graphics.so",
    },
    "macos": {
        "target": "aarch64-apple-darwin",
        "cli": "bin/stasis",
        "runtime": "bin/libstasis_graphics.dylib",
    },
}
OBSOLETE_FIXTURE_LABEL = "staged-archive-obsolete-fixture"
OBSOLETE_FIXTURE_MODULE = "vendor/stasis/stdlib/staged_archive_obsolete_fixture.stasis"
OBSOLETE_FIXTURE_KIND = "obsolete_unimported_vendor_module"
OBSOLETE_FIXTURE_SOURCE = (
    "function staged_archive_obsolete_fixture(): i32 {\n"
    "    return 0;\n"
    "}\n"
)
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


def toolchain_environment() -> dict[str, str]:
    """Avoid ambient runtime overrides when probing or using the archived CLI."""
    environment = os.environ.copy()
    for key in (
        "STASIS_RUNTIME_LIBRARY_PATH",
        "STASIS_RUNTIME_DLL_PATH",
        "STASIS_RUNTIME_RUNNER_PATH",
    ):
        environment.pop(key, None)
    return environment


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def tree_sha256(root: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise RuntimeError(f"consumer contains an unexpected symlink: {path}")
        if path.is_file():
            digest.update(path.relative_to(root).as_posix().encode("utf-8"))
            digest.update(b"\0")
            digest.update(bytes.fromhex(sha256_file(path)))
            digest.update(b"\n")
    return digest.hexdigest()


def find_cli(root: Path) -> Path:
    candidates = (root / "stasis.exe", root / "bin" / "stasis")
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    raise RuntimeError(f"release archive has no supported CLI executable under {root}")


def run(
    command: list[str],
    *,
    cwd: Path,
    log: Path,
    env: dict[str, str] | None = None,
    timeout: int = 900,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    log.parent.mkdir(parents=True, exist_ok=True)
    print("+ " + subprocess.list2cmdline(command), flush=True)
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=timeout,
        check=False,
    )
    log.write_text(
        f"$ {subprocess.list2cmdline(command)}\n"
        f"exit_code={completed.returncode}\n"
        f"--- stdout ---\n{completed.stdout}\n"
        f"--- stderr ---\n{completed.stderr}\n",
        encoding="utf-8",
    )
    if check and completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}); see {log}: "
            f"{completed.stderr.strip() or completed.stdout.strip()}"
        )
    return completed


def run_json(
    cli: Path,
    args: list[str],
    *,
    cwd: Path,
    log: Path,
    check: bool = True,
) -> dict[str, Any]:
    completed = run(
        [str(cli), "--json", *args],
        cwd=cwd,
        log=log,
        env=toolchain_environment(),
        check=check,
    )
    output = completed.stdout.strip() or completed.stderr.strip()
    if not output:
        raise RuntimeError(f"CLI returned no JSON output; see {log}")
    try:
        value = json.loads(output)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"CLI returned malformed JSON; see {log}: {error}") from error
    if not isinstance(value, dict) or value.get("ok") is not (completed.returncode == 0):
        raise RuntimeError(f"CLI JSON status did not match its exit code; see {log}")
    return value


def copy_sample_sources(source: Path, generated: Path) -> None:
    excluded = {"vendor", "stasis.json", "build", "dist", "target", ".git"}
    for child in source.iterdir():
        if child.name in excluded:
            continue
        destination = generated / child.name
        if destination.is_dir():
            shutil.rmtree(destination)
        elif destination.exists():
            destination.unlink()
        if child.is_dir():
            shutil.copytree(child, destination)
        else:
            shutil.copy2(child, destination)

    generated_manifest_path = generated / "stasis.json"
    generated_manifest = json.loads(generated_manifest_path.read_text(encoding="utf-8"))
    sample_manifest = json.loads((source / "stasis.json").read_text(encoding="utf-8"))
    generated_name = generated_manifest["name"]
    generated_vendor = generated_manifest.get("vendor")
    generated_manifest.update(
        {key: value for key, value in sample_manifest.items() if key not in {"name", "vendor"}}
    )
    generated_manifest["name"] = generated_name
    if generated_vendor is not None:
        generated_manifest["vendor"] = generated_vendor
    generated_manifest_path.write_text(
        json.dumps(generated_manifest, indent=2) + "\n", encoding="utf-8"
    )


def verify_identity(
    cli: Path,
    archive_root: Path,
    *,
    target: str,
    expected_release: str,
    expected_source: str,
    evidence: Path,
) -> dict[str, Any]:
    archive_platform = {"android": "linux", "ios": "macos"}.get(target, target)
    expected_layout = ARCHIVE_LAYOUT[archive_platform]
    expected_cli = (archive_root / expected_layout["cli"]).resolve(strict=True)
    if cli.resolve(strict=True) != expected_cli:
        raise RuntimeError("selected CLI path is not the canonical executable in this archive")
    provenance_path = archive_root / "stasis_release_provenance.json"
    if not provenance_path.is_file():
        raise RuntimeError(f"release provenance is missing: {provenance_path}")
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if provenance.get("schema") != "stasis.release_provenance.v1":
        raise RuntimeError("release provenance schema is not stasis.release_provenance.v1")
    if re.fullmatch(r"nightly-\d{8}-\d+", expected_release) is None:
        raise RuntimeError("release ID must have the exact nightly-YYYYMMDD-NNN form")
    if re.fullmatch(r"[0-9a-f]{40}", expected_source) is None:
        raise RuntimeError("source commit must be a lowercase 40-character Git object ID")
    if provenance.get("release_tag") != expected_release:
        raise RuntimeError(
            f"archive tag {provenance.get('release_tag')!r} != {expected_release!r}"
        )
    if provenance.get("source_commit") != expected_source:
        raise RuntimeError(
            f"archive source {provenance.get('source_commit')!r} != {expected_source!r}"
        )
    if provenance.get("dirty_state") is not False or provenance.get("development_build") is not False:
        raise RuntimeError("staged archive is not a clean release build")

    response = run_json(cli, ["editor-info"], cwd=archive_root, log=evidence / "editor-info.log")
    editor = response.get("result")
    if not isinstance(editor, dict):
        raise RuntimeError("editor-info omitted its result object")
    if editor.get("release_id") != expected_release or editor.get("source_commit") != expected_source:
        raise RuntimeError("editor-info identity differs from the release provenance")
    if editor.get("target") != expected_layout["target"]:
        raise RuntimeError(
            f"archive target {editor.get('target')!r} != {expected_layout['target']!r}"
        )
    executable_info = editor.get("executable")
    runtime_info = editor.get("graphics_runtime")
    if not isinstance(executable_info, dict) or not isinstance(runtime_info, dict):
        raise RuntimeError("editor-info omitted executable or graphics runtime identity")

    def require_archive_file(info: dict[str, Any], relative: str, label: str) -> tuple[Path, str]:
        raw_path = info.get("path")
        if not isinstance(raw_path, str) or not raw_path:
            raise RuntimeError(f"editor-info omitted {label} path")
        # Windows may return the Win32 extended-length prefix for an ordinary path.
        path_text = raw_path[4:] if raw_path.startswith("\\\\?\\") else raw_path
        resolved = Path(path_text).resolve(strict=True)
        archive_path = (archive_root / relative).resolve(strict=True)
        try:
            resolved.relative_to(archive_root.resolve(strict=True))
        except ValueError as error:
            raise RuntimeError(f"editor-info {label} path escapes the extracted archive") from error
        if resolved != archive_path:
            raise RuntimeError(f"editor-info selected an unexpected {label} path: {resolved}")
        digest = info.get("sha256")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise RuntimeError(f"editor-info {label} SHA-256 is malformed")
        if sha256_file(resolved) != digest:
            raise RuntimeError(f"editor-info {label} SHA-256 does not match its archive file")
        return resolved, digest

    _, cli_sha256 = require_archive_file(executable_info, expected_layout["cli"], "compiler")
    _, runtime_sha256 = require_archive_file(
        runtime_info, expected_layout["runtime"], "graphics runtime"
    )
    compiler_provenance = provenance.get("compiler")
    if not isinstance(compiler_provenance, dict):
        raise RuntimeError("release provenance omitted the compiler artifact identity")
    if compiler_provenance.get("path") != expected_layout["cli"]:
        raise RuntimeError("release provenance compiler path differs from archive layout")
    if compiler_provenance.get("sha256") != cli_sha256:
        raise RuntimeError("release provenance compiler hash differs from the archived executable")
    runtime_sources = provenance.get("runtime_sources")
    if not isinstance(runtime_sources, dict) or any(
        not isinstance(path, str)
        or not isinstance(digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", digest) is None
        for path, digest in runtime_sources.items()
    ):
        raise RuntimeError("release provenance runtime source hashes are malformed")
    if runtime_info.get("release_id") != expected_release:
        raise RuntimeError("graphics runtime release ID differs from the accepted archive")
    fingerprint = editor.get("build_fingerprint")
    if not isinstance(fingerprint, str) or re.fullmatch(r"[0-9a-f]{64}", fingerprint) is None:
        raise RuntimeError("archive build fingerprint is malformed")
    if fingerprint != runtime_info.get("build_fingerprint"):
        raise RuntimeError("CLI and graphics runtime fingerprints differ")
    return {
        "release_id": expected_release,
        "source_commit": expected_source,
        "target": editor["target"],
        "executable_path": expected_layout["cli"],
        "runtime_path": expected_layout["runtime"],
        "build_fingerprint": fingerprint,
        "runtime_build_fingerprint": runtime_info["build_fingerprint"],
        "cli_sha256": cli_sha256,
        "runtime_sha256": runtime_sha256,
        "runtime_sources": runtime_sources,
    }


def require_vendor_current(
    cli: Path,
    project: Path,
    *,
    expected_release: str,
    evidence: Path,
    label: str,
) -> dict[str, Any]:
    status = run_json(
        cli,
        ["--workspace", str(project), "vendor", "status"],
        cwd=project,
        log=evidence / f"{label}-vendor-status.json.log",
    ).get("result")
    if not isinstance(status, dict):
        raise RuntimeError(f"{label} vendor status omitted its result")
    normalized = normalize_vendor_status(status, expected_release=expected_release, label=label)
    if (
        normalized["current"] is not True
        or normalized["update_available"] is not False
        or normalized["pin_verified"] is not True
        or normalized["legacy_pin_unverified"] is not False
        or normalized["local_changes"] is not False
        or normalized["recorded_sha256"] != normalized["actual_sha256"]
        or normalized["installed_sha256"] != normalized["actual_sha256"]
    ):
        raise RuntimeError(f"{label} vendor snapshot is not current and clean: {status}")
    if normalized["recorded_release_id"] != expected_release:
        raise RuntimeError(f"{label} recorded vendor pin names another release: {status}")
    return status


def normalize_vendor_status(
    status: dict[str, Any], *, expected_release: str, label: str
) -> dict[str, Any]:
    """Validate and retain the status fields needed to prove a vendor transition."""
    for field in ("current", "update_available", "pin_verified", "legacy_pin_unverified", "local_changes"):
        if type(status.get(field)) is not bool:
            raise RuntimeError(f"{label} vendor status omitted boolean {field}: {status}")
    recorded = status.get("recorded")
    installed = status.get("installed")
    if not isinstance(recorded, dict) or not isinstance(installed, dict):
        raise RuntimeError(f"{label} vendor status omitted recorded or installed pin: {status}")
    recorded_release = recorded.get("release_id")
    installed_release = installed.get("release_id")
    if not isinstance(recorded_release, str) or not recorded_release:
        raise RuntimeError(f"{label} recorded vendor release is malformed: {status}")
    if installed_release != expected_release:
        raise RuntimeError(f"{label} is pinned to another release: {installed}")
    installed_sha256 = installed.get("sha256")
    if (
        type(installed.get("hash_version")) is not int
        or installed["hash_version"] != 2
        or not isinstance(installed_sha256, str)
        or SHA256_PATTERN.fullmatch(installed_sha256) is None
    ):
        raise RuntimeError(f"{label} installed vendor hash is not canonical v2: {status}")
    for field in ("recorded_hash_version", "actual_hash_version"):
        if type(status.get(field)) is not int or status[field] != 2:
            raise RuntimeError(f"{label} vendor status is not canonical hash version 2: {status}")
    recorded_sha256 = status.get("recorded_sha256")
    actual_sha256 = status.get("actual_sha256")
    if (
        not isinstance(recorded_sha256, str)
        or SHA256_PATTERN.fullmatch(recorded_sha256) is None
        or not isinstance(actual_sha256, str)
        or SHA256_PATTERN.fullmatch(actual_sha256) is None
    ):
        raise RuntimeError(f"{label} vendor status contains a malformed SHA-256: {status}")
    return {
        "current": status["current"],
        "update_available": status["update_available"],
        "pin_verified": status["pin_verified"],
        "legacy_pin_unverified": status["legacy_pin_unverified"],
        "local_changes": status["local_changes"],
        "recorded_release_id": recorded_release,
        "installed_release_id": installed_release,
        "installed_sha256": installed_sha256,
        "recorded_hash_version": status["recorded_hash_version"],
        "actual_hash_version": status["actual_hash_version"],
        "recorded_sha256": recorded_sha256,
        "actual_sha256": actual_sha256,
    }


def require_verified_source_vendor(
    status: dict[str, Any], *, expected_release: str
) -> dict[str, Any]:
    normalized = normalize_vendor_status(
        status, expected_release=expected_release, label="bundled source"
    )
    if (
        normalized["pin_verified"] is not True
        or normalized["legacy_pin_unverified"] is not False
        or normalized["local_changes"] is not False
        or normalized["recorded_sha256"] != normalized["actual_sha256"]
        or normalized["current"] == normalized["update_available"]
        or (
            normalized["current"]
            and normalized["actual_sha256"] != normalized["installed_sha256"]
        )
        or (
            normalized["update_available"]
            and normalized["actual_sha256"] == normalized["installed_sha256"]
        )
    ):
        raise RuntimeError(f"bundled source vendor pin is not verified and clean: {status}")
    return normalized


def stage_obsolete_vendor_fixture(
    cli: Path,
    project: Path,
    evidence: Path,
    *,
    expected_release: str,
) -> tuple[dict[str, Any], dict[str, Any], Path]:
    """Create a CLI-hashed stale pin whose obsolete unimported module must be removed."""
    source_status = run_json(
        cli,
        ["--workspace", str(project), "vendor", "status"],
        cwd=project,
        log=evidence / "bundled-source-vendor-status.log",
    ).get("result")
    if not isinstance(source_status, dict):
        raise RuntimeError("bundled source vendor status omitted its result")
    source_vendor = require_verified_source_vendor(
        source_status, expected_release=expected_release
    )

    fixture = project / OBSOLETE_FIXTURE_MODULE
    if fixture.exists() or fixture.is_symlink():
        raise RuntimeError(f"staged-archive stale fixture already exists: {fixture}")
    fixture.parent.mkdir(parents=True, exist_ok=True)
    fixture.write_text(OBSOLETE_FIXTURE_SOURCE, encoding="utf-8", newline="\n")

    fixture_hash_status = run_json(
        cli,
        ["--workspace", str(project), "vendor", "status"],
        cwd=project,
        log=evidence / "bundled-stale-fixture-hash.log",
    ).get("result")
    if not isinstance(fixture_hash_status, dict):
        raise RuntimeError("CLI vendor status omitted the stale fixture tree hash")
    fixture_hash = fixture_hash_status.get("actual_sha256")
    if (
        not isinstance(fixture_hash, str)
        or SHA256_PATTERN.fullmatch(fixture_hash) is None
        or type(fixture_hash_status.get("actual_hash_version")) is not int
        or fixture_hash_status["actual_hash_version"] != 2
    ):
        raise RuntimeError(f"CLI did not return a canonical v2 fixture tree hash: {fixture_hash_status}")

    manifest_path = project / "stasis.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    vendor = manifest.get("vendor")
    if not isinstance(vendor, dict) or not isinstance(vendor.get("stasis"), dict):
        raise RuntimeError("bundled sample manifest omitted vendor.stasis")
    vendor["stasis"] = {
        "release_id": OBSOLETE_FIXTURE_LABEL,
        "sha256": fixture_hash,
        "hash_version": 2,
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

    stale_status = run_json(
        cli,
        ["--workspace", str(project), "vendor", "status"],
        cwd=project,
        log=evidence / "bundled-vendor-before.log",
    ).get("result")
    if not isinstance(stale_status, dict):
        raise RuntimeError("bundled sample vendor status omitted its stale fixture result")
    normalized_stale = normalize_vendor_status(
        stale_status, expected_release=expected_release, label="bundled stale fixture"
    )
    if (
        normalized_stale["pin_verified"] is not True
        or normalized_stale["legacy_pin_unverified"] is not False
        or normalized_stale["local_changes"] is not False
        or normalized_stale["current"] is not False
        or normalized_stale["update_available"] is not True
        or normalized_stale["recorded_release_id"] != OBSOLETE_FIXTURE_LABEL
        or normalized_stale["recorded_sha256"] != fixture_hash
        or normalized_stale["actual_sha256"] != fixture_hash
    ):
        raise RuntimeError(f"bundled sample did not become a verified clean stale fixture: {stale_status}")
    return source_vendor, normalized_stale, fixture


def require_obsolete_fixture_removed(fixture: Path) -> None:
    if fixture.exists() or fixture.is_symlink():
        raise RuntimeError(f"vendor update retained the obsolete fixture module: {fixture}")


def run_consumer_checks(
    cli: Path, project: Path, evidence: Path, *, label: str
) -> dict[str, str]:
    results: dict[str, str] = {}
    commands = (
        ("fmt-check", ["fmt", "--check"]),
        ("check", ["check"]),
        ("test", ["test"]),
        ("jit-headless", ["run", "--headless", "--ticks", "1"]),
    )
    for name, args in commands:
        run(
            [str(cli), "--workspace", str(project), *args],
            cwd=project,
            log=evidence / f"{label}-{name}.log",
        )
        results[name] = "passed"
    return results


def probe_vendor_failure_rollback(
    archive_root: Path,
    cli: Path,
    consumer_source: Path,
    evidence: Path,
) -> dict[str, str]:
    with tempfile.TemporaryDirectory(prefix="stasis-vendor-rollback-") as temporary:
        probe_root = Path(temporary)
        fault_toolchain = probe_root / "fault-toolchain"
        rollback_consumer = probe_root / "consumer"
        shutil.copytree(archive_root, fault_toolchain)
        shutil.copytree(consumer_source, rollback_consumer)
        fault_cli = find_cli(fault_toolchain)

        candidates = sorted((fault_toolchain / "src" / "stdlib").rglob("*.stasis"))
        if not candidates:
            raise RuntimeError("archive has no stdlib source to fault-inject")
        corrupted = candidates[0]
        with corrupted.open("a", encoding="utf-8", newline="\n") as stream:
            stream.write("\nfunction staged_archive_corruption():i32{return 0;}\n")

        before_tree = tree_sha256(rollback_consumer)
        before_manifest = (rollback_consumer / "stasis.json").read_bytes()
        result = run_json(
            fault_cli,
            ["--workspace", str(rollback_consumer), "vendor", "update"],
            cwd=rollback_consumer,
            log=evidence / "vendor-rollback-probe.log",
            check=False,
        )
        if result.get("ok") is not False:
            raise RuntimeError("tampered archive unexpectedly updated a consumer")
        after_tree = tree_sha256(rollback_consumer)
        if before_tree != after_tree or before_manifest != (rollback_consumer / "stasis.json").read_bytes():
            raise RuntimeError("failed vendor update changed the consumer tree or manifest")
        leftovers = [
            path
            for path in rollback_consumer.rglob("*")
            if ".stasis.sync-" in path.name
            or ".stasis.previous-" in path.name
            or ".vendor-sync-" in path.name
            or ".vendor-previous-" in path.name
        ]
        if leftovers:
            raise RuntimeError(f"failed vendor update left transaction files: {leftovers}")
        return {
            "result": "rejected_tampered_archive_without_consumer_changes",
            "consumer_tree_sha256": before_tree,
            "consumer_manifest_sha256": hashlib.sha256(before_manifest).hexdigest(),
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-root", type=Path, required=True)
    parser.add_argument("--archive-file", type=Path, required=True)
    parser.add_argument("--target", choices=("windows", "linux", "macos", "android", "ios"), required=True)
    parser.add_argument("--artifact-name", required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--browser", action="store_true")
    args = parser.parse_args()

    archive_root = args.archive_root.resolve()
    archive_file = args.archive_file.resolve()
    evidence = args.evidence_dir.resolve()
    sample_source = archive_root / "samples" / "generics_collections"
    if not archive_root.is_dir() or not sample_source.is_dir() or not archive_file.is_file():
        raise RuntimeError("archive root, archive file, or bundled generics sample is missing")
    evidence.mkdir(parents=True, exist_ok=True)
    cli = find_cli(archive_root)
    identity = verify_identity(
        cli,
        archive_root,
        target=args.target,
        expected_release=args.release_id,
        expected_source=args.source_commit,
        evidence=evidence,
    )

    if args.target in {"windows", "linux", "macos", "android", "ios"}:
        audit_platform = {"android": "linux", "ios": "macos"}.get(args.target, args.target)
        run(
            [
                sys.executable,
                "tools/audit_release_bundle.py",
                "--root",
                str(archive_root),
                "--platform",
                audit_platform,
                "--archive",
                str(archive_file),
                "--output",
                str(evidence / "bundle-audit.json"),
            ],
            cwd=Path(__file__).resolve().parents[2],
            log=evidence / "bundle-audit.log",
        )

    workspaces = evidence / "consumers"
    if workspaces.exists():
        shutil.rmtree(workspaces)
    workspaces.mkdir(parents=True)
    bundled = workspaces / "bundled-generics"
    shutil.copytree(sample_source, bundled)
    rollback = probe_vendor_failure_rollback(archive_root, cli, sample_source, evidence)

    source_vendor, bundled_before, obsolete_fixture = stage_obsolete_vendor_fixture(
        cli, bundled, evidence, expected_release=args.release_id
    )
    run_json(
        cli,
        ["--workspace", str(bundled), "vendor", "update"],
        cwd=bundled,
        log=evidence / "bundled-vendor-update.log",
    )
    require_obsolete_fixture_removed(obsolete_fixture)
    bundled_after = require_vendor_current(
        cli, bundled, expected_release=args.release_id, evidence=evidence, label="bundled"
    )

    generated = workspaces / "generated-generics"
    generated_name = f"ArchiveFresh{args.target.title()}"
    run_json(
        cli,
        ["new", "--dir", str(generated), generated_name],
        cwd=workspaces,
        log=evidence / "generated-project-new.log",
    )
    copy_sample_sources(sample_source, generated)
    generated_vendor = require_vendor_current(
        cli, generated, expected_release=args.release_id, evidence=evidence, label="generated"
    )

    consumers = {
        "bundled": {
            "pre_update_release": bundled_before["recorded_release_id"],
            "pre_update_sha256": bundled_before["recorded_sha256"],
            "post_update_release": bundled_after.get("installed", {}).get("release_id"),
            "post_update_sha256": bundled_after.get("actual_sha256"),
            "source_vendor": source_vendor,
            "stale_fixture": {
                "kind": OBSOLETE_FIXTURE_KIND,
                "label": OBSOLETE_FIXTURE_LABEL,
                "module_path": OBSOLETE_FIXTURE_MODULE,
                "pre_update_sha256": bundled_before["actual_sha256"],
                "pre_update_status": bundled_before,
                "removed": True,
            },
            "commands": run_consumer_checks(cli, bundled, evidence, label="bundled"),
        },
        "generated": {
            "release": generated_vendor.get("installed", {}).get("release_id"),
            "sha256": generated_vendor.get("actual_sha256"),
            "commands": run_consumer_checks(cli, generated, evidence, label="generated"),
        },
    }

    desktop: dict[str, Any] | None = None
    if args.target in {"windows", "linux", "macos"}:
        desktop = {
            "bundled": qualify_desktop(
                cli,
                bundled,
                evidence / "desktop" / "bundled",
                args.target,
                identity=identity,
                expected_release=args.release_id,
                expected_source=args.source_commit,
                archive_root=archive_root,
            ),
            "generated": qualify_desktop(
                cli,
                generated,
                evidence / "desktop" / "generated",
                args.target,
                identity=identity,
                expected_release=args.release_id,
                expected_source=args.source_commit,
                archive_root=archive_root,
            ),
        }

    browser: dict[str, Any] | None = None
    if args.browser:
        browser = {
            "bundled": qualify_web(
                cli,
                bundled,
                evidence / "web" / "bundled",
                identity=identity,
                expected_release=args.release_id,
                expected_source=args.source_commit,
                archive_root=archive_root,
            ),
            "generated": qualify_web(
                cli,
                generated,
                evidence / "web" / "generated",
                identity=identity,
                expected_release=args.release_id,
                expected_source=args.source_commit,
                archive_root=archive_root,
            ),
        }

    if desktop is not None:
        consumers["bundled"]["native"] = desktop["bundled"]
        consumers["generated"]["native"] = desktop["generated"]
    if browser is not None:
        consumers["bundled"]["web"] = browser["bundled"]
        consumers["generated"]["web"] = browser["generated"]

    receipt = {
        "schema": "stasis.staged_archive_acceptance.v3",
        "target": args.target,
        "repository": args.repository,
        "workflow_run_id": str(args.run_id),
        "artifact_name": args.artifact_name,
        "archive_file": archive_file.name,
        "archive_sha256": sha256_file(archive_file),
        "release_id": args.release_id,
        "source_commit": args.source_commit,
        "toolchain": identity,
        "consumers": consumers,
        "vendor_failure_rollback": rollback,
        "qualified_at_unix": int(time.time()),
    }
    (evidence / "staged-archive-receipt.json").write_text(
        json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(receipt, indent=2, sort_keys=True))
    return 0


def project_name(project: Path) -> str:
    manifest = json.loads((project / "stasis.json").read_text(encoding="utf-8"))
    name = manifest.get("name")
    if not isinstance(name, str) or not name or Path(name).name != name:
        raise RuntimeError(f"project manifest has an invalid package name: {name!r}")
    return name


def validate_package_identity(
    provenance_path: Path,
    *,
    identity: dict[str, Any],
    expected_release: str,
    expected_source: str,
) -> dict[str, Any]:
    if not provenance_path.is_file():
        raise RuntimeError(f"package provenance is missing: {provenance_path}")
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if provenance.get("schema") != "stasis.release_provenance.v1":
        raise RuntimeError("package provenance has an unsupported schema")
    if provenance.get("release_tag") != expected_release:
        raise RuntimeError("package provenance release tag differs from the accepted archive")
    if provenance.get("source_commit") != expected_source:
        raise RuntimeError("package provenance source commit differs from the accepted archive")
    if provenance.get("dirty_state") is not False or provenance.get("development_build") is not False:
        raise RuntimeError("package provenance is not an official clean release build")
    compiler = provenance.get("compiler")
    if not isinstance(compiler, dict) or compiler.get("sha256") != identity["cli_sha256"]:
        raise RuntimeError("package compiler hash differs from the accepted archive CLI")
    if provenance.get("runtime_sources") != identity.get("runtime_sources"):
        raise RuntimeError("package runtime source hashes differ from the accepted archive")
    if identity.get("build_fingerprint") != identity.get("runtime_build_fingerprint"):
        raise RuntimeError("archive compiler and runtime fingerprints differ")
    return provenance


def qualify_desktop(
    cli: Path,
    project: Path,
    evidence: Path,
    target: str,
    *,
    identity: dict[str, Any],
    expected_release: str,
    expected_source: str,
    archive_root: Path,
) -> dict[str, Any]:
    name = project_name(project)
    package_output = f"dist/staged-desktop-{target}"
    run_json(
        cli,
        [
            "--workspace",
            str(project),
            "package",
            "--target",
            "desktop",
            "--out",
            package_output,
        ],
        cwd=project,
        log=evidence / "desktop-package.log",
    )
    package = project / package_output
    if os.name == "nt":
        executable = package / f"{name}.exe"
        payload = package / "app"
    elif sys.platform == "darwin":
        executable = package / f"{name}.app" / "Contents" / "MacOS" / name
        payload = package
    else:
        executable = package / name
        payload = package
    if not executable.is_file():
        raise RuntimeError(f"desktop package executable is missing: {executable}")
    provenance_path = payload / "stasis_provenance.json"
    provenance = validate_package_identity(
        provenance_path,
        identity=identity,
        expected_release=expected_release,
        expected_source=expected_source,
    )
    verifier = Path(__file__).resolve().parents[1] / "verify_package_provenance.py"
    run(
        [
            sys.executable,
            str(verifier),
            "--release-root",
            str(archive_root),
            "--package-root",
            str(payload),
            "--expect-desktop-package",
        ],
        cwd=Path(__file__).resolve().parents[2],
        log=evidence / "desktop-provenance-audit.log",
    )
    vendor = provenance.get("desktop_package", {}).get("project", {}).get("vendor", {})
    if (
        vendor.get("release_id") != expected_release
        or vendor.get("recorded_sha256") != vendor.get("actual_sha256")
    ):
        raise RuntimeError("desktop package provenance reports vendor drift or wrong release")

    screenshot = evidence / "desktop-frame.png"
    environment = toolchain_environment()
    environment.update(
        {
            "STASIS_SCREENSHOT_ONCE": str(screenshot),
            "STASIS_SCREENSHOT_FRAME": "2",
            "STASIS_EXIT_AFTER_SCREENSHOT": "1",
            "STASIS_RECORDING_PRESENTATION": "1",
            "STASIS_RECORDING_WIDTH": "640",
            "STASIS_RECORDING_HEIGHT": "360",
            "STASIS_RECORDING_FPS": "60",
            "SDL_RENDER_DRIVER": "software",
            "SDL_AUDIODRIVER": "dummy",
            "STASIS_RUNNER_DIAG": "1",
        }
    )
    command = [str(executable)]
    if sys.platform.startswith("linux"):
        command = ["xvfb-run", "-a", *command]
    launched = run(
        command,
        cwd=package,
        log=evidence / "desktop-runtime.log",
        env=environment,
        timeout=120,
    )
    diagnostics = launched.stdout + "\n" + launched.stderr
    if target == "windows":
        if "invalid_magic" in diagnostics:
            raise RuntimeError("packaged Windows renderer reported invalid command magic")
    elif not any(
        line.startswith("RUNNER_DIAG:")
        and "render_construction_lifecycle_version=1" in line.split()
        for line in diagnostics.splitlines()
    ):
        raise RuntimeError("packaged runner omitted lifecycle-v1 diagnostics")
    if not screenshot.is_file():
        raise RuntimeError("packaged native app did not capture the generics frame")
    frame = inspect_png(screenshot)
    pixels = frame["width"] * frame["height"]
    if frame["teal"] <= pixels // 100:
        raise RuntimeError(f"digest-success rectangle missing: {frame['teal']}/{pixels}")
    if frame["red"] >= 100:
        raise RuntimeError(f"digest-failure rectangle rendered: {frame['red']} pixels")
    if frame["blue"] <= pixels // 2:
        raise RuntimeError(f"authored dark-blue background missing: {frame['blue']}/{pixels}")
    return {
        "project_name": name,
        "package_provenance_sha256": sha256_file(provenance_path),
        "executable_sha256": sha256_file(executable),
        "build_fingerprint": identity["build_fingerprint"],
        "frame": frame,
        "frame_sha256": sha256_file(screenshot),
        "result": "passed",
    }


def validate_generics_browser_receipt(result: Any) -> None:
    """Require the v2 browser oracle receipt with its exact raw math result."""
    if not isinstance(result, dict):
        raise RuntimeError("browser acceptance receipt is not a JSON object")
    if (
        result.get("schema") != "stasis.generics_collections_browser_acceptance.v2"
        or result.get("stateDigest") != 507
        or type(result.get("mathRawDigest")) is not int
        or result.get("mathRawDigest") != -1430176193
        or result.get("mainResult") != 0
    ):
        raise RuntimeError("browser schema, collection, or raw math digest did not match the oracle")
    if result.get("bounds") != {"low": True, "high": True}:
        raise RuntimeError("browser bounds checks did not trap both invalid indices")
    if result.get("failures") != []:
        raise RuntimeError("browser acceptance reported runtime or console failures")


def qualify_web(
    cli: Path,
    project: Path,
    evidence: Path,
    *,
    identity: dict[str, Any],
    expected_release: str,
    expected_source: str,
    archive_root: Path,
) -> dict[str, Any]:
    package_output = "build/staged-web"
    run_json(
        cli,
        [
            "--workspace",
            str(project),
            "package",
            "--target",
            "web",
            "--out",
            package_output,
        ],
        cwd=project,
        log=evidence / "web-package.log",
    )
    bundle = project / package_output
    provenance = validate_package_identity(
        bundle / "stasis_provenance.json",
        identity=identity,
        expected_release=expected_release,
        expected_source=expected_source,
    )
    archived_provenance = json.loads(
        (archive_root / "stasis_release_provenance.json").read_text(encoding="utf-8")
    )
    package_release_identity = dict(provenance)
    for package_field in ("project_configuration", "web_package"):
        package_release_identity.pop(package_field, None)
    if package_release_identity != archived_provenance:
        raise RuntimeError("Web package release provenance differs from the staged archive")
    vendor = provenance.get("web_package", {}).get("project", {}).get("vendor", {})
    if (
        vendor.get("release_id") != expected_release
        or vendor.get("recorded_sha256") != vendor.get("actual_sha256")
    ):
        raise RuntimeError("Web package provenance reports vendor drift or wrong release")
    browser_evidence = evidence / "browser"
    command = [
        "node",
        "tools/run_generics_collections_browser_acceptance.mjs",
        str(bundle),
        str(browser_evidence),
    ]
    run(
        command,
        cwd=Path(__file__).resolve().parents[2],
        log=evidence / "web-browser.log",
        timeout=120,
    )
    result_path = browser_evidence / "receipt.json"
    if not result_path.is_file():
        raise RuntimeError("browser acceptance did not write receipt.json")
    result = json.loads(result_path.read_text(encoding="utf-8"))
    validate_generics_browser_receipt(result)
    return {
        "project_name": project_name(project),
        "package_provenance_sha256": sha256_file(bundle / "stasis_provenance.json"),
        "build_fingerprint": identity["build_fingerprint"],
        "runtime_source_count": len(provenance.get("runtime_sources", {})),
        "result_sha256": sha256_file(result_path),
        "frame_sha256": sha256_file(browser_evidence / "browser.png"),
        "state_digest": result["stateDigest"],
        "math_raw_digest": result["mathRawDigest"],
        "bounds": result["bounds"],
        "result": "passed",
    }


def inspect_png(path: Path) -> dict[str, int]:
    """Decode the small RGB/RGBA screenshot PNGs emitted by the SDL acceptance app."""
    import struct
    import zlib

    data = path.read_bytes()
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise RuntimeError(f"invalid PNG signature: {path}")
    offset = 8
    compressed = bytearray()
    width = height = color_type = bit_depth = 0
    while offset < len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += 12 + length
        if kind == b"IHDR":
            width, height, bit_depth, color_type, compression, filtering, interlace = struct.unpack(
                ">IIBBBBB", payload
            )
            if compression or filtering or interlace or bit_depth != 8:
                raise RuntimeError("desktop screenshot uses an unsupported PNG layout")
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    channels = {2: 3, 6: 4}.get(color_type)
    if not width or not height or channels is None:
        raise RuntimeError("desktop screenshot must be 8-bit RGB or RGBA")
    raw = zlib.decompress(compressed)
    stride = width * channels
    previous = bytearray(stride)
    teal = red = blue = 0
    cursor = 0
    for _ in range(height):
        filter_type = raw[cursor]
        cursor += 1
        row = bytearray(raw[cursor : cursor + stride])
        cursor += stride
        for index in range(stride):
            left = row[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 1:
                row[index] = (row[index] + left) & 0xFF
            elif filter_type == 2:
                row[index] = (row[index] + above) & 0xFF
            elif filter_type == 3:
                row[index] = (row[index] + ((left + above) // 2)) & 0xFF
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                predictor = left if distances[0] <= distances[1] and distances[0] <= distances[2] else (
                    above if distances[1] <= distances[2] else upper_left
                )
                row[index] = (row[index] + predictor) & 0xFF
            elif filter_type != 0:
                raise RuntimeError(f"unsupported PNG row filter {filter_type}")
        for index in range(0, stride, channels):
            r, g, b = row[index : index + 3]
            if g > 130 and g > r + 60 and b > 70:
                teal += 1
            if r > 140 and r > g + 80:
                red += 1
            if b > r + 15 and b > g:
                blue += 1
        previous = row
    return {"width": width, "height": height, "teal": teal, "red": red, "blue": blue}


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        print(f"staged archive qualification failed: {error}", file=sys.stderr)
        raise SystemExit(1)

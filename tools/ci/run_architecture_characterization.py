#!/usr/bin/env python3
"""Validate and optionally run the bounded architecture characterization lane.

The manifest is deliberately data-only.  It records which boundary is covered,
the fixture/evidence source, and the command that owns execution.  The default
gate runs only rows explicitly marked ``default_gate`` in the
``fast-hermetic`` lane; ``--run-lane`` is available for intentional full-lane
runs.  Platform and device rows remain visible but are never silently treated
as local passes.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = ROOT / "tests" / "characterization" / "manifest.json"
SCHEMA = "stasis.architecture_characterization_manifest.v1"
LANES = {"fast-hermetic", "platform-host", "device-browser"}
EVIDENCE = {"behavioral", "structural-lint"}
REQUIRED_ROW_FIELDS = {
    "id",
    "boundary",
    "fixture",
    "owner",
    "evidence",
    "lane",
    "default_gate",
    "command",
    "expected_evidence",
}


class ManifestError(ValueError):
    """Raised when the characterization inventory is incomplete or unsafe."""


def _load(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise ManifestError(f"cannot read manifest {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise ManifestError(f"manifest is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ManifestError("manifest root must be an object")
    return value


def _relative_path(root: Path, raw: str, label: str) -> Path:
    if not isinstance(raw, str) or not raw.strip():
        raise ManifestError(f"{label} must be a non-empty relative path")
    candidate = Path(raw)
    if candidate.is_absolute():
        raise ManifestError(f"{label} must be relative to the repository: {raw}")
    resolved = (root / candidate).resolve()
    try:
        resolved.relative_to(root.resolve())
    except ValueError as error:
        raise ManifestError(f"{label} escapes the repository: {raw}") from error
    if not resolved.is_file():
        raise ManifestError(f"{label} does not exist: {raw}")
    return resolved


def _fixtures(root: Path, row: dict[str, Any]) -> list[Path]:
    fixture = row["fixture"]
    values: Iterable[Any]
    if isinstance(fixture, str):
        values = [fixture]
    elif isinstance(fixture, list):
        values = fixture
    else:
        raise ManifestError(f"row {row.get('id', '<unknown>')} fixture must be a string or list")
    paths = []
    for index, value in enumerate(values):
        paths.append(_relative_path(root, value, f"row {row.get('id', '<unknown>')} fixture[{index}]"))
    if not paths:
        raise ManifestError(f"row {row.get('id', '<unknown>')} must list at least one fixture")
    return paths


def validate_manifest(path: Path = DEFAULT_MANIFEST, root: Path = ROOT) -> dict[str, Any]:
    """Return a validated manifest or raise :class:`ManifestError`."""

    manifest = _load(path)
    if manifest.get("schema") != SCHEMA:
        raise ManifestError(
            f"manifest schema must be {SCHEMA}, got {manifest.get('schema')!r}"
        )
    if manifest.get("version") != 1:
        raise ManifestError("manifest version must be integer 1")
    rows = manifest.get("rows")
    if not isinstance(rows, list) or not rows:
        raise ManifestError("manifest rows must be a non-empty list")

    seen: set[str] = set()
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ManifestError(f"row {index} must be an object")
        missing = REQUIRED_ROW_FIELDS - row.keys()
        if missing:
            raise ManifestError(f"row {index} missing fields: {sorted(missing)}")
        row_id = row["id"]
        if not isinstance(row_id, str) or not row_id.strip():
            raise ManifestError(f"row {index} id must be a non-empty string")
        if row_id in seen:
            raise ManifestError(f"duplicate characterization id: {row_id}")
        seen.add(row_id)
        for field in ("boundary", "owner", "expected_evidence"):
            if not isinstance(row[field], str) or not row[field].strip():
                raise ManifestError(f"row {row_id} {field} must be a non-empty string")
        evidence = row["evidence"]
        if evidence not in EVIDENCE:
            raise ManifestError(
                f"row {row_id} evidence must be one of {sorted(EVIDENCE)}, got {evidence!r}"
            )
        lane = row["lane"]
        if lane not in LANES:
            raise ManifestError(
                f"row {row_id} lane must be one of {sorted(LANES)}, got {lane!r}"
            )
        if not isinstance(row["default_gate"], bool):
            raise ManifestError(f"row {row_id} default_gate must be boolean")
        if row["default_gate"] and lane != "fast-hermetic":
            raise ManifestError(
                f"row {row_id} default_gate is only valid for fast-hermetic rows"
            )
        command = row["command"]
        if not isinstance(command, str) or not command.strip():
            raise ManifestError(f"row {row_id} command must be a non-empty string")
        _fixtures(root, row)

    return manifest


def _lane_rows(manifest: dict[str, Any], lane: str) -> list[dict[str, Any]]:
    return [row for row in manifest["rows"] if row["lane"] == lane]


def _default_gate_rows(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        row
        for row in _lane_rows(manifest, "fast-hermetic")
        if row["default_gate"]
    ]


def run_rows(
    rows: list[dict[str, Any]],
    root: Path = ROOT,
    timeout_seconds: int = 900,
    label: str = "characterization lane",
) -> int:
    """Run each distinct command from a validated set of manifest rows."""

    commands: list[tuple[str, str]] = []
    seen: set[str] = set()
    for row in rows:
        command = row["command"]
        if command not in seen:
            seen.add(command)
            commands.append((row["id"], command))
    if not commands:
        raise ManifestError(f"manifest has no rows for {label}")

    for row_id, command in commands:
        print(f"[architecture-characterization] {row_id}: {command}")
        try:
            result = subprocess.run(
                command,
                cwd=root,
                shell=True,
                check=False,
                timeout=timeout_seconds,
                env=os.environ.copy(),
            )
        except subprocess.TimeoutExpired:
            print(
                f"[architecture-characterization] {row_id} timed out after {timeout_seconds}s",
                file=sys.stderr,
            )
            return 124
        if result.returncode != 0:
            print(
                f"[architecture-characterization] {row_id} failed with exit code {result.returncode}",
                file=sys.stderr,
            )
            return result.returncode
    print(f"architecture characterization {label} passed ({len(commands)} commands)")
    return 0


def run_fast_lane(
    manifest: dict[str, Any], root: Path = ROOT, timeout_seconds: int = 900
) -> int:
    """Run the small, non-duplicative default PR/local characterization gate."""

    return run_rows(
        _default_gate_rows(manifest), root, timeout_seconds, "default gate"
    )


def run_lane(
    manifest: dict[str, Any],
    lane: str,
    root: Path = ROOT,
    timeout_seconds: int = 900,
) -> int:
    """Run every distinct command in a named lane on demand."""

    if lane not in LANES:
        raise ManifestError(f"unknown characterization lane: {lane}")
    return run_rows(
        _lane_rows(manifest, lane), root, timeout_seconds, f"{lane} lane"
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--check",
        action="store_true",
        help="validate the manifest without running test commands",
    )
    parser.add_argument(
        "--run-fast",
        action="store_true",
        help="validate and run only the default PR/local gate rows",
    )
    parser.add_argument(
        "--run-lane",
        choices=sorted(LANES),
        help="validate and run every row in one lane (for intentional full runs)",
    )
    parser.add_argument(
        "--timeout-seconds",
        type=int,
        default=900,
        help="per-command timeout for --run-fast or --run-lane (default: 900)",
    )
    args = parser.parse_args(argv)
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    selected_modes = sum(bool(value) for value in (args.check, args.run_fast, args.run_lane))
    if selected_modes != 1:
        parser.error("choose exactly one of --check, --run-fast, or --run-lane")
    try:
        manifest = validate_manifest(args.manifest.resolve(), ROOT)
        print(f"architecture characterization manifest passed ({len(manifest['rows'])} rows)")
        if args.run_fast:
            return run_fast_lane(manifest, ROOT, args.timeout_seconds)
        if args.run_lane:
            return run_lane(manifest, args.run_lane, ROOT, args.timeout_seconds)
        return 0
    except ManifestError as error:
        print(f"architecture characterization check failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

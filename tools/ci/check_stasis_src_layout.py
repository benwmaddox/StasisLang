#!/usr/bin/env python3
from __future__ import annotations

import os
import pathlib
import re
import sys


RE_IMPORT = re.compile(r'^\s*import\s+"([^"]+)"\s*;\s*(?://.*)?$')
IGNORED_SOURCE_DIRS = {
    ".git",
    ".gradle",
    ".stasis_cache",
    "build",
    "dist",
    "node_modules",
    "target",
    "vendor",
}


def discover_stasis_files(scan_roots: list[pathlib.Path]) -> list[pathlib.Path]:
    """Return application Stasis sources, excluding vendored source trees."""
    stasis_files: list[pathlib.Path] = []
    for root in scan_roots:
        if not root.is_dir():
            continue
        for current_root, directories, filenames in os.walk(root):
            directories[:] = sorted(
                name for name in directories if name not in IGNORED_SOURCE_DIRS
            )
            for filename in sorted(filenames):
                if filename.endswith(".stasis"):
                    stasis_files.append(pathlib.Path(current_root) / filename)
    return sorted(stasis_files, key=lambda path: path.as_posix())


def main() -> int:
    repo_root = pathlib.Path(__file__).resolve().parents[2]
    src_dir = repo_root / "src"
    if not src_dir.is_dir():
        print(f"error: missing src/ directory at {src_dir}", file=sys.stderr)
        return 2

    # Framework-owned Stasis modules must not live directly under src/ root.
    root_level_stasis = sorted(src_dir.glob("*.stasis"))
    if root_level_stasis:
        print(
            "error: framework .stasis files must not live directly under src/ root",
            file=sys.stderr,
        )
        for path in root_level_stasis:
            print(f"- {path.relative_to(repo_root).as_posix()}", file=sys.stderr)
        print("Move them into src/stdlib/.", file=sys.stderr)
        return 1

    stdlib_dir = src_dir / "stdlib"
    if not stdlib_dir.is_dir():
        print(f"error: missing src/stdlib/ directory at {stdlib_dir}", file=sys.stderr)
        return 2

    internal_dir = stdlib_dir / "internal"
    testing_dir = stdlib_dir / "testing"
    required_internal = {
        "gfx_cmd.stasis",
        "host_frame_raw.stasis",
        "host_window_request.stasis",
    }
    missing_internal = sorted(
        name for name in required_internal if not (internal_dir / name).is_file()
    )
    if missing_internal:
        print("error: missing canonical stdlib internal ABI modules:", file=sys.stderr)
        for name in missing_internal:
            print(f"- src/stdlib/internal/{name}", file=sys.stderr)
        return 2
    required_testing = {
        "input_testkit.stasis",
        "ui_layout_audit.stasis",
    }
    missing_testing = sorted(
        name for name in required_testing if not (testing_dir / name).is_file()
    )
    if missing_testing:
        print("error: missing canonical stdlib testing modules:", file=sys.stderr)
        for name in missing_testing:
            print(f"- src/stdlib/testing/{name}", file=sys.stderr)
        return 2

    obsolete_paths = [stdlib_dir / "gfx_cmd.stasis"]
    for obsolete in obsolete_paths:
        if obsolete.exists():
            print(
                f"error: obsolete duplicate module path still exists: "
                f"{obsolete.relative_to(repo_root).as_posix()}",
                file=sys.stderr,
            )
            return 1
    runtime_dir = src_dir / "runtime"
    obsolete_runtime_modules = sorted(runtime_dir.rglob("*.stasis")) if runtime_dir.is_dir() else []
    if obsolete_runtime_modules:
        print("error: obsolete src/runtime Stasis modules still exist:", file=sys.stderr)
        for path in obsolete_runtime_modules:
            print(f"- {path.relative_to(repo_root).as_posix()}", file=sys.stderr)
        return 1

    scan_roots = [src_dir, repo_root / "samples", repo_root / "tests"]
    stasis_files = discover_stasis_files(scan_roots)

    errors: list[str] = []
    for file_path in stasis_files:
        try:
            rel_path = file_path.relative_to(repo_root)
        except ValueError:
            continue

        try:
            text = file_path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            text = file_path.read_text(encoding="utf-8", errors="replace")

        for line_no, line in enumerate(text.splitlines(), start=1):
            match = RE_IMPORT.match(line)
            if not match:
                continue
            import_path = match.group(1)
            norm = import_path.replace("\\", "/")
            if "/runtime/" in norm or norm.startswith("../runtime/"):
                errors.append(
                    f'{rel_path.as_posix()}:{line_no}: obsolete runtime import "{import_path}" '
                    "(host ABI modules live under src/stdlib/internal/)"
                )
                continue

            imports_internal = "/internal/" in norm or norm.startswith("internal/")
            if imports_internal:
                inside_stdlib = rel_path.parts[:2] == ("src", "stdlib")
                integration_test = rel_path.as_posix() in {
                    "tests/stasis/rust_native_tick_input_snapshot.stasis",
                    "tests/stasis/seams/gfx_cmd_capacity_probe.stasis",
                    "tests/stasis/seams/desktop_input_frame_probe.stasis",
                    "tests/stasis/seams/desktop_display_metrics_probe.stasis",
                    "tests/stasis/seams/desktop_manifest_assets_probe.stasis",
                    "tests/stasis/seams/window_request_mailbox_probe.stasis",
                }
                if not inside_stdlib and not integration_test:
                    errors.append(
                        f'{rel_path.as_posix()}:{line_no}: private ABI import "{import_path}" '
                        "(application code must import a public stdlib module)"
                    )

            imports_testing = "/testing/" in norm or norm.startswith("testing/")
            diagnostic_sample = rel_path.as_posix() in {
                "samples/immediate_axis_layout/audit.stasis",
                "samples/immediate_axis_layout/verify.stasis",
                "samples/immediate_axis_layout/verify_jit.stasis",
            }
            if (
                imports_testing
                and not rel_path.name.endswith(".test.stasis")
                and not diagnostic_sample
            ):
                errors.append(
                    f'{rel_path.as_posix()}:{line_no}: test-only import "{import_path}" '
                    "is limited to .test.stasis files"
                )

    if errors:
        print("error: invalid Stasis source-layer imports found:", file=sys.stderr)
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())


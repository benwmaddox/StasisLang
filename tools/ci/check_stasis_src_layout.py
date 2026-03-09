#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import re
import sys


RE_IMPORT = re.compile(r'^\s*import\s+"([^"]+)"\s*;\s*(?://.*)?$')


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
        print("Move them into src/stdlib/ or src/runtime/.", file=sys.stderr)
        return 1

    runtime_dir = src_dir / "runtime"
    stdlib_dir = src_dir / "stdlib"
    if not runtime_dir.is_dir():
        print(f"error: missing src/runtime/ directory at {runtime_dir}", file=sys.stderr)
        return 2
    if not stdlib_dir.is_dir():
        print(f"error: missing src/stdlib/ directory at {stdlib_dir}", file=sys.stderr)
        return 2

    # These runtime support modules were moved out of src/ root. Reject any imports that still
    # target the old locations (either via ../../src/<file>.stasis or via ../<file>.stasis).
    runtime_modules = {
        "gfx_cmd.stasis": "src/runtime/gfx_cmd.stasis",
        "host_frame.stasis": "src/runtime/host_frame.stasis",
        "host_window_request.stasis": "src/runtime/host_window_request.stasis",
        "input_testkit.stasis": "src/runtime/input_testkit.stasis",
    }

    scan_roots = [src_dir, repo_root / "samples", repo_root / "tests"]
    stasis_files: list[pathlib.Path] = []
    for root in scan_roots:
        if not root.is_dir():
            continue
        stasis_files.extend(root.rglob("*.stasis"))

    errors: list[str] = []
    for file_path in stasis_files:
        try:
            rel_path = file_path.relative_to(repo_root)
        except ValueError:
            continue

        in_runtime = rel_path.parts[:2] == ("src", "runtime")
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
            base = norm.rsplit("/", 1)[-1]
            if base not in runtime_modules:
                continue

            # Allow same-folder imports from within src/runtime (e.g. input_testkit -> host_frame).
            if in_runtime and norm == base:
                continue

            # For all other modules, the path must include /runtime/ somewhere.
            if "/runtime/" in norm:
                continue

            expected = runtime_modules[base]
            errors.append(
                f"{rel_path.as_posix()}:{line_no}: deprecated import \"{import_path}\" "
                f"(use \"{expected}\" or a relative path containing /runtime/)"
            )

    if errors:
        print("error: deprecated runtime module imports found:", file=sys.stderr)
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())


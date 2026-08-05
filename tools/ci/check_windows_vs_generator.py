#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import sys


ROOT = pathlib.Path(__file__).resolve().parents[2]
HELPER = "tools/windows/select-cmake-vs-generator.ps1"
CALLERS = (
    ".github/workflows/pr-ci.yml",
    ".github/workflows/bootstrap-artifacts.yml",
    ".github/workflows/nightly-release.yml",
    "scripts/build_local_editor_release.ps1",
)
HELPER_MARKERS = (
    "vswhere.exe",
    "Microsoft.Component.MSBuild Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
    "Visual Studio 18 2026",
    "Visual Studio 17 2022",
    "cmake -E capabilities",
)


def validate(root: pathlib.Path = ROOT) -> list[str]:
    errors: list[str] = []
    helper = root / HELPER
    if not helper.is_file():
        return [f"missing Visual Studio generator helper: {HELPER}"]
    helper_text = helper.read_text(encoding="utf-8")
    for marker in HELPER_MARKERS:
        if marker not in helper_text:
            errors.append(f"{HELPER}: missing {marker!r}")

    for relative in CALLERS:
        path = root / relative
        if not path.is_file():
            errors.append(f"missing Visual Studio generator caller: {relative}")
            continue
        text = path.read_text(encoding="utf-8")
        if "select-cmake-vs-generator.ps1" not in text:
            errors.append(f"{relative}: does not use the installed-instance helper")
        if "cmake --help" in text:
            errors.append(f"{relative}: treats advertised CMake generators as installed")
    return errors


def main() -> int:
    errors = validate()
    if errors:
        print("Windows Visual Studio generator contract failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Windows Visual Studio generator contract passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

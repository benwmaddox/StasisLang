#!/usr/bin/env python3
"""Reject unsafe Rust outside the repository's audited platform boundaries."""

from __future__ import annotations

import re
import sys
from pathlib import Path


UNSAFE_RUST = re.compile(r"\bunsafe\s*(?:\{|fn\b|impl\b|trait\b|extern\b)")
SOURCE_ROOTS = ("apps", "crates", "mobile", "tests")
ALLOWED_PREFIXES = (
    "crates/stasis_dynload/src/",
    "crates/stasis_android_bridge/src/",
    "mobile/android/codex_native/src/",
)
ALLOWED_FILES = {
    "crates/stasis_network/src/lib.rs",
    "crates/stasis_network/src/client.rs",
    "crates/stasis_network/tests/realtime_controls.rs",
    "apps/stasis/tests/desktop_asset_load_stress.rs",
    "apps/stasis/tests/desktop_display_metrics_seam.rs",
    "apps/stasis/tests/desktop_input_frame_seam.rs",
    "apps/stasis/tests/desktop_manifest_assets_seam.rs",
    "apps/stasis/tests/desktop_render_recovery_seam.rs",
    "crates/stasis_ai/src/lib.rs",
}


def unsafe_files(root: Path) -> list[str]:
    found: list[str] = []
    for source_root in SOURCE_ROOTS:
        directory = root / source_root
        if not directory.exists():
            continue
        for path in directory.rglob("*.rs"):
            if UNSAFE_RUST.search(path.read_text(encoding="utf-8")):
                found.append(path.relative_to(root).as_posix())
    return sorted(found)


def unexpected_unsafe_files(root: Path) -> list[str]:
    return [
        path
        for path in unsafe_files(root)
        if path not in ALLOWED_FILES and not path.startswith(ALLOWED_PREFIXES)
    ]


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    unexpected = unexpected_unsafe_files(root)
    if unexpected:
        print("Unsafe Rust is restricted to audited platform boundaries:", file=sys.stderr)
        for path in unexpected:
            print(f"- {path}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

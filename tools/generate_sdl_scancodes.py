#!/usr/bin/env python3
"""Generate the typed SCANCODE group from the repository's pinned SDL 3 header."""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / "src" / "stdlib" / "sdl_scancodes.stasis"
PINNED_HEADER_SHA256 = "d5e3bb25b523b61ed354189919f2bcb058f0f2fa08e0529ce8e21480937c8ca5"
PINNED_ARCHIVE_URL = (
    "https://github.com/libsdl-org/SDL/releases/download/"
    "release-3.4.10/SDL3-3.4.10.tar.gz"
)
PINNED_ARCHIVE_SHA256 = "12b34280415ec8418c864408b93d008a20a6530687ee613d60bfbd20411f2785"
MARKERS = {"UNKNOWN": 0, "RESERVED": 400, "COUNT": 512}
BEGIN_MARKER = "// BEGIN GENERATED SDL SCANCODE GROUP"
END_MARKER = "// END GENERATED SDL SCANCODE GROUP"
ENUM_PATTERN = re.compile(
    r"\btypedef\s+enum\s+SDL_Scancode\s*\{(.*?)\}\s*SDL_Scancode\s*;",
    re.DOTALL,
)
DECLARATION_PATTERN = re.compile(r"SDL_SCANCODE_([A-Z0-9_]+)\s*=\s*([0-9]+)")
IDENTIFIER_PATTERN = re.compile(r"[A-Z_][A-Z0-9_]*\Z")


@dataclass(frozen=True)
class Scancode:
    name: str
    value: int
    reserved: bool = False


def _without_c_comments(source: str) -> str:
    return re.sub(r"/\*.*?\*/|//[^\r\n]*", "", source, flags=re.DOTALL)


def parse_scancode_header(source: str) -> list[Scancode]:
    """Parse the explicit SDL enum, rejecting expressions and conditional entries."""
    clean_source = _without_c_comments(source)
    matches = list(ENUM_PATTERN.finditer(clean_source))
    if len(matches) != 1:
        raise ValueError(f"expected exactly one SDL_Scancode enum, found {len(matches)}")

    body = matches[0].group(1)
    if "#" in body:
        raise ValueError("preprocessor conditionals inside SDL_Scancode are unsupported")

    declarations: list[tuple[str, int]] = []
    seen_names: set[str] = set()
    for part in body.split(","):
        declaration = part.strip()
        if not declaration:
            continue
        match = DECLARATION_PATTERN.fullmatch(declaration)
        if match is None:
            raise ValueError(f"unrecognized SDL_Scancode declaration: {declaration!r}")
        name, value_text = match.groups()
        if name in seen_names:
            raise ValueError(f"duplicate SDL scancode name: {name}")
        seen_names.add(name)
        declarations.append((name, int(value_text)))

    seen_markers: dict[str, int] = {}
    entries: list[Scancode] = []
    seen_emitted_names: set[str] = set()
    seen_values: dict[int, str] = {}
    marker_by_value = {value: name for name, value in MARKERS.items()}
    for suffix, value in declarations:
        if suffix in MARKERS:
            if suffix in seen_markers:
                raise ValueError(f"duplicate SDL marker: {suffix}")
            seen_markers[suffix] = value
            if value != MARKERS[suffix]:
                raise ValueError(
                    f"SDL_SCANCODE_{suffix} changed from {MARKERS[suffix]} to {value}"
                )
            continue

        if suffix.isdecimal():
            if len(suffix) != 1 or suffix not in "0123456789":
                raise ValueError(f"unsupported numeric SDL scancode suffix: {suffix}")
            name = f"DIGIT_{suffix}"
        else:
            name = suffix
        if not IDENTIFIER_PATTERN.fullmatch(name):
            raise ValueError(f"SDL scancode cannot be represented as a Stasis identifier: {suffix}")
        if name in seen_emitted_names:
            raise ValueError(f"duplicate emitted SDL scancode name: {name}")
        seen_emitted_names.add(name)
        if not 0 <= value < MARKERS["COUNT"]:
            raise ValueError(f"SDL scancode {suffix} is outside HostFrame.keys: {value}")
        previous = seen_values.get(value)
        if previous is not None:
            raise ValueError(
                f"SDL scancode aliases need an explicit policy: {previous} and {suffix} = {value}"
            )
        marker = marker_by_value.get(value)
        if marker is not None:
            raise ValueError(f"SDL scancode {suffix} aliases marker {marker} = {value}")
        seen_values[value] = suffix
        entries.append(Scancode(name, value, suffix in {"LANG6", "LANG7", "LANG8", "LANG9"}))

    if seen_markers != MARKERS:
        missing = sorted(set(MARKERS) - set(seen_markers))
        raise ValueError(f"SDL_Scancode marker set changed; missing {missing}")
    return entries


def render_group(entries: list[Scancode], newline: str = "\n") -> str:
    lines = [BEGIN_MARKER, "const SCANCODE {"]
    for entry in entries:
        if entry.name == "LANG6" and entry.reserved:
            lines.append("    // SDL documents LANG6-LANG9 as reserved named slots.")
        lines.append(f"    {entry.name}: i32 = {entry.value};")
    lines.extend(["}", "", END_MARKER])
    return newline.join(lines)


def replace_generated_group(source: str, group: str) -> str:
    if source.count(BEGIN_MARKER) != 1 or source.count(END_MARKER) != 1:
        raise ValueError("stdlib source must contain one generated-group marker pair")
    start = source.index(BEGIN_MARKER)
    end = source.index(END_MARKER)
    if end < start:
        raise ValueError("generated-group markers are out of order")
    line_ending = "\r\n" if "\r\n" in source else "\n"
    replacement = group.replace("\n", line_ending)
    return source[:start] + replacement + source[end + len(END_MARKER) :]


def verify_cmake_pin(root: Path = ROOT) -> None:
    cmake = (root / "runtime" / "CMakeLists.txt").read_text(encoding="utf-8")
    declaration = re.search(
        r"FetchContent_Declare\s*\(\s*stasis_sdl3\s+URL\s+(\S+)\s+"
        r"URL_HASH\s+SHA256=([0-9a-fA-F]+)",
        cmake,
        flags=re.DOTALL,
    )
    if declaration is None:
        raise ValueError("could not verify the stasis_sdl3 FetchContent pin")
    url, digest = declaration.groups()
    if url != PINNED_ARCHIVE_URL or digest.lower() != PINNED_ARCHIVE_SHA256:
        raise ValueError("runtime/CMakeLists.txt SDL 3.4.10 archive pin changed")


def pinned_entries(header_path: Path, root: Path = ROOT) -> list[Scancode]:
    verify_cmake_pin(root)
    header = header_path.read_bytes()
    digest = hashlib.sha256(header).hexdigest()
    if digest != PINNED_HEADER_SHA256:
        raise ValueError(
            "SDL_scancode.h does not match the reviewed SDL 3.4.10 header hash: "
            f"expected {PINNED_HEADER_SHA256}, got {digest}"
        )
    entries = parse_scancode_header(header.decode("utf-8"))
    if len(entries) != 246:
        raise ValueError(f"expected 246 ordinary SDL scancodes, found {len(entries)}")
    return entries


def run(header_path: Path, output_path: Path, check: bool) -> int:
    entries = pinned_entries(header_path)
    original = output_path.read_text(encoding="utf-8")
    group = render_group(entries)
    updated = replace_generated_group(original, group)
    if check:
        if updated != original:
            print(
                f"{output_path} is stale; run tools/generate_sdl_scancodes.py "
                f"--header {header_path}",
                file=sys.stderr,
            )
            return 1
        print(f"SDL scancode table matches pinned header ({len(entries)} entries)")
        return 0

    with output_path.open("w", encoding="utf-8", newline="") as output:
        output.write(updated)
    print(f"Wrote {len(entries)} SDL scancode entries to {output_path}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--header", required=True, type=Path, help="pinned SDL3 SDL_scancode.h")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--check", action="store_true", help="verify output without modifying it")
    args = parser.parse_args()
    try:
        return run(args.header, args.output, args.check)
    except (OSError, UnicodeError, ValueError) as error:
        print(f"SDL scancode generation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

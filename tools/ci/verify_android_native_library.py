#!/usr/bin/env python3
"""Audit the generated Android game manifest, link map, and final libmain.so."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

SCHEMA = "stasis.seam_test.v1"
REQUIRED_DEFINED = {
    "SDL_main",
    "stasis_aot_bind_runtime_globals",
    "stasis_mobile_main_entry",
    "stasis_mobile_tick_entry",
    "stasis_mobile_render_entry",
    "stasis_mobile_runtime_initialize",
    "stasis_mobile_runtime_step",
}
REQUIRED_NEEDED = {"liblog.so", "libandroid.so"}
FORBIDDEN_SEPARATE_RUNTIME_LIBRARIES = {"libSDL3.so", "libSDL3_image.so"}
FORBIDDEN_LINK_MARKERS = (
    "stasis_dynload",
    "stasis_runner.c",
    "cranelift_jit",
    "inotify_",
    "ReadDirectoryChangesW",
)


class AuditError(RuntimeError):
    pass


def _load_json(path: Path, label: str) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise AuditError(f"{label} could not be read from {path}: {error}") from error
    if not isinstance(value, dict):
        raise AuditError(f"{label} must be a JSON object: {path}")
    return value


def _relative_file(root: Path, value: object, field: str) -> Path:
    if not isinstance(value, str) or not value:
        raise AuditError(f"bundle manifest field '{field}' must be a non-empty path")
    relative = Path(value)
    if relative.is_absolute() or ".." in relative.parts:
        raise AuditError(f"bundle manifest field '{field}' escapes the AOT directory: {value}")
    path = root / relative
    if not path.is_file():
        raise AuditError(f"bundle manifest field '{field}' is missing: {path}")
    return path


def _run_readelf(readelf: Path, library: Path, *options: str) -> str:
    result = subprocess.run(
        [str(readelf), *options, str(library)],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise AuditError(
            f"llvm-readelf {' '.join(options)} failed for {library}: "
            f"{result.stderr.strip() or result.stdout.strip()}"
        )
    return result.stdout


def _symbols(text: str) -> tuple[set[str], set[str]]:
    defined: set[str] = set()
    undefined: set[str] = set()
    for line in text.splitlines():
        fields = line.split()
        if len(fields) < 8 or not fields[0].rstrip(":").isdigit():
            continue
        name = fields[7].split("@", 1)[0]
        if not name:
            continue
        if fields[6] == "UND":
            undefined.add(name)
        else:
            defined.add(name)
    return defined, undefined


def _needed_libraries(text: str) -> set[str]:
    return set(re.findall(r"\(NEEDED\).*?\[([^]]+)\]", text))


def _cmake_objects(text: str) -> set[str]:
    return set(re.findall(r'\$\{CMAKE_CURRENT_LIST_DIR\}/([^"\r\n]+\.o)', text))


def audit(
    library: Path,
    bundle_manifest_path: Path,
    link_map_path: Path,
    readelf: Path,
) -> dict:
    if not library.is_file():
        raise AuditError(f"final Android native library is missing: {library}")
    if not link_map_path.is_file():
        raise AuditError(f"Android libmain link map is missing: {link_map_path}")
    if not readelf.is_file():
        raise AuditError(f"Android llvm-readelf is missing: {readelf}")

    bundle = _load_json(bundle_manifest_path, "mobile AOT bundle manifest")
    if bundle.get("schema") != "stasis.mobile_aot_bundle.v1":
        raise AuditError(f"unexpected mobile AOT bundle schema: {bundle.get('schema')!r}")
    if bundle.get("target") != "android-arm64":
        raise AuditError(f"mobile AOT bundle target must be android-arm64, got {bundle.get('target')!r}")
    aot_root = bundle_manifest_path.parent
    engine_path = _relative_file(aot_root, bundle.get("engine_manifest"), "engine_manifest")
    bindings_path = _relative_file(aot_root, bundle.get("bindings_source"), "bindings_source")
    cmake_path = _relative_file(aot_root, bundle.get("android_cmake_file"), "android_cmake_file")
    engine = _load_json(engine_path, "engine bundle manifest")

    objects = bundle.get("objects")
    if not isinstance(objects, list) or not objects:
        raise AuditError("mobile AOT bundle manifest must list at least one object")
    manifest_objects: set[str] = set()
    manifest_function_ids: set[int] = set()
    for index, entry in enumerate(objects):
        if not isinstance(entry, dict):
            raise AuditError(f"bundle object {index} must be an object")
        path = _relative_file(aot_root, entry.get("path"), f"objects[{index}].path")
        manifest_objects.add(path.name)
        function_id = entry.get("function_id")
        if not isinstance(function_id, int):
            raise AuditError(f"bundle object {index} is missing integer function_id")
        manifest_function_ids.add(function_id)

    cmake_objects = _cmake_objects(cmake_path.read_text(encoding="utf-8"))
    if cmake_objects != manifest_objects:
        raise AuditError(
            "published Android AOT object list differs from bundle manifest: "
            f"missing={sorted(manifest_objects - cmake_objects)} "
            f"extra={sorted(cmake_objects - manifest_objects)}"
        )

    engine_functions = engine.get("functions")
    if not isinstance(engine_functions, list):
        raise AuditError("engine bundle manifest is missing functions")
    engine_symbols: set[str] = set()
    engine_function_ids: set[int] = set()
    for entry in engine_functions:
        if not isinstance(entry, dict):
            continue
        function_id = entry.get("function_id")
        symbol = entry.get("symbol")
        if isinstance(function_id, int) and isinstance(symbol, str):
            engine_function_ids.add(function_id)
            engine_symbols.add(symbol)
    if engine_function_ids != manifest_function_ids:
        raise AuditError(
            "engine functions differ from packaged object identities: "
            f"missing={sorted(manifest_function_ids - engine_function_ids)} "
            f"extra={sorted(engine_function_ids - manifest_function_ids)}"
        )

    header = _run_readelf(readelf, library, "-h")
    if "Class:                             ELF64" not in header:
        raise AuditError("libmain.so must be ELF64")
    if "Type:                              DYN" not in header:
        raise AuditError("libmain.so must be a shared ELF object (DYN)")
    if "Machine:                           AArch64" not in header:
        raise AuditError("libmain.so must target AArch64")

    dynamic = _run_readelf(readelf, library, "-d")
    needed = _needed_libraries(dynamic)
    missing_needed = REQUIRED_NEEDED - needed
    if missing_needed:
        raise AuditError(f"libmain.so is missing native dependencies: {sorted(missing_needed)}")
    separate_runtime_libraries = sorted(FORBIDDEN_SEPARATE_RUNTIME_LIBRARIES & needed)
    if separate_runtime_libraries:
        raise AuditError(
            "libmain.so retained separately packaged SDL dependencies: "
            f"{separate_runtime_libraries}"
        )
    forbidden_dependencies = sorted(
        name for name in needed if any(marker.lower() in name.lower() for marker in FORBIDDEN_LINK_MARKERS)
    )
    if forbidden_dependencies:
        raise AuditError(f"libmain.so retained desktop-only dependencies: {forbidden_dependencies}")

    symbol_text = _run_readelf(readelf, library, "-Ws")
    defined, undefined = _symbols(symbol_text)
    expected_defined = REQUIRED_DEFINED | engine_symbols
    missing_defined = sorted(expected_defined - defined)
    if missing_defined:
        raise AuditError(f"libmain.so is missing generated/mobile symbols: {missing_defined}")
    unresolved_stasis = sorted(
        symbol for symbol in undefined if symbol.startswith("stasis_") or symbol.startswith("aot_fn_")
    )
    if unresolved_stasis:
        raise AuditError(f"libmain.so retains unresolved Stasis symbols: {unresolved_stasis}")

    link_map = link_map_path.read_text(encoding="utf-8", errors="replace")
    missing_link_inputs = sorted(name for name in manifest_objects if name not in link_map)
    if missing_link_inputs:
        raise AuditError(f"link map is missing packaged AOT objects: {missing_link_inputs}")
    bindings_object = f"{bindings_path.name}.o"
    bindings_link_evidence = (
        bindings_object if bindings_object in link_map else "LTO-folded; verified by exported binding symbols"
    )
    forbidden_map_markers = sorted(marker for marker in FORBIDDEN_LINK_MARKERS if marker in link_map)
    if forbidden_map_markers:
        raise AuditError(f"link map retained desktop-only inputs: {forbidden_map_markers}")

    return {
        "schema": SCHEMA,
        "test_id": "IT-016",
        "status": "passed",
        "target": "android-arm64-libmain",
        "library": str(library),
        "elf": {"class": "ELF64", "type": "DYN", "machine": "AArch64"},
        "needed_libraries": sorted(needed),
        "generated_objects": len(manifest_objects),
        "manifest_objects": sorted(manifest_objects),
        "generated_symbols": len(engine_symbols),
        "engine_symbols": sorted(engine_symbols),
        "mobile_symbols": sorted(REQUIRED_DEFINED),
        "bindings_object": bindings_link_evidence,
        "bundle_manifest": str(bundle_manifest_path),
        "link_map": str(link_map_path),
        "unresolved_stasis_symbols": [],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--bundle-manifest", type=Path, required=True)
    parser.add_argument("--link-map", type=Path, required=True)
    parser.add_argument("--readelf", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    try:
        evidence = audit(args.library, args.bundle_manifest, args.link_map, args.readelf)
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        args.evidence.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(json.dumps(evidence, sort_keys=True))
        return 0
    except AuditError as error:
        print(f"Android native library audit failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

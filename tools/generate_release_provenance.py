#!/usr/bin/env python3
"""Generate the content-addressed provenance manifest shipped by Stasis releases."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import subprocess
import tomllib


PINNED_SDL_DEPENDENCIES = {
    "sdl3": "3.4.10-static",
    "sdl3_image": "3.4.4-static",
}


RUNTIME_FILES = (
    "CMakeLists.txt",
    "MINIMP3-LICENSE.txt",
    "minimp3.h",
    "minimp3_ex.h",
    "nanosvg.h",
    "nanosvgrast.h",
    "stasis_display_scale.h",
    "stasis_asset_path.h",
    "stasis_render_contract.h",
    "stasis_renderer_lifecycle.h",
    "stasis_performance_metrics.h",
    "stasis_audio_assets.c",
    "stasis_audio_assets.h",
    "stasis_graphics.c",
    "stasis_runner.manifest",
    "stasis_runner_macos.plist.in",
    "stasis_mobile_aot_runtime.c",
    "stasis_mobile_aot_runtime.h",
    "stasis_mobile_runtime.c",
    "stasis_mobile_runtime.h",
    "stasis_platform_storage.c",
    "stasis_platform_storage.h",
    "stb_truetype.h",
)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive-root", required=True, type=pathlib.Path)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--compiler", required=True, type=pathlib.PurePosixPath)
    parser.add_argument("--development-build", action="store_true")
    parser.add_argument("--dependency", action="append", default=[])
    args = parser.parse_args()

    root = args.archive_root.resolve()
    if args.compiler.is_absolute() or ".." in args.compiler.parts:
        parser.error("compiler must be a confined archive-relative path")
    compiler_relative = pathlib.Path(*args.compiler.parts)
    compiler = root / compiler_relative
    if not compiler.is_file():
        parser.error(f"compiler does not exist: {compiler}")
    if not args.release_tag or not args.source_commit:
        parser.error("release tag and source commit must be non-empty")
    if not args.development_build and not (
        args.release_tag.startswith("v") or args.release_tag.startswith("nightly-")
    ):
        parser.error("official release tags must start with v or nightly-")
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()
    if args.source_commit != head:
        parser.error(f"source commit does not match checkout HEAD: {args.source_commit} != {head}")

    dirty = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=no"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if dirty and not args.development_build:
        parser.error("official release provenance requires a clean tracked worktree")

    runtime_sources = {}
    for name in RUNTIME_FILES:
        path = root / "runtime" / name
        if not path.is_file():
            parser.error(f"release runtime source is missing: {path}")
        runtime_sources[f"runtime/{name}"] = sha256(path)
    mobile_shell_sources = {
        path.relative_to(root).as_posix(): sha256(path)
        for path in sorted((root / "mobile" / "shells").rglob("*"))
        if path.is_file()
    }
    if not mobile_shell_sources:
        parser.error("release mobile shell templates are missing")

    rustc = subprocess.run(
        ["rustc", "--version"], check=True, capture_output=True, text=True
    ).stdout.strip()
    cargo_lock = pathlib.Path("Cargo.lock")
    cargo_packages = sorted(
        f"{package['name']} {package['version']} {package.get('source', 'workspace')}"
        for package in tomllib.loads(cargo_lock.read_text(encoding="utf-8"))["package"]
    )
    dependencies = {
        "rustc": rustc,
        "cargo_lock_sha256": sha256(cargo_lock),
        "cargo_packages": cargo_packages,
    }
    for item in args.dependency:
        name, separator, version = item.partition("=")
        if not separator or not name or not version:
            parser.error(f"dependency must use NAME=VERSION: {item}")
        dependencies[name] = version
    resolved_sdl = {name: dependencies.get(name) for name in PINNED_SDL_DEPENDENCIES}
    if resolved_sdl != PINNED_SDL_DEPENDENCIES:
        parser.error(
            "release provenance requires sdl3=3.4.10-static and "
            "sdl3_image=3.4.4-static"
        )
    manifest = {
        "schema": "stasis.release_provenance.v1",
        "release_tag": args.release_tag,
        "source_commit": args.source_commit,
        "dirty_state": bool(args.development_build),
        "development_build": bool(args.development_build),
        "compiler": {
            "path": args.compiler.as_posix(),
            "sha256": sha256(compiler),
        },
        "runtime_sources": runtime_sources,
        "mobile_shell_sources": mobile_shell_sources,
        "command_buffer": {"name": "gfx_cmd", "version": 4},
        "backends": ["sdl3"],
        "features": ["aot", "jit", "mobile-aot", "shared-renderer"],
        "dependencies": dependencies,
    }
    output = root / "stasis_release_provenance.json"
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

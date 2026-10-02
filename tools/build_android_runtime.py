#!/usr/bin/env python3
"""Build and describe the immutable Android arm64 runtime release kit."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


SCHEMA = "stasis.android_runtime.v1"
NDK_VERSION = "27.0.12077973"
ANDROID_ABI = "arm64-v8a"
ANDROID_API = 26
SDL3_VERSION = "3.4.10"
SDL3_IMAGE_VERSION = "3.4.4"
VARIANTS = ("offline", "host", "client")
ARCHIVES = {
    "sdl3": "libSDL3.a",
    "sdl3_image": "libSDL3_image.a",
    "thorvg": "libstasis_thorvg.a",
}


class BuildError(RuntimeError):
    pass


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def source_tree_hash(root: Path) -> str:
    """Hash a source tree without VCS/build metadata using stable POSIX paths."""
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if any(part in {".git", "build", "build_ci"} for part in relative.parts):
            continue
        if path.is_symlink() or not path.is_file():
            continue
        digest.update(relative.as_posix().encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(sha256(path)))
        digest.update(b"\n")
    return digest.hexdigest()


def run(command: list[str], *, env: dict[str, str] | None = None) -> None:
    print("+ " + " ".join(command), flush=True)
    completed = subprocess.run(command, env=env, check=False)
    if completed.returncode:
        raise BuildError(
            f"command failed with exit code {completed.returncode}: {' '.join(command)}"
        )


def find_archive(root: Path, filename: str) -> Path:
    matches = sorted(path for path in root.rglob(filename) if path.is_file())
    if len(matches) != 1:
        raise BuildError(
            f"expected exactly one {filename} below {root}, found {len(matches)}"
        )
    return matches[0]


def copy_tree(source: Path, destination: Path) -> None:
    if not source.is_dir():
        raise BuildError(f"required source directory is missing: {source}")
    shutil.copytree(source, destination, dirs_exist_ok=True)


def build_runtime(
    args: argparse.Namespace,
    build_root: Path,
    environment: dict[str, str],
) -> None:
    command = [
        args.cmake,
        "-S",
        str(args.runtime_source),
        "-B",
        str(build_root),
        "-G",
        "Ninja",
        f"-DCMAKE_MAKE_PROGRAM={args.ninja}",
        "-DCMAKE_BUILD_TYPE=Release",
        f"-DCMAKE_TOOLCHAIN_FILE={args.ndk_root / 'build/cmake/android.toolchain.cmake'}",
        f"-DANDROID_ABI={ANDROID_ABI}",
        f"-DANDROID_PLATFORM=android-{ANDROID_API}",
        "-DANDROID_STL=c++_static",
        "-DSTASIS_GRAPHICS_BUNDLE_SDL=ON",
        "-DSTASIS_GRAPHICS_BUILD_SHARED=OFF",
        "-DSTASIS_GRAPHICS_BUILD_STATIC=OFF",
        "-DSTASIS_BUILD_MOBILE_RUNTIME=ON",
        "-DSTASIS_BUILD_MOBILE_RUNTIME_VARIANTS=ON",
        f"-DSTASIS_MOBILE_RUNTIME_NETWORK_INCLUDE_DIR={args.network_include.resolve()}",
        "-DSTASIS_BUILD_RUNNER=OFF",
        "-DSTASIS_BUILD_SYS=OFF",
        f"-DSTASIS_RELEASE_ID={args.release_id}",
        f"-DSTASIS_BUILD_FINGERPRINT={args.build_fingerprint}",
    ]
    run(command, env=environment)
    targets = [
        "stasis_mobile_runtime_offline",
        "stasis_mobile_runtime_host",
        "stasis_mobile_runtime_client",
        "SDL3-static",
        "SDL3_image-static",
        "stasis_thorvg",
    ]
    run(
        [args.cmake, "--build", str(build_root), "--config", "Release", "--target", *targets],
        env=environment,
    )


def assemble(args: argparse.Namespace, build_root: Path) -> dict[str, object]:
    output = args.out
    if output.exists():
        raise BuildError(f"output already exists: {output}")
    staging = output.with_name(f".{output.name}.staging")
    if staging.exists():
        shutil.rmtree(staging)
    (staging / "lib").mkdir(parents=True)
    (staging / "include" / "stasis").mkdir(parents=True)
    (staging / "licenses").mkdir(parents=True)

    for key in ("sdl3", "sdl3_image", "thorvg"):
        shutil.copy2(find_archive(build_root, ARCHIVES[key]), staging / "lib" / ARCHIVES[key])
    runtime_variants: dict[str, str] = {}
    for variant in VARIANTS:
        filename = f"libstasis_mobile_runtime_{variant}.a"
        shutil.copy2(find_archive(build_root, filename), staging / "lib" / filename)
        runtime_variants[variant] = f"lib/{filename}"

    copy_tree(args.sdl3_source / "include" / "SDL3", staging / "include" / "SDL3")
    copy_tree(
        args.sdl3_image_source / "include" / "SDL3_image",
        staging / "include" / "SDL3_image",
    )
    copy_tree(
        args.sdl3_source / "android-project" / "app" / "src" / "main" / "java",
        staging / "java",
    )
    for header in sorted(args.runtime_source.glob("*.h")):
        shutil.copy2(header, staging / "include" / "stasis" / header.name)
    for source, name in (
        (args.sdl3_source / "LICENSE.txt", "SDL3-LICENSE.txt"),
        (args.sdl3_image_source / "LICENSE.txt", "SDL3_image-LICENSE.txt"),
    ):
        if not source.is_file():
            raise BuildError(f"required license is missing: {source}")
        shutil.copy2(source, staging / "licenses" / name)

    files = {
        path.relative_to(staging).as_posix(): sha256(path)
        for path in sorted(staging.rglob("*"))
        if path.is_file()
    }
    manifest: dict[str, object] = {
        "schema": SCHEMA,
        "release_id": args.release_id,
        "source_commit": args.source_commit,
        "build_fingerprint": args.build_fingerprint,
        "target": "android-arm64",
        "abi": ANDROID_ABI,
        "android_api": ANDROID_API,
        "ndk_version": NDK_VERSION,
        "cmake_version": args.cmake_version,
        "generator": "Ninja",
        "cxx_runtime": "c++_static",
        "runtime_abi_version": 2,
        "graphics_abi_version": 4,
        "dependencies": {
            "sdl3": SDL3_VERSION,
            "sdl3_image": SDL3_IMAGE_VERSION,
        },
        "source_hashes": {
            "stasis_runtime": source_tree_hash(args.runtime_source),
            "sdl3": source_tree_hash(args.sdl3_source),
            "sdl3_image": source_tree_hash(args.sdl3_image_source),
        },
        "compile_contract": {
            "build_type": "Release",
            "position_independent_code": True,
            "disabled_sdl_subsystems": [
                "camera", "dialog", "gpu", "haptic", "hidapi", "joystick",
                "power", "sensor", "tray", "vulkan",
            ],
            "sdl_image_decoders": ["png-stb"],
            "runtime_variants": list(VARIANTS),
            "system_libraries": [
                "m", "dl", "OpenSLES", "log", "android", "GLESv1_CM", "GLESv2",
            ],
        },
        "libraries": {
            "sdl3": "lib/libSDL3.a",
            "sdl3_image": "lib/libSDL3_image.a",
            "thorvg": "lib/libstasis_thorvg.a",
        },
        "runtime_variants": runtime_variants,
        "files": files,
    }
    (staging / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    staging.rename(output)
    return manifest


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime-source", type=Path, required=True)
    parser.add_argument("--sdl3-source", type=Path, required=True)
    parser.add_argument("--sdl3-image-source", type=Path, required=True)
    parser.add_argument("--network-include", type=Path, required=True)
    parser.add_argument("--ndk-root", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--build-fingerprint", required=True)
    parser.add_argument("--cmake", default="cmake")
    parser.add_argument("--ninja", default="ninja")
    parser.add_argument("--cmake-version", default="3.22.1")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    ndk_properties = args.ndk_root / "source.properties"
    if not ndk_properties.is_file() or f"Pkg.Revision = {NDK_VERSION}" not in ndk_properties.read_text(
        encoding="utf-8"
    ):
        raise BuildError(f"Android runtime requires NDK {NDK_VERSION}: {args.ndk_root}")
    if len(args.source_commit) != 40 or any(ch not in "0123456789abcdefABCDEF" for ch in args.source_commit):
        raise BuildError("source commit must be a 40-character hexadecimal Git commit")
    environment = os.environ.copy()
    environment["STASIS_SDL3_SOURCE"] = str(args.sdl3_source.resolve())
    environment["STASIS_SDL3_IMAGE_SOURCE"] = str(args.sdl3_image_source.resolve())
    with tempfile.TemporaryDirectory(prefix="stasis-android-runtime-") as directory:
        build_root = Path(directory)
        build_runtime(args, build_root, environment)
        manifest = assemble(args, build_root)
    print(json.dumps({"output": str(args.out), "files": len(manifest["files"])}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parents[2]
PINNED = {
    "runtime/CMakeLists.txt": (
        "SDL3-3.4.10.tar.gz",
        "SHA256=12b34280415ec8418c864408b93d008a20a6530687ee613d60bfbd20411f2785",
        "SDL3_image-3.4.4.tar.gz",
        "SHA256=29751304a13d25ac513f24305fa25b06a6edd9607718c90129b8350d35fc5573",
        "set(CMAKE_POSITION_INDEPENDENT_CODE ON)",
    ),
    "runtime/stasis_graphics.c": (
        "#include <SDL3/SDL.h>",
        "#include <SDL3_image/SDL_image.h>",
        "SDL_OpenAudioDeviceStream(",
        "SDL_SetRenderLogicalPresentation(",
        "SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED",
    ),
    "mobile/shells/android/app/src/main/cpp/CMakeLists.txt": (
        "SDL3::SDL3",
        "SDL3_image::SDL3_image",
    ),
    "mobile/shells/android/app/src/main/java/com/stasislang/game/MainActivity.java": (
        'System.loadLibrary("SDL3")',
        'System.loadLibrary("SDL3_image")',
    ),
    "mobile/shells/ios/StasisMobile/main.m": (
        "#include <SDL3/SDL_main.h>",
    ),
    "tools/ci/check_android_release_package.py": (
        '"libSDL3.so"',
        '"libSDL3_image.so"',
    ),
    ".github/workflows/pr-ci.yml": (
        "STASIS_GRAPHICS_BUNDLE_SDL=ON",
        "libx11-dev",
        "libxrandr-dev",
    ),
    "scripts/build_local_editor_release.ps1": (
        "STASIS_GRAPHICS_BUNDLE_SDL=ON",
        "STASIS_GRAPHICS_SDL_ONLY=ON",
    ),
    "docs/sdl3_migration.md": (
        "There is no SDL2 or `sdl2-compat` fallback.",
        "Windows x64",
        "Android arm64",
        "iOS arm64/simulator",
    ),
}

NO_SDL2_PATHS = (
    "runtime/CMakeLists.txt",
    "runtime/stasis_graphics.c",
    "runtime/build.bat",
    "runtime/build_android.ps1",
    "mobile/shells",
    "mobile/android/build_release.ps1",
    "apps/stasis/src/toolchain_cli.rs",
    "apps/stasis/tests/toolchain_cli.rs",
    "tools/generate_release_provenance.py",
    "tools/ci/check_android_release_package.py",
    "tools/ci/check_android_shell.py",
    ".github/workflows",
    "scripts/build_local_editor_release.ps1",
    "docs/mobile_packaging.md",
    "docs/mobile_runtime_core.md",
    "docs/release_provenance.md",
)
SDL2_TOKEN = re.compile(r"(?i)\bsdl2(?:_image|-image)?\b|sdl2-compat")
SDL3_BOOL_AS_INT = re.compile(r"SDL_UpdateTexture\([^;]+?\)\s*!=\s*0", re.DOTALL)


def iter_files(root: pathlib.Path, item: str):
    path = root / item
    if path.is_file():
        yield path
    elif path.is_dir():
        yield from (entry for entry in path.rglob("*") if entry.is_file())


def validate(root: pathlib.Path = ROOT) -> list[str]:
    errors: list[str] = []
    for relative, required in PINNED.items():
        path = root / relative
        if not path.is_file():
            errors.append(f"missing SDL3 contract file: {relative}")
            continue
        text = path.read_text(encoding="utf-8")
        for marker in required:
            if marker not in text:
                errors.append(f"{relative}: missing {marker!r}")

    for item in NO_SDL2_PATHS:
        for path in iter_files(root, item):
            if path.suffix.lower() in {".png", ".jpg", ".jpeg", ".zip", ".dll", ".so"}:
                continue
            text = path.read_text(encoding="utf-8", errors="replace")
            match = SDL2_TOKEN.search(text)
            if match:
                relative = path.relative_to(root).as_posix()
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{relative}:{line}: obsolete {match.group(0)!r}")

    runtime = root / "runtime/stasis_graphics.c"
    if runtime.is_file():
        text = runtime.read_text(encoding="utf-8")
        match = SDL3_BOOL_AS_INT.search(text)
        if match:
            line = text.count("\n", 0, match.start()) + 1
            errors.append(
                "runtime/stasis_graphics.c:"
                f"{line}: SDL3 boolean return compared with SDL2 integer convention"
            )
    ios_main = root / "mobile/shells/ios/StasisMobile/main.m"
    if ios_main.is_file():
        ios_main_text = ios_main.read_text(encoding="utf-8")
        if "SDL_UIKitRunApp" in ios_main_text:
            errors.append("mobile/shells/ios/StasisMobile/main.m: obsolete SDL2 startup wrapper")
        if ios_main_text.strip() != "#include <SDL3/SDL_main.h>":
            errors.append(
                "mobile/shells/ios/StasisMobile/main.m: SDL3 must own the iOS main wrapper"
            )
    return errors


def main() -> int:
    errors = validate()
    if errors:
        print("SDL3 migration contract failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("SDL3 migration contract passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

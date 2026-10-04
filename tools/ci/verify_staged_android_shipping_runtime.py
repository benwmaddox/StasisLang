#!/usr/bin/env python3
"""Verify that an official Android arm64 package embeds the accepted archive kit."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
from typing import Any


KIT_RELATIVE = PurePosixPath("mobile/android-runtime/arm64-v8a")
PACKAGE_KIT_RELATIVE = PurePosixPath("android/runtime")
KIT_SCHEMA = "stasis.android_runtime.v1"
RELEASE_SCHEMA = "stasis.release_provenance.v1"
EXPECTED_SOURCE_HASHES = {"stasis_runtime", "sdl3", "sdl3_image"}
EXPECTED_VARIANTS = {
    "offline": "lib/libstasis_mobile_runtime_offline.a",
    "host": "lib/libstasis_mobile_runtime_host.a",
    "client": "lib/libstasis_mobile_runtime_client.a",
}
EXPECTED_IDENTITY_FIELDS = (
    "schema",
    "release_id",
    "source_commit",
    "build_fingerprint",
    "abi",
    "android_api",
    "ndk_version",
    "cmake_version",
    "generator",
    "cxx_runtime",
    "runtime_abi_version",
    "graphics_abi_version",
)
EXPECTED_SYSTEM_LIBRARIES = [
    "m",
    "dl",
    "OpenSLES",
    "log",
    "android",
    "GLESv1_CM",
    "GLESv2",
]
SHA256_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")


class VerificationError(ValueError):
    """An Android shipping package is not bound to the accepted archive kit."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise VerificationError(message)


def read_json(path: Path, label: str) -> dict[str, Any]:
    require(path.is_file() and not path.is_symlink(), f"{label} is missing: {path}")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise VerificationError(f"{label} is invalid: {path}: {error}") from error
    require(isinstance(value, dict), f"{label} is not an object: {path}")
    return value


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def file_hashes(root: Path) -> dict[str, str]:
    require(root.is_dir() and not root.is_symlink(), f"Android runtime kit directory is missing: {root}")
    hashes: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        require(not path.is_symlink(), f"Android runtime kit contains a symbolic link: {path}")
        if path.is_file():
            hashes[path.relative_to(root).as_posix()] = sha256(path)
    return hashes


def validate_runtime_manifest(
    manifest: dict[str, Any],
    *,
    expected_release: str,
    expected_source: str,
    expected_fingerprint: str,
) -> tuple[dict[str, Any], dict[str, str]]:
    expected_fields: dict[str, Any] = {
        "schema": KIT_SCHEMA,
        "release_id": expected_release,
        "source_commit": expected_source,
        "build_fingerprint": expected_fingerprint,
        "target": "android-arm64",
        "abi": "arm64-v8a",
        "android_api": 26,
        "ndk_version": "27.0.12077973",
        "cmake_version": "3.22.1",
        "generator": "Ninja",
        "cxx_runtime": "c++_static",
        "runtime_abi_version": 2,
        "graphics_abi_version": 4,
        "dependencies": {"sdl3": "3.4.10", "sdl3_image": "3.4.4"},
    }
    for field, expected in expected_fields.items():
        require(manifest.get(field) == expected, f"Android runtime kit {field} differs from the accepted archive")

    sources = manifest.get("source_hashes")
    require(
        isinstance(sources, dict) and set(sources) == EXPECTED_SOURCE_HASHES,
        "Android runtime kit source hash inventory is missing or unexpected",
    )
    for name, digest in sources.items():
        require(isinstance(digest, str) and SHA256_RE.fullmatch(digest) is not None,
                f"Android runtime kit source hash is malformed: {name}")

    compile_contract = manifest.get("compile_contract")
    require(isinstance(compile_contract, dict), "Android runtime kit compile contract is missing")
    require(
        compile_contract.get("build_type") == "Release"
        and compile_contract.get("position_independent_code") is True
        and compile_contract.get("runtime_variants") == ["offline", "host", "client"]
        and compile_contract.get("system_libraries") == EXPECTED_SYSTEM_LIBRARIES,
        "Android runtime kit compile contract differs from the supported archive contract",
    )
    runtime_variants = manifest.get("runtime_variants")
    require(runtime_variants == EXPECTED_VARIANTS,
            "Android runtime kit variant inventory differs from the supported archive contract")

    files = manifest.get("files")
    require(isinstance(files, dict) and len(files) >= 8,
            "Android runtime kit file hash inventory is missing or incomplete")
    for relative, digest in files.items():
        require(isinstance(relative, str), "Android runtime kit file path is malformed")
        path = PurePosixPath(relative)
        require(
            not path.is_absolute() and bool(path.parts) and ".." not in path.parts
            and "\\" not in relative and relative != "manifest.json",
            f"Android runtime kit file path is unsafe: {relative!r}",
        )
        require(isinstance(digest, str) and SHA256_RE.fullmatch(digest) is not None,
                f"Android runtime kit file hash is malformed: {relative}")
    return sources, files


def verify_shipping_runtime(
    archive_root: Path,
    package_root: Path,
    *,
    expected_release: str,
    expected_source: str,
    expected_fingerprint: str,
) -> dict[str, Any]:
    require(COMMIT_RE.fullmatch(expected_source) is not None,
            "accepted source commit is malformed")
    require(SHA256_RE.fullmatch(expected_fingerprint) is not None,
            "accepted runtime fingerprint is malformed")

    release_provenance = read_json(
        archive_root / "stasis_release_provenance.json", "accepted release provenance"
    )
    package_provenance = read_json(package_root / "stasis_provenance.json", "shipping package provenance")
    require(release_provenance.get("schema") == RELEASE_SCHEMA,
            "accepted release provenance schema is unsupported")
    for field, expected in (
        ("release_tag", expected_release),
        ("source_commit", expected_source),
        ("dirty_state", False),
        ("development_build", False),
    ):
        require(release_provenance.get(field) == expected,
                f"accepted release provenance {field} differs from the staged identity")
        require(package_provenance.get(field) == expected,
                f"shipping package provenance {field} differs from the staged identity")
    require(package_provenance.get("schema") == RELEASE_SCHEMA,
            "shipping package provenance schema is unsupported")

    archive_kit_root = archive_root.joinpath(*KIT_RELATIVE.parts)
    archive_manifest_path = archive_kit_root / "manifest.json"
    archive_manifest = read_json(archive_manifest_path, "accepted Android runtime kit manifest")
    sources, declared_files = validate_runtime_manifest(
        archive_manifest,
        expected_release=expected_release,
        expected_source=expected_source,
        expected_fingerprint=expected_fingerprint,
    )

    archive_files = file_hashes(archive_kit_root)
    require("manifest.json" in archive_files,
            "accepted Android runtime kit manifest is absent from its file inventory")
    archive_runtime_files = {name: digest for name, digest in archive_files.items() if name != "manifest.json"}
    require(archive_runtime_files == declared_files,
            "accepted Android runtime kit files differ from its declared file hashes")

    prefix = KIT_RELATIVE.as_posix() + "/"
    provenance_artifacts = release_provenance.get("android_runtime_artifacts")
    require(isinstance(provenance_artifacts, dict),
            "accepted release provenance omits Android runtime artifact hashes")
    require(
        all(isinstance(name, str) and name.startswith(prefix) for name in provenance_artifacts),
        "accepted release provenance contains unrelated Android runtime artifact paths",
    )
    declared_archive_artifacts = {
        name[len(prefix):]: digest
        for name, digest in provenance_artifacts.items()
    }
    require(
        declared_archive_artifacts == archive_files
        and all(isinstance(digest, str) and SHA256_RE.fullmatch(digest) is not None
                for digest in declared_archive_artifacts.values()),
        "accepted Android runtime kit files differ from release provenance hashes",
    )
    require(package_provenance.get("android_runtime_artifacts") == provenance_artifacts,
            "shipping package Android runtime artifact provenance differs from the accepted archive")

    mobile_manifest = read_json(package_root / "stasis_mobile_package.json", "shipping mobile package manifest")
    require(mobile_manifest.get("target") == "android-arm64",
            "shipping package target is not android-arm64")
    require(mobile_manifest.get("development_build") is False,
            "shipping package is marked as a development build")
    runtime_identity = mobile_manifest.get("android_runtime")
    require(isinstance(runtime_identity, dict), "shipping package Android runtime identity is missing")
    require(runtime_identity.get("mode") == "prebuilt",
            "official Android arm64 package did not select the archive prebuilt runtime")
    variant = runtime_identity.get("variant")
    require(variant in EXPECTED_VARIANTS,
            "shipping Android package selected an unsupported runtime variant")
    require(runtime_identity.get("manifest") == "android/runtime/manifest.json",
            "shipping package points to an unexpected Android runtime manifest")
    expected_identity = {field: archive_manifest.get(field) for field in EXPECTED_IDENTITY_FIELDS}
    expected_identity.update(
        {
            "variant": variant,
            "runtime_library": EXPECTED_VARIANTS[variant],
            "source_hashes": sources,
            "compile_contract": archive_manifest["compile_contract"],
        }
    )
    require(runtime_identity.get("identity") == expected_identity,
            "shipping Android runtime identity differs from the accepted archive kit")

    packaged_kit_root = package_root.joinpath(*PACKAGE_KIT_RELATIVE.parts)
    package_manifest = read_json(
        packaged_kit_root / "manifest.json", "packaged Android runtime kit manifest"
    )
    package_files = file_hashes(packaged_kit_root)
    require(package_files == archive_files,
            "shipping Android runtime kit is missing, stale, tampered, or unrelated to the accepted archive")
    require(package_manifest == archive_manifest,
            "packaged Android runtime manifest differs from the accepted archive")

    return {
        "result": "passed",
        "target": "android-arm64",
        "mode": "prebuilt",
        "variant": variant,
        "release_id": expected_release,
        "source_commit": expected_source,
        "abi": "arm64-v8a",
        "android_api": 26,
        "ndk_version": "27.0.12077973",
        "build_fingerprint": expected_fingerprint,
        "runtime_source_hashes": sources,
        "runtime_file_count": len(archive_files),
        "manifest_sha256": archive_files["manifest.json"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-root", type=Path, required=True)
    parser.add_argument("--package-root", type=Path, required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--build-fingerprint", required=True)
    args = parser.parse_args()
    try:
        result = verify_shipping_runtime(
            args.archive_root,
            args.package_root,
            expected_release=args.release_id,
            expected_source=args.source_commit,
            expected_fingerprint=args.build_fingerprint,
        )
    except VerificationError as error:
        parser.error(str(error))
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

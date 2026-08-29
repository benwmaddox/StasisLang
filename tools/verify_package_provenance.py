#!/usr/bin/env python3
"""Verify that a generated package contains the release's exact runtime sources."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib


COMMAND_BUFFER_NAME = "gfx_cmd"
CURRENT_COMMAND_BUFFER_VERSION = 6
LEGACY_COMMAND_BUFFER_VERSION = 4
ASSET_PACKAGE_IDENTITY_NAME = "stasis_asset_package.json"
ASSET_MANIFEST_RELATIVE_PATH = pathlib.PurePosixPath("assets/manifest.json")
ASSET_PACKAGE_IDENTITY_SCHEMA = "stasis.asset_package"
ASSET_PACKAGE_IDENTITY_VERSION = 1


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def mobile_package_id(name: str) -> str:
    component = "game"
    for byte in name.encode("utf-8"):
        if chr(byte).isascii() and chr(byte).isalnum():
            component += chr(byte).lower()
        else:
            component += f"x{byte:02x}"
    return f"com.stasislang.{component}"


def verify_asset_package_identities(
    parser: argparse.ArgumentParser, package_root: pathlib.Path
) -> None:
    packaged_manifests = sorted(package_root.rglob("assets/manifest.json"))
    for manifest_path in packaged_manifests:
        identity_path = manifest_path.parent.parent / ASSET_PACKAGE_IDENTITY_NAME
        if not identity_path.is_file():
            parser.error(
                f"asset package identity is missing for packaged manifest: {manifest_path}"
            )

    for identity_path in sorted(package_root.rglob(ASSET_PACKAGE_IDENTITY_NAME)):
        identity = json.loads(identity_path.read_text(encoding="utf-8"))
        if set(identity) != {"schema", "version", "manifest_path", "manifest_sha256"}:
            parser.error(f"malformed asset package identity: {identity_path}")
        if identity["schema"] != ASSET_PACKAGE_IDENTITY_SCHEMA:
            parser.error(f"unsupported asset package identity schema: {identity_path}")
        if identity["version"] != ASSET_PACKAGE_IDENTITY_VERSION:
            parser.error(f"unsupported asset package identity version: {identity_path}")
        relative = pathlib.PurePosixPath(identity["manifest_path"])
        if relative.is_absolute() or ".." in relative.parts:
            parser.error(f"unsafe asset manifest identity path: {identity_path}")
        if relative != ASSET_MANIFEST_RELATIVE_PATH:
            parser.error(f"unsupported asset manifest identity path: {identity_path}")
        manifest_path = identity_path.parent / pathlib.Path(*relative.parts)
        if not manifest_path.is_file():
            parser.error(f"asset package identity manifest is missing: {manifest_path}")
        actual = sha256(manifest_path)
        if actual != identity["manifest_sha256"]:
            parser.error(
                f"asset package manifest hash mismatch: expected "
                f"{identity['manifest_sha256']}, found {actual}"
            )


def validate_command_buffer(
    parser: argparse.ArgumentParser, manifest: dict, *, allow_legacy: bool
) -> None:
    command_buffer = manifest.get("command_buffer")
    if not isinstance(command_buffer, dict):
        parser.error("provenance is missing command_buffer contract")
    if command_buffer.get("name") != COMMAND_BUFFER_NAME:
        parser.error(
            "provenance command_buffer family must be gfx_cmd: "
            f"{command_buffer.get('name')!r}"
        )
    version = command_buffer.get("version")
    if type(version) is int and version == CURRENT_COMMAND_BUFFER_VERSION:
        return
    if (
        allow_legacy
        and type(version) is int
        and version == LEGACY_COMMAND_BUFFER_VERSION
        and manifest.get("development_build") is False
        and manifest.get("dirty_state") is False
        and isinstance(manifest.get("release_tag"), str)
        and (
            manifest["release_tag"].startswith("v")
            or manifest["release_tag"].startswith("nightly-")
        )
    ):
        return
    parser.error(
        "unsupported gfx_cmd command_buffer schema: "
        f"expected current {CURRENT_COMMAND_BUFFER_VERSION}"
        + (f" or official legacy {LEGACY_COMMAND_BUFFER_VERSION}" if allow_legacy else "")
        + f", found {version!r}"
    )


def verify_mobile_shells(
    parser: argparse.ArgumentParser,
    release_root: pathlib.Path,
    package_root: pathlib.Path,
    manifest: dict,
) -> None:
    shell_root = release_root / "mobile" / "shells"
    actual_shell_hashes = {
        path.relative_to(release_root).as_posix(): sha256(path)
        for path in sorted(shell_root.rglob("*"))
        if path.is_file()
    }
    if actual_shell_hashes != manifest.get("mobile_shell_sources"):
        parser.error("release mobile shell tree does not match its provenance hashes")
    receipt = json.loads(
        (package_root / "stasis_mobile_package.json").read_text(encoding="utf-8")
    )
    target = receipt.get("target")
    if target not in ("android-arm64", "ios-arm64"):
        parser.error(f"unsupported mobile package target: {target!r}")
    platform = target.split("-", 1)[0]
    package_id = receipt.get("package_id") or mobile_package_id(receipt["name"])
    network_enabled = receipt.get("network") is True
    replacements = {
        "@STASIS_APP_NAME@": receipt.get("app_name") or receipt["name"],
        "@STASIS_PACKAGE_ID@": package_id,
        "@STASIS_JNI_PACKAGE@": package_id.replace(".", "_"),
        "@STASIS_ASSET_BASE@": ".",
        "@STASIS_ANDROID_ORIENTATION@": receipt.get("android_orientation")
        or "sensorLandscape",
        "@STASIS_ANDROID_VERSION_CODE@": str(
            receipt.get("android_version_code") or "1"
        ),
        "@STASIS_ANDROID_VERSION_NAME@": receipt.get("android_version_name") or "1.0",
        "@STASIS_ANDROID_ABI@": "arm64-v8a" if target == "android-arm64" else "",
        "@STASIS_NETWORK_ENABLED@": "1" if network_enabled else "0",
        "@STASIS_NETWORK_PERMISSION@": (
            '    <uses-permission android:name="android.permission.INTERNET" />\n'
            if network_enabled and platform == "android"
            else ""
        ),
        "@STASIS_LOCAL_NETWORK_USAGE@": (
            f"    <key>NSLocalNetworkUsageDescription</key><string>{receipt.get('app_name') or receipt['name']} uses your local network so nearby friends can join games hosted on this device.</string>\n"
            if network_enabled and target == "ios-arm64"
            else ""
        ),
    }
    expected_paths = set()
    for source_group in ("common", platform):
        source_root = release_root / "mobile" / "shells" / source_group
        for source in sorted(source_root.rglob("*")):
            if not source.is_file():
                continue
            relative = source.relative_to(source_root)
            expected_paths.add((source_group, relative.as_posix()))
            expected = source.read_bytes()
            try:
                text = expected.decode("utf-8")
            except UnicodeDecodeError:
                pass
            else:
                for token, value in replacements.items():
                    text = text.replace(token, value)
                expected = text.encode("utf-8")
            destination = package_root / source_group / relative
            if not destination.is_file() or destination.read_bytes() != expected:
                parser.error(f"packaged mobile shell does not match release transform: {destination}")

    expected_paths.add(("common", "stasis_package_provenance.h"))
    if target == "ios-arm64":
        expected_paths.add(("ios", "StasisMobile.xcconfig"))
    if network_enabled:
        if target == "ios-arm64":
            expected_paths.update(
                {
                    ("ios", "network/libstasis_network.a"),
                    ("ios", "network/include/stasis_network.h"),
                }
            )
        else:
            expected_paths.update(
                {
                    ("android", "app/src/main/cpp/network/libstasis_network.a"),
                    ("android", "app/src/main/cpp/network/include/stasis_network.h"),
                }
            )
    asset_prefix = (
        "app/src/main/assets/stasis_game/"
        if target == "android-arm64"
        else "StasisMobile/stasis_game/"
    )
    actual_paths = {
        (source_group, path.relative_to(package_root / source_group).as_posix())
        for source_group in ("common", platform)
        for path in sorted((package_root / source_group).rglob("*"))
        if path.is_file()
        and not (
            source_group == platform
            and path.relative_to(package_root / source_group)
            .as_posix()
            .startswith(asset_prefix)
        )
    }
    unexpected = sorted(actual_paths - expected_paths)
    missing = sorted(expected_paths - actual_paths)
    if unexpected or missing:
        parser.error(
            f"packaged mobile source tree differs from release transform: "
            f"unexpected={unexpected}, missing={missing}"
        )

    tag = manifest.get("release_tag") or "development"
    commit = manifest.get("source_commit") or "unknown"
    label = (
        "non-release development build"
        if manifest.get("development_build")
        else "official release"
    )
    header = (
        "#ifndef STASIS_PACKAGE_PROVENANCE_H\n"
        "#define STASIS_PACKAGE_PROVENANCE_H\n"
        f"#define STASIS_PACKAGE_RELEASE_TAG {json.dumps(tag)}\n"
        f"#define STASIS_PACKAGE_SOURCE_COMMIT {json.dumps(commit)}\n"
        f"#define STASIS_PACKAGE_BUILD_LABEL {json.dumps(label)}\n"
        "#endif\n"
    ).encode("utf-8")
    generated = package_root / "common" / "stasis_package_provenance.h"
    if not generated.is_file() or generated.read_bytes() != header:
        parser.error("generated mobile provenance header does not match the release manifest")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--release-root", required=True, type=pathlib.Path)
    parser.add_argument("--package-root", required=True, type=pathlib.Path)
    parser.add_argument("--expect-runtime-sources", action="store_true")
    args = parser.parse_args()

    release = json.loads(
        (args.release_root / "stasis_release_provenance.json").read_text(encoding="utf-8")
    )
    packaged = json.loads(
        (args.package_root / "stasis_provenance.json").read_text(encoding="utf-8")
    )
    validate_command_buffer(parser, release, allow_legacy=True)
    validate_command_buffer(parser, packaged, allow_legacy=True)
    if release != packaged:
        parser.error("packaged provenance does not exactly match the release manifest")
    verify_asset_package_identities(parser, args.package_root)
    runtime_sources = release["runtime_sources"] if args.expect_runtime_sources else {}
    for relative, expected in runtime_sources.items():
        relative_path = pathlib.PurePosixPath(relative)
        if relative_path.is_absolute() or ".." in relative_path.parts:
            parser.error(f"unsafe runtime provenance path: {relative}")
        packaged_path = args.package_root / pathlib.Path(*relative_path.parts)
        actual = sha256(packaged_path)
        if actual != expected:
            parser.error(
                f"packaged runtime hash mismatch for {relative}: expected {expected}, found {actual}"
            )
    if args.expect_runtime_sources:
        verify_mobile_shells(parser, args.release_root, args.package_root, release)
    print(f"verified {args.package_root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

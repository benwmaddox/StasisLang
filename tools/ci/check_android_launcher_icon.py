#!/usr/bin/env python3
"""Verify that a compiled Android package resolves the project launcher icon."""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

DENSITIES = ("mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi")
AAB_DENSITIES = (160, 240, 320, 480, 640, 65534)
ICON_NAME = "mipmap/ic_launcher"


def _tool(name: str, explicit: str = "") -> str:
    if explicit:
        path = Path(explicit).expanduser().resolve()
        if not path.is_file():
            raise ValueError(f"Android {name} tool was not found: {path}")
        return str(path)
    found = shutil.which(name) or shutil.which(f"{name}.exe")
    if found:
        return found
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if sdk:
        candidates = sorted(
            (Path(sdk) / "build-tools").glob(f"*/{name}.exe"), reverse=True
        )
        if candidates:
            return str(candidates[0])
    raise ValueError(f"Android {name} tool is required for compiled launcher verification")


def _run(command: list[str]) -> str:
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, check=False, timeout=90
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"compiled launcher inspection could not run {Path(command[0]).name}: {error}") from error
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()[-1000:]
        raise ValueError(f"compiled launcher inspection failed ({Path(command[0]).name}): {detail}")
    return result.stdout


def _verify_apk(package: Path, aapt: str) -> dict[str, object]:
    manifest = _run([aapt, "dump", "xmltree", str(package), "AndroidManifest.xml"])
    ids = {}
    for name in ("icon", "roundIcon"):
        match = re.search(rf"A: android:{name}\([^)]*\)=@(0x[0-9a-fA-F]+)", manifest)
        if not match:
            raise ValueError(f"compiled Android application is missing android:{name}")
        ids[name] = match.group(1).lower()
    if ids["icon"] != ids["roundIcon"]:
        raise ValueError("compiled android:icon and android:roundIcon resolve to different resources")

    resources = _run([aapt, "dump", "resources", str(package)])
    pattern = rf"spec resource {re.escape(ids['icon'])} [^\n]*:{ICON_NAME}:"
    match = re.search(pattern, resources, re.I)
    if not match:
        raise ValueError("compiled launcher icon does not resolve to mipmap/ic_launcher")
    block = resources[match.end():]
    next_spec = re.search(r"\n\s*spec resource ", block)
    if next_spec:
        block = block[:next_spec.start()]
    missing = [density for density in (*DENSITIES, "anydpi") if not re.search(
        rf"\bconfig {density}:\s*\n\s*resource {re.escape(ids['icon'])}\b", block, re.I
    )]
    if missing:
        raise ValueError(f"compiled launcher icon is missing density/adaptive variants: {missing}")

    badging = _run([aapt, "dump", "badging", str(package)])
    match = re.search(r"^application-icon-65534:'([^']+)'", badging, re.M)
    if not match:
        raise ValueError("compiled launcher icon has no adaptive anydpi resource")
    adaptive = _run([aapt, "dump", "xmltree", str(package), match.group(1)])
    if not re.search(r"E: adaptive-icon\b", adaptive):
        raise ValueError("compiled anydpi launcher resource is not an adaptive icon")
    return {"format": "apk", "icon_resource": f"@{ICON_NAME}", "density_count": 6}


def _verify_aab(package: Path, bundletool: str, java: str) -> dict[str, object]:
    if not bundletool:
        bundletool = os.environ.get("BUNDLETOOL_PATH", "")
    jar = Path(bundletool).expanduser().resolve() if bundletool else None
    if jar is None or not jar.is_file():
        raise ValueError("AAB launcher verification requires the official bundletool-all JAR via --bundletool or BUNDLETOOL_PATH")
    java_tool = _tool("java", java)
    prefix = [java_tool, "-jar", str(jar)]
    manifest = _run([*prefix, "dump", "manifest", f"--bundle={package}"])
    application = re.search(r"<application\b[^>]*>", manifest, re.S)
    if not application:
        raise ValueError("compiled AAB manifest has no application element")
    for name in ("icon", "roundIcon"):
        if not re.search(rf'android:{name}="@{ICON_NAME}"', application.group(0)):
            raise ValueError(f"compiled AAB application is missing android:{name}=\"@{ICON_NAME}\"")
    resources = _run([*prefix, "dump", "resources", f"--bundle={package}", f"--resource={ICON_NAME}", "--values"])
    if not re.search(r"\bmipmap/ic_launcher\b", resources):
        raise ValueError("compiled AAB launcher resource table lacks mipmap/ic_launcher")
    values: dict[int, str] = {}
    for density, path in re.findall(r"density:\s*(\d+)\s*-\s*\[FILE\]\s+(\S+)", resources):
        values[int(density)] = path
    missing = [density for density in AAB_DENSITIES if density not in values]
    if missing:
        raise ValueError(f"compiled AAB launcher icon is missing densities: {missing}")
    with zipfile.ZipFile(package) as archive:
        entries = set(archive.namelist())
    for density, path in values.items():
        if not path.startswith("res/") or f"base/{path}" not in entries:
            raise ValueError(f"compiled AAB launcher resource is absent from bundle: {path}")
        if density == 65534:
            if not path.endswith("/ic_launcher.xml") or "anydpi-v26" not in path:
                raise ValueError("compiled AAB launcher adaptive resource is invalid")
        elif not path.endswith("/ic_launcher.png"):
            raise ValueError(f"compiled AAB density resource is not a PNG: {path}")
    return {"format": "aab", "icon_resource": f"@{ICON_NAME}", "density_count": 6}


def verify_compiled_launcher(
    package: Path, *, aapt: str = "", bundletool: str = "", java: str = ""
) -> dict[str, object]:
    package = Path(package).resolve()
    if not package.is_file():
        raise ValueError(f"Android package was not found: {package}")
    if package.suffix.lower() == ".apk":
        return _verify_apk(package, _tool("aapt", aapt))
    if package.suffix.lower() == ".aab":
        return _verify_aab(package, bundletool, java)
    raise ValueError(f"compiled launcher inspection requires an APK or AAB: {package}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--aapt", default="")
    parser.add_argument("--bundletool", default="")
    parser.add_argument("--java", default="")
    args = parser.parse_args()
    try:
        summary = verify_compiled_launcher(
            args.package, aapt=args.aapt, bundletool=args.bundletool, java=args.java
        )
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        print(f"Android launcher verification failed: {error}", file=sys.stderr)
        return 1
    print(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
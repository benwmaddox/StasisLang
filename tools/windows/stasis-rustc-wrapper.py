#!/usr/bin/env python3
"""Sign rustc-produced Windows executables before Cargo can use them."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import Mapping, Sequence


def signing_is_configured(environment: Mapping[str, str]) -> bool:
    if any(
        environment.get(name)
        for name in (
            "STASIS_AOT_SIGN_TOOL",
            "STASIS_SIGNING_CERTIFICATE",
            "STASIS_SIGNING_CERT_THUMBPRINT",
        )
    ):
        return True
    if environment.get("STASIS_REQUIRE_SIGNED_EXECUTION") == "1":
        return True
    signing_mode = environment.get("STASIS_SIGNING_MODE", "").casefold()
    signing_profile = environment.get("STASIS_SIGNING_PROFILE", "").casefold()
    if signing_mode == "required":
        return True
    production = (
        signing_mode == "production" or signing_profile == "production"
    )
    record = environment.get("STASIS_SIGNING_LOCAL_RECORD")
    if record is not None:
        return not production and bool(record) and Path(record).is_file()
    if production:
        return False
    local_app_data = environment.get("LOCALAPPDATA")
    return bool(
        local_app_data
        and (
            Path(local_app_data)
            / "Stasis"
            / "signing"
            / "development-thumbprint.txt"
        ).is_file()
    )


def _option_values(arguments: Sequence[str], option: str) -> list[str]:
    values: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == option and index + 1 < len(arguments):
            values.append(arguments[index + 1])
            index += 2
            continue
        prefix = f"{option}="
        if argument.startswith(prefix):
            values.append(argument[len(prefix) :])
        elif option == "-o" and argument.startswith("-o") and len(argument) > 2:
            values.append(argument[2:])
        index += 1
    return values


def _codegen_value(arguments: Sequence[str], name: str) -> str:
    values: list[str] = []
    index = 0
    while index < len(arguments):
        argument = arguments[index]
        if argument == "-C" and index + 1 < len(arguments):
            values.append(arguments[index + 1])
            index += 2
            continue
        if argument.startswith("-C") and len(argument) > 2:
            values.append(argument[2:])
        index += 1
    prefix = f"{name}="
    for value in reversed(values):
        if value.startswith(prefix):
            return value[len(prefix) :]
    return ""


def _emits_link_artifact(arguments: Sequence[str]) -> bool:
    emits = _option_values(arguments, "--emit")
    return not emits or any(
        item.split("=", 1)[0] == "link"
        for value in emits
        for item in value.split(",")
    )


def emitted_windows_artifacts(arguments: Sequence[str]) -> list[Path]:
    if any(argument in {"-vV", "-V", "--version"} for argument in arguments):
        return []
    print_kinds = _option_values(arguments, "--print")
    if print_kinds and not any(
        kind in {"link-args", "native-static-libs"} for kind in print_kinds
    ):
        return []
    if not _emits_link_artifact(arguments):
        return []
    targets = _option_values(arguments, "--target")
    if targets and Path(targets[-1]).suffix.casefold() == ".json":
        raise ValueError(
            "custom JSON rustc targets are unsupported by the signing wrapper; "
            "use a built-in target triple so emitted PE paths are deterministic"
        )

    link_outputs = [
        Path(item.split("=", 1)[1])
        for value in _option_values(arguments, "--emit")
        for item in value.split(",")
        if item.startswith("link=")
    ]
    outputs = _option_values(arguments, "-o")
    if outputs:
        output = Path(outputs[-1])
        return [output] if output.suffix.casefold() in {".exe", ".dll"} else []
    if link_outputs:
        return [
            output
            for output in link_outputs
            if output.suffix.casefold() in {".exe", ".dll"}
        ]

    out_dirs = _option_values(arguments, "--out-dir")
    crate_names = _option_values(arguments, "--crate-name")
    crate_types = [
        crate_type
        for value in _option_values(arguments, "--crate-type")
        for crate_type in value.split(",")
    ]
    if not out_dirs or not crate_names:
        return []
    if targets and "windows" not in targets[-1].casefold():
        return []

    stem = f"{crate_names[-1]}{_codegen_value(arguments, 'extra-filename')}"
    suffixes: list[str] = []
    if "--test" in arguments or "bin" in crate_types or not crate_types:
        suffixes.append(".exe")
    if "--test" not in arguments and any(
        crate_type in {"cdylib", "dylib", "proc-macro"}
        for crate_type in crate_types
    ):
        suffixes.append(".dll")
    return [Path(out_dirs[-1]) / f"{stem}{suffix}" for suffix in dict.fromkeys(suffixes)]


def signing_command(policy: Path, artifact: Path) -> list[str]:
    return [
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        str(policy),
        "sign",
        "-Artifact",
        str(artifact),
    ]


def run(
    arguments: Sequence[str],
    environment: Mapping[str, str],
    *,
    process_run=subprocess.run,
) -> int:
    if not arguments:
        print("[stasis-rustc-wrapper] missing rustc executable", file=sys.stderr)
        return 2

    try:
        artifacts = emitted_windows_artifacts(arguments[1:])
    except ValueError as error:
        print(f"[stasis-rustc-wrapper] {error}", file=sys.stderr)
        return 2
    rustc = process_run(list(arguments))
    if rustc.returncode != 0:
        return rustc.returncode

    policy_text = environment.get("STASIS_RUSTC_SIGNING_POLICY")
    if not policy_text:
        print(
            "[stasis-rustc-wrapper] signing policy path is not configured",
            file=sys.stderr,
        )
        return 2
    policy = Path(policy_text)
    for artifact in artifacts:
        if not artifact.is_file():
            print(
                f"[stasis-rustc-wrapper] rustc did not create expected artifact {artifact}",
                file=sys.stderr,
            )
            return 2
        signed = process_run(signing_command(policy, artifact))
        if signed.returncode != 0:
            print(
                f"[stasis-rustc-wrapper] signing failed for {artifact}",
                file=sys.stderr,
            )
            return signed.returncode or 1
    return 0


def sign_explicit_artifact(
    artifact: Path,
    environment: Mapping[str, str],
    *,
    windows: bool,
    process_run=subprocess.run,
) -> int:
    if not windows or not signing_is_configured(environment):
        return 0
    if artifact.suffix.casefold() not in {".exe", ".dll"}:
        return 0
    if not artifact.is_file():
        print(
            f"[stasis-rustc-wrapper] artifact does not exist: {artifact}",
            file=sys.stderr,
        )
        return 2
    policy = Path(
        environment.get("STASIS_RUSTC_SIGNING_POLICY")
        or Path(__file__).with_name("stasis-signing.ps1")
    ).resolve()
    signed = process_run(signing_command(policy, artifact.resolve()))
    if signed.returncode != 0:
        print(f"[stasis-rustc-wrapper] signing failed for {artifact}", file=sys.stderr)
        return signed.returncode or 1
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments[:1] == ["--sign-artifact"]:
        if len(arguments) != 2:
            print(
                "usage: stasis-rustc-wrapper.py --sign-artifact PATH",
                file=sys.stderr,
            )
            return 2
        return sign_explicit_artifact(
            Path(arguments[1]), os.environ, windows=sys.platform == "win32"
        )
    return run(arguments, os.environ)


if __name__ == "__main__":
    raise SystemExit(main())

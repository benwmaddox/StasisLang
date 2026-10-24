#!/usr/bin/env python3
"""Prepare the canonical Git Bash validation process for Windows MSVC."""

from __future__ import annotations

import argparse
import json
import ntpath
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Mapping, Sequence


FALLBACK_TOOLCHAIN = "stable-x86_64-pc-windows-msvc"
MSVC_TARGET = "x86_64-pc-windows-msvc"
GNU_TARGET = "x86_64-pc-windows-gnu"
VC_COMPONENT = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
SUPPORTED_VS_MAJORS = {17, 18}
RUNNER = Callable[..., subprocess.CompletedProcess[str]]


class ValidationSetupError(RuntimeError):
    """An actionable error while preparing Windows validation."""


@dataclass(frozen=True)
class RustSelection:
    active_toolchain: str
    toolchain: str
    target: str | None
    target_mode: str
    target_was_normalized: bool


@dataclass(frozen=True)
class VisualStudioSelection:
    version: str
    installation_path: Path
    vcvars64: Path


def _run(
    runner: RUNNER,
    arguments: Sequence[str] | str,
    environment: Mapping[str, str],
) -> subprocess.CompletedProcess[str]:
    try:
        return runner(
            arguments,
            env=dict(environment),
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError as error:
        raise ValidationSetupError(f"could not start {arguments!r}: {error}") from error


def _rustup_executable(
    environment: Mapping[str, str], explicit: str | os.PathLike[str] | None
) -> str:
    if explicit is not None:
        return str(explicit)
    executable = shutil.which("rustup", path=environment.get("PATH"))
    if executable:
        return executable
    cargo_home = environment.get("CARGO_HOME")
    if not cargo_home:
        user_profile = environment.get("USERPROFILE")
        if user_profile:
            cargo_home = str(Path(user_profile) / ".cargo")
    if cargo_home:
        candidate = Path(cargo_home) / "bin" / "rustup.exe"
        if candidate.is_file():
            return str(candidate)
    raise ValidationSetupError(
        "rustup was not found on PATH; install Rust with rustup and reopen Git Bash"
    )


def _checked_output(
    runner: RUNNER,
    executable: str,
    arguments: Sequence[str],
    environment: Mapping[str, str],
    *,
    purpose: str,
) -> str:
    command = [executable, *arguments]
    result = _run(runner, command, environment)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        if detail:
            detail = f" Details: {detail}"
        raise ValidationSetupError(
            f"could not {purpose} with rustup ({' '.join(command)}).{detail}"
        )
    return result.stdout or ""


def _active_toolchain(
    environment: Mapping[str, str], runner: RUNNER, rustup: str
) -> tuple[str, bool]:
    explicit = environment.get("RUSTUP_TOOLCHAIN", "").strip()
    if explicit:
        return explicit, True
    output = _checked_output(
        runner,
        rustup,
        ["show", "active-toolchain"],
        environment,
        purpose="read the active Rust toolchain",
    )
    toolchain = output.strip().split(maxsplit=1)[0] if output.strip() else ""
    if not toolchain:
        raise ValidationSetupError(
            "rustup did not report an active toolchain; install an x86_64 Windows MSVC toolchain"
        )
    return toolchain, False


def _rustc_host(
    toolchain: str,
    environment: Mapping[str, str],
    runner: RUNNER,
    rustup: str,
    *,
    explicit: bool,
) -> str:
    try:
        output = _checked_output(
            runner,
            rustup,
            ["run", toolchain, "rustc", "-vV"],
            environment,
            purpose=f"verify Rust toolchain {toolchain!r}",
        )
    except ValidationSetupError as error:
        if explicit:
            raise ValidationSetupError(
                f"RUSTUP_TOOLCHAIN={toolchain!r} is unavailable. Install it with "
                f"'rustup toolchain install {toolchain}' or select an installed toolchain. "
                f"Automatic installation is disabled. {error}"
            ) from error
        if toolchain == FALLBACK_TOOLCHAIN:
            raise ValidationSetupError(
                f"the Windows validator requires the installed {FALLBACK_TOOLCHAIN!r} "
                f"toolchain when the active toolchain is not MSVC. Install it with "
                f"'rustup toolchain install {FALLBACK_TOOLCHAIN}'. Automatic installation "
                f"is disabled. {error}"
            ) from error
        raise
    for line in output.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ").strip()
    raise ValidationSetupError(
        f"rustup toolchain {toolchain!r} did not report a rustc host target"
    )


def _installed_targets(
    toolchain: str,
    environment: Mapping[str, str],
    runner: RUNNER,
    rustup: str,
) -> set[str]:
    output = _checked_output(
        runner,
        rustup,
        ["target", "list", "--installed", "--toolchain", toolchain],
        environment,
        purpose=f"check installed targets for Rust toolchain {toolchain!r}",
    )
    return {line.strip() for line in output.splitlines() if line.strip()}


def _require_msvc_target(
    toolchain: str,
    environment: Mapping[str, str],
    runner: RUNNER,
    rustup: str,
) -> None:
    try:
        targets = _installed_targets(toolchain, environment, runner, rustup)
    except ValidationSetupError as error:
        raise ValidationSetupError(
            f"could not verify target {MSVC_TARGET!r} for Rust toolchain {toolchain!r}: {error}"
        ) from error
    if MSVC_TARGET not in targets:
        raise ValidationSetupError(
            f"Rust toolchain {toolchain!r} is missing required target {MSVC_TARGET!r}. "
            f"Install it with 'rustup target add --toolchain {toolchain} {MSVC_TARGET}'. "
            "Automatic installation is disabled."
        )


def resolve_rust_selection(
    environment: Mapping[str, str],
    *,
    run: RUNNER = subprocess.run,
    rustup_executable: str | os.PathLike[str] | None = None,
) -> RustSelection:
    """Choose an installed x64 MSVC toolchain and validate the target contract."""
    target_input = environment.get("CARGO_BUILD_TARGET", "").strip()
    if target_input and target_input not in {MSVC_TARGET, GNU_TARGET}:
        raise ValidationSetupError(
            f"CARGO_BUILD_TARGET={target_input!r} is unsupported by canonical Windows "
            "validation; only x86_64 Windows validation is supported"
        )

    rustup = _rustup_executable(environment, rustup_executable)
    active, explicit = _active_toolchain(environment, run, rustup)
    active_host = _rustc_host(active, environment, run, rustup, explicit=explicit)
    selected = active
    selected_host = active_host

    if active_host != MSVC_TARGET:
        if explicit and active_host.endswith("-windows-msvc"):
            raise ValidationSetupError(
                f"explicit RUSTUP_TOOLCHAIN={active!r} targets {active_host!r}, but "
                f"canonical Windows validation supports {MSVC_TARGET!r}; select an "
                "installed x86_64 MSVC toolchain"
            )
        selected = FALLBACK_TOOLCHAIN
        selected_host = _rustc_host(
            selected, environment, run, rustup, explicit=False
        )
        if selected_host != MSVC_TARGET:
            raise ValidationSetupError(
                f"required toolchain {selected!r} reports host {selected_host!r}; "
                f"expected {MSVC_TARGET!r}"
            )

    _require_msvc_target(selected, environment, run, rustup)

    target_was_normalized = target_input == GNU_TARGET
    target = MSVC_TARGET if target_input and target_was_normalized else target_input or None
    return RustSelection(
        active_toolchain=active,
        toolchain=selected,
        target=target,
        target_mode="set" if target else "unset",
        target_was_normalized=target_was_normalized,
    )


def _version_key(version: str) -> tuple[int, ...]:
    try:
        parts = tuple(int(part) for part in version.split("."))
    except ValueError:
        return ()
    return parts


def _vswhere_path(environment: Mapping[str, str]) -> Path:
    program_files = environment.get("ProgramFiles(x86)") or environment.get(
        "ProgramFiles", ""
    )
    if not program_files:
        raise ValidationSetupError(
            "ProgramFiles(x86) is unavailable; install Visual Studio 2022 or 2026 with the VC x64 tools"
        )
    return (
        Path(program_files)
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe"
    )


def discover_visual_studio(
    environment: Mapping[str, str], *, run: RUNNER = subprocess.run
) -> VisualStudioSelection:
    """Find the newest supported installed VS instance with x64 VC tools."""
    vswhere = _vswhere_path(environment)
    if not vswhere.is_file():
        raise ValidationSetupError(
            f"vswhere.exe was not found at {vswhere}; install Visual Studio 2022 or 2026 "
            "with the VC x64 build tools"
        )
    arguments = [
        str(vswhere),
        "-all",
        "-products",
        "*",
        "-requires",
        VC_COMPONENT,
        "-format",
        "json",
        "-utf8",
    ]
    result = _run(run, arguments, environment)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip()
        raise ValidationSetupError(
            f"vswhere.exe could not inspect Visual Studio installations: {detail or result.returncode}"
        )
    try:
        records = json.loads(result.stdout or "[]")
    except json.JSONDecodeError as error:
        raise ValidationSetupError(
            f"vswhere.exe returned invalid installation data: {error}"
        ) from error
    if isinstance(records, dict):
        records = [records]
    candidates: list[tuple[tuple[int, ...], str, Path]] = []
    for record in records if isinstance(records, list) else []:
        if not isinstance(record, dict):
            continue
        version = str(record.get("installationVersion", ""))
        installation_text = record.get("installationPath")
        version_key = _version_key(version)
        if (
            not version_key
            or version_key[0] not in SUPPORTED_VS_MAJORS
            or not installation_text
            or record.get("isPrerelease") is True
        ):
            continue
        installation = Path(str(installation_text))
        vcvars = installation / "VC" / "Auxiliary" / "Build" / "vcvars64.bat"
        if vcvars.is_file():
            candidates.append((version_key, version, installation))
    if not candidates:
        raise ValidationSetupError(
            "Visual Studio 2022 or 2026 with the VC x64 build tools and vcvars64.bat "
            "was not found; install the 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64' component"
        )
    _, version, installation = max(candidates, key=lambda item: item[0])
    return VisualStudioSelection(
        version=version,
        installation_path=installation,
        vcvars64=installation / "VC" / "Auxiliary" / "Build" / "vcvars64.bat",
    )


def bridge_batch_contents() -> str:
    """Return a CMD bridge that loads vcvars and re-enters the exact Git Bash."""
    return "\r\n".join(
        (
            "@echo off",
            "setlocal",
            'call "%STASIS_VALIDATE_VCVARS64%"',
            "if errorlevel 1 goto :vcvars_failed",
            'set "STASIS_VALIDATE_MSVC_BIN=%VCToolsInstallDir%\\bin\\Hostx64\\x64"',
            'if not exist "%STASIS_VALIDATE_MSVC_BIN%\\cl.exe" goto :missing_cl',
            'if not exist "%STASIS_VALIDATE_MSVC_BIN%\\link.exe" goto :missing_link',
            'echo Windows validation: Visual Studio %STASIS_VALIDATE_VS_VERSION% at %VSINSTALLDIR%',
            'echo Windows validation: Rust toolchain %STASIS_VALIDATE_RUSTUP_TOOLCHAIN%',
            'set "STASIS_VALIDATE_REENTRY=1"',
            '"%STASIS_VALIDATE_BASH_EXE%" -l "%STASIS_VALIDATE_SCRIPT%"',
            "exit /b %ERRORLEVEL%",
            ":vcvars_failed",
            'echo ERROR: vcvars64.bat failed to load the selected MSVC environment. 1>&2',
            "exit /b 1",
            ":missing_cl",
            'echo ERROR: selected MSVC installation is missing cl.exe under %STASIS_VALIDATE_MSVC_BIN%. 1>&2',
            "exit /b 1",
            ":missing_link",
            'echo ERROR: selected MSVC installation is missing link.exe under %STASIS_VALIDATE_MSVC_BIN%. 1>&2',
            "exit /b 1",
            "",
        )
    )


def build_cmd_invocation(cmd_executable: Path, bridge_path: Path) -> str:
    """Build the cmd.exe /c form that safely runs a .cmd path containing spaces."""
    cmd = str(cmd_executable)
    bridge = str(bridge_path)
    if '"' in cmd or '"' in bridge:
        raise ValidationSetupError("Windows command paths cannot contain a quote character")
    return f'"{cmd}" /d /s /c ""{bridge}""'


def _cmd_executable(environment: Mapping[str, str]) -> Path:
    folded_environment = {name.casefold(): value for name, value in environment.items()}
    system_root = folded_environment.get("systemroot") or folded_environment.get("windir")
    if not system_root:
        raise ValidationSetupError(
            "SystemRoot is unavailable; run canonical Windows validation from Git Bash on Windows"
        )
    executable = Path(system_root) / "System32" / "cmd.exe"
    if not executable.is_file():
        raise ValidationSetupError(f"Windows command processor was not found at {executable}")
    return executable


def launch_windows_validation(
    environment: Mapping[str, str],
    bash_executable: str | os.PathLike[str],
    script_path: str | os.PathLike[str],
    *,
    run: RUNNER = subprocess.run,
    rustup_executable: str | os.PathLike[str] | None = None,
    temp_root: str | os.PathLike[str] | None = None,
) -> int:
    """Load MSVC in a child CMD process and re-run the canonical Git Bash script."""
    rust = resolve_rust_selection(
        environment, run=run, rustup_executable=rustup_executable
    )
    visual_studio = discover_visual_studio(environment, run=run)
    child_environment = dict(environment)
    child_environment.update(
        {
            "STASIS_VALIDATE_VCVARS64": str(visual_studio.vcvars64),
            "STASIS_VALIDATE_VS_VERSION": visual_studio.version,
            "STASIS_VALIDATE_BASH_EXE": str(bash_executable),
            "STASIS_VALIDATE_SCRIPT": str(script_path),
            "STASIS_VALIDATE_RUSTUP_TOOLCHAIN": rust.toolchain,
            "STASIS_VALIDATE_TARGET_MODE": rust.target_mode,
            "STASIS_VALIDATE_CARGO_BUILD_TARGET": rust.target or "",
        }
    )
    target_description = rust.target or f"host ({MSVC_TARGET}; CARGO_BUILD_TARGET unset)"
    if rust.toolchain != rust.active_toolchain:
        print(
            "Windows validation: replacing active Rust toolchain "
            f"{rust.active_toolchain} with installed MSVC toolchain {rust.toolchain}",
            flush=True,
        )
    if rust.target_was_normalized:
        print(
            "Windows validation: normalizing CARGO_BUILD_TARGET "
            f"{GNU_TARGET} -> {MSVC_TARGET}",
            flush=True,
        )
    print(
        f"Windows validation: Rust toolchain {rust.toolchain}; target {target_description}",
        flush=True,
    )
    print(
        f"Windows validation: Visual Studio {visual_studio.version} at "
        f"{visual_studio.installation_path}",
        flush=True,
    )
    command_processor = _cmd_executable(child_environment)
    with tempfile.TemporaryDirectory(
        prefix="stasis-validation-", dir=str(temp_root) if temp_root else None
    ) as temporary_directory:
        bridge_path = Path(temporary_directory) / "run-validation.cmd"
        bridge_path.write_text(bridge_batch_contents(), encoding="ascii", newline="")
        command_line = build_cmd_invocation(command_processor, bridge_path)
        try:
            result = run(
                command_line,
                env=child_environment,
                check=False,
            )
        except OSError as error:
            raise ValidationSetupError(
                f"could not start the Windows validation bridge: {error}"
            ) from error
        return result.returncode


def _path_key(value: str | os.PathLike[str]) -> str:
    return ntpath.normcase(ntpath.normpath(str(value))).casefold()


def verify_msvc_tools(
    environment: Mapping[str, str], *, which: Callable[..., str | None] = shutil.which
) -> tuple[str, str]:
    """Verify PATH selects the cl/link pair installed by the loaded vcvars64."""
    directory = environment.get("STASIS_VALIDATE_MSVC_BIN", "")
    if not directory:
        raise ValidationSetupError(
            "MSVC validation state is missing; rerun tools/validate_repo.sh from Git Bash"
        )
    resolved: list[str] = []
    for executable in ("cl.exe", "link.exe"):
        expected = ntpath.join(directory, executable)
        actual = which(executable, path=environment.get("PATH"))
        if not actual:
            raise ValidationSetupError(
                f"{executable} was not found after loading MSVC; expected {expected}"
            )
        if _path_key(actual) != _path_key(expected):
            raise ValidationSetupError(
                f"{executable} resolved to {actual!r}; expected the selected MSVC tool at {expected!r}. "
                "The validator prepends MSVC after Bash login startup; check for a shell startup script that rewrites PATH."
            )
        resolved.append(actual)
    return resolved[0], resolved[1]


def verify_reentry(environment: Mapping[str, str]) -> tuple[str, str]:
    toolchain = environment.get("STASIS_VALIDATE_RUSTUP_TOOLCHAIN", "")
    mode = environment.get("STASIS_VALIDATE_TARGET_MODE", "")
    if not toolchain or environment.get("RUSTUP_TOOLCHAIN") != toolchain:
        raise ValidationSetupError(
            "Rust toolchain selection was not restored after Bash login startup"
        )
    target = environment.get("STASIS_VALIDATE_CARGO_BUILD_TARGET", "")
    if mode == "set":
        if not target or environment.get("CARGO_BUILD_TARGET") != target:
            raise ValidationSetupError(
                "CARGO_BUILD_TARGET selection was not restored after Bash login startup"
            )
    elif mode == "unset":
        if environment.get("CARGO_BUILD_TARGET"):
            raise ValidationSetupError(
                "CARGO_BUILD_TARGET should be unset for the selected MSVC host toolchain"
            )
    else:
        raise ValidationSetupError(
            "MSVC validation state has an unknown CARGO_BUILD_TARGET mode"
        )
    cl_path, link_path = verify_msvc_tools(environment)
    print(f"Windows validation: cl.exe {cl_path}")
    print(f"Windows validation: link.exe {link_path}")
    return cl_path, link_path


def _main(arguments: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--launch", action="store_true")
    parser.add_argument("--verify-reentry", action="store_true")
    parser.add_argument("--bash-exe")
    parser.add_argument("--script")
    options = parser.parse_args(arguments)
    try:
        if options.verify_reentry:
            verify_reentry(os.environ)
            return 0
        if not options.launch:
            parser.error("choose --launch or --verify-reentry")
        if os.name != "nt":
            raise ValidationSetupError(
                "the Windows validation coordinator can only launch on Windows"
            )
        if not options.bash_exe or not options.script:
            parser.error("--launch requires --bash-exe and --script")
        return launch_windows_validation(
            os.environ, options.bash_exe, options.script
        )
    except ValidationSetupError as error:
        print(f"Windows validation setup failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(_main())

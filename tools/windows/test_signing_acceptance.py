#!/usr/bin/env python3
"""Live acceptance for the signed native rustc launcher on Windows."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Mapping, Sequence


ROOT = Path(__file__).resolve().parents[2]


class AcceptanceError(RuntimeError):
    pass


def _run(command: Sequence[str], *, environment: Mapping[str, str]) -> None:
    try:
        result = subprocess.run(list(command), env=environment, check=False, timeout=900)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AcceptanceError(f"failed to execute {command[0]}: {error}") from error
    if result.returncode != 0:
        raise AcceptanceError(
            f"command failed with exit {result.returncode}: {command[0]}"
        )


def run_acceptance(target_dir: Path, environment: Mapping[str, str]) -> None:
    if sys.platform != "win32":
        raise AcceptanceError("the signing acceptance command requires Windows")
    output = target_dir.resolve() / "stasis-signing-acceptance"
    output.mkdir(parents=True, exist_ok=True)
    launcher = output / "stasis-rustc-wrapper-acceptance.exe"
    recorder = output / "record-long-argument.py"
    marker = output / "long-argument-length.txt"
    policy = ROOT / "tools" / "windows" / "stasis-signing.ps1"
    source = ROOT / "tools" / "windows" / "stasis-rustc-wrapper.rs"

    child = dict(environment)
    _run(["rustc", str(source), "-o", str(launcher)], environment=child)
    try:
        _run(
            [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(policy),
                "sign",
                "-Artifact",
                str(launcher),
            ],
            environment=child,
        )
        _run(
            [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                str(policy),
                "verify",
                "-Artifact",
                str(launcher),
            ],
            environment=child,
        )
    except AcceptanceError as error:
        if launcher.is_file():
            launcher.unlink()
        launcher_pdb = launcher.with_suffix(".pdb")
        if launcher_pdb.is_file():
            launcher_pdb.unlink()
        raise AcceptanceError(
            "launcher signing and Authenticode verification failed; configure a "
            "code-signing certificate or provision the explicit local development "
            "certificate before running this acceptance command"
        ) from error

    recorder.write_text(
        "import os, sys\n"
        "from pathlib import Path\n"
        "Path(os.environ['STASIS_TEST_MARKER']).write_text(str(len(sys.argv[1])))\n",
        encoding="ascii",
    )
    if marker.is_file():
        marker.unlink()
    child["STASIS_RUSTC_WRAPPER_PYTHON"] = sys.executable
    child["STASIS_RUSTC_WRAPPER_SCRIPT"] = str(recorder)
    child["STASIS_TEST_MARKER"] = str(marker)
    _run([str(launcher), "x" * 9000], environment=child)
    if not marker.is_file() or marker.read_text(encoding="ascii") != "9000":
        raise AcceptanceError(
            "signed launcher did not preserve the 9,000-character argument"
        )
    run_cargo_acceptance(output, environment)


def run_cargo_acceptance(output: Path, environment: Mapping[str, str]) -> None:
    # Always compile fresh host artifacts, including paths containing spaces.
    with tempfile.TemporaryDirectory(prefix="cargo signing spaces ", dir=output) as directory:
        fixture = Path(directory)
        (fixture / "src").mkdir()
        (fixture / "macros" / "src").mkdir(parents=True)
        (fixture / "Cargo.toml").write_text(
            '[package]\nname="signing-acceptance"\nversion="0.1.0"\nedition="2021"\n'
            '[workspace]\n[dependencies]\nacceptance-macros={path="macros"}\n',
            encoding="ascii",
        )
        (fixture / "build.rs").write_text(
            'fn main() { println!("cargo:rustc-env=BUILD_SCRIPT_EXECUTED=yes"); }\n',
            encoding="ascii",
        )
        (fixture / "src" / "main.rs").write_text(
            'fn main() { assert_eq!(env!("BUILD_SCRIPT_EXECUTED"), "yes"); '
            'assert_eq!(acceptance_macros::answer!(), 42); }\n', encoding="ascii",
        )
        (fixture / "macros" / "Cargo.toml").write_text(
            '[package]\nname="acceptance-macros"\nversion="0.1.0"\nedition="2021"\n'
            '[lib]\nproc-macro=true\n', encoding="ascii",
        )
        (fixture / "macros" / "src" / "lib.rs").write_text(
            'extern crate proc_macro;\n#[proc_macro]\n'
            'pub fn answer(_: proc_macro::TokenStream) -> proc_macro::TokenStream '
            '{ "42".parse().unwrap() }\n', encoding="ascii",
        )
        child = dict(environment)
        child["CARGO_TARGET_DIR"] = str(fixture / "target")
        child["STASIS_SIGNING_MODE"] = "required"
        _run(
            [sys.executable, str(ROOT / "tools" / "cargo_cache.py"), "run", "--",
             "cargo", "run", "--offline", "--manifest-path", str(fixture / "Cargo.toml")],
            environment=child,
        )
        profile = fixture / "target" / "debug"
        artifacts = [
            *profile.glob("build/*/build-script-build.exe"),
            *profile.glob("deps/acceptance_macros*.dll"),
            profile / "signing-acceptance.exe",
        ]
        if len(artifacts) != 3 or not all(path.is_file() for path in artifacts):
            raise AcceptanceError("Cargo did not emit the build script, proc macro, and executable")
        for artifact in artifacts:
            _run(
                ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy",
                 "Bypass", "-File", str(ROOT / "tools/windows/stasis-signing.ps1"),
                 "verify", "-Artifact", str(artifact)], environment=child,
            )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")),
    )
    args = parser.parse_args(argv)
    try:
        run_acceptance(args.target_dir, os.environ)
    except AcceptanceError as error:
        print(f"signing acceptance failed: {error}", file=sys.stderr)
        return 1
    print("signed launcher preserved a 9,000-character argument; fresh Cargo build script, proc macro, and executable passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

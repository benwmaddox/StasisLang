#!/usr/bin/env python3
"""Run the complete Cargo workspace test matrix in bounded semantic shards."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


ROOT = Path(__file__).resolve().parents[2]
SHARD_TIMEOUT_SECONDS = 900
LIBRARY_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}


@dataclass(frozen=True)
class Shard:
    name: str
    cargo_arguments: tuple[str, ...]
    targets: tuple[tuple[str, str, str], ...]

    def command(self, root: Path) -> list[str]:
        return [
            sys.executable,
            str(root / "tools/cargo_cache.py"),
            "run",
            "--",
            "cargo",
            "test",
            *self.cargo_arguments,
            "--",
            "--test-threads=1",
        ]


@dataclass(frozen=True)
class ProcessResult:
    exit_code: int
    elapsed_seconds: float
    timed_out: bool


def _target_category(package: str, target: dict[str, object]) -> str | None:
    name = str(target["name"])
    kinds = set(target["kind"])
    if kinds == {"custom-build"}:
        return None
    if kinds == {"test"}:
        return "test"
    if kinds == {"bin"}:
        return "bin"
    if kinds == {"example"}:
        return "example"
    if kinds == {"bench"}:
        return "bench"
    if kinds and kinds <= LIBRARY_KINDS:
        return "lib"
    rendered = ", ".join(sorted(kinds)) or "<empty>"
    raise ValueError(
        f"unsupported Cargo target kind [{rendered}] for {package}:{name}; "
        "classify it explicitly so validation cannot lose coverage"
    )


def _core_arguments(prefix: Sequence[str], categories: set[str]) -> tuple[str, ...]:
    flags = []
    for category, flag in (
        ("lib", "--lib"),
        ("bin", "--bins"),
        ("example", "--examples"),
    ):
        if category in categories:
            flags.append(flag)
    # --all-targets expands to --lib --bins --tests --benches --examples.
    # Preserve implicit lib/bin benchmark modes even when metadata has no
    # standalone `bench` target; explicit benchmark targets use the same flag.
    flags.append("--benches")
    if categories == {"bench"}:
        return (*prefix, *flags)
    if len(flags) == 1:
        raise ValueError(f"core shard {' '.join(prefix)} has no selectable targets")
    return (*prefix, *flags)


def _integration_arguments(prefix: Sequence[str], targets: Sequence[str]) -> tuple[str, ...]:
    if not targets:
        raise ValueError(f"integration shard {' '.join(prefix)} has no targets")
    arguments = list(prefix)
    for target in sorted(targets):
        arguments.extend(("--test", target))
    return tuple(arguments)


def plan_workspace_shards(metadata: dict[str, object]) -> list[Shard]:
    workspace_members = set(metadata["workspace_members"])
    packages = sorted(
        (
            package
            for package in metadata["packages"]
            if package["id"] in workspace_members
        ),
        key=lambda package: package["name"],
    )
    by_name = {package["name"]: package for package in packages}
    for required in ("stasis", "stasis_compiler"):
        if required not in by_name:
            raise ValueError(f"workspace metadata is missing required package {required}")

    categories: dict[tuple[str, str, str], str] = {}
    for package in packages:
        package_name = str(package["name"])
        for target in package["targets"]:
            category = _target_category(package_name, target)
            if category is not None:
                key = (package_name, str(target["name"]), category)
                if key in categories:
                    raise ValueError(f"duplicate Cargo target identity {package_name}:{key[1]}")
                categories[key] = category

    windows_key = ("stasis", "windows_game_launch", "test")
    if categories.get(windows_key) != "test":
        raise ValueError(
            "the calibrated stasis:windows_game_launch integration target is missing; "
            "update the bounded shard plan before changing its ownership"
        )

    integration_names: dict[str, list[tuple[str, str, str]]] = {}
    for key, category in categories.items():
        if category == "test":
            integration_names.setdefault(key[1], []).append(key)
    duplicate_integration_names = {
        name: keys for name, keys in integration_names.items() if len(keys) > 1
    }
    if duplicate_integration_names:
        name, keys = sorted(duplicate_integration_names.items())[0]
        owners = ", ".join(sorted(key[0] for key in keys))
        raise ValueError(
            f"duplicate integration target name {name} across packages {owners}; "
            "global workspace test selection would execute it more than once, so "
            "give the targets unique names or update their shard ownership"
        )

    def selected(package_names: set[str], selected_categories: set[str]):
        return tuple(
            key
            for key, category in categories.items()
            if key[0] in package_names and category in selected_categories
        )

    all_names = set(by_name)
    core_categories = {"lib", "bin", "example", "bench"}
    workspace_core = selected(all_names, core_categories)
    stasis_integrations = tuple(
        key
        for key in selected({"stasis"}, {"test"})
        if key != windows_key
    )
    other_integrations = selected(all_names - {"stasis"}, {"test"})

    shards = [
        Shard(
            "workspace-core",
            _core_arguments(
                ("--workspace",),
                {categories[key] for key in workspace_core},
            ),
            workspace_core,
        ),
        Shard(
            "stasis-integrations",
            _integration_arguments(
                ("--workspace",), [key[1] for key in stasis_integrations]
            ),
            stasis_integrations,
        ),
        Shard(
            "windows-game-launch",
            _integration_arguments(("--workspace",), [windows_key[1]]),
            (windows_key,),
        ),
        Shard(
            "other-integrations",
            _integration_arguments(
                ("--workspace",),
                [key[1] for key in other_integrations],
            ),
            other_integrations,
        ),
    ]

    covered = [target for shard in shards for target in shard.targets]
    if len(covered) != len(set(covered)) or set(covered) != set(categories):
        missing = sorted(set(categories) - set(covered))
        duplicate_count = len(covered) - len(set(covered))
        raise ValueError(
            "workspace shard coverage is incomplete or duplicated: "
            f"missing={missing}, duplicate_count={duplicate_count}"
        )
    return shards


def _popen_options() -> dict[str, object]:
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def terminate_process_tree(process: subprocess.Popen[object]) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        termination = subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            capture_output=True,
            text=True,
        )
        if termination.returncode != 0:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=30)
            raise RuntimeError(
                f"taskkill could not verify termination of process tree {process.pid}; "
                "descendants may still be running: "
                f"{(termination.stderr or termination.stdout).strip()}"
            )
    else:
        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
    try:
        process.wait(timeout=30)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=30)


def run_bounded(
    command: Sequence[str], timeout_seconds: float = SHARD_TIMEOUT_SECONDS
) -> ProcessResult:
    started = time.monotonic()
    process = subprocess.Popen(command, **_popen_options())
    try:
        exit_code = process.wait(timeout=timeout_seconds)
        timed_out = False
    except subprocess.TimeoutExpired:
        terminate_process_tree(process)
        exit_code = 124
        timed_out = True
    except BaseException:
        terminate_process_tree(process)
        raise
    return ProcessResult(exit_code, time.monotonic() - started, timed_out)


def load_metadata(root: Path) -> dict[str, object]:
    command = [
        sys.executable,
        str(root / "tools/cargo_cache.py"),
        "run",
        "--",
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
    ]
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        **_popen_options(),
    )
    try:
        stdout, stderr = process.communicate(timeout=SHARD_TIMEOUT_SECONDS)
    except subprocess.TimeoutExpired as error:
        terminate_process_tree(process)
        raise RuntimeError(
            f"Cargo metadata timed out after {SHARD_TIMEOUT_SECONDS} seconds"
        ) from error
    except BaseException:
        terminate_process_tree(process)
        raise
    if stderr:
        sys.stderr.write(stderr)
    if process.returncode != 0:
        raise RuntimeError(f"Cargo metadata failed with exit code {process.returncode}")
    metadata = None
    diagnostics = []
    for line in stdout.splitlines():
        try:
            candidate = json.loads(line)
        except json.JSONDecodeError:
            diagnostics.append(line)
            continue
        if isinstance(candidate, dict) and {
            "packages",
            "workspace_members",
        } <= candidate.keys():
            if metadata is not None:
                raise ValueError("Cargo metadata emitted more than one workspace document")
            metadata = candidate
        else:
            diagnostics.append(line)
    if diagnostics:
        print("\n".join(diagnostics), flush=True)
    if metadata is None:
        raise ValueError("Cargo metadata output did not contain a workspace document")
    return metadata


def main() -> int:
    try:
        shards = plan_workspace_shards(load_metadata(ROOT))
    except (OSError, ValueError, RuntimeError, json.JSONDecodeError) as error:
        print(f"workspace test planning failed: {error}", file=sys.stderr)
        return 1

    print(
        f"Workspace target inventory: {sum(len(shard.targets) for shard in shards)} "
        f"selectable targets in {len(shards)} bounded shards.",
        flush=True,
    )
    for index, shard in enumerate(shards, start=1):
        print(
            f"::group::workspace tests {index}/{len(shards)}: {shard.name} "
            f"({len(shard.targets)} targets)",
            flush=True,
        )
        try:
            result = run_bounded(shard.command(ROOT))
        except (OSError, RuntimeError, subprocess.SubprocessError) as error:
            print(f"::endgroup::", flush=True)
            print(f"{shard.name} could not run: {error}", file=sys.stderr)
            return 1
        print(
            f"{shard.name}: exit={result.exit_code} "
            f"elapsed={result.elapsed_seconds:.3f}s timed_out={str(result.timed_out).lower()}",
            flush=True,
        )
        print("::endgroup::", flush=True)
        if result.exit_code != 0:
            if result.timed_out:
                print(
                    f"{shard.name} exceeded the {SHARD_TIMEOUT_SECONDS}-second limit; "
                    "its owned process tree was terminated.",
                    file=sys.stderr,
                )
            return result.exit_code
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Bounded Cargo cache policy for Stasis automation worktrees."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping, Sequence


SHARED_TARGET_RELATIVE = Path("build") / "codex-cargo-target"


@dataclass(frozen=True)
class RepoContext:
    worktree_root: Path
    common_git_dir: Path
    repository_root: Path
    shared_target: Path


def _git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(cwd), *args],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def shared_target_for(common_git_dir: Path) -> Path:
    return common_git_dir.parent / SHARED_TARGET_RELATIVE


def discover_context(cwd: Path) -> RepoContext:
    worktree_root = Path(_git(cwd, "rev-parse", "--show-toplevel")).resolve()
    common_git_text = _git(
        cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"
    )
    common_git_dir = Path(common_git_text).resolve()
    repository_root = common_git_dir.parent
    return RepoContext(
        worktree_root=worktree_root,
        common_git_dir=common_git_dir,
        repository_root=repository_root,
        shared_target=shared_target_for(common_git_dir).resolve(),
    )


def agent_environment(
    parent: Mapping[str, str], shared_target: Path
) -> dict[str, str]:
    child = dict(parent)
    child.setdefault("CARGO_TARGET_DIR", str(shared_target))
    child["CARGO_INCREMENTAL"] = "0"
    return child


def _directory_size(path: Path) -> int:
    total = 0
    if not path.is_dir():
        return total
    for root, directories, files in os.walk(path, followlinks=False):
        root_path = Path(root)
        directories[:] = [
            name for name in directories if not (root_path / name).is_symlink()
        ]
        for name in files:
            candidate = root_path / name
            if candidate.is_symlink():
                continue
            try:
                total += candidate.stat().st_size
            except FileNotFoundError:
                continue
    return total


def _incremental_size(profile: Path) -> int:
    total = 0
    for root, directories, _files in os.walk(profile, followlinks=False):
        root_path = Path(root)
        kept: list[str] = []
        for name in directories:
            candidate = root_path / name
            if candidate.is_symlink():
                continue
            if name == "incremental":
                total += _directory_size(candidate)
            else:
                kept.append(name)
        directories[:] = kept
    return total


def measure_target(target: Path) -> dict[str, object]:
    profiles = []
    if target.is_dir():
        for profile in sorted(
            (path for path in target.iterdir() if path.is_dir() and not path.is_symlink()),
            key=lambda path: path.name.casefold(),
        ):
            profiles.append(
                {
                    "name": profile.name,
                    "bytes": _directory_size(profile),
                    "incremental_bytes": _incremental_size(profile),
                }
            )
    return {
        "path": str(target.resolve()),
        "bytes": _directory_size(target),
        "profiles": profiles,
    }


def registered_worktrees(context: RepoContext) -> list[Path]:
    output = _git(context.worktree_root, "worktree", "list", "--porcelain")
    worktrees = []
    for line in output.splitlines():
        if line.startswith("worktree "):
            worktrees.append(Path(line.removeprefix("worktree ")).resolve())
    return sorted(worktrees, key=lambda path: os.path.normcase(str(path)))


def measure_all(context: RepoContext) -> dict[str, object]:
    worktree_reports = []
    for worktree in registered_worktrees(context):
        target = worktree / "target"
        if target.is_dir():
            report = measure_target(target)
            report["worktree"] = str(worktree)
            worktree_reports.append(report)
    return {
        "shared": measure_target(context.shared_target),
        "worktrees": worktree_reports,
        "worktree_bytes": sum(int(report["bytes"]) for report in worktree_reports),
    }


def _same_path(left: Path, right: Path) -> bool:
    return os.path.normcase(os.path.abspath(left)) == os.path.normcase(
        os.path.abspath(right)
    )


def _validated_tree(owner: Path, target: Path, expected: Path) -> tuple[Path, Path]:
    owner_resolved = owner.resolve()
    if not _same_path(target, expected):
        raise ValueError(f"cleanup path is not the exact worktree target: {expected}")
    target_resolved = target.resolve()
    try:
        target_resolved.relative_to(owner_resolved)
    except ValueError as error:
        raise ValueError(f"cleanup target escapes worktree: {target}") from error
    if target_resolved == owner_resolved:
        raise ValueError("cleanup target cannot be the worktree root")
    return owner_resolved, target_resolved


def _incremental_directories(target: Path) -> list[Path]:
    found = []
    if not target.is_dir():
        return found
    target_resolved = target.resolve()
    for root, directories, _files in os.walk(target, followlinks=False):
        root_path = Path(root)
        kept = []
        for name in directories:
            candidate = root_path / name
            if candidate.is_symlink():
                continue
            if name == "incremental":
                resolved = candidate.resolve()
                try:
                    resolved.relative_to(target_resolved)
                except ValueError as error:
                    raise ValueError(
                        f"incremental directory escapes target: {candidate}"
                    ) from error
                found.append(resolved)
            else:
                kept.append(name)
        directories[:] = kept
    return sorted(found, key=lambda path: os.path.normcase(str(path)))


def _remove(paths: Iterable[Path], apply: bool) -> list[Path]:
    selected = list(paths)
    if apply:
        for path in selected:
            if path.is_dir():
                shutil.rmtree(path)
    return selected


def clean_target(
    worktree: Path, target: Path, *, incremental_only: bool, apply: bool
) -> list[Path]:
    _owner, target_resolved = _validated_tree(
        worktree, target, worktree / "target"
    )
    if incremental_only:
        return _remove(_incremental_directories(target_resolved), apply)
    return _remove([target_resolved] if target_resolved.is_dir() else [], apply)


def clean_shared(
    context: RepoContext, *, incremental_only: bool, apply: bool
) -> list[Path]:
    _owner, target_resolved = _validated_tree(
        context.repository_root,
        context.shared_target,
        context.repository_root / SHARED_TARGET_RELATIVE,
    )
    if incremental_only:
        return _remove(_incremental_directories(target_resolved), apply)
    return _remove([target_resolved] if target_resolved.is_dir() else [], apply)


def _require_registered_worktree(context: RepoContext, requested: Path) -> Path:
    resolved = requested.resolve()
    if resolved not in registered_worktrees(context):
        raise ValueError(f"not a registered worktree for this repository: {requested}")
    requested_common = Path(
        _git(resolved, "rev-parse", "--path-format=absolute", "--git-common-dir")
    ).resolve()
    if requested_common != context.common_git_dir:
        raise ValueError(f"worktree belongs to another repository: {requested}")
    return resolved


def _format_gib(value: int) -> str:
    return f"{value / (1024 ** 3):.2f} GiB"


def _print_measurement(report: dict[str, object]) -> None:
    worktrees = report["worktrees"]
    assert isinstance(worktrees, list)
    for item in worktrees:
        assert isinstance(item, dict)
        print(f"{item['worktree']}: {_format_gib(int(item['bytes']))}")
        profiles = item["profiles"]
        assert isinstance(profiles, list)
        for profile in profiles:
            assert isinstance(profile, dict)
            print(
                "  "
                f"{profile['name']}: {_format_gib(int(profile['bytes']))} "
                f"(incremental {_format_gib(int(profile['incremental_bytes']))})"
            )
    shared = report["shared"]
    assert isinstance(shared, dict)
    print(f"shared {shared['path']}: {_format_gib(int(shared['bytes']))}")
    print(f"registered worktree total: {_format_gib(int(report['worktree_bytes']))}")


def _run_command(args: argparse.Namespace, context: RepoContext) -> int:
    command = list(args.command)
    if command and command[0] == "--":
        command.pop(0)
    if not command:
        raise ValueError("run requires a cargo command after --")
    executable = Path(command[0]).name.casefold()
    if executable not in {"cargo", "cargo.exe"}:
        raise ValueError("run accepts only cargo or cargo.exe commands")
    environment = agent_environment(os.environ, context.shared_target)
    print(
        f"agent cargo target={environment['CARGO_TARGET_DIR']} incremental=0",
        file=sys.stderr,
    )
    return subprocess.run(command, env=environment).returncode


def _measure_command(args: argparse.Namespace, context: RepoContext) -> int:
    report = measure_all(context)
    if args.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        _print_measurement(report)
    return 0


def _clean_command(args: argparse.Namespace, context: RepoContext) -> int:
    if args.shared:
        selected = clean_shared(
            context, incremental_only=args.incremental_only, apply=args.apply
        )
    else:
        worktree = _require_registered_worktree(context, Path(args.worktree))
        selected = clean_target(
            worktree,
            worktree / "target",
            incremental_only=args.incremental_only,
            apply=args.apply,
        )
    mode = "removed" if args.apply else "would remove"
    for path in selected:
        print(f"{mode}: {path}")
    if not selected:
        print("nothing to remove")
    if not args.apply:
        print("dry run; pass --apply to remove the listed paths")
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="subcommand", required=True)

    run_parser = subparsers.add_parser("run", help="run Cargo with the agent cache policy")
    run_parser.add_argument("command", nargs=argparse.REMAINDER)

    measure_parser = subparsers.add_parser(
        "measure", help="measure registered worktree and shared targets"
    )
    measure_parser.add_argument("--json", action="store_true")

    clean_parser = subparsers.add_parser(
        "clean", help="safely clean one verified Cargo target"
    )
    clean_target_group = clean_parser.add_mutually_exclusive_group(required=True)
    clean_target_group.add_argument("--worktree")
    clean_target_group.add_argument("--shared", action="store_true")
    clean_parser.add_argument("--incremental-only", action="store_true")
    clean_parser.add_argument("--apply", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        context = discover_context(Path.cwd())
        if args.subcommand == "run":
            return _run_command(args, context)
        if args.subcommand == "measure":
            return _measure_command(args, context)
        if args.subcommand == "clean":
            return _clean_command(args, context)
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        parser.error(str(error))
    raise AssertionError(f"unhandled subcommand: {args.subcommand}")


if __name__ == "__main__":
    raise SystemExit(main())

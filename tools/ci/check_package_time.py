"""Time one installed-toolchain package command and enforce the CI budget."""

import argparse
import os
from pathlib import Path
import subprocess
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--label", required=True)
    parser.add_argument("--max-seconds", type=float, default=120)
    parser.add_argument("--expect-output", type=Path, required=True)
    parser.add_argument("--expect-log")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or args.max_seconds <= 0:
        parser.error("a command and a positive time limit are required")
    if args.expect_output.exists():
        parser.error(f"expected output already exists: {args.expect_output}")

    started = time.perf_counter()
    try:
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            timeout=args.max_seconds,
            check=False,
        )
        output = result.stdout
        exit_code = result.returncode
        timed_out = False
    except subprocess.TimeoutExpired as error:
        output = error.stdout or b""
        if isinstance(output, bytes):
            output = output.decode(errors="replace")
        exit_code = None
        timed_out = True
    elapsed = time.perf_counter() - started
    if output:
        print(output, end="" if output.endswith("\n") else "\n")

    errors = []
    if timed_out or elapsed >= args.max_seconds:
        errors.append(f"exceeded {args.max_seconds:g}s packaging limit")
    if exit_code not in (None, 0):
        errors.append(f"package command exited with {exit_code}")
    if not args.expect_output.is_file():
        errors.append(f"expected package output missing: {args.expect_output}")
    if args.expect_log and args.expect_log not in output:
        errors.append(f"package log did not contain: {args.expect_log}")

    verdict = "PASS" if not errors else "FAIL"
    line = f"{args.label}: {elapsed:.2f}s / {args.max_seconds:g}s ({verdict})"
    print(line)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        summary_path = Path(summary)
        with open(summary, "a", encoding="utf-8") as handle:
            if not summary_path.exists() or summary_path.stat().st_size == 0:
                handle.write("| Package target | Seconds | Limit | Result |\n")
                handle.write("| --- | ---: | ---: | --- |\n")
            handle.write(f"| {args.label} | {elapsed:.2f} | {args.max_seconds:g} | {verdict} |\n")
    for error in errors:
        print(error, file=sys.stderr)
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())

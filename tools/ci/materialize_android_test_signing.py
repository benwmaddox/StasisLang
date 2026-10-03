#!/usr/bin/env python3
"""Materialize the documented CI-only Android debug keystore in runner temp."""

from __future__ import annotations

import argparse
import base64
import os
from pathlib import Path
import sys


KEYSTORE_NAME = "stasis-android-test.keystore"
INIT_SCRIPT = "tools/ci/android_test_debug_signing.init.gradle"


def materialize(
    encoded: str,
    *,
    runner_temp: Path,
    workspace: Path,
    github_env: Path,
) -> Path:
    normalized = "".join(encoded.split())
    try:
        payload = base64.b64decode(normalized, validate=True)
    except (ValueError, base64.binascii.Error) as error:
        raise ValueError("Android test keystore secret is not valid base64") from error
    if len(payload) < 16:
        raise ValueError("Android test keystore secret decoded to an implausibly small file")

    temporary_root = runner_temp.resolve()
    temporary_root.mkdir(parents=True, exist_ok=True)
    keystore = (temporary_root / KEYSTORE_NAME).resolve()
    if keystore.parent != temporary_root:
        raise ValueError("Android test keystore path escaped runner temporary storage")
    if keystore.exists():
        raise ValueError("Android test keystore already exists in runner temporary storage")
    with keystore.open("xb") as destination:
        destination.write(payload)
    keystore.chmod(0o600)

    init_script = (workspace / INIT_SCRIPT).resolve()
    if not init_script.is_file():
        keystore.unlink(missing_ok=True)
        raise ValueError(f"Android test signing init script is missing: {init_script}")
    with github_env.open("a", encoding="utf-8", newline="\n") as environment:
        environment.write(f"STASIS_ANDROID_TEST_KEYSTORE_PATH={keystore}\n")
        environment.write(f"STASIS_GRADLE_INIT_SCRIPT={init_script}\n")
    return keystore


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runner-temp", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--github-env", type=Path, required=True)
    args = parser.parse_args()
    encoded = os.environ.get("STASIS_ANDROID_TEST_KEYSTORE_BASE64", "")
    if not encoded:
        print("Android test keystore secret is unavailable", file=sys.stderr)
        return 1
    try:
        materialize(
            encoded,
            runner_temp=args.runner_temp,
            workspace=args.workspace,
            github_env=args.github_env,
        )
    except (OSError, ValueError) as error:
        print(f"Android test signing setup failed: {error}", file=sys.stderr)
        return 1
    print("Materialized the documented Android test keystore in runner temporary storage.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Verify that a staged Android test APK uses the documented debug certificate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys
from typing import Any


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))

from tools.android_release import (  # noqa: E402
    ReleaseError,
    normalize_digest,
    verify_apk_signer,
)


def verify(apk: Path, apksigner: str, expected_sha256: str, output: Path) -> dict[str, Any]:
    try:
        expected = normalize_digest(expected_sha256).lower()
    except ReleaseError as error:
        raise ValueError(
            "expected test signer SHA-256 must contain 64 hexadecimal digits"
        ) from error
    if not apk.is_file() or apk.is_symlink():
        raise ValueError("staged Android test APK is missing or is a symbolic link")
    actual = normalize_digest(verify_apk_signer(apk, apksigner)).lower()
    if actual != expected:
        raise ValueError(f"Android test APK signer differs from the documented test certificate: {actual}")
    result = {
        "schema": "stasis.android_test_signer.v1",
        "artifact": apk.name,
        "apk_sha256": hashlib.sha256(apk.read_bytes()).hexdigest(),
        "signer_sha256": actual,
        "expected_signer_sha256": expected,
        "result": "passed",
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--apksigner", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = verify(args.apk, args.apksigner, args.expected_sha256, args.output)
    except (OSError, ReleaseError, ValueError) as error:
        print(f"Android test signer verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"result": result["result"], "signer_sha256": result["signer_sha256"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

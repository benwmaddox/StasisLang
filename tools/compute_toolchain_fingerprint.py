"""Compute the shared CLI/native toolchain fingerprint."""

import argparse
import hashlib


def fingerprint(source_commit: str, release_id: str) -> str:
    payload = f"stasis-toolchain-v1\n{source_commit}\n{release_id}\n".encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--release-id", required=True)
    args = parser.parse_args()
    print(fingerprint(args.source_commit, args.release_id))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Create and verify the complete Windows release signing receipt."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath


SCHEMA = "stasis-windows-signing-v1"
REQUIRED_STASIS_FILES = {
    "stasis.exe",
    "stasis_runner.exe",
    "stasis-network-supervise.exe",
    "stasis_dynload.dll",
    "stasis_graphics.dll",
}
THIRD_PARTY_NATIVE_FILES = {
    "clang-cl.exe": "bundled LLVM toolchain",
    "lld-link.exe": "bundled LLVM toolchain",
    "sdl3.dll": "SDL runtime",
    "sdl3_image.dll": "SDL image runtime",
}


class ReceiptError(RuntimeError):
    pass


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _relative(root: Path, path: Path) -> str:
    return PurePosixPath(path.relative_to(root)).as_posix()


def _inventory(root: Path) -> tuple[list[Path], list[tuple[Path, str]]]:
    native = sorted(
        (
            path
            for path in root.rglob("*")
            if path.is_file() and path.suffix.casefold() in {".exe", ".dll"}
        ),
        key=lambda path: _relative(root, path).casefold(),
    )
    signed: list[Path] = []
    excluded: list[tuple[Path, str]] = []
    for path in native:
        reason = THIRD_PARTY_NATIVE_FILES.get(path.name.casefold())
        if reason:
            excluded.append((path, reason))
        else:
            signed.append(path)

    available = {_relative(root, path).casefold() for path in signed}
    missing = sorted(REQUIRED_STASIS_FILES - available)
    if missing:
        raise ReceiptError(f"required Stasis signing inputs are missing: {', '.join(missing)}")
    return signed, excluded


def _file_records(root: Path, paths: list[Path], hash_field: str) -> list[dict[str, str]]:
    return [
        {"path": _relative(root, path), hash_field: _sha256(path)}
        for path in paths
    ]


def _load_receipt(path: Path) -> dict:
    try:
        receipt = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReceiptError(f"could not read Windows signing receipt: {error}") from error
    if receipt.get("schema") != SCHEMA:
        raise ReceiptError("Windows signing receipt has an unsupported schema")
    return receipt


def _assert_exact_paths(root: Path, receipt: dict) -> tuple[list[Path], list[tuple[Path, str]]]:
    signed, excluded = _inventory(root)
    actual_signed = {_relative(root, path) for path in signed}
    actual_excluded = {_relative(root, path) for path, _ in excluded}
    receipt_signed = {entry.get("path") for entry in receipt.get("files", [])}
    receipt_excluded = {entry.get("path") for entry in receipt.get("excluded_files", [])}
    if actual_signed != receipt_signed:
        raise ReceiptError(
            "signed-file receipt differs from the complete native file set: "
            f"missing={sorted(actual_signed - receipt_signed)} "
            f"extra={sorted(receipt_signed - actual_signed)}"
        )
    if actual_excluded != receipt_excluded:
        raise ReceiptError(
            "third-party native exclusions differ from the receipt: "
            f"missing={sorted(actual_excluded - receipt_excluded)} "
            f"extra={sorted(receipt_excluded - actual_excluded)}"
        )
    excluded_reasons = {_relative(root, path): reason for path, reason in excluded}
    for entry in receipt.get("excluded_files", []):
        if entry.get("reason") != excluded_reasons[entry["path"]]:
            raise ReceiptError(f"third-party native exclusion reason mismatch: {entry['path']}")
    return signed, excluded


def create(root: Path, receipt_path: Path, source_commit: str) -> None:
    signed, excluded = _inventory(root)
    receipt = {
        "schema": SCHEMA,
        "status": "unsigned",
        "source_commit": source_commit,
        "files": _file_records(root, signed, "unsigned_sha256"),
        "excluded_files": [
            {
                "path": _relative(root, path),
                "sha256": _sha256(path),
                "reason": reason,
            }
            for path, reason in excluded
        ],
    }
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


def list_paths(root: Path, receipt_path: Path) -> None:
    receipt = _load_receipt(receipt_path)
    signed, _ = _assert_exact_paths(root, receipt)
    for path in signed:
        print(path.resolve())


def finalize(root: Path, receipt_path: Path, identity_path: Path, source_commit: str) -> None:
    receipt = _load_receipt(receipt_path)
    signed, _ = _assert_exact_paths(root, receipt)
    if receipt.get("status") != "unsigned" or receipt.get("source_commit") != source_commit:
        raise ReceiptError("unsigned signing receipt does not match the trusted source commit")
    try:
        identity = json.loads(identity_path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise ReceiptError(f"could not read signing identity: {error}") from error
    if not identity.get("thumbprint") or not identity.get("subject"):
        raise ReceiptError("signing identity is incomplete")
    signed_hashes = {_relative(root, path): _sha256(path) for path in signed}
    for entry in receipt["files"]:
        entry["signed_sha256"] = signed_hashes[entry["path"]]
    receipt.update(
        status="signed",
        thumbprint=identity["thumbprint"],
        subject=identity["subject"],
        self_signed=bool(identity.get("self_signed")),
    )
    receipt_path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


def verify_files(root: Path, receipt_path: Path, source_commit: str, thumbprint: str) -> None:
    receipt = _load_receipt(receipt_path)
    signed, excluded = _assert_exact_paths(root, receipt)
    if receipt.get("status") != "signed":
        raise ReceiptError("Windows signing receipt is not finalized")
    if receipt.get("source_commit") != source_commit:
        raise ReceiptError("Windows signing receipt source commit mismatch")
    if receipt.get("thumbprint", "").casefold() != thumbprint.casefold():
        raise ReceiptError("Windows signing receipt thumbprint mismatch")
    signed_by_path = {_relative(root, path): path for path in signed}
    for entry in receipt["files"]:
        if _sha256(signed_by_path[entry["path"]]) != entry.get("signed_sha256"):
            raise ReceiptError(f"signed file hash mismatch: {entry['path']}")
    excluded_by_path = {_relative(root, path): path for path, _ in excluded}
    for entry in receipt["excluded_files"]:
        if _sha256(excluded_by_path[entry["path"]]) != entry.get("sha256"):
            raise ReceiptError(f"excluded third-party file hash mismatch: {entry['path']}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("create", "list", "finalize", "verify-files"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--source-commit")
    parser.add_argument("--identity", type=Path)
    parser.add_argument("--thumbprint")
    args = parser.parse_args()
    root = args.root.resolve()
    try:
        if args.command == "create":
            if not args.source_commit:
                parser.error("create requires --source-commit")
            create(root, args.receipt, args.source_commit)
        elif args.command == "list":
            list_paths(root, args.receipt)
        elif args.command == "finalize":
            if not args.source_commit or not args.identity:
                parser.error("finalize requires --source-commit and --identity")
            finalize(root, args.receipt, args.identity, args.source_commit)
        else:
            if not args.source_commit or not args.thumbprint:
                parser.error("verify-files requires --source-commit and --thumbprint")
            verify_files(root, args.receipt, args.source_commit, args.thumbprint)
    except ReceiptError as error:
        parser.exit(1, f"error: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

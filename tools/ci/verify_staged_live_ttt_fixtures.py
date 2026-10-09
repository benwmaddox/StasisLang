"""Validate the byte-pinned upstream TTT fixtures in Git's index.

The pre-commit hook uses the printed paths as the complete formatting
exemption. The allowlist and hashes are deliberately repeated here as trusted
policy, so editing the adjacent provenance manifest cannot authorize new
bytes.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path


FIXTURE_ROOT = "tests/fixtures/network_supervision/live_ttt/"
MANIFEST_PATH = FIXTURE_ROOT + "provenance.json"
ATTRIBUTES_PATH = FIXTURE_ROOT + "authority/src/.gitattributes"
SOURCE_HASHES = {
    FIXTURE_ROOT + "authority/src/protocol.stasis": "20fe9ed589fe878f975ab2fa29a3820efe55b6c7c8820a632879984bf5d283c9",
    FIXTURE_ROOT + "authority/src/tic_tac_toe.stasis": "e290d38cacb3384e6adf11f26c1c0f8c16e6ad48b6fd3e11a14b89ef7ab37da3",
    FIXTURE_ROOT + "authority/src/terminal_flow.stasis": "13b87cdbaaf5db3d18e6072f2b3d608e2a8b1d292b13bde67a0975e18b32b62e",
    FIXTURE_ROOT + "authority/src/network_protocol.stasis": "a9eb9377df74d56c635636bf79d6b450913bd1130471d6643699d6f65fd12a9f",
}
PINNED_PATHS = set(SOURCE_HASHES) | {MANIFEST_PATH, ATTRIBUTES_PATH}
EXPECTED_ATTRIBUTES = (
    b"# These four files are byte-frozen upstream fixtures; preserve their CRLF bytes.\n"
    b"protocol.stasis -text whitespace=cr-at-eol\n"
    b"tic_tac_toe.stasis -text whitespace=cr-at-eol\n"
    b"terminal_flow.stasis -text whitespace=cr-at-eol\n"
    b"network_protocol.stasis -text whitespace=cr-at-eol\n"
)
EXPECTED_MANIFEST = {
    "schema_version": 1,
    "upstream_repository": "maddox-and-friends",
    "upstream_commit": "cb43e41a2c0a9c1f1123365885f731a35184a351",
    "copied_files": [
        {"path": "src/protocol.stasis", "sha256": SOURCE_HASHES[FIXTURE_ROOT + "authority/src/protocol.stasis"]},
        {"path": "src/tic_tac_toe.stasis", "sha256": SOURCE_HASHES[FIXTURE_ROOT + "authority/src/tic_tac_toe.stasis"]},
        {"path": "src/terminal_flow.stasis", "sha256": SOURCE_HASHES[FIXTURE_ROOT + "authority/src/terminal_flow.stasis"]},
        {"path": "src/network_protocol.stasis", "sha256": SOURCE_HASHES[FIXTURE_ROOT + "authority/src/network_protocol.stasis"]},
    ],
    "transformations": [],
    "upstream_license_or_notice_files": [],
}


class VerificationError(RuntimeError):
    """The staged fixture set is missing, modified, or no longer pinned."""


def _git(repo_root: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise VerificationError("could not inspect the staged fixture index")
    return result.stdout


def _index_blob(repo_root: Path, path: str) -> bytes:
    return _git(repo_root, "cat-file", "blob", f":{path}")


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise VerificationError("the staged provenance manifest has duplicate keys")
        result[key] = value
    return result


def verify_staged_fixtures(repo_root: Path) -> list[str]:
    """Return the exact formatter exemptions after validating staged bytes.

    If none of the pinned paths is staged, return an empty list. Any touched
    pinned path activates full manifest, attributes, and blob verification.
    """
    staged_paths = {
        path.decode("utf-8")
        for path in _git(
            repo_root,
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACMRD",
            "--no-renames",
            "-z",
        ).split(b"\0")
        if path
    }
    if not staged_paths.intersection(PINNED_PATHS):
        return []

    try:
        manifest = json.loads(
            _index_blob(repo_root, MANIFEST_PATH),
            object_pairs_hook=_reject_duplicate_keys,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, VerificationError) as error:
        raise VerificationError("the staged provenance manifest is invalid") from error
    if manifest != EXPECTED_MANIFEST:
        raise VerificationError("the staged provenance manifest differs from the trusted pin")

    if _index_blob(repo_root, ATTRIBUTES_PATH) != EXPECTED_ATTRIBUTES:
        raise VerificationError("the staged fixture attributes differ from the trusted pin")

    for path, expected_hash in SOURCE_HASHES.items():
        actual_hash = hashlib.sha256(_index_blob(repo_root, path)).hexdigest()
        if actual_hash != expected_hash:
            raise VerificationError(f"staged fixture bytes differ from the trusted pin: {path}")

    return list(SOURCE_HASHES)


def main() -> int:
    repo_root = Path(__file__).resolve().parents[2]
    try:
        for path in verify_staged_fixtures(repo_root):
            print(path)
    except VerificationError as error:
        print(f"Stasis pre-commit: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

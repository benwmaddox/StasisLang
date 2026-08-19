"""Verify one real Java/JNI/Rust-JIT/GLES Workshop frame from log evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


COMPILE = re.compile(
    r"CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=(\d+)"
)
MARKER = re.compile(r"Stasis Workshop IT-025: (\{[^\r\n]+\})")
PRESENT = re.compile(r"Stasis Workshop IT-025 GLES: (\{[^\r\n]+\})")
FRAME = re.compile(r"RenderAcceptanceFrame: count=(\d+) frame_token=(\d+)")
FORBIDDEN = re.compile(
    r"(?:CompileError|native preview frame failed|FATAL EXCEPTION|stub path|fallback path|IT-025 state checksum was unavailable)",
    re.IGNORECASE,
)


class SeamError(RuntimeError):
    pass


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _read_json(path: Path) -> dict:
    """Read JSON emitted by both BOM-free and Windows PowerShell 5.1 writers."""
    return json.loads(path.read_text(encoding="utf-8-sig"))


def verify_log(log: str, manifest: dict, *, minimum_frames: int = 30) -> dict:
    compile_matches = COMPILE.findall(log)
    if not compile_matches or max(map(int, compile_matches)) <= 0:
        raise SeamError("missing successful non-empty CompileReady JIT evidence")
    if FORBIDDEN.search(log):
        raise SeamError("Workshop log contains a forbidden fallback, stub, or fatal diagnostic")
    markers = []
    for match in MARKER.finditer(log):
        try:
            marker = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-025 marker JSON: {error}") from error
        if marker.get("test_id") == "IT-025":
            markers.append((match, marker))
    if not markers:
        raise SeamError("missing IT-025 native frame markers")
    presentations = []
    for match in PRESENT.finditer(log):
        try:
            presentation = json.loads(match.group(1))
            if presentation.get("test_id") == "IT-025":
                presentations.append((match, presentation))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-025 GLES marker JSON: {error}") from error
    stable_presentations = [
        item for item in presentations
        if item[1].get("event") == "present"
        and isinstance(item[1].get("count"), int)
        and isinstance(item[1].get("frame_token"), int)
        and item[1]["count"] >= minimum_frames
    ]
    if not stable_presentations:
        raise SeamError("missing stable IT-025 GLES presentation marker")
    presentation_match, presentation = max(stable_presentations, key=lambda item: item[1]["count"])
    frames = [(int(count), int(token)) for count, token in FRAME.findall(log)]
    stable_count = presentation["count"]
    stable_token = presentation["frame_token"]
    if not any(count == stable_count and token == stable_token for count, token in frames):
        raise SeamError("stable GLES presentation did not match its RenderAcceptanceFrame token")
    native_match, marker = next(
        ((candidate_match, candidate) for candidate_match, candidate in reversed(markers)
         if candidate.get("frame_token") == stable_token
         and candidate_match.start() < presentation_match.start()),
        (None, None),
    )
    if native_match is None or marker is None:
        raise SeamError("stable GLES presentation did not consume a preceding native frame token")
    expected = {
        "state_checksum": manifest["state_checksum"],
        "command_trace": manifest["workshop_command_trace"],
        "render_version": manifest["render_contract_version"],
    }
    for key, value in expected.items():
        if marker.get(key) != value:
            raise SeamError(f"IT-025 {key} mismatch: expected={value} actual={marker.get(key)}")
    if marker.get("schema") != "stasis.workshop_seam.v1" or marker.get("event") != "frame":
        raise SeamError("IT-025 marker schema/event is not the Workshop frame contract")
    if not isinstance(marker.get("rust_bridge_version"), str) or not marker["rust_bridge_version"]:
        raise SeamError("IT-025 marker lacks the real Rust bridge version")
    if not isinstance(marker.get("jni_version"), int) or marker["jni_version"] <= 0:
        raise SeamError("IT-025 marker lacks the JNI runtime version")
    if marker.get("fallback") != 0 or marker.get("stub") != 0:
        raise SeamError("IT-025 marker reports a fallback or stub")
    return {
        "compile_functions": max(map(int, compile_matches)),
        "presented_frames": stable_count,
        "frame_token": marker["frame_token"],
        "stable_frame_token": stable_token,
        "presentation": presentation,
        "marker": marker,
    }


def verify_files(log_path: Path, capture: Path, manifest_path: Path, apk: Path,
                 metadata_path: Path, evidence_path: Path) -> dict:
    for path in (log_path, capture, manifest_path, apk, metadata_path):
        if not path.is_file():
            raise SeamError(f"required Workshop evidence file is missing: {path}")
    manifest = _read_json(manifest_path)
    metadata = _read_json(metadata_path)
    result = verify_log(log_path.read_text(encoding="utf-8", errors="replace"), manifest)
    evidence = {
        "schema": "stasis.workshop_seam.evidence.v1",
        "test_id": "IT-025",
        "status": "passed",
        "source_revision": metadata.get("git_revision", ""),
        "apk_sha256": _sha256(apk),
        "capture_sha256": _sha256(capture),
        "metadata": metadata,
        **result,
    }
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    try:
        evidence = verify_files(args.log, args.capture, args.manifest, args.apk, args.metadata, args.evidence)
    except (OSError, json.JSONDecodeError, SeamError) as error:
        parser.error(str(error))
    print(json.dumps({"status": evidence["status"], "evidence": str(args.evidence)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

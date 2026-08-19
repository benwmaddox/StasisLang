"""Verify one real Java/JNI/Rust-JIT/GLES Workshop frame from log evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

try:
    from .check_runtime_abi_contract import c_constants
except ImportError:
    from check_runtime_abi_contract import c_constants


COMPILE = re.compile(
    r"CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=(\d+)"
)
MARKER = re.compile(r"Stasis Workshop IT-025: (\{[^\r\n]+\})")
PRESENT = re.compile(r"Stasis Workshop IT-025 GLES: (\{[^\r\n]+\})")
ABI_MARKER = re.compile(r"Stasis Workshop IT-026: (\{[^\r\n]+\})")
ABI_CASE_MARKER = re.compile(r"Stasis Workshop IT-026 case: (\{[^\r\n]+\})")
FRAME = re.compile(r"RenderAcceptanceFrame: count=(\d+) frame_token=(\d+)")
FORBIDDEN = re.compile(
    r"(?:CompileError|native preview frame failed|FATAL EXCEPTION|stub path|fallback path|IT-025 state checksum was unavailable)",
    re.IGNORECASE,
)
EXPECTED_INVALID = {
    "short_i32": ("i32", "capacity", "short"), "short_f32": ("f32", "capacity", "short"),
    "short_u8": ("u8", "capacity", "short"), "oversized_i32": ("i32", "capacity", "oversized"),
    "oversized_f32": ("f32", "capacity", "oversized"), "oversized_u8": ("u8", "capacity", "oversized"),
    "swapped_i32_f32": ("i32", "capacity", "swapped"),
    "wrong_order_i32": ("i32", "byte_order", "order"), "wrong_order_f32": ("f32", "byte_order", "order"),
    "wrong_order_u8": ("u8", "byte_order", "order"), "heap_i32": ("i32", "not_direct", "heap"),
    "heap_f32": ("f32", "not_direct", "heap"), "heap_u8": ("u8", "not_direct", "heap"),
    "null_i32": ("i32", "null_buffer", "null"), "null_f32": ("f32", "null_buffer", "null"),
    "null_u8": ("u8", "null_buffer", "null"), "misaligned_i32": ("i32", "alignment", "alignment"),
    "misaligned_f32": ("f32", "alignment", "alignment"),
}


def canonical_frame_descriptor() -> dict[str, dict[str, int]]:
    """Resolve the JNI descriptor from the render header's canonical counts."""
    render = c_constants((Path(__file__).resolve().parents[2] / "runtime/stasis_render_contract.h").read_text())
    sizes = {"i32": ("STASIS_RENDER_I32_COUNT", 4, 4),
             "f32": ("STASIS_RENDER_F32_COUNT", 4, 4),
             "u8": ("STASIS_RENDER_U8_COUNT", 1, 1)}
    return {lane: {"bytes": render[count] * size, "alignment": alignment}
            for lane, (count, size, alignment) in sizes.items()}


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
    abi_markers = []
    for match in ABI_MARKER.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-026 marker JSON: {error}") from error
        if candidate.get("test_id") == "IT-026":
            abi_markers.append((match, candidate))
    if len(abi_markers) != 1:
        raise SeamError(f"expected exactly one IT-026 summary, found {len(abi_markers)}")
    abi_match, abi = abi_markers[0]
    case_candidates = []
    for match in ABI_CASE_MARKER.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-026 case JSON: {error}") from error
        if candidate.get("test_id") == "IT-026" and candidate.get("event") == "case":
            case_candidates.append((match, candidate))
    stale_cases = [candidate for match, candidate in case_candidates if match.start() < abi_match.start()]
    if stale_cases:
        raise SeamError("IT-026 case marker occurred before its summary")
    case_markers = [candidate for match, candidate in case_candidates if match.start() > abi_match.start()]
    if len(case_markers) != len(EXPECTED_INVALID):
        raise SeamError(f"expected exactly {len(EXPECTED_INVALID)} IT-026 cases after summary, found {len(case_markers)}")
    if abi.get("schema") != "stasis.workshop_jni_frame_abi.v1" \
            or abi.get("event") != "buffer_abi" \
            or abi.get("status") != "passed":
        raise SeamError("IT-026 marker does not report a passed buffer ABI acceptance")
    canonical_descriptor = canonical_frame_descriptor()
    descriptor = abi.get("descriptor")
    if not isinstance(descriptor, dict) or not isinstance(descriptor.get("lanes"), list):
        raise SeamError("IT-026 marker lacks canonical lane descriptor")
    lanes = descriptor["lanes"]
    if len(lanes) != 3 or any(not isinstance(lane, dict) for lane in lanes):
        raise SeamError("IT-026 descriptor must contain exactly three lanes")
    lane_names = [lane.get("lane") for lane in lanes]
    if lane_names != ["i32", "f32", "u8"] or len(set(lane_names)) != 3:
        raise SeamError("IT-026 descriptor lanes are not ordered and unique")
    observed_descriptor = {
        lane["lane"]: {"bytes": lane.get("bytes"), "alignment": lane.get("alignment")}
        for lane in lanes
    }
    if observed_descriptor != canonical_descriptor:
        raise SeamError(f"IT-026 lane descriptor mismatch: expected={canonical_descriptor} actual={observed_descriptor}")
    if abi.get("valid_calls") != 1 or abi.get("invalid_calls") != len(EXPECTED_INVALID) \
            or len(case_markers) != len(EXPECTED_INVALID):
        raise SeamError("IT-026 marker lacks exact and invalid JNI buffer scenarios")
    if abi.get("valid_guards_intact") is not True or abi.get("all_invalid_unchanged") is not True:
        raise SeamError("IT-026 marker lacks guard/canary preservation proof")
    invalid = case_markers
    observed_scenarios = set()
    for scenario in invalid:
        if not isinstance(scenario, dict) or not isinstance(scenario.get("name"), str):
            raise SeamError("IT-026 invalid scenario is not structured")
        observed_scenarios.add(scenario["name"])
        if scenario.get("unchanged") is not True:
            raise SeamError(f"IT-026 {scenario['name']} did not prove unchanged buffers")
        error = scenario.get("error")
        if scenario["name"] not in EXPECTED_INVALID:
            raise SeamError(f"IT-026 unexpected scenario {scenario['name']}")
        lane, reason, kind = EXPECTED_INVALID[scenario["name"]]
        if not isinstance(error, dict) or error.get("schema") != "stasis.workshop_jni_frame_abi.v1" \
                or error.get("test_id") != "IT-026" or error.get("event") != "error" \
                or (error.get("lane"), error.get("reason")) != (lane, reason) \
                or not all(isinstance(error.get(field), (str, int))
                           for field in ("lane", "reason", "expected", "actual")):
            raise SeamError(f"IT-026 {scenario['name']} lacks lane/reason/expected/actual error")
        expected_bytes = canonical_descriptor[lane]["bytes"]
        if kind == "short" and (error.get("expected"), error.get("actual")) != (expected_bytes, expected_bytes - 1):
            raise SeamError(f"IT-026 {scenario['name']} capacity proof is incorrect")
        if kind == "oversized" and (error.get("expected"), error.get("actual")) != (expected_bytes, expected_bytes + 1):
            raise SeamError(f"IT-026 {scenario['name']} capacity proof is incorrect")
        if kind == "swapped" and (error.get("expected"), error.get("actual")) != (expected_bytes, canonical_descriptor["f32"]["bytes"]):
            raise SeamError(f"IT-026 {scenario['name']} swap proof is incorrect")
        if kind in ("heap", "null") and (error.get("expected"), error.get("actual")) != (expected_bytes, -1):
            raise SeamError(f"IT-026 {scenario['name']} pointer proof is incorrect")
        if kind == "order" and (error.get("expected"), error.get("actual")) != ("native", "non_native"):
            raise SeamError(f"IT-026 {scenario['name']} byte-order proof is incorrect")
        if kind == "alignment" and (error.get("expected"), error.get("actual")) != (canonical_descriptor[lane]["alignment"], 1):
            raise SeamError(f"IT-026 {scenario['name']} alignment proof is incorrect")
    if observed_scenarios != set(EXPECTED_INVALID):
        raise SeamError(f"IT-026 scenarios mismatch: expected={sorted(EXPECTED_INVALID)} actual={sorted(observed_scenarios)}")
    return {
        "compile_functions": max(map(int, compile_matches)),
        "presented_frames": stable_count,
        "frame_token": marker["frame_token"],
        "stable_frame_token": stable_token,
        "presentation": presentation,
        "marker": marker,
        "it026": abi,
        "it026_cases": invalid,
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

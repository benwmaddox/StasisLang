"""Verify one real Java/JNI/Rust-JIT/GLES Workshop frame from log evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path
from urllib.parse import quote

try:
    from .check_runtime_abi_contract import c_constants
    from .verify_render_parity import read_capture
except ImportError:
    from check_runtime_abi_contract import c_constants
    from verify_render_parity import read_capture


COMPILE = re.compile(
    r"CompileReady: backend=cranelift-jit reload=InitialCompile status=0 functions=(\d+)"
)
MARKER = re.compile(r"Stasis Workshop IT-025: (\{[^\r\n]+\})")
PRESENT = re.compile(r"Stasis Workshop IT-025 GLES: (\{[^\r\n]+\})")
ABI_MARKER = re.compile(r"Stasis Workshop IT-026: (\{[^\r\n]+\})")
ABI_CASE_MARKER = re.compile(r"Stasis Workshop IT-026 case: (\{[^\r\n]+\})")
TOUCH_MARKER = re.compile(r"Stasis Workshop IT-027: (\{[^\r\n]+\})")
TOUCH_CASE_MARKER = re.compile(r"Stasis Workshop IT-027 case: (\{[^\r\n]+\})")
TOUCH_PRESENT = re.compile(r"Stasis Workshop IT-027 GLES: (\{[^\r\n]+\})")
ACCEPTANCE_MARKER_ORDER_COUNT = 8
HOT_EDIT_MARKER = re.compile(r"Stasis Workshop IT-028: (\{[^\r\n]+\})")
HOT_EDIT_CASE_MARKER = re.compile(r"Stasis Workshop IT-028 case: (\{[^\r\n]+\})")
HOT_EDIT_PRESENT = re.compile(r"Stasis Workshop IT-028 GLES: (\{[^\r\n]+\})")
RESOURCE_SCOPE_CASE_MARKER = re.compile(
    r"Stasis Workshop IT-029 case: (\{[^\r\n]+\})"
)
RESOURCE_SCOPE_MARKER = re.compile(r"Stasis Workshop IT-029: (\{[^\r\n]+\})")
TEST_RUNNER_CASE_MARKER = re.compile(r"Stasis Workshop IT-030 case: (\{[^\r\n]+\})")
TEST_RUNNER_MARKER = re.compile(r"Stasis Workshop IT-030: (\{[^\r\n]+\})")
DIAGNOSTIC_CASE_MARKER = re.compile(r"Stasis Workshop IT-031 case: (\{[^\r\n]+\})")
DIAGNOSTIC_MARKER = re.compile(r"Stasis Workshop IT-031: (\{[^\r\n]+\})")
COMPILE_ERROR_LINE = re.compile(r"^[^\r\n]*CompileError[^\r\n]*\r?$", re.MULTILINE)
RAW_COMPILE_ERROR_LINE = re.compile(
    r"^(?:(?:\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}\.\d{3}\s+\d+\s+\d+\s+"
    r"[VDIWEF]\s+StasisWorkshop:\s+)?(?P<payload>CompileError: [^\r\n]+))\r?$"
)
FRAME = re.compile(r"RenderAcceptanceFrame: count=(\d+) frame_token=(\d+)")
FORBIDDEN = re.compile(
    r"(?:native preview frame failed|FATAL EXCEPTION|stub path|fallback path|IT-025 state checksum was unavailable|IT-028 cleanup failed)",
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


def _rust_percent_encode(value: str) -> str:
    return quote(value, safe="-_.~/")


def _json_markers(pattern: re.Pattern[str], log: str, label: str,
                  test_id: str = "IT-028") -> list[tuple[re.Match[str], dict]]:
    markers = []
    for match in pattern.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid {label} JSON: {error}") from error
        if candidate.get("test_id") == test_id:
            markers.append((match, candidate))
    return markers


def verify_it028(log: str, after_position: int) -> dict:
    summaries = _json_markers(HOT_EDIT_MARKER, log, "IT-028 marker")
    if len(summaries) != 1:
        raise SeamError(f"expected exactly one IT-028 summary, found {len(summaries)}")
    summary_match, summary = summaries[0]
    if summary_match.start() <= after_position:
        raise SeamError("IT-028 summary must follow IT-027")
    cases = _json_markers(HOT_EDIT_CASE_MARKER, log, "IT-028 case")
    presents = _json_markers(HOT_EDIT_PRESENT, log, "IT-028 GLES marker")
    if len(cases) != 3 or len(presents) != 3:
        raise SeamError(
            f"expected exactly 3 IT-028 cases and GLES markers, found {len(cases)} and {len(presents)}"
        )
    if any(match.start() <= after_position or match.start() >= summary_match.start()
           for match, _ in cases + presents):
        raise SeamError("IT-028 evidence must follow IT-027 and precede its summary")
    if summary.get("schema") != "stasis.workshop_hot_edit.v1" \
            or summary.get("event") != "hot_edit" \
            or summary.get("status") != "passed" \
            or summary.get("ordered") is not True \
            or summary.get("unique") is not True \
            or summary.get("atomic") is not True:
        raise SeamError("IT-028 marker does not report a passed atomic hot edit")
    expected_phases = [("baseline", 1, 1), ("published", 2, 2), ("post_invalid", 3, 2)]
    observed_tokens = []
    observed_traces = []
    observed_generations = []
    observed_sources = []
    for (case_match, case), (phase, sequence, expected_revision) in zip(cases, expected_phases):
        if case.get("schema") != "stasis.workshop_hot_edit.v1" \
                or case.get("event") != "case" \
                or case.get("status") != "passed" \
                or (case.get("phase"), case.get("sequence")) != (phase, sequence):
            raise SeamError("IT-028 cases are not the required ordered phases")
        runtime = case.get("runtime")
        guest = case.get("guest")
        render = case.get("render")
        if not isinstance(runtime, dict) or not isinstance(guest, dict) or not isinstance(render, dict):
            raise SeamError(f"IT-028 {phase} lacks runtime, guest, or render evidence")
        generation = runtime.get("generation")
        source = runtime.get("source_fingerprint")
        if not isinstance(generation, int) or generation <= 0 or not isinstance(source, str) or not source:
            raise SeamError(f"IT-028 {phase} lacks active generation/source identity")
        observed_generations.append(generation)
        observed_sources.append(source)
        if (guest.get("tick_revision"), guest.get("render_revision")) != (
                expected_revision, expected_revision):
            raise SeamError(f"IT-028 {phase} tick/render revisions are mixed or stale")
        if guest.get("state_counter") != sequence:
            raise SeamError(f"IT-028 {phase} guest state was not migrated: expected {sequence}")
        token = render.get("frame_token")
        trace = render.get("trace")
        marker = render.get("marker")
        if not isinstance(token, int) or token <= 0 or token in observed_tokens \
                or not isinstance(trace, int) or trace <= 0 or not isinstance(marker, dict):
            raise SeamError(f"IT-028 {phase} lacks unique direct-buffer evidence")
        observed_tokens.append(token)
        observed_traces.append(trace)
        expected_marker = {
            "x": 48.0 + expected_revision * 64.0,
            "y": 48.0,
            "w": 24.0,
            "h": 24.0,
            "r": 0.2,
            "g": 0.9,
            "b": 0.95,
            "a": 1.0,
        }
        if marker.get("active") is not True:
            raise SeamError(f"IT-028 {phase} lacks active marker evidence")
        for key, expected in expected_marker.items():
            if not isinstance(marker.get(key), (int, float)) \
                    or abs(marker[key] - expected) > 0.01:
                raise SeamError(f"IT-028 {phase} marker geometry/revision mismatch")
        if case.get("gles_presented") is not True \
                or case.get("gles_frame_token") != token \
                or case.get("java_only") is not False \
                or case.get("fallback") != 0 or case.get("stub") != 0:
            raise SeamError(f"IT-028 {phase} lacks native GLES/token proof")
    if observed_generations != [observed_generations[0], observed_generations[0] + 1,
                                observed_generations[0] + 1]:
        raise SeamError("IT-028 generations did not prove one publication boundary")
    if observed_sources[0] == observed_sources[1] or observed_sources[1] != observed_sources[2]:
        raise SeamError("IT-028 source identities did not prove rollback to accepted code")
    if observed_traces[0] == observed_traces[1] or observed_traces[1] != observed_traces[2]:
        raise SeamError("IT-028 traces did not prove compatible accepted/post-invalid code")
    if observed_tokens != sorted(observed_tokens):
        raise SeamError("IT-028 frame tokens are not strictly ordered")
    diagnostic = summary.get("invalid_compile")
    structured = diagnostic.get("diagnostic") if isinstance(diagnostic, dict) else None
    if not isinstance(diagnostic, dict) or diagnostic.get("ok") is not False \
            or diagnostic.get("kind") != "compile_error" or "raw" in diagnostic \
            or not isinstance(structured, dict):
        raise SeamError("IT-028 invalid edit lacks an isolated structured diagnostic")
    hook_source_line = summary.get("hook_source_line")
    expected_diagnostic = {
        "file": "src/main.stasis",
        "line": hook_source_line,
        "column": 31,
        "end_line": hook_source_line + 2,
        "end_column": 2,
        "symbol": "on_code_swap",
        "message": "cannot resolve call 'IT028_missing_target'",
    }
    if not isinstance(hook_source_line, int) or hook_source_line <= 0 \
            or set(structured) != set(expected_diagnostic) \
            or any(structured.get(key) != value for key, value in expected_diagnostic.items()):
        raise SeamError("IT-028 invalid edit diagnostic is incomplete")
    receipt = summary.get("restore_receipt")
    if not isinstance(receipt, dict) or receipt.get("status") != "NoChange" \
            or not isinstance(receipt.get("compile"), str) \
            or not receipt["compile"].startswith("CompileReady") \
            or "reload=NoChange" not in receipt["compile"] \
            or "status=0" not in receipt["compile"]:
        raise SeamError("IT-028 accepted-source restore receipt is not exact NoChange")
    cleanup = summary.get("cleanup_receipt")
    cleanup_frame = cleanup.get("frame") if isinstance(cleanup, dict) else None
    cleanup_runtime = cleanup_frame.get("runtime") if isinstance(cleanup_frame, dict) else None
    cleanup_render = cleanup_frame.get("render") if isinstance(cleanup_frame, dict) else None
    cleanup_marker = cleanup_render.get("marker") if isinstance(cleanup_render, dict) else None
    if not isinstance(cleanup, dict) or cleanup.get("status") != "Restored" \
            or not isinstance(cleanup.get("compile"), str) \
            or not cleanup["compile"].startswith("CompileReady") \
            or "status=0" not in cleanup["compile"] \
            or not isinstance(cleanup_frame, dict) or cleanup_frame.get("status") != "passed" \
            or cleanup_frame.get("java_only") is not False \
            or cleanup_frame.get("fallback") != 0 or cleanup_frame.get("stub") != 0 \
            or not isinstance(cleanup_runtime, dict) \
            or cleanup_runtime.get("generation") != observed_generations[1] + 1 \
            or cleanup_runtime.get("source_fingerprint") != observed_sources[0] \
            or not isinstance(cleanup_marker, dict) or cleanup_marker.get("active") is not False:
        raise SeamError("IT-028 cleanup did not prove restored packaged source/frame")
    present_positions = [match.start() for match, _ in presents]
    case_positions = [match.start() for match, _ in cases]
    for index, ((present_match, present), expected_token, expected_trace, expected_marker) in enumerate(
            zip(presents, observed_tokens, observed_traces,
                [case["render"]["marker"] for _, case in cases])):
        if present_match.start() >= case_positions[index]:
            raise SeamError("IT-028 GLES marker must precede its matching case")
        if present.get("schema") != "stasis.workshop_hot_edit.v1" \
                or present.get("event") != "present" \
                or present.get("frame_token") != expected_token \
                or present.get("trace") != expected_trace \
                or present.get("rect_count") != 2 \
                or present.get("order_count") != ACCEPTANCE_MARKER_ORDER_COUNT:
            raise SeamError("IT-028 GLES marker did not match its exact token/trace")
        presented_marker = present.get("marker")
        if not isinstance(presented_marker, dict) or presented_marker.get("active") is not True:
            raise SeamError("IT-028 GLES marker lacks active evidence")
        for key in ("x", "y", "w", "h", "r", "g", "b", "a"):
            if not isinstance(presented_marker.get(key), (int, float)) \
                    or abs(presented_marker[key] - expected_marker[key]) > 0.01:
                raise SeamError("IT-028 GLES marker geometry/color mismatch")
    interleaved = sorted(
        [(present_positions[index], "present") for index in range(3)]
        + [(case_positions[index], "case") for index in range(3)])
    if [kind for _, kind in interleaved] != ["present", "case"] * 3:
        raise SeamError("IT-028 GLES and case evidence is not strictly interleaved")
    # IT-031 deliberately emits later runtime/resource failures and includes
    # displayed diagnostic text in its JSON summary. Only IT-028's own evidence
    # window may satisfy the exact raw compile-error proof.
    raw_compile_error_lines = list(COMPILE_ERROR_LINE.finditer(
        log, after_position, summary_match.start()))
    if len(raw_compile_error_lines) != 1:
        raise SeamError(
            f"expected exactly one raw CompileError line, found {len(raw_compile_error_lines)}"
        )
    raw_expected = (
        f"CompileError: {structured['file']}: {structured['message']}"
        f"|diagnostic_file={_rust_percent_encode(structured['file'])}"
        f"|diagnostic_line={structured['line']}"
        f"|diagnostic_column={structured['column']}"
        f"|diagnostic_end_line={structured['end_line']}"
        f"|diagnostic_end_column={structured['end_column']}"
        f"|diagnostic_symbol={_rust_percent_encode(structured['symbol'])}"
        f"|diagnostic_message={_rust_percent_encode(structured['message'])}"
    )
    raw_line = raw_compile_error_lines[0]
    raw_match = RAW_COMPILE_ERROR_LINE.fullmatch(raw_line.group(0))
    raw_start = raw_line.start() + (raw_match.start("payload") if raw_match else 0)
    if raw_match is None or raw_match.group("payload") != raw_expected \
            or raw_start <= cases[1][0].start() \
            or raw_start >= presents[2][0].start():
        raise SeamError("raw CompileError diagnostic was missing, truncated, or out of order")
    return {
        "summary": summary,
        "cases": [candidate for _, candidate in cases],
        "gles": [candidate for _, candidate in presents],
        "_position": summary_match.start(),
    }


def verify_it029(log: str, after_position: int) -> dict:
    summaries = _json_markers(
        RESOURCE_SCOPE_MARKER, log, "IT-029 marker", "IT-029"
    )
    if len(summaries) != 1:
        raise SeamError(f"expected exactly one IT-029 summary, found {len(summaries)}")
    summary_match, summary = summaries[0]
    cases = _json_markers(
        RESOURCE_SCOPE_CASE_MARKER, log, "IT-029 case", "IT-029"
    )
    if len(cases) != 4:
        raise SeamError(f"expected exactly 4 IT-029 cases, found {len(cases)}")
    if summary_match.start() <= after_position or any(
        match.start() <= after_position or match.start() >= summary_match.start()
        for match, _ in cases
    ):
        raise SeamError("IT-029 cases must follow IT-028 and precede their summary")
    expected_phases = [
        "project_a_first",
        "project_b_before_recreation",
        "project_b_after_recreation",
        "project_a_return",
    ]
    values = [case for _, case in cases]
    if [case.get("phase") for case in values] != expected_phases \
            or [case.get("sequence") for case in values] != [1, 2, 3, 4]:
        raise SeamError("IT-029 cases are missing or reordered")
    required_summary = {
        "schema": "stasis.workshop_resource_scope.v1",
        "test_id": "IT-029",
        "event": "resource_scope",
        "status": "passed",
        "ordered": True,
        "same_handles": True,
        "distinct_projects": True,
        "distinct_assets": True,
        "surface_recreated": True,
        "restore_once": True,
        "bounded": True,
    }
    if any(summary.get(key) != expected for key, expected in required_summary.items()):
        raise SeamError("IT-029 summary does not report the complete scoped restore proof")
    cleanup = summary.get("cleanup", {})
    if cleanup.get("status") != "Restored" \
            or cleanup.get("frame_status") != "passed" \
            or not isinstance(cleanup.get("frame_token"), int) \
            or cleanup["frame_token"] <= 0:
        raise SeamError("IT-029 cleanup did not restore the packaged project")
    if "cases" in summary:
        raise SeamError("IT-029 summary must keep case evidence in bounded log records")

    for case in values:
        if case.get("schema") != "stasis.workshop_resource_scope.v1" \
                or case.get("event") != "case" or case.get("status") != "passed" \
                or case.get("gles_presented") is not True \
                or case.get("java_only") is not False \
                or case.get("fallback") != 0 or case.get("stub") != 0:
            raise SeamError("IT-029 case lacks real JNI/GLES presentation evidence")
        if not isinstance(case.get("frame_token"), int) or case["frame_token"] <= 0:
            raise SeamError("IT-029 case lacks a positive frame token")
        command_trace = case.get("command_trace")
        if type(command_trace) is not int or not 0 <= command_trace <= 0xFFFFFFFF:
            raise SeamError("IT-029 command_trace is not an unsigned 32-bit integer")
        for field in ("sprite_handles", "font_handles", "cached_text_handles"):
            handles = case.get(field)
            if not isinstance(handles, list) or not handles \
                    or any(not isinstance(handle, int) or handle == 0 for handle in handles):
                raise SeamError(f"IT-029 {field} lacks stable nonzero handles")
        for field in ("direct_text_sha256", "capture_sha256"):
            if not re.fullmatch(r"[0-9a-f]{64}", str(case.get(field, ""))):
                raise SeamError(f"IT-029 {field} is not exact SHA-256 evidence")
        if not str(case.get("capture_path", "")).endswith(".png"):
            raise SeamError("IT-029 capture evidence is missing a PNG path")
        resources = case.get("resources")
        if not isinstance(resources, dict) \
                or resources.get("project_root") != case.get("project_root") \
                or resources.get("resources_ready") is not True:
            raise SeamError("IT-029 resource snapshot is not bound to its active project")
        identities = resources.get("identities")
        required_identity_kinds = ("sprite:", "font:", "cached_text:", "text:")
        if not isinstance(identities, list) or len(identities) < 4 \
                or any(case["project_root"] not in identity for identity in identities):
            raise SeamError("IT-029 exact resource identities lost project scope")
        if any(not any(identity.startswith(kind) for identity in identities)
               for kind in required_identity_kinds):
            raise SeamError("IT-029 sprite/font/text identity evidence is incomplete")
        bounds = {
            "maximum_atlas_pages": 2,
            "maximum_live_regions": 6,
            "maximum_text_textures": 2,
            "maximum_font_entries": 1,
        }
        if any(not isinstance(resources.get(field), int)
               or resources[field] < 0 or resources[field] > maximum
               for field, maximum in bounds.items()):
            raise SeamError("IT-029 resource counts are unbounded")
        byte_receipts = ("source_bytes", "decode_bytes", "upload_bytes",
                         "texture_bytes", "maximum_cache_bytes", "atlas_capacity_bytes")
        if any(type(resources.get(field)) is not int or resources[field] <= 0
               for field in byte_receipts):
            raise SeamError("IT-029 physical resource byte receipts are incomplete")
        if resources["upload_bytes"] < resources["texture_bytes"] \
                or resources["maximum_cache_bytes"] > 64 * 1024 * 1024 \
                or resources["atlas_capacity_bytes"] > 64 * 1024 * 1024:
            raise SeamError("IT-029 physical resource byte receipts exceed their bounds")

    alpha, beta_before, beta_after, alpha_return = values
    for field in ("sprite_handles", "font_handles", "cached_text_handles"):
        if any(case[field] != alpha[field] for case in values[1:]):
            raise SeamError(f"IT-029 colliding {field} changed across projects")
    if alpha["project_root"] == beta_before["project_root"] \
            or alpha["project_root"] != alpha_return["project_root"] \
            or beta_before["project_root"] != beta_after["project_root"]:
        raise SeamError("IT-029 project roots were reused or restored incorrectly")
    if alpha["direct_text_sha256"] == beta_before["direct_text_sha256"] \
            or alpha["capture_sha256"] == beta_before["capture_sha256"] \
            or alpha["command_trace"] == beta_before["command_trace"]:
        raise SeamError("IT-029 project A/B asset identity was reused")
    if beta_before["direct_text_sha256"] != beta_after["direct_text_sha256"] \
            or alpha["direct_text_sha256"] != alpha_return["direct_text_sha256"] \
            or beta_before["command_trace"] != beta_after["command_trace"] \
            or alpha["command_trace"] != alpha_return["command_trace"]:
        raise SeamError("IT-029 recreated or returned project restored the wrong identity")
    alpha_identities = alpha["resources"]["identities"]
    beta_identities = beta_before["resources"]["identities"]
    if alpha_identities == beta_identities \
            or alpha_identities != alpha_return["resources"]["identities"] \
            or beta_identities != beta_after["resources"]["identities"]:
        raise SeamError("IT-029 exact resource identities crossed a project or surface epoch")
    before_resources = beta_before["resources"]
    after_resources = beta_after["resources"]
    if after_resources.get("lifecycle_renderer_generation", 0) \
            <= before_resources.get("lifecycle_renderer_generation", 0) \
            or after_resources.get("stale_generation_rejections", 0) \
            <= before_resources.get("stale_generation_rejections", 0) \
            or after_resources.get("restore_uploads") != 5 \
            or after_resources.get("duplicate_restore_uploads") != 0:
        raise SeamError("IT-029 stale generation was not rejected and restored exactly once")
    captures = summary.get("captures")
    if captures != [case["capture_path"] for case in values] \
            or len(set(captures)) != 4:
        raise SeamError("IT-029 summary capture artifacts are missing or reused")
    return {"summary": summary, "cases": values, "_position": summary_match.start()}


def verify_it031(log: str, after_position: int) -> dict | None:
    markers = _json_markers(DIAGNOSTIC_MARKER, log, "IT-031 diagnostic marker", "IT-031")
    if not markers:
        return None
    if len(markers) != 1:
        raise SeamError(f"expected exactly one IT-031 summary, found {len(markers)}")
    marker_match, marker = markers[0]
    if marker_match.start() <= after_position:
        raise SeamError("IT-031 summary must follow IT-029")
    case_markers = _json_markers(DIAGNOSTIC_CASE_MARKER, log, "IT-031 case", "IT-031")
    if len(case_markers) != 5:
        raise SeamError(f"expected exactly 5 IT-031 cases, found {len(case_markers)}")
    if any(match.start() <= after_position or match.start() >= marker_match.start()
           for match, _ in case_markers):
        raise SeamError("IT-031 cases must follow IT-029 and precede its summary")
    if (marker.get("schema"), marker.get("test_id"), marker.get("event"),
            marker.get("status"), marker.get("ordered")) != (
                "stasis.workshop_diagnostic_seam.v1", "IT-031", "diagnostic_seam",
                "passed", True):
        raise SeamError("IT-031 marker does not report an ordered native diagnostic seam")
    expected = [
        ("parse", "parse", "stasis.parse"),
        ("extern_resolution", "extern_resolution", "stasis.unresolvedExtern"),
        ("runtime_entry", "runtime_entry", "stasis.runtimeEntry"),
        ("render_schema", "render_schema", "stasis.renderSchema"),
        ("missing_resource", "resource", "stasis.missingResource"),
    ]
    case_names = [case.get("name") for _, case in case_markers]
    if marker.get("case_count") != len(expected) \
            or marker.get("case_names") != [name for name, _, _ in expected] \
            or case_names != [name for name, _, _ in expected]:
        raise SeamError("IT-031 must contain exactly five ordered native cases")
    for (_, case), (name, stage, code) in zip(case_markers, expected):
        if not isinstance(case, dict) or case.get("name") != name or case.get("equal") is not True:
            raise SeamError(f"IT-031 case {name} is missing native/UI equality evidence")
        native = case.get("native")
        ui = case.get("ui")
        if not isinstance(native, dict) or native != ui:
            raise SeamError(f"IT-031 case {name} changed between native and UI")
        if not isinstance(case.get("displayed_text"), str) \
                or native.get("detail", "") not in case["displayed_text"]:
            raise SeamError(f"IT-031 case {name} lost detail in the displayed UI status")
        if native.get("schema") != "stasis.native_diagnostic.v1" \
                or native.get("version") != 1 \
                or native.get("stage") != stage \
                or native.get("code") != code \
                or not isinstance(native.get("detail"), str) \
                or not native["detail"] \
                or not isinstance(native.get("causes"), list) \
                or not native["causes"]:
            raise SeamError(f"IT-031 case {name} lost stage, code, detail, or causes")
        if native["detail"] == "native preview frame failed":
            raise SeamError(f"IT-031 case {name} replaced detail with a generic fallback")
        if native["causes"][0] != f"{stage} phase" \
                or native["causes"][-1] != native["detail"]:
            raise SeamError(f"IT-031 case {name} has reversed or incomplete cause ordering")
        context = native.get("context")
        if not isinstance(context, dict):
            raise SeamError(f"IT-031 case {name} lost diagnostic context")
        if name == "parse":
            location = case.get("location")
            expected_location = location.get("expected") if isinstance(location, dict) else None
            actual_location = location.get("actual") if isinstance(location, dict) else None
            if context.get("file") != "src/main.stasis" \
                    or context.get("symbol") != "on_code_swap" \
                    or not isinstance(expected_location, dict) \
                    or expected_location != actual_location \
                    or not all(isinstance(expected_location.get(key), int)
                               and expected_location[key] > 0
                               for key in ("line", "column", "end_line", "end_column")) \
                    or (expected_location["end_line"], expected_location["end_column"]) < \
                    (expected_location["line"], expected_location["column"]):
                raise SeamError("IT-031 parse diagnostic lost final-function span or symbol")
        if name == "extern_resolution" and (
                context.get("file") != "src/main.stasis"
                or context.get("symbol") != "IT031_missing_extern"):
            raise SeamError("IT-031 extern diagnostic lost its source file or symbol")
        if name == "missing_resource" \
                and context.get("resource") != "assets/IT031_missing.svg":
            raise SeamError("IT-031 resource diagnostic lost its resource path")
        if name == "runtime_entry" and context.get("symbol") != "tick":
            raise SeamError("IT-031 runtime diagnostic lost the tick symbol")
        if name == "render_schema" and context.get("symbol") != "render":
            raise SeamError("IT-031 render diagnostic lost the render symbol")
    cleanup = marker.get("cleanup_receipt")
    cleanup_ui = cleanup.get("ui") if isinstance(cleanup, dict) else None
    if not isinstance(cleanup, dict) or cleanup.get("status") != "Restored" \
            or cleanup.get("frame") != "passed" \
            or not isinstance(cleanup.get("compile"), str) \
            or not cleanup["compile"].startswith("CompileReady") \
            or "status=0" not in cleanup["compile"] \
            or not isinstance(cleanup.get("source_fingerprint"), str) \
            or not cleanup["source_fingerprint"] \
            or cleanup.get("source_fingerprint") != cleanup.get("baseline_source_fingerprint") \
            or not isinstance(cleanup.get("generation"), int) \
            or cleanup.get("generation") <= 0 \
            or not isinstance(cleanup.get("baseline_generation"), int) \
            or cleanup.get("generation") <= cleanup.get("baseline_generation") \
            or not isinstance(cleanup_ui, dict) \
            or cleanup_ui.get("blocking_error_visible") is not False \
            or cleanup_ui.get("status_healthy") is not True \
            or cleanup_ui.get("compile_ready") is not True \
            or cleanup_ui.get("compile_attempted") is not True \
            or cleanup_ui.get("game_runtime_active") is not True \
            or not isinstance(cleanup_ui.get("displayed_status"), str) \
            or not cleanup_ui.get("displayed_status") \
            or cleanup_ui.get("displayed_status").startswith(("CompileError", "RunError")):
        raise SeamError("IT-031 cleanup did not prove original source, healthy frame, and UI recovery")
    return marker


def verify_it030(log: str, after_position: int) -> dict:
    summaries = _json_markers(TEST_RUNNER_MARKER, log, "IT-030 marker", "IT-030")
    if len(summaries) != 1:
        raise SeamError(f"expected exactly one IT-030 summary, found {len(summaries)}")
    summary_match, summary = summaries[0]
    cases = _json_markers(TEST_RUNNER_CASE_MARKER, log, "IT-030 case", "IT-030")
    if len(cases) != 3:
        raise SeamError(f"expected exactly 3 IT-030 cases, found {len(cases)}")
    if summary_match.start() <= after_position or any(
            match.start() <= after_position or match.start() >= summary_match.start()
            for match, _ in cases):
        raise SeamError("IT-030 cases must follow IT-029 and precede their summary")
    values = [case for _, case in cases]
    phases = ["pass", "fail", "subsequent_pass"]
    if [case.get("phase") for case in values] != phases \
            or [case.get("sequence") for case in values] != [1, 2, 3]:
        raise SeamError("IT-030 pass/fail/rollback cases are missing or reordered")
    for index, case in enumerate(values):
        result = case.get("result")
        test_file = case.get("test_file")
        runtime = case.get("runtime")
        passed = case.get("passed")
        failed = case.get("failed")
        expected_status = "failed" if index == 1 else "passed"
        if case.get("schema") != "stasis.workshop_test_runner.v1" \
                or case.get("event") != "case" or case.get("status") != "passed" \
                or not isinstance(passed, int) or passed < 0 \
                or not isinstance(failed, int) or failed < 0 \
                or (failed == 0) != case.get("all_passed") \
                or not isinstance(result, dict) \
                or result.get("file") != "tests/it030_workshop_jni.test.stasis" \
                or result.get("line") != 3 or result.get("column") != 1 \
                or result.get("name") != "IT-030 Workshop JNI rollback" \
                or result.get("status") != expected_status \
                or result.get("passed") != (index != 1):
            raise SeamError("IT-030 result lost exact counts, status, name, or source location")
        if (index == 1 and failed != 1) or (index != 1 and failed != 0):
            raise SeamError("IT-030 pass/fail counts do not prove the expected result")
        if not isinstance(test_file, dict) \
                or test_file.get("path") != "tests/it030_workshop_jni.test.stasis" \
                or test_file.get("exists") is not True:
            raise SeamError("IT-030 temporary test lifecycle is incomplete")
        if not re.fullmatch(r"[0-9a-f]{64}", str(case.get("source_sha256", ""))):
            raise SeamError("IT-030 source checksum is not exact SHA-256 evidence")
        if not isinstance(runtime, dict) or not runtime.get("fingerprint") \
                or not isinstance(runtime.get("generation"), int) \
                or runtime["generation"] <= 0 \
                or runtime.get("activation") != "native_frame":
            raise SeamError("IT-030 runtime identity is incomplete")
    passed_case, failed_case, subsequent = values
    if passed_case["passed"] <= 0 \
            or failed_case["passed"] + 1 != passed_case["passed"] \
            or passed_case["passed"] != subsequent["passed"] \
            or passed_case["source_sha256"] != subsequent["source_sha256"] \
            or passed_case["source_sha256"] == failed_case["source_sha256"] \
            or passed_case["runtime"]["fingerprint"] != subsequent["runtime"]["fingerprint"] \
            or passed_case["runtime"]["fingerprint"] == failed_case["runtime"]["fingerprint"] \
            or failed_case["runtime"]["generation"] != passed_case["runtime"]["generation"] + 1 \
            or subsequent["runtime"]["generation"] != failed_case["runtime"]["generation"] + 1:
        raise SeamError("IT-030 rollback checksum/runtime identity or subsequent success mismatched")
    required = {
        "schema": "stasis.workshop_test_runner.v1", "test_id": "IT-030",
        "event": "test_runner", "status": "passed", "ordered": True,
        "case_count": 3, "case_phases": phases, "transport": "rust_owned_json",
    }
    if any(summary.get(key) != expected for key, expected in required.items()) \
            or "cases" in summary:
        raise SeamError("IT-030 summary is incomplete or duplicates case evidence")
    if summary.get("accepted_source_sha256") != passed_case["source_sha256"] \
            or summary.get("failing_source_sha256") != failed_case["source_sha256"] \
            or summary.get("rollback_source_sha256") != subsequent["source_sha256"] \
            or summary.get("accepted_runtime") != passed_case["runtime"] \
            or summary.get("failing_runtime") != failed_case["runtime"] \
            or summary.get("rollback_runtime") != subsequent["runtime"]:
        raise SeamError("IT-030 summary does not match ordered case identities")
    temporary = summary.get("temporary_test")
    cleanup = summary.get("cleanup_receipt")
    cleanup_runtime = cleanup.get("runtime") if isinstance(cleanup, dict) else None
    if not isinstance(temporary, dict) \
            or temporary.get("path") != "tests/it030_workshop_jni.test.stasis" \
            or temporary.get("created") is not True or temporary.get("removed") is not True \
            or not isinstance(cleanup, dict) or cleanup.get("status") != "Restored" \
            or cleanup.get("test_removed") is not True \
            or not re.fullmatch(r"[0-9a-f]{64}", str(cleanup.get("packaged_source_sha256", ""))) \
            or not isinstance(cleanup.get("compile"), str) \
            or not cleanup["compile"].startswith("CompileReady") \
            or "status=0" not in cleanup["compile"] \
            or not isinstance(cleanup_runtime, dict) \
            or cleanup_runtime.get("generation", 0) <= subsequent["runtime"]["generation"] \
            or cleanup_runtime.get("activation") != "native_frame" \
            or cleanup.get("packaged_source_sha256") == passed_case["source_sha256"]:
        raise SeamError("IT-030 cleanup did not restore the packaged project and remove its test")
    return {"summary": summary, "cases": values, "_position": summary_match.start()}


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
    presentation_match, presentation = max(
        stable_presentations,
        key=lambda item: (item[1]["count"], item[0].start()),
    )
    presentation_index = next(
        index
        for index, (candidate_match, _) in enumerate(presentations)
        if candidate_match.start() == presentation_match.start()
    )
    if presentation_index == 0:
        raise SeamError("stable IT-025 GLES presentation lacks a preceding presentation")
    predecessor_match, predecessor = presentations[presentation_index - 1]
    predecessor_token = predecessor.get("frame_token")
    if predecessor.get("event") != "present" or not isinstance(predecessor_token, int):
        raise SeamError("preceding IT-025 GLES presentation is not a valid presentation")
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
    touch_markers = []
    for match in TOUCH_MARKER.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-027 marker JSON: {error}") from error
        if candidate.get("test_id") == "IT-027":
            touch_markers.append((match, candidate))
    if len(touch_markers) != 1:
        raise SeamError(f"expected exactly one IT-027 summary, found {len(touch_markers)}")
    touch_summary_match, touch_summary = touch_markers[0]
    touch_cases = []
    for match in TOUCH_CASE_MARKER.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-027 case JSON: {error}") from error
        if candidate.get("test_id") == "IT-027":
            touch_cases.append((match, candidate))
    if any(match.start() <= abi_match.start() or match.start() >= touch_summary_match.start()
           for match, _ in touch_cases):
        raise SeamError("IT-027 cases must follow IT-026 and precede the IT-027 summary")
    cases = [candidate for _, candidate in touch_cases]
    if len(cases) != 3:
        raise SeamError(f"expected exactly 3 IT-027 cases, found {len(cases)}")
    if touch_summary.get("schema") != "stasis.workshop_touch_roundtrip.v1" \
            or touch_summary.get("event") != "touch_roundtrip" \
            or touch_summary.get("status") != "passed" \
            or touch_summary.get("phases") != 3 \
            or touch_summary.get("ordered") is not True \
            or touch_summary.get("unique") is not True \
            or touch_summary.get("java_motion_events") != 3 \
            or touch_summary.get("jni_jit_frames") != 3 \
            or touch_summary.get("gles_presented_frames") != 3 \
            or touch_summary.get("java_only") is not False:
        raise SeamError("IT-027 marker does not report a passed ordered JNI/GLES acceptance")
    expected_phases = [("down", 1, 160, 90, 1, 0, 1, 0, 0, 0),
                       ("move", 2, 320, 180, 1, 2, 0, 0, 160, 90),
                       ("up", 3, 400, 225, 0, 1, 0, 1, 80, 45)]
    observed_phases = []
    observed_tokens = []
    for candidate, expected in zip(cases, expected_phases):
        phase, sequence, x, y, active, action, down_edge, up_edge, dx, dy = expected
        if candidate.get("schema") != "stasis.workshop_touch_roundtrip.v1" \
                or candidate.get("event") != "case" \
                or candidate.get("status") != "passed" \
                or (candidate.get("phase"), candidate.get("sequence")) != (phase, sequence):
            raise SeamError("IT-027 cases are not the required ordered phases")
        guest = candidate.get("guest")
        input_state = candidate.get("input")
        if not isinstance(input_state, dict) \
                or (input_state.get("x"), input_state.get("y"), input_state.get("active"),
                    input_state.get("action")) != (x, y, active, action):
            raise SeamError(f"IT-027 {phase} input action/coordinates mismatch")
        if not isinstance(guest, dict) or (guest.get("x"), guest.get("y"), guest.get("active"),
                                           guest.get("down_edge"), guest.get("up_edge"),
                                           guest.get("dx"), guest.get("dy")) != \
                (x, y, active, down_edge, up_edge, dx, dy):
            raise SeamError(f"IT-027 {phase} HostFrame/edge/delta mismatch")
        if guest.get("x_norm_x1000") != x * 1000 // 640 \
                or guest.get("y_norm_x1000") != y * 1000 // 360:
            raise SeamError(f"IT-027 {phase} normalized coordinate mismatch")
        expected_checksum = x + y * 3 + dx * 5 + dy * 7 \
            + guest["x_norm_x1000"] * 11 + guest["y_norm_x1000"] * 13 \
            + active * 17 + down_edge * 19 + up_edge * 23
        if guest.get("checksum") != expected_checksum:
            raise SeamError(f"IT-027 {phase} guest checksum mismatch")
        render = candidate.get("render")
        if not isinstance(render, dict):
            raise SeamError(f"IT-027 {phase} lacks render evidence")
        touch_marker = render.get("marker")
        token = render.get("frame_token")
        if not isinstance(token, int) or token <= 0 or token in observed_tokens:
            raise SeamError("IT-027 frame tokens are missing or duplicated")
        observed_tokens.append(token)
        if not isinstance(render.get("trace"), int) or render["trace"] == 0 \
                or not isinstance(touch_marker, dict) or touch_marker.get("active") is not True \
                or guest.get("marker_active") != 1:
            raise SeamError(f"IT-027 {phase} lacks direct command-buffer trace/marker")
        for key, value in (("x", x - 8), ("y", y - 8), ("w", 16), ("h", 16),
                           ("r", 1.0), ("g", 0.65), ("b", 0.08), ("a", 1.0)):
            if not isinstance(touch_marker.get(key), (int, float)) or abs(touch_marker[key] - value) > 0.01:
                raise SeamError(f"IT-027 {phase} marker geometry mismatch")
        if candidate.get("gles_presented") is not True \
                or candidate.get("gles_frame_token") != token \
                or candidate.get("java_only") is not False:
            raise SeamError(f"IT-027 {phase} lacks matching GLES presentation")
        observed_phases.append(phase)
    traces = [candidate["render"]["trace"] for candidate in cases]
    if len(set(traces)) != 3 or any(trace <= 0 for trace in traces):
        raise SeamError("IT-027 command traces are missing or duplicated")
    if observed_phases != [item[0] for item in expected_phases] \
            or observed_tokens != sorted(observed_tokens):
        raise SeamError("IT-027 phases or GLES tokens are not strictly ordered")
    touch_present = []
    for match in TOUCH_PRESENT.finditer(log):
        try:
            candidate = json.loads(match.group(1))
        except json.JSONDecodeError as error:
            raise SeamError(f"invalid IT-027 GLES marker JSON: {error}") from error
        if candidate.get("test_id") == "IT-027":
            touch_present.append((match, candidate))
    if len(touch_present) != 3:
        raise SeamError(f"expected exactly 3 IT-027 GLES markers, found {len(touch_present)}")
    if any(match.start() <= abi_match.start() or match.start() >= touch_summary_match.start()
           for match, _ in touch_present):
        raise SeamError("IT-027 GLES markers must follow IT-026 and precede the summary")
    presented_tokens = []
    present_positions = []
    case_positions = [match.start() for match, _ in touch_cases]
    for index, ((present_match, present), expected_token, expected_marker) in enumerate(zip(
            touch_present, observed_tokens, [candidate["render"]["marker"] for candidate in cases])):
        present_positions.append(present_match.start())
        if present_match.start() >= case_positions[index]:
            raise SeamError("IT-027 GLES marker must precede its matching case")
        if present.get("schema") != "stasis.workshop_touch_roundtrip.v1" \
                or present.get("event") != "present" \
                or present.get("frame_token") != expected_token \
                or present.get("trace") != cases[index]["render"]["trace"] \
                or present.get("rect_count") != 2 \
                or present.get("order_count") != ACCEPTANCE_MARKER_ORDER_COUNT:
            raise SeamError("IT-027 GLES marker did not match its ordered case token")
        presented_marker = present.get("marker")
        if not isinstance(presented_marker, dict) or presented_marker.get("active") is not True:
            raise SeamError("IT-027 GLES marker lacks active direct marker evidence")
        for key in ("x", "y", "w", "h", "r", "g", "b", "a"):
            observed_value = presented_marker.get(key)
            expected_value = expected_marker.get(key)
            if not isinstance(observed_value, (int, float)) \
                    or not isinstance(expected_value, (int, float)) \
                    or abs(observed_value - expected_value) > 0.01:
                raise SeamError("IT-027 GLES marker geometry/color mismatch")
        presented_tokens.append(present.get("frame_token"))
    if presented_tokens != observed_tokens:
        raise SeamError("IT-027 GLES tokens do not independently correlate with cases")
    interleaved = sorted(
        [(present_positions[index], "present") for index in range(3)]
        + [(case_positions[index], "case") for index in range(3)])
    if [kind for _, kind in interleaved] != ["present", "case"] * 3:
        raise SeamError("IT-027 GLES and case evidence is not strictly interleaved")
    hot_edit = verify_it028(log, touch_summary_match.start())
    resource_scope = verify_it029(log, hot_edit["_position"])
    test_runner = verify_it030(log, resource_scope["_position"])
    diagnostic_seam = verify_it031(log, test_runner["_position"])
    if diagnostic_seam is None:
        raise SeamError("missing mandatory IT-031 diagnostic seam evidence")
    diagnostic_boundary = next(DIAGNOSTIC_MARKER.finditer(log)).end()
    idle_window_start = diagnostic_boundary
    if predecessor_match.start() > diagnostic_boundary:
        predecessor_native_match, _ = next(
            (
                (candidate_match, candidate)
                for candidate_match, candidate in reversed(markers)
                if candidate.get("frame_token") == predecessor_token
                and diagnostic_boundary <= candidate_match.start()
                < predecessor_match.start()
            ),
            (None, None),
        )
        if predecessor_native_match is None:
            raise SeamError(
                "preceding stable-runtime IT-025 presentation lacks a matching native frame"
            )
        idle_window_start = predecessor_native_match.start()
    idle_markers = [
        candidate
        for candidate_match, candidate in markers
        if idle_window_start <= candidate_match.start() <= presentation_match.start()
    ]
    if not idle_markers:
        raise SeamError("stable IT-025 presentation lacks native markers after IT-031")
    command_traces = [candidate.get("command_trace") for candidate in idle_markers]
    if any(not isinstance(trace, int) or trace <= 0 for trace in command_traces):
        raise SeamError("IT-025 idle command_trace must be a positive current-build diagnostic")
    if len(set(command_traces)) != 1:
        raise SeamError("IT-025 command_trace changed within the stable idle proof window")
    return {
        "compile_functions": max(map(int, compile_matches)),
        "presented_frames": stable_count,
        "frame_token": marker["frame_token"],
        "stable_frame_token": stable_token,
        "presentation": presentation,
        "marker": marker,
        "it026": abi,
        "it026_cases": invalid,
        "it027": touch_summary,
        "it027_cases": cases,
        "it027_gles": [candidate for _, candidate in touch_present],
        "it028": hot_edit["summary"],
        "it028_cases": hot_edit["cases"],
        "it028_gles": hot_edit["gles"],
        "it029": resource_scope["summary"],
        "it029_cases": resource_scope["cases"],
        "it030": test_runner["summary"],
        "it030_cases": test_runner["cases"],
        "it031": diagnostic_seam,
    }


def verify_files(log_path: Path, capture: Path, manifest_path: Path, apk: Path,
                 metadata_path: Path, evidence_path: Path,
                 it029_captures: list[Path] | None = None) -> dict:
    for path in (log_path, capture, manifest_path, apk, metadata_path):
        if not path.is_file():
            raise SeamError(f"required Workshop evidence file is missing: {path}")
    manifest = _read_json(manifest_path)
    metadata = _read_json(metadata_path)
    result = verify_log(log_path.read_text(encoding="utf-8", errors="replace"), manifest)
    supplied_it029 = it029_captures or []
    if len(supplied_it029) != 4:
        raise SeamError("exactly four IT-029 PNG captures are required")
    expected_cases = result["it029_cases"]
    capture_evidence = []
    captures_by_phase = {}
    for path, case in zip(supplied_it029, expected_cases):
        if not path.is_file():
            raise SeamError(f"required IT-029 capture is missing: {path}")
        digest = _sha256(path)
        if digest != case["capture_sha256"]:
            raise SeamError(f"IT-029 capture hash does not match {case['phase']}")
        pixel_oracle = _verify_it029_capture_pixels(path, case["phase"])
        capture_evidence.append({
            "phase": case["phase"], "path": str(path), "sha256": digest,
            "pixel_oracle": pixel_oracle,
        })
        captures_by_phase[case["phase"]] = path
    restore_pixel_comparisons = [
        _compare_it029_restore_pixels(
            captures_by_phase[before], captures_by_phase[after], before, after
        )
        for before, after in (
            ("project_a_first", "project_a_return"),
            ("project_b_before_recreation", "project_b_after_recreation"),
        )
    ]
    framebuffer_width, framebuffer_height, _ = read_capture(supplied_it029[-1])
    evidence = {
        "schema": "stasis.workshop_seam.evidence.v1",
        "test_id": "IT-025",
        "status": "passed",
        "source_revision": metadata.get("git_revision", ""),
        "apk_sha256": _sha256(apk),
        "capture_sha256": _sha256(capture),
        "costs": {
            "apk_bytes": apk.stat().st_size,
            "capture_png_bytes": capture.stat().st_size,
            "framebuffer_bytes": framebuffer_width * framebuffer_height * 4,
            "framebuffer_size": [framebuffer_width, framebuffer_height],
        },
        "metadata": metadata,
        "it029_capture_artifacts": capture_evidence,
        "it029_restore_pixel_comparisons": restore_pixel_comparisons,
        **result,
    }
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    evidence_path.write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return evidence


def _compare_it029_restore_pixels(
        before_path: Path, after_path: Path, before_phase: str, after_phase: str) -> dict:
    try:
        before_width, before_height, before_rgba = read_capture(before_path)
        after_width, after_height, after_rgba = read_capture(after_path)
    except Exception as error:
        raise SeamError(
            f"IT-029 restore pair {before_phase}/{after_phase} is not a supported PNG: {error}"
        ) from error
    if (before_width, before_height) != (after_width, after_height):
        raise SeamError(
            f"IT-029 restore pair {before_phase}/{after_phase} has different dimensions"
        )
    pixel_count = before_width * before_height
    if len(before_rgba) != pixel_count * 4 or len(after_rgba) != pixel_count * 4:
        raise SeamError(
            f"IT-029 restore pair {before_phase}/{after_phase} has invalid decoded dimensions"
        )
    changed_pixels = 0
    max_channel_delta = 0
    for offset in range(0, len(before_rgba), 4):
        deltas = [
            abs(before_rgba[offset + channel] - after_rgba[offset + channel])
            for channel in range(4)
        ]
        if any(deltas):
            changed_pixels += 1
            max_channel_delta = max(max_channel_delta, *deltas)
    allowance = max(256, pixel_count // 1000)
    if max_channel_delta > 1:
        raise SeamError(
            f"IT-029 restore pair {before_phase}/{after_phase} max channel delta "
            f"{max_channel_delta} exceeds 1"
        )
    if changed_pixels > allowance:
        raise SeamError(
            f"IT-029 restore pair {before_phase}/{after_phase} changed {changed_pixels} "
            f"pixels, exceeding allowance {allowance}"
        )
    return {
        "phases": [before_phase, after_phase],
        "pixel_count": pixel_count,
        "changed_pixels": changed_pixels,
        "max_channel_delta": max_channel_delta,
        "allowance": allowance,
    }


def _verify_it029_capture_pixels(path: Path, phase: str) -> dict:
    expected_colors = {
        "project_a_first": (0x12, 0x61, 0xA0, 0xFF),
        "project_a_return": (0x12, 0x61, 0xA0, 0xFF),
        "project_b_before_recreation": (0xA0, 0x38, 0x12, 0xFF),
        "project_b_after_recreation": (0xA0, 0x38, 0x12, 0xFF),
    }
    expected = expected_colors.get(phase)
    if expected is None:
        raise SeamError(f"IT-029 capture has unknown phase {phase}")
    if path.suffix.lower() != ".png":
        raise SeamError(f"IT-029 capture for {phase} must be a PNG")
    try:
        width, height, rgba = read_capture(path)
    except Exception as error:
        raise SeamError(f"IT-029 capture for {phase} is not a supported PNG: {error}") from error
    if width <= 0 or height <= 0 or len(rgba) != width * height * 4:
        raise SeamError(f"IT-029 capture for {phase} has invalid decoded dimensions")

    non_black = []
    for offset in range(0, len(rgba), 4):
        pixel = tuple(rgba[offset:offset + 4])
        if pixel[3] != 0 and pixel[:3] != (0, 0, 0):
            index = offset // 4
            non_black.append((index % width, index // width))
    if not non_black:
        raise SeamError(f"IT-029 capture for {phase} has no visible logical viewport")
    left = min(point[0] for point in non_black)
    top = min(point[1] for point in non_black)
    right = max(point[0] for point in non_black) + 1
    bottom = max(point[1] for point in non_black) + 1
    viewport_width = right - left
    viewport_height = bottom - top
    if viewport_width < 32 or viewport_height < 32:
        raise SeamError(f"IT-029 capture for {phase} has an implausibly small viewport")

    def pixels_in(x0: int, y0: int, x1: int, y1: int) -> list[tuple[int, ...]]:
        return [
            tuple(rgba[(y * width + x) * 4:(y * width + x + 1) * 4])
            for y in range(y0, y1)
            for x in range(x0, x1)
        ]

    viewport = pixels_in(left, top, right, bottom)
    project_pixels = viewport.count(expected)
    shared_pixels = viewport.count((0x31, 0xD1, 0x7C, 0xFF))
    viewport_area = viewport_width * viewport_height
    if project_pixels < max(32, int(viewport_area * 0.005)):
        raise SeamError(
            f"IT-029 {phase} capture lacks expected project color "
            f"#{expected[0]:02x}{expected[1]:02x}{expected[2]:02x}"
        )
    if shared_pixels < max(48, int(viewport_area * 0.007)):
        raise SeamError(f"IT-029 {phase} capture lacks the shared sprite color #31d17c")

    text_measurements = {}
    for name, x_start, x_end in (
        ("left", 0.07, 0.43),
        ("right", 0.50, 0.90),
    ):
        roi_left = left + int(viewport_width * x_start)
        roi_right = left + int(viewport_width * x_end)
        roi_top = top + int(viewport_height * 0.68)
        roi_bottom = top + int(viewport_height * 0.88)
        roi = pixels_in(roi_left, roi_top, roi_right, roi_bottom)
        background = Counter(roi).most_common(1)[0][0]
        contrast = sum(
            1 for pixel in roi
            if pixel[3] >= 128
            and sum(abs(pixel[channel] - background[channel]) for channel in range(3)) >= 60
        )
        if contrast < max(24, int(len(roi) * 0.03)):
            raise SeamError(f"IT-029 {phase} capture lacks visible {name} text-band pixels")
        text_measurements[f"{name}_text_contrast_pixels"] = contrast

    return {
        "decoded_size": [width, height],
        "logical_viewport": [left, top, viewport_width, viewport_height],
        "expected_project_rgba": "#%02x%02x%02xff" % expected[:3],
        "project_pixels": project_pixels,
        "shared_sprite_pixels": shared_pixels,
        **text_measurements,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--capture", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--apk", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--it029-capture", type=Path, action="append", default=[])
    args = parser.parse_args()
    try:
        evidence = verify_files(args.log, args.capture, args.manifest, args.apk,
                                args.metadata, args.evidence, args.it029_capture)
    except (OSError, json.JSONDecodeError, SeamError) as error:
        parser.error(str(error))
    print(json.dumps({"status": evidence["status"], "evidence": str(args.evidence)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

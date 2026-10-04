#!/usr/bin/env python3
"""Verify iOS simulator evidence for the generics collections acceptance."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
import zlib
from pathlib import Path
from typing import Sequence


RECEIPT_SCHEMA = "stasis.ios.generics.v2"
BOUNDS_SCHEMA = "stasis.ios.generics.bounds.v1"
EVIDENCE_SCHEMA = "stasis.ios.generics.evidence.v2"
EXPECTED_DIGEST = 507
EXPECTED_MATH_RAW_DIGEST = -1430176193
AUTHORED_TEAL_RGB = (41, 184, 133)
MIN_FRAME_PIXELS = 1024
MAX_LOG_BYTES = 8 * 1024 * 1024
IOS_SIMULATOR_FRAME_SIZES = {
    (1179, 2556),  # iPhone 14 Pro and iPhone 15 Pro
    (1206, 2622),  # iPhone 16 Pro
}

LOG_FAILURES = (
    ("crash", re.compile(
        r"EXC_(?:BAD_ACCESS|CRASH|BREAKPOINT)|SIG(?:ABRT|BUS|ILL|SEGV|TRAP)|"
        r"Fatal signal|Abort message|uncaught exception|\bcrash(?:ed)?\b",
        re.IGNORECASE,
    )),
    ("link", re.compile(
        r"undefined symbol|unresolved symbol|symbol not found|library not loaded|"
        r"image not found|linker command failed|dyld[^\n]*(?:failed|error|terminated)",
        re.IGNORECASE,
    )),
    ("import", re.compile(
        r"(?:import|imported function)[^\n]*(?:error|failed|missing|not found|unresolved)|"
        r"(?:error|failed|missing|unresolved)[^\n]*\bimport\b",
        re.IGNORECASE,
    )),
)


class EvidenceError(ValueError):
    """An evidence input did not prove the required runtime behavior."""


def _load_json_object(path: Path) -> dict:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise EvidenceError(f"{path}: could not read JSON: {error}") from error
    if not isinstance(value, dict):
        raise EvidenceError(f"{path}: expected a JSON object")
    return value


def _require_exact_int(value: dict, key: str, expected: int, path: Path) -> None:
    actual = value.get(key)
    if isinstance(actual, bool) or not isinstance(actual, int) or actual != expected:
        raise EvidenceError(f"{path}: expected {key}={expected}, got {actual!r}")


def validate_receipt(path: Path) -> dict:
    value = _load_json_object(path)
    if value.get("schema") != RECEIPT_SCHEMA:
        raise EvidenceError(f"{path}: unexpected schema {value.get('schema')!r}")
    for key in ("main_result", "tick_result", "render_result"):
        _require_exact_int(value, key, 0, path)
    _require_exact_int(value, "digest", EXPECTED_DIGEST, path)
    _require_exact_int(value, "math_raw_digest", EXPECTED_MATH_RAW_DIGEST, path)
    frame = value.get("frame")
    if isinstance(frame, bool) or not isinstance(frame, int) or frame < 1:
        raise EvidenceError(f"{path}: expected frame >= 1, got {frame!r}")
    return value


def _paeth(left: int, above: int, upper_left: int) -> int:
    estimate = left + above - upper_left
    left_distance = abs(estimate - left)
    above_distance = abs(estimate - above)
    upper_left_distance = abs(estimate - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def _unfilter_png(raw: bytes, width: int, height: int, channels: int) -> bytes:
    stride = width * channels
    expected = (stride + 1) * height
    if len(raw) != expected:
        raise EvidenceError(
            f"PNG decompressed to {len(raw)} bytes; expected {expected}"
        )
    pixels = bytearray(stride * height)
    source = 0
    for row_index in range(height):
        filter_type = raw[source]
        source += 1
        if filter_type > 4:
            raise EvidenceError(f"PNG row {row_index} has invalid filter {filter_type}")
        row_start = row_index * stride
        previous_start = row_start - stride
        if filter_type == 0:
            pixels[row_start : row_start + stride] = raw[source : source + stride]
            source += stride
            continue
        for column in range(stride):
            encoded = raw[source]
            source += 1
            left = pixels[row_start + column - channels] if column >= channels else 0
            above = pixels[previous_start + column] if row_index else 0
            upper_left = (
                pixels[previous_start + column - channels]
                if row_index and column >= channels
                else 0
            )
            if filter_type == 1:
                predictor = left
            elif filter_type == 2:
                predictor = above
            elif filter_type == 3:
                predictor = (left + above) // 2
            else:
                predictor = _paeth(left, above, upper_left)
            pixels[row_start + column] = (encoded + predictor) & 0xFF
    return bytes(pixels)


def _decode_png(path: Path) -> tuple[int, int, int, bytes, bytes]:
    try:
        data = path.read_bytes()
    except OSError as error:
        raise EvidenceError(f"{path}: could not read PNG: {error}") from error
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise EvidenceError(f"{path}: invalid PNG signature")

    position = 8
    ihdr = None
    compressed = bytearray()
    saw_iend = False
    chunk_index = 0
    while position < len(data):
        if len(data) - position < 12:
            raise EvidenceError(f"{path}: truncated PNG chunk")
        length = struct.unpack_from(">I", data, position)[0]
        position += 4
        chunk_type = data[position : position + 4]
        position += 4
        if length > len(data) - position - 4:
            raise EvidenceError(f"{path}: truncated {chunk_type!r} chunk")
        payload = data[position : position + length]
        position += length
        expected_crc = struct.unpack_from(">I", data, position)[0]
        position += 4
        actual_crc = zlib.crc32(chunk_type + payload) & 0xFFFFFFFF
        if actual_crc != expected_crc:
            raise EvidenceError(f"{path}: invalid {chunk_type!r} chunk CRC")
        if chunk_index == 0 and chunk_type != b"IHDR":
            raise EvidenceError(f"{path}: IHDR is not the first PNG chunk")
        if chunk_type == b"IHDR":
            if ihdr is not None or len(payload) != 13:
                raise EvidenceError(f"{path}: invalid IHDR chunk")
            ihdr = struct.unpack(">IIBBBBB", payload)
        elif chunk_type == b"IDAT":
            if ihdr is None or saw_iend:
                raise EvidenceError(f"{path}: misplaced IDAT chunk")
            compressed.extend(payload)
        elif chunk_type == b"IEND":
            if payload or saw_iend:
                raise EvidenceError(f"{path}: invalid IEND chunk")
            saw_iend = True
            if position != len(data):
                raise EvidenceError(f"{path}: data follows IEND")
            break
        chunk_index += 1

    if ihdr is None or not compressed or not saw_iend:
        raise EvidenceError(f"{path}: PNG is missing IHDR, IDAT, or IEND")
    width, height, bit_depth, color_type, compression, filtering, interlace = ihdr
    if width == 0 or height == 0:
        raise EvidenceError(f"{path}: PNG has an empty frame")
    if width * height > 100_000_000:
        raise EvidenceError(f"{path}: PNG dimensions are unreasonably large")
    if bit_depth != 8 or color_type not in (2, 6):
        raise EvidenceError(
            f"{path}: expected an 8-bit RGB or RGBA PNG, got "
            f"bit depth {bit_depth} color type {color_type}"
        )
    if compression != 0 or filtering != 0 or interlace != 0:
        raise EvidenceError(f"{path}: unsupported PNG encoding")
    channels = 3 if color_type == 2 else 4
    expected_raw_bytes = (width * channels + 1) * height
    try:
        decoder = zlib.decompressobj()
        raw = decoder.decompress(bytes(compressed), expected_raw_bytes + 1)
    except zlib.error as error:
        raise EvidenceError(f"{path}: invalid compressed PNG data: {error}") from error
    if (
        not decoder.eof
        or decoder.unused_data
        or decoder.unconsumed_tail
        or len(raw) != expected_raw_bytes
    ):
        raise EvidenceError(f"{path}: compressed PNG stream has invalid length or framing")
    pixels = _unfilter_png(raw, width, height, channels)
    return width, height, channels, pixels, data


def validate_frame(path: Path) -> dict:
    width, height, channels, pixels, encoded = _decode_png(path)
    total_pixels = width * height
    if total_pixels < MIN_FRAME_PIXELS:
        raise EvidenceError(
            f"{path}: frame is too small to be nontrivial ({width}x{height})"
        )
    if tuple(sorted((width, height))) not in IOS_SIMULATOR_FRAME_SIZES:
        expected = ", ".join(
            f"{width}x{height}" for width, height in sorted(IOS_SIMULATOR_FRAME_SIZES)
        )
        raise EvidenceError(
            f"{path}: frame {width}x{height} does not match the configured "
            f"iPhone simulator captures ({expected}, either orientation)"
        )

    teal_pixels = 0
    failure_pixels = 0
    background_pixels = 0
    for offset in range(0, len(pixels), channels):
        red = pixels[offset]
        green = pixels[offset + 1]
        blue = pixels[offset + 2]
        visible = channels == 3 or pixels[offset + 3] >= 250
        if visible and green > 130 and green > red + 60 and blue > 70:
            teal_pixels += 1
        if visible and red > 140 and red > green + 80:
            failure_pixels += 1
        if visible and blue > red + 15 and blue > green:
            background_pixels += 1
    minimum_teal = total_pixels // 100 + 1
    if teal_pixels < minimum_teal:
        raise EvidenceError(
            f"{path}: authored digest-success teal rectangle is missing "
            f"({teal_pixels}/{total_pixels}, need {minimum_teal})"
        )
    if failure_pixels >= 100:
        raise EvidenceError(
            f"{path}: digest-failure red rectangle is visible "
            f"({failure_pixels} pixels, need fewer than 100)"
        )
    minimum_background = total_pixels // 2 + 1
    if background_pixels < minimum_background:
        raise EvidenceError(
            f"{path}: authored dark-blue background is missing "
            f"({background_pixels}/{total_pixels}, need {minimum_background})"
        )
    return {
        "path": str(path),
        "width": width,
        "height": height,
        "total_pixels": total_pixels,
        "teal_pixels": teal_pixels,
        "minimum_teal_pixels": minimum_teal,
        "failure_pixels": failure_pixels,
        "background_pixels": background_pixels,
        "minimum_background_pixels": minimum_background,
        "authored_success_rgb": list(AUTHORED_TEAL_RGB),
        "sha256": hashlib.sha256(encoded).hexdigest(),
    }


def validate_logs(paths: Sequence[Path], receipt: dict) -> tuple[list[dict], dict]:
    evidence = []
    texts = []
    for path in paths:
        try:
            data = path.read_bytes()
        except OSError as error:
            raise EvidenceError(f"{path}: could not read log: {error}") from error
        if len(data) > MAX_LOG_BYTES:
            raise EvidenceError(
                f"{path}: log exceeds the {MAX_LOG_BYTES}-byte bounded input limit"
            )
        if not data:
            raise EvidenceError(f"{path}: green-launch log is empty")
        log = data.decode("utf-8", errors="replace")
        for category, pattern in LOG_FAILURES:
            match = pattern.search(log)
            if match is not None:
                excerpt = " ".join(match.group(0).split())
                raise EvidenceError(
                    f"{path}: green launch contains {category} evidence: {excerpt!r}"
                )
        texts.append(log)
        evidence.append({
            "path": str(path),
            "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        })
    combined = "\n".join(texts)
    acceptance_pattern = re.compile(
        rf"Stasis iOS generics acceptance digest=507 math_raw_digest={EXPECTED_MATH_RAW_DIGEST} "
        rf"frame={receipt['frame']}\b[^\n]*"
    )
    acceptance = acceptance_pattern.search(combined)
    provenance_pattern = re.compile(
        r"Stasis provenance: [^\n]+ tag=\S* commit=\S* "
        r"renderer=gfx_cmd schema=7"
    )
    provenance = provenance_pattern.search(combined)
    if provenance is None:
        raise EvidenceError("green-launch logs are missing the package provenance marker")
    return evidence, {
        "generics_acceptance": (
            acceptance.group(0).strip()
            if acceptance is not None
            else (
                f"verified receipt digest={receipt['digest']} "
                f"math_raw_digest={receipt['math_raw_digest']} frame={receipt['frame']}"
            )
        ),
        "provenance": provenance.group(0).strip(),
    }


def validate_bounds(path: Path, label: str, expected_index: int) -> dict:
    value = _load_json_object(path)
    if value.get("schema") != BOUNDS_SCHEMA:
        raise EvidenceError(f"{path}: unexpected schema {value.get('schema')!r}")
    if value.get("label") != label:
        raise EvidenceError(f"{path}: expected label {label!r}, got {value.get('label')!r}")
    _require_exact_int(value, "index", expected_index, path)
    if value.get("process") != "StasisMobile":
        raise EvidenceError(f"{path}: expected process StasisMobile")
    pid = value.get("pid")
    if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
        raise EvidenceError(f"{path}: expected a positive launch pid, got {pid!r}")
    if value.get("fatal") is not True:
        raise EvidenceError(f"{path}: bounds evidence is not fatal")
    signal = value.get("signal")
    exception = value.get("exception")
    accepted_traps = {
        ("EXC_BREAKPOINT", "SIGTRAP"),
        ("EXC_BAD_INSTRUCTION", "SIGILL"),
    }
    if (exception, signal) not in accepted_traps:
        raise EvidenceError(
            f"{path}: unrecognized fatal trap pair exception={exception!r} signal={signal!r}"
        )
    crash_report_value = value.get("crash_report")
    if not isinstance(crash_report_value, str) or not crash_report_value:
        raise EvidenceError(f"{path}: missing crash_report identity")
    crash_report = Path(crash_report_value)
    if not crash_report.is_absolute():
        crash_report = path.parent / crash_report
    try:
        crash_data = crash_report.read_bytes()
    except OSError as error:
        raise EvidenceError(f"{path}: could not read crash report: {error}") from error
    if not crash_data or len(crash_data) > MAX_LOG_BYTES:
        raise EvidenceError(f"{path}: crash report is empty or exceeds the bounded limit")
    crash_text = crash_data.decode("utf-8", errors="replace")
    if exception not in crash_text:
        raise EvidenceError(
            f"{path}: crash report does not contain exception {exception}"
        )
    if signal not in crash_text and not (
        signal == "SIGTRAP" and "Trace/BPT trap" in crash_text
    ) and not (signal == "SIGILL" and "Illegal instruction" in crash_text):
        raise EvidenceError(f"{path}: crash report does not contain signal {signal}")
    if re.search(
        r'"procName"\s*:\s*"StasisMobile"|Process:\s+StasisMobile\s+\[\d+\]',
        crash_text,
    ) is None:
        raise EvidenceError(f"{path}: crash report does not identify StasisMobile")
    if re.search(
        rf'"pid"\s*:\s*{pid}(?:\D|$)|Process:\s+StasisMobile\s+\[{pid}\]',
        crash_text,
    ) is None:
        raise EvidenceError(
            f"{path}: crash report does not match launch pid {pid}"
        )
    normalized = dict(value)
    normalized["crash_report_evidence"] = {
        "path": str(crash_report),
        "bytes": len(crash_data),
        "sha256": hashlib.sha256(crash_data).hexdigest(),
    }
    return normalized


def build_evidence(
    receipt_path: Path,
    frame_path: Path,
    log_paths: Sequence[Path],
    bounds_low_path: Path,
    bounds_high_path: Path,
) -> dict:
    if not log_paths:
        raise EvidenceError("at least one bounded green-launch log is required")
    receipt = validate_receipt(receipt_path)
    logs, log_markers = validate_logs(log_paths, receipt)
    bounds_low = validate_bounds(bounds_low_path, "low", -1)
    bounds_high = validate_bounds(bounds_high_path, "high", 2)
    if bounds_low["pid"] == bounds_high["pid"]:
        raise EvidenceError("low/high bounds evidence reused one launch pid")
    low_crash = bounds_low["crash_report_evidence"]
    high_crash = bounds_high["crash_report_evidence"]
    if low_crash["sha256"] == high_crash["sha256"]:
        raise EvidenceError("low/high bounds evidence reused one crash report")
    frame = validate_frame(frame_path)
    return {
        "schema": EVIDENCE_SCHEMA,
        "status": "passed",
        "receipt": receipt,
        "frame": frame,
        "logs": logs,
        "log_markers": log_markers,
        "bounds": {"low": bounds_low, "high": bounds_high},
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--frame", type=Path, required=True)
    parser.add_argument(
        "--log",
        type=Path,
        action="append",
        required=True,
        help="bounded green-launch log; may be supplied more than once",
    )
    parser.add_argument("--bounds-low", type=Path, required=True)
    parser.add_argument("--bounds-high", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        evidence = build_evidence(
            args.receipt,
            args.frame,
            args.log,
            args.bounds_low,
            args.bounds_high,
        )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(
            json.dumps(evidence, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    except (EvidenceError, OSError) as error:
        print(f"iOS generics evidence verification failed: {error}", file=sys.stderr)
        return 1
    print(f"iOS generics evidence verified: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

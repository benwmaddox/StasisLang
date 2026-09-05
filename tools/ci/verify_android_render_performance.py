#!/usr/bin/env python3
"""Parse one bounded Workshop renderer benchmark from Android logcat."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


PREFIX = "RenderPerformance: "
REQUIRED_METRICS = {
    "schema",
    "warmup",
    "samples",
    "total_p50_us",
    "total_p95_us",
    "resource_p50_us",
    "resource_p95_us",
    "draw_p50_us",
    "draw_p95_us",
    "draw_calls_min",
    "draw_calls_max",
    "lines",
    "rects",
    "sprites",
    "text",
    "order",
    "mixed_runs",
    "texture_binds",
    "submitted_quads",
    "atlas_pages",
    "atlas_page_creates",
    "atlas_live_regions",
    "atlas_upload_bytes",
}
ATLAS_METRICS = {
    "mixed_runs",
    "texture_binds",
    "submitted_quads",
    "atlas_pages",
    "atlas_page_creates",
    "atlas_live_regions",
    "atlas_upload_bytes",
}


def parse_report(log: str) -> dict[str, int]:
    reports = []
    for line in log.splitlines():
        if PREFIX not in line:
            continue
        fields = line.split(PREFIX, 1)[1].strip().split()
        report: dict[str, int] = {}
        for field in fields:
            match = re.fullmatch(r"([a-z0-9_]+)=([0-9]+)", field)
            if not match:
                raise ValueError(f"invalid render performance field: {field}")
            key = match.group(1)
            if key in report:
                raise ValueError(f"duplicate render performance field: {key}")
            if key not in REQUIRED_METRICS:
                raise ValueError(f"unexpected render performance field: {key}")
            report[key] = int(match.group(2))
        reports.append(report)
    if len(reports) != 1:
        raise ValueError(f"expected one render performance report, found {len(reports)}")
    missing = REQUIRED_METRICS - reports[0].keys()
    if missing:
        raise ValueError(f"render performance report is missing: {', '.join(sorted(missing))}")
    report = reports[0]
    if report["schema"] != 1 or report["warmup"] < 30 or report["samples"] < 120:
        raise ValueError("render performance report has invalid schema or sample bounds")
    if report["total_p50_us"] <= 0 or report["total_p95_us"] < report["total_p50_us"]:
        raise ValueError("render performance total percentiles are invalid")
    if report["draw_calls_min"] <= 0 or report["draw_calls_max"] < report["draw_calls_min"]:
        raise ValueError("render performance draw-call bounds are invalid")
    non_positive_atlas_metrics = sorted(key for key in ATLAS_METRICS if report[key] <= 0)
    if non_positive_atlas_metrics:
        raise ValueError(
            "render performance atlas metrics must be positive: "
            + ", ".join(non_positive_atlas_metrics)
        )
    return report


def build_evidence(log: str, metadata: dict[str, object]) -> dict[str, object]:
    required_metadata = {
        "scene",
        "git_revision",
        "source_dirty",
        "apk_sha256",
        "package_version",
        "device_model",
        "device_fingerprint",
        "serial",
        "avd",
        "android_sdk",
    }
    missing = required_metadata - metadata.keys()
    if missing:
        raise ValueError(f"render performance metadata is missing: {', '.join(sorted(missing))}")
    if any(not str(metadata[key]).strip() for key in required_metadata):
        raise ValueError("render performance metadata contains an empty identity")
    if metadata["source_dirty"] is not False:
        raise ValueError("render performance evidence must identify a clean committed source tree")
    if metadata["android_sdk"] != 35:
        raise ValueError("render performance evidence must use Android API 35")
    return {
        "schema_version": 1,
        "benchmark": "android_workshop_preview_render",
        "metadata": metadata,
        "metrics": parse_report(log),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--max-p50-ms", type=float)
    parser.add_argument("--max-p95-ms", type=float)
    args = parser.parse_args()
    try:
        evidence = build_evidence(
            args.log.read_text(encoding="utf-8", errors="replace"),
            json.loads(args.metadata.read_text(encoding="utf-8-sig")),
        )
        metrics = evidence["metrics"]
        if args.max_p50_ms is not None and metrics["total_p50_us"] > args.max_p50_ms * 1000:
            raise ValueError(
                f"render p50 {metrics['total_p50_us'] / 1000:.3f}ms exceeds "
                f"{args.max_p50_ms:.3f}ms"
            )
        if args.max_p95_ms is not None and metrics["total_p95_us"] > args.max_p95_ms * 1000:
            raise ValueError(
                f"render p95 {metrics['total_p95_us'] / 1000:.3f}ms exceeds "
                f"{args.max_p95_ms:.3f}ms"
            )
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        args.evidence.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
        print(
            "Android Workshop render performance passed: "
            f"p50={metrics['total_p50_us'] / 1000:.3f}ms "
            f"p95={metrics['total_p95_us'] / 1000:.3f}ms"
        )
        return 0
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Android Workshop render performance failed: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Validate the canonical deterministic live simulation roadmap."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Mapping


ROOT = Path(__file__).resolve().parents[2]
ROADMAP = "docs/deterministic_live_simulation_roadmap.md"
ENTRY_LINKS = {
    "README.md": "(docs/deterministic_live_simulation_roadmap.md)",
    "docs/spec.md": "(deterministic_live_simulation_roadmap.md)",
    "docs/live-compilation-prd.md": "(deterministic_live_simulation_roadmap.md)",
    "docs/build_checklist.md": "(deterministic_live_simulation_roadmap.md)",
}
ISSUE_ORDER = (
    146,
    155,
    147,
    148,
    156,
    149,
    150,
    151,
    152,
    153,
    154,
    157,
    158,
    159,
    160,
    161,
    273,
    275,
    276,
)
REQUIRED_HEADINGS = (
    "# Deterministic Live Simulation Roadmap",
    "## Product promise: questions the product must answer",
    "## Product boundaries and ownership",
    "## Determinism profiles",
    "### Strict profile",
    "### Replay profile",
    "### Local profile",
    "## Capability gates and dependencies",
    "## Issue-to-outcome traceability",
    "## Completion gates and evidence",
    "## Exclusions and non-promises",
    "## Document ownership",
)
REQUIRED_PHRASES = (
    "statically bounded simulation",
    "The simulation owns declared state, tick order, bounded collections",
    "The host owns windowing, rendering, audio, filesystem, network",
    "publication still occurs at a defined tick safe point",
    "Strict cross-target simulation state uses integer and Q16.16 deterministic operations",
    "Native float disqualifies cross-architecture strict claims",
    "Replay may include native float only under a recorded same-target/toolchain profile",
    "A replay is evidence of reproducibility",
    "Local is the weakest profile",
    "not a mutable task-status inventory",
    "normative language semantics",
    "hot-swap architecture",
    "temporary sequencing",
    "Focused subsystem documents own detailed contracts",
    "Deterministic headless video recording for dev builds",
    "Captures deterministic game audio in headless recordings",
    "Adds deterministic pre-tick recording hooks and MP3 export",
)
LINK_PATTERN = re.compile(r"\[[^\]]+\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
GATE_PATTERN = re.compile(r"^\|\s*G\d+\s*\|\s*#(\d+)\s*\|")
DEPENDENCY_PATTERN = re.compile(r"^\|\s*G\d+\s*\|\s*#(\d+)\s*\|[^|]*\|\s*([^|]+)\|")


class RoadmapError(ValueError):
    """The canonical roadmap or its integration links are invalid."""


def normalized(text: str) -> str:
    return " ".join(text.split())


def load_documents(root: Path = ROOT) -> dict[str, str]:
    relatives = (ROADMAP, *ENTRY_LINKS)
    return {
        relative: (root / relative).read_text(encoding="utf-8")
        for relative in relatives
    }


def require(text: str, phrase: str, relative: str) -> None:
    if normalized(phrase) not in normalized(text):
        raise RoadmapError(f"{relative}: missing required roadmap text: {phrase!r}")


def validate_links(roadmap: str, root: Path = ROOT) -> None:
    roadmap_path = root / ROADMAP
    for target in LINK_PATTERN.findall(roadmap):
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        relative_target = target.split("#", 1)[0].split("?", 1)[0]
        if not relative_target:
            continue
        resolved = (roadmap_path.parent / relative_target).resolve()
        try:
            resolved.relative_to(root.resolve())
        except ValueError:
            raise RoadmapError(
                f"{ROADMAP}: link escapes repository root: {target!r}"
            ) from None
        if not resolved.is_file():
            raise RoadmapError(
                f"{ROADMAP}: broken repository-relative Markdown link: {target!r}"
            )


def validate_gate_order(roadmap: str) -> None:
    gates = [int(match.group(1)) for line in roadmap.splitlines() if (match := GATE_PATTERN.match(line))]
    if tuple(gates) != ISSUE_ORDER:
        raise RoadmapError(
            f"{ROADMAP}: capability gate issue order is {gates!r}; expected {list(ISSUE_ORDER)!r}"
        )

    positions = {issue: index for index, issue in enumerate(ISSUE_ORDER)}
    for line in roadmap.splitlines():
        match = DEPENDENCY_PATTERN.match(line)
        if not match:
            continue
        issue = int(match.group(1))
        for dependency in re.findall(r"#(\d+)", match.group(2)):
            dependency_id = int(dependency)
            if dependency_id not in positions or positions[dependency_id] >= positions[issue]:
                raise RoadmapError(
                    f"{ROADMAP}: gate #{issue} depends on non-earlier issue #{dependency_id}"
                )


def validate_documents(documents: Mapping[str, str], root: Path = ROOT) -> None:
    if ROADMAP not in documents:
        raise RoadmapError(f"missing required document: {ROADMAP}")
    roadmap = documents[ROADMAP]
    for heading in REQUIRED_HEADINGS:
        require(roadmap, heading, ROADMAP)
    for phrase in REQUIRED_PHRASES:
        require(roadmap, phrase, ROADMAP)
    validate_gate_order(roadmap)
    validate_links(roadmap, root)

    for relative, link in ENTRY_LINKS.items():
        if relative not in documents:
            raise RoadmapError(f"missing required entry document: {relative}")
        if link not in documents[relative]:
            raise RoadmapError(f"{relative}: missing roadmap backlink: {link}")


def main() -> None:
    try:
        validate_documents(load_documents())
    except (OSError, RoadmapError) as error:
        raise SystemExit(str(error)) from error
    print("Deterministic live simulation roadmap contract is consistent")


if __name__ == "__main__":
    main()

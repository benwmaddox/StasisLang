#!/usr/bin/env python3
"""Reject contradictory direct-call generation documentation."""

from pathlib import Path
from typing import Mapping


ROOT = Path(__file__).resolve().parents[2]
DOCUMENTS = (
    "docs/jit_generation_contract.md",
    "docs/live-compilation-prd.md",
    "docs/spec.md",
    "docs/build_checklist.md",
    "docs/shared_cranelift_backend_contract.md",
    "docs/cranelift_backend_guardrails.md",
)


class ContractError(ValueError):
    """A canonical document is missing or contradicts the generation contract."""


def normalized(text: str) -> str:
    return " ".join(text.split())


def require(text: str, needle: str, relative: str) -> None:
    if normalized(needle) not in normalized(text):
        raise ContractError(
            f"{relative}: missing required generation contract text: {needle!r}"
        )


def forbid(text: str, needle: str, relative: str) -> None:
    if normalized(needle) in normalized(text):
        raise ContractError(
            f"{relative}: obsolete generation contract text remains: {needle!r}"
        )


def load_documents(root: Path = ROOT) -> dict[str, str]:
    return {
        relative: (root / relative).read_text(encoding="utf-8")
        for relative in DOCUMENTS
    }


def validate_documents(documents: Mapping[str, str]) -> None:
    contract_path = "docs/jit_generation_contract.md"
    contract = documents[contract_path]
    for heading in (
        "## Non-negotiable invariants",
        "## One generation state machine",
        "## Failure table",
        "## Target and platform matrix",
        "## Performance and memory budgets",
        "## Implementation sequence and bounded verification gates",
        "## Obsolete paths to remove",
    ):
        require(contract, heading, contract_path)

    for invariant in (
        "one `ActiveGeneration` reference",
        "Calls between Stasis functions are direct Cranelift calls",
        "No Stasis frame or raw compiled pointer may outlive its execution window",
        "A failed or superseded build never becomes visible",
        "never run on the runtime thread",
    ):
        require(contract, invariant, contract_path)

    for state_rule in (
        "`Preparing(N,R)` | newer revision queued or supersession observed",
        "`Hook(N,R)` | newer revision queued while the hook is running",
        "`Publishing(N,R)` | current-request compare-and-exchange fails",
        "`Publishing(N,R)` | current-request compare-and-exchange succeeds",
        "a revision ordered after it is a new request based on `N+1`",
    ):
        require(contract, state_rule, contract_path)

    for failure_rule in (
        "Request is superseded during migration or hook execution",
        "Current-request compare-and-exchange fails",
        "Internal unresolved call or cross-generation import",
        "Attempted retained pointer, callback, fiber, or guest thread",
    ):
        require(contract, failure_rule, contract_path)

    for target_rule in (
        "Windows x86_64 PR CI plus the pinned performance runner",
        "Linux x86_64 PR CI",
        "Native x86_64 macOS CI runner",
        "Native arm64 macOS CI runner",
        "Named physical arm64 device for Workshop JIT",
        "standard `Stasis_API_35` AVD is x86_64",
        "JIT_TARGET_MUST_MATCH_HOST",
    ):
        require(contract, target_rule, contract_path)

    for performance_rule in (
        "tests/perf/generation_reference_profile.json",
        "five unmeasured warmups",
        "30 measured samples for 100/1,000 functions",
        "10 measured samples for 5,000 functions and Brickout-scale",
        "nearest-rank p95",
        "Run the parent commit and candidate commit on the same profile",
        "100-swap stress test",
    ):
        require(contract, performance_rule, contract_path)

    for relative in DOCUMENTS[1:]:
        require(documents[relative], contract_path, relative)

    prd_path = "docs/live-compilation-prd.md"
    prd = documents[prd_path]
    require(prd, "Ordinary internal functions are not compatibility boundaries", prd_path)
    require(prd, "global state layout is unchanged or the compiler-owned bounded migration plan is compatible", prd_path)
    require(prd, "May mutate only isolated candidate global data", prd_path)
    require(
        prd,
        "- `main` - `tick` (when present) - `render` (when present) "
        "- `on_code_swap` (when present) - host-required exported entry symbols",
        prd_path,
    )

    spec_path = "docs/spec.md"
    spec = documents[spec_path]
    require(spec, "(`main`, `tick`, `render`, `on_code_swap`)", spec_path)
    require(spec, "May mutate only isolated candidate global data", spec_path)
    require(
        spec,
        "If supersession arrives while a synchronous hook is already running, the hook may finish "
        "only to unwind; all isolated effects are discarded and that candidate never publishes",
        spec_path,
    )

    checklist_path = "docs/build_checklist.md"
    checklist = documents[checklist_path]
    require(
        checklist,
        "Reachability-DCE roots are lifecycle entries present in the program "
        "(`main`, `tick`, `render`, `on_code_swap`) plus host-exported required entry symbols",
        checklist_path,
    )
    for child in ("#174", "#175", "#176", "#177", "#178"):
        require(checklist, f"Maddox task: {child}.", checklist_path)
    require(checklist, "#### Superseded checklist requirements", checklist_path)

    forbidden_by_file = {
        prd_path: (
            "FnId -> code_ptr",
            "fn_patch_set",
            "swapped_fn_ids",
            "global struct layouts are unchanged",
            "function signatures are unchanged",
            "rejection restores the old code and state",
            "Runs once per successful swap attempt",
        ),
        spec_path: (
            "FnId -> code_ptr",
            "fn_patch_set",
            "swapped_fn_ids",
            "Unchanged `fnBodyHash` can reuse generated machine code",
            "failed migration or swap hook restores the complete old generation",
            "runtime restores the old code and complete bounded state snapshot",
            "Superseded candidates never run `on_code_swap()`",
            "Runs once per successful swap attempt",
        ),
    }
    for relative, obsolete_phrases in forbidden_by_file.items():
        for obsolete in obsolete_phrases:
            forbid(documents[relative], obsolete, relative)


def main() -> None:
    try:
        validate_documents(load_documents())
    except ContractError as error:
        raise SystemExit(str(error)) from error
    print("JIT generation documentation contract is consistent")


if __name__ == "__main__":
    main()

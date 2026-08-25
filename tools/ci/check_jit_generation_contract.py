#!/usr/bin/env python3
"""Reject contradictory selective direct-call JIT patch documentation."""

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
    """A canonical document is missing or contradicts the selective patch contract."""


def normalized(text: str) -> str:
    return " ".join(text.split())


def require(text: str, needle: str, relative: str) -> None:
    if normalized(needle) not in normalized(text):
        raise ContractError(
            f"{relative}: missing required selective JIT contract text: {needle!r}"
        )


def forbid(text: str, needle: str, relative: str) -> None:
    if normalized(needle) in normalized(text):
        raise ContractError(
            f"{relative}: superseded whole-generation text remains: {needle!r}"
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
        "## Host-entry boundary and reachability",
        "## Patch contents and exact invalidation",
        "### Worked call-graph examples",
        "## Direct-call patch ABI",
        "## Selective patch state machine",
        "## Failure table",
        "## Target and platform matrix",
        "## Performance and memory budgets",
        "## Implementation sequence and bounded verification gates",
        "## Superseded architecture to remove",
    ):
        require(contract, heading, contract_path)

    for invariant in (
        "Full semantic analysis runs for every changed file",
        "Calls between Stasis functions are direct Cranelift calls",
        "Stable trampolines exist only for host-to-Stasis entries",
        "A warm edit emits only the exact affected reverse-caller closure",
        "Recursive and mutually recursive functions invalidate and emit as strongly connected components",
        "Unaffected reachable bodies keep their addresses",
        "Automatic retirement and compaction are not priority-1 correctness requirements",
        "AOT emits the complete reachable program",
    ):
        require(contract, invariant, contract_path)

    require(
        contract,
        "Reachability starts from lifecycle entries present in the program (`main`, `tick`, "
        "`render`, and `on_code_swap`)",
        contract_path,
    )

    for example in (
        "Editing `C` emits `{C,B,A,H}`",
        "Editing shared `S` emits `{S,A,B,H}`",
        "Editing `S` emits `{S,tick,render}`",
        "Editing either `A` or `B` emits `{A,B,H}`",
        "Editing `A` emits `{A,H}` and reuses `S`",
        "New `A` binds a direct native call to the retained accepted address of `U`",
    ):
        require(contract, example, contract_path)

    message_shapes = (
        "FileChangeEvent(path, revision, text_source, change_kind)",
        "BuildPatch(request_id, revision, source_snapshot_id, target, host_set, active_contract)",
        "BuildFinished(request_id, revision, status, diagnostics[], pending_patch?)",
        "CommitPatch(request_id, pending_patch)",
        "CommitFinished(request_id, status, active_patch_number?, diagnostic?)",
        "CancelBuild(request_id, superseded_by_request_id)",
    )
    for message_shape in message_shapes:
        require(contract, message_shape, contract_path)

    for state_rule in (
        "Exchange one immutable `ActiveEntryTable`",
        "Continue the complete old window",
        "Let synchronous work unwind, discard candidate effects, never publish",
        "freshness compare-and-exchange fails",
        "a new request based on `N+1`",
    ):
        require(contract, state_rule, contract_path)

    for failure_rule in (
        "Invalid or non-minimal affected closure",
        "Missing retained callee address or retained ABI mismatch",
        "Unsupported internal pointer escape",
        "Executable-memory growth during a long dev session",
        "restart the process to reclaim code",
    ):
        require(contract, failure_rule, contract_path)

    for performance_rule in (
        "five unmeasured warmups",
        "30 measured samples for 100/1,000-function and real-game narrow edits",
        "10 measured samples for 5,000 functions and broad shared-helper cases",
        "nearest-rank p95",
        "Chess TD narrow body edits",
        "Commonly fewer than ten functions",
        "Executable-memory retirement has no priority-1 budget",
    ):
        require(contract, performance_rule, contract_path)

    for relative in DOCUMENTS[1:]:
        require(documents[relative], contract_path, relative)

    prd_path = "docs/live-compilation-prd.md"
    prd = documents[prd_path]
    for phrase in (
        "Exact changed/SCC/reverse-caller patch planning",
        "Publication unit: one validated selective patch through the host-entry table",
        "Unaffected machine-code bodies keep their accepted addresses",
        "Warm JIT patches may call unchanged retained bodies",
        "Retain old JIT arenas until a development process restart",
        "Phase P0 (#184)",
        "Phase P4 (#188)",
    ):
        require(prd, phrase, prd_path)

    spec_path = "docs/spec.md"
    spec = documents[spec_path]
    for phrase in (
        "Publication unit: one validated selective patch through stable host-entry trampolines",
        "Unchanged reachable JIT functions may retain their accepted machine code and addresses",
        "Finalize the changed function/SCC plus exact reverse direct callers",
        "Retain superseded JIT code until process restart",
    ):
        require(spec, phrase, spec_path)

    checklist_path = "docs/build_checklist.md"
    checklist = documents[checklist_path]
    for child in ("#184", "#185", "#186", "#187", "#188"):
        require(checklist, f"Maddox task: {child}.", checklist_path)
    for phrase in (
        "### Selective Direct-Call JIT Patch Track",
        "#### Superseded checklist requirements",
        "typical narrow Chess TD edits commonly re-JIT fewer than ten functions",
    ):
        require(checklist, phrase, checklist_path)

    shared_path = "docs/shared_cranelift_backend_contract.md"
    require(
        documents[shared_path],
        "JIT may bind an unchanged accepted callee address from an older retained arena",
        shared_path,
    )
    guardrails_path = "docs/cranelift_backend_guardrails.md"
    require(
        documents[guardrails_path],
        "JIT must reuse unchanged accepted bodies",
        guardrails_path,
    )

    forbidden_by_file = {
        prd_path: (
            "Publication unit: complete reachable generation",
            "Every accepted development build still finalizes one complete reachable machine-code generation",
            "Semantic hashes never reuse live machine code, relocations, or pointers from another generation",
            "Release the old generation when its last execution-window reference ends",
        ),
        spec_path: (
            "Publication unit: complete reachable generation",
            "Live machine code, relocations, and code pointers cannot be reused across generations",
            "Finalize every reachable function into one direct-call `PendingGeneration`",
            "Release the previous generation when its last execution-window owner ends",
        ),
        guardrails_path: (
            "reuse live machine code from a different generation",
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
    print("Selective JIT patch documentation contract is consistent")


if __name__ == "__main__":
    main()

import copy
import unittest

from tools.ci import check_jit_generation_contract as contract_check


class JitGenerationContractCheckTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.documents = contract_check.load_documents()

    def assert_mutation_fails(self, relative: str, old: str, new: str = "") -> None:
        mutated = copy.deepcopy(self.documents)
        self.assertIn(old, mutated[relative])
        mutated[relative] = mutated[relative].replace(old, new, 1)
        with self.assertRaises(contract_check.ContractError):
            contract_check.validate_documents(mutated)

    def test_current_documents_pass(self) -> None:
        contract_check.validate_documents(self.documents)

    def test_whole_generation_publication_is_rejected(self) -> None:
        self.assert_mutation_fails(
            "docs/live-compilation-prd.md",
            "Publication unit: one validated selective patch through the host-entry table",
            "Publication unit: complete reachable generation",
        )

    def test_internal_trampolines_are_rejected(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Stable trampolines exist only for host-to-Stasis entries",
            "Stable trampolines exist for every Stasis function",
        )

    def test_full_changed_file_semantics_are_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Full semantic analysis runs for every changed file",
            "Only changed function bodies are checked",
        )

    def test_exact_reverse_closure_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "A warm edit emits only the exact affected reverse-caller closure",
            "A warm edit emits every reachable body",
        )

    def test_unchanged_body_reuse_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/spec.md",
            "Unchanged reachable JIT functions may retain their accepted machine code and addresses",
            "Live machine code, relocations, and code pointers cannot be reused across generations",
        )

    def test_cross_patch_retained_call_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "New `A` binds a direct native call\n"
            "to the retained accepted address of `U`",
            "New `A` forces unchanged `U` to be recompiled",
        )

    def test_multi_root_atomicity_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Editing `S` emits `{S,tick,render}`",
            "Editing `S` emits `{S,tick}`",
        )

    def test_scc_unit_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Editing either `A` or `B` emits `{A,B,H}`",
            "Editing `A` emits `{A,H}`",
        )

    def test_render_root_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "`main`, `tick`, `render`, and\n`on_code_swap`",
            "`main`, `tick`, and\n+`on_code_swap`",
        )

    def test_build_patch_message_shape_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "BuildPatch(request_id, revision, source_snapshot_id, target, host_set, active_contract)",
            "BuildGeneration(request_id, revision, target)",
        )

    def test_entry_table_exchange_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Exchange one immutable `ActiveEntryTable`",
            "Store each root pointer independently",
        )

    def test_restart_reclamation_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "restart the process to reclaim code",
            "retire every arena immediately",
        )

    def test_retirement_is_not_priority_one(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "Executable-memory retirement has no priority-1 budget",
            "Every patch must reclaim old code",
        )

    def test_chess_td_small_patch_evidence_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/build_checklist.md",
            "typical narrow\n  Chess TD edits commonly re-JIT fewer than ten functions",
            "Chess TD recompiles every reachable function",
        )

    def test_new_task_sequence_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/build_checklist.md",
            "Maddox task: #185.",
            "Maddox task: #175.",
        )


if __name__ == "__main__":
    unittest.main()

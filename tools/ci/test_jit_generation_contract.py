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

    def test_missing_preparing_supersession_transition_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "`Preparing(N,R)` | newer revision queued or supersession observed",
            "`Preparing(N,R)` | source event",
        )

    def test_build_finished_status_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "BuildFinished(request_id, revision, status, diagnostics[], pending_generation?)",
            "BuildFinished(request_id, revision, diagnostics[], pending_generation?)",
        )

    def test_build_generation_snapshot_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "BuildGeneration(request_id, revision, source_snapshot_id, target, host_set, active_contract)",
            "BuildGeneration(request_id, revision, target, host_set, active_contract)",
        )

    def test_file_change_event_shape_is_required(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "FileChangeEvent(path, revision, text_source, change_kind)",
            "SourceChange(revision, changed_files, source_snapshot_id)",
        )

    def test_missing_hook_supersession_transition_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "`Hook(N,R)` | newer revision queued while the hook is running",
            "`Hook(N,R)` | source event",
        )

    def test_old_prd_signature_rule_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/live-compilation-prd.md",
            "Ordinary internal functions are not compatibility boundaries.",
            "function signatures are unchanged",
        )

    def test_missing_render_root_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/build_checklist.md",
            "Reachability-DCE roots are lifecycle entries present in the program (`main`, `tick`, `render`,\n  `on_code_swap`) plus host-exported required entry symbols",
            "Reachability-DCE roots are lifecycle entries present in the program (`main`, `tick`,\n  `on_code_swap`) plus host-exported required entry symbols",
        )

    def test_missing_prd_render_root_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/live-compilation-prd.md",
            "- `render` (when present)\n",
        )

    def test_old_spec_restore_rule_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/spec.md",
            "the runtime destroys the candidate, and the old active\n  code/state remain unchanged",
            "the runtime restores the old code and complete bounded state snapshot",
        )

    def test_missing_benchmark_protocol_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "five unmeasured warmups",
            "some warmups",
        )

    def test_missing_mid_hook_supersession_rule_fails(self) -> None:
        self.assert_mutation_fails(
            "docs/spec.md",
            "If supersession arrives while\n  a synchronous hook is already running, the hook may finish only to unwind; all isolated effects\n  are discarded and that candidate never publishes",
            "A superseded hook does not publish",
        )

    def test_android_arm64_gate_cannot_use_x86_avd(self) -> None:
        self.assert_mutation_fails(
            "docs/jit_generation_contract.md",
            "The repository's standard `Stasis_API_35` AVD is x86_64",
            "The repository's standard `Stasis_API_35` AVD satisfies Android arm64",
        )


if __name__ == "__main__":
    unittest.main()

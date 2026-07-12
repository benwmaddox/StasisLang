#!/usr/bin/env python3
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import android_ai_agent_host as host
import run_android_ai_model_comparison as comparison


class AndroidAiAgentHostTests(unittest.TestCase):
    def test_agent_turn_limit_is_twenty_five(self) -> None:
        self.assertEqual(25, host.MAX_TURNS)

    def test_all_gpt_5_6_comparison_models_have_pricing(self) -> None:
        self.assertEqual(
            {"gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"},
            set(host.DEFAULT_MODEL_PRICING_PER_MILLION),
        )

    def test_comparison_defaults_cover_all_gpt_5_6_models(self) -> None:
        self.assertEqual(
            ("gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"),
            comparison.DEFAULT_MODELS,
        )
        self.assertTrue(comparison.DEFAULT_ACCEPTANCE_TEST.is_file())

    def test_host_run_finishes_before_repository_command_limit(self) -> None:
        self.assertLess(host.DEFAULT_MAX_RUN_SECONDS, 300.0)

    def test_prebuilt_test_runner_command_avoids_cargo_run(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        command = host.stasis_test_command(project)
        self.assertNotEqual("cargo", Path(command[0]).name.lower())
        self.assertNotIn("run", command[1:])
        self.assertIn("stasis", " ".join(command).lower())

    def make_project(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        project = Path(temporary.name)
        (project / "src").mkdir()
        (project / "tests").mkdir()
        (project / "src/main.stasis").write_text(
            "function main(): void {\n}\n\nfunction tick(): void {\n}\n",
            encoding="utf-8",
        )
        return temporary, project

    def test_source_paths_cannot_escape_selected_project(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        with self.assertRaisesRegex(ValueError, "under src"):
            host.source_file_path(project, "../outside.stasis")

    def test_tool_call_batch_limit_matches_android(self) -> None:
        response = {
            "mode": "tool_calls",
            "working_notes": "Intent: inspect. Observed: none. Next: inspect. Blocker: none.",
            "tool_calls": [{"tool": "list_symbols", "args": {}}] * 13,
        }
        calls, errors = host.validate_response_shape(response)
        self.assertEqual([], calls)
        self.assertIn("exceeds 12 calls", errors[0]["error"])

    def test_tool_batch_keys_ignore_object_key_order(self) -> None:
        first = [{"tool": "read_symbol", "args": {"name": "tick", "file": "src/main.stasis"}}]
        second = [{"args": {"file": "src/main.stasis", "name": "tick"}, "tool": "read_symbol"}]
        self.assertEqual(host.tool_call_batch_key(first), host.tool_call_batch_key(second))

    def test_observation_memory_is_deduplicated_and_bounded(self) -> None:
        memory: dict[str, dict] = {}
        for index in range(20):
            host.remember_observations(memory, [{"tool": "read_symbol", "args": {"name": str(index)}, "result": {"source": "x"}}])
        host.remember_observations(memory, [{"tool": "read_symbol", "args": {"name": "19"}, "result": {"source": "new"}}])
        retained = host.retained_observations(memory)
        self.assertEqual(16, len(retained))
        self.assertEqual("new", retained[0]["result"]["source"])

    def test_followup_keeps_shared_context_byte_stable(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        initial = host.build_agent_request(project, "change tick", {"phase": "initial"})
        stable = json.dumps(initial["shared_context"], separators=(",", ":"))
        (project / "src/main.stasis").write_text("function main(): void {\n}\n", encoding="utf-8")
        followup = host.build_followup_request(initial["shared_context"], {"phase": "tools"})
        self.assertEqual(stable, json.dumps(followup["shared_context"], separators=(",", ":")))

    def test_failed_selected_project_compile_rolls_back_source_batch(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        original = (project / "src/main.stasis").read_text(encoding="utf-8")
        replacement = "function tick(): void {\n    missing();\n}"
        with mock.patch.object(host, "run_compile_check", side_effect=[
            {"ok": False, "status": "compile_failed"},
            {"ok": True, "status": "restored"},
        ]):
            observations, diagnostics = host.execute_tool_batch(project, [{
                "tool": "write_symbol",
                "args": {"file": "src/main.stasis", "name": "tick", "new_source": replacement},
            }], {})
        self.assertFalse(diagnostics["ok"])
        self.assertEqual("rolled_back", observations[0]["result"]["status"])
        self.assertEqual(original, (project / "src/main.stasis").read_text(encoding="utf-8"))

    def test_invalid_test_write_is_removed_by_atomic_rollback(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        test_path = project / "tests/new.test.stasis"
        with mock.patch.object(host, "run_compile_check", side_effect=[
            {"ok": True, "status": "compiled"},
            {"ok": True, "status": "restored"},
        ]), mock.patch.object(host, "run_behavior_tests", return_value={
            "kind": "behavior_tests", "ok": False, "status": "tests_failed"
        }):
            observations, diagnostics = host.execute_tool_batch(project, [{
                "tool": "write_test_file",
                "args": {"file": "tests/new.test.stasis", "source": "test `bad`(): bool { return missing; }"},
            }], {})
        self.assertFalse(diagnostics["ok"])
        self.assertEqual("rolled_back", observations[0]["result"]["status"])
        self.assertFalse(test_path.exists())

    def test_run_tests_before_write_observes_completed_batch(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        replacement = "function tick(): void {\n    main();\n}"
        test_result = {"kind": "behavior_tests", "ok": True, "status": "passed"}
        with mock.patch.object(host, "run_compile_check", return_value={"ok": True, "status": "compiled"}), \
                mock.patch.object(host, "run_behavior_tests", return_value=test_result) as run_tests:
            observations, diagnostics = host.execute_tool_batch(project, [
                {"tool": "run_tests", "args": {}},
                {"tool": "write_symbol", "args": {"file": "src/main.stasis", "name": "tick", "new_source": replacement}},
            ], {})
        self.assertEqual(test_result, observations[0]["result"])
        self.assertEqual(test_result, diagnostics)
        self.assertIn("main();", (project / "src/main.stasis").read_text(encoding="utf-8"))
        run_tests.assert_called_once_with(project, compile_first=False)

    def test_rolled_back_writes_do_not_count_as_successful(self) -> None:
        observations = [{"tool": "write_test_file", "args": {}, "result": {"status": "rolled_back"}}]
        self.assertEqual((0, 1), host.write_outcome_counts(observations))
        self.assertFalse(host.can_auto_finalize_tested_writes(True, {"ok": True}, 0))
        self.assertTrue(host.can_auto_finalize_tested_writes(True, {"ok": True}, 1))


if __name__ == "__main__":
    unittest.main()

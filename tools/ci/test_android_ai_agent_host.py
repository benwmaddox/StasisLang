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
    def test_agent_turn_limit_is_fifteen(self) -> None:
        self.assertEqual(15, host.MAX_TURNS)

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

    def test_comparison_reports_acceptance_ratio_and_cache_rate(self) -> None:
        trace = {
            "meta": {"model": "gpt-5.6-luna", "total_actions": 2},
            "usage_summary": {"calls": 1, "totals": {"input_tokens": 100, "cached_input_tokens": 60}},
            "events": [
                {"kind": "openai_exchange", "response": {"response_model": "gpt-5.6-luna"}},
                {"kind": "response_validation_errors", "errors": [{}]},
                {"kind": "tool_observations", "summary": [
                    {"tool": "write_symbol", "status": "rolled_back"},
                    {"tool": "write_test_file", "status": "rolled_back"},
                ]},
            ],
            "exit_code": 0,
        }
        row = comparison.summarize(trace, 0, {
            "ok": False,
            "acceptance_tests_passed": 3,
            "acceptance_tests_total": 4,
        })
        self.assertEqual((3, 4), (row["acceptance_tests_passed"], row["acceptance_tests_total"]))
        self.assertEqual(60.0, row["cached_input_percent"])
        self.assertEqual(1, row["validation_retries"])
        self.assertEqual(1, row["rollback_batches"])

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
            "tool_calls": [{"tool": "list_symbols", "args": {}}] * 51,
        }
        calls, errors = host.validate_response_shape(response)
        self.assertEqual([], calls)
        self.assertIn("exceeds 50 calls", errors[0]["error"])

    def test_fifty_tool_calls_are_accepted(self) -> None:
        response = {
            "mode": "tool_calls",
            "working_notes": "Intent: inspect. Observed: targets. Next: read. Blocker: none.",
            "tool_calls": [{"tool": "read_symbol", "args": {"name": "tick"}}] * 50,
        }
        calls, errors = host.validate_response_shape(response)
        self.assertEqual([], errors)
        self.assertEqual(50, len(calls))

    def test_empty_irrelevant_action_array_is_harmless(self) -> None:
        response = {
            "mode": "tool_calls",
            "working_notes": "Intent: inspect. Observed: none. Next: inspect. Blocker: none.",
            "tool_calls": [{"tool": "list_symbols", "args": {}}],
            "edits": [],
        }
        calls, errors = host.validate_response_shape(response)
        self.assertEqual([], errors)
        self.assertEqual("list_symbols", calls[0]["tool"])
        response["edits"] = [{"name": "conflict"}]
        self.assertTrue(host.validate_response_shape(response)[1])

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

    def test_successful_write_memory_omits_source_and_hash(self) -> None:
        memory: dict[str, dict] = {}
        host.remember_observations(memory, [{
            "tool": "write_symbol",
            "args": {"name": "tick", "new_source": "function tick(): void {}"},
            "result": {"status": "written"},
        }])
        args = host.retained_observations(memory)[0]["args"]
        self.assertNotIn("new_source", args)
        self.assertNotIn("new_source_sha256", args)
        self.assertEqual(24, args["new_source_chars"])

    def test_initial_symbols_target_entry_and_direct_imports(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        (project / "src/main.stasis").write_text(
            'import "paddle.stasis";\nfunction main(): void {}\nfunction tick(): void {}\n',
            encoding="utf-8",
        )
        (project / "src/paddle.stasis").write_text(
            "function update_paddle(): void {}\n", encoding="utf-8")
        (project / "src/unrelated.stasis").write_text(
            "function unrelated(): void {}\n", encoding="utf-8")
        index = host.build_shared_context(project, "resize paddle")["project_context"]["project_symbol_index"]
        self.assertEqual(["src/main.stasis", "src/paddle.stasis"], index["files"])
        self.assertEqual(["src/paddle.stasis"], index["imports"]["src/main.stasis"])
        self.assertNotIn("unrelated", [item["name"] for item in index["symbols"]])

    def test_list_symbols_filters_and_references_are_compact(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        (project / "src/main.stasis").write_text(
            "global GameState { paddle_y: i32; }\n"
            "function tick(): void { GameState.paddle_y += 1; }\n"
            "function render(): void { let y = GameState.paddle_y; }\n",
            encoding="utf-8",
        )
        listing = host.compact_symbol_listing(project, {"query": "tick"})
        self.assertEqual(["tick"], [item["name"] for item in listing["items"]])
        references = host.find_symbol_references(project, "GameState.paddle_y")
        self.assertEqual({"write", "read"}, {item["kind"] for item in references["references"]})
        self.assertTrue(all("source" not in item and "source_hash" not in item
                            for item in references["references"]))

    def test_followup_keeps_shared_context_byte_stable(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        initial = host.build_agent_request(project, "change tick", {"phase": "initial"})
        stable = json.dumps(initial["shared_context"], separators=(",", ":"))
        (project / "src/main.stasis").write_text("function main(): void {\n}\n", encoding="utf-8")
        followup = host.build_followup_request(initial["shared_context"], {"phase": "tools"})
        self.assertEqual(stable, json.dumps(followup["shared_context"], separators=(",", ":")))

    def test_behavior_expectations_are_request_generic(self) -> None:
        expectations = host.behavior_test_expectations()
        encoded = json.dumps(expectations)
        self.assertNotIn("enemy_paddle", encoded)
        self.assertNotIn("1500", encoded)
        self.assertIn("both sides", encoded)

    def test_shared_context_includes_geometry_invariants(self) -> None:
        temporary, project = self.make_project()
        self.addCleanup(temporary.cleanup)
        context = host.build_shared_context(project, "resize a sprite")
        recommendations = context["workflow_rules"]["architecture_recommendations"]
        self.assertTrue(any("rendered rectangles as one contract" in item for item in recommendations))
        self.assertTrue(any("just-inside" in item for item in recommendations))

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

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


class AndroidAiAgentHostTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()

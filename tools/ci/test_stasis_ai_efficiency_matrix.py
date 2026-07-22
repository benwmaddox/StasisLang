import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("matrix", ROOT / "tools/run_stasis_ai_efficiency_matrix.py")
matrix = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = matrix
SPEC.loader.exec_module(matrix)


class StasisAiEfficiencyMatrixTests(unittest.TestCase):
    def test_scaled_projects_are_deterministic_and_agent_ready(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            expected = {"small": 0, "medium": 200, "large": 1600}
            for scale, padding_functions in expected.items():
                project = root / scale
                stats = matrix.prepare_project(project, scale)
                manifest = json.loads((project / "stasis.json").read_text(encoding="utf-8"))
                self.assertEqual(padding_functions, stats["padding_functions"])
                self.assertIn("stasis --json symbol list", (project / "AGENTS.md").read_text(encoding="utf-8"))
                self.assertEqual("src/main.stasis" if scale == "small" else "src/eval_entry.stasis", manifest["entry"])
                if scale != "small":
                    entry = (project / manifest["entry"]).read_text(encoding="utf-8")
                    self.assertIn('import "main.stasis";', entry)

    def test_usage_parser_supports_codex_and_stasis_usage_lines(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "usage.jsonl"
            path.write_text(
                '{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}}\n'
                '{"input_tokens":20,"cached_input_tokens":10,"cache_write_input_tokens":3,"output_tokens":5}\n',
                encoding="utf-8",
            )
            self.assertEqual(30, matrix.usage_from_jsonl(path, last_only=False)["input_tokens"])
            self.assertEqual(20, matrix.usage_from_jsonl(path, last_only=True)["input_tokens"])

    def test_cost_uses_cached_input_at_ten_percent(self) -> None:
        cost = matrix.estimated_cost({
            "input_tokens": 1_000_000,
            "cached_input_tokens": 1_000_000,
            "cache_write_input_tokens": 0,
            "output_tokens": 0,
        })
        self.assertEqual(0.5, cost)

    def test_event_counts_do_not_double_count_started_commands(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            path.write_text(
                '{"type":"item.started","item":{"type":"command_execution"}}\n'
                '{"type":"item.completed","item":{"type":"command_execution"}}\n'
                '{"event":"tool_calls","calls":[{"tool":"read_symbol"},{"tool":"find_references"}]}\n',
                encoding="utf-8",
            )
            self.assertEqual(1, matrix.count_events(path, "item.completed", "command_execution"))
            self.assertEqual(2, matrix.count_events(path, "tool_calls"))


if __name__ == "__main__":
    unittest.main()

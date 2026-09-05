"""Source-level guard for Cargo commands in PR CI (no YAML dependency)."""

import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
WRAPPED = re.compile(r"\bpython3?\s+tools/cargo_cache\.py\s+run\s+--\s+cargo(?:\.exe)?\b")
CARGO = re.compile(r"(?<![\w/-])cargo(?:\.exe)?(?=[\s\"']|$)")


def raw_cargo_lines(source):
    """Conservatively reject unwrapped Cargo tokens in run scalar source."""
    violations = []
    run_indent = None
    for number, line in enumerate(source.splitlines(), 1):
        stripped = line.lstrip()
        if not stripped or stripped.startswith("#"):
            continue
        indent = len(line) - len(stripped)
        if run_indent is not None and indent <= run_indent:
            run_indent = None
        match = re.match(r"(?:-\s+)?run:\s*(.*)", stripped)
        if match:
            run_indent = indent
            stripped = match.group(1)
        elif run_indent is None:
            continue
        if CARGO.search(WRAPPED.sub("wrapped_command", stripped)):
            violations.append(number)
    return violations


class PrCiCargoPolicyTests(unittest.TestCase):
    def test_pr_ci_routes_all_cargo_through_cache(self):
        source = (ROOT / ".github/workflows/pr-ci.yml").read_text(encoding="utf-8")
        self.assertEqual(raw_cargo_lines(source), [], "raw Cargo run lines")

    def test_rejects_inline_block_folded_and_chained_commands(self):
        for command in (
            "cargo test", "cargo +stable build", "cargo.exe check",
            "FOO=1 cargo test", "& cargo test", "echo ready && cargo build",
            "python tools/cargo_cache.py run -- cargo test; cargo build",
        ):
            for scalar in (command, "|\n    " + command, ">-\n    " + command):
                with self.subTest(command=command, scalar=scalar):
                    self.assertTrue(raw_cargo_lines("  run: " + scalar))

    def test_accepts_wrapper_and_ignores_non_run_metadata(self):
        source = """  - name: cargo test
    run: |
      # cargo build is forbidden
      python tools/cargo_cache.py run -- cargo test
      python3 tools/cargo_cache.py run -- cargo build
    env:
      DESCRIPTION: cargo test
"""
        self.assertEqual(raw_cargo_lines(source), [])

    def test_rejects_run_as_first_step_key(self):
        self.assertEqual(raw_cargo_lines("  - run: cargo check"), [1])

    def test_staging_uses_shared_cargo_target(self):
        source = (ROOT / ".github/workflows/pr-ci.yml").read_text(encoding="utf-8")
        self.assertIn(
            "cp build/codex-cargo-target/debug/stasis target/vscode-e2e-toolchain/bin/",
            source,
        )
        self.assertNotIn("cp target/debug/stasis", source)


if __name__ == "__main__":
    unittest.main()

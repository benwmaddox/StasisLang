import os
import tempfile
import unittest
from pathlib import Path

from tools import cargo_cache


ROOT = Path(__file__).resolve().parents[2]


class CargoCacheTests(unittest.TestCase):
    def test_repository_validation_routes_cargo_through_shared_policy(self) -> None:
        validation = (ROOT / "tools" / "validate_repo.sh").read_text(encoding="utf-8")

        self.assertIn(
            "python3 tools/cargo_cache.py run -- cargo test --workspace --all-targets",
            validation,
        )

    def test_precommit_routes_cargo_through_shared_policy(self) -> None:
        hook = (ROOT / ".githooks" / "pre-commit.ps1").read_text(encoding="utf-8")

        self.assertEqual(hook.count("$cargoPolicy run -- cargo"), 2)
        self.assertNotIn("& cargo ", hook)

    def test_shared_target_is_owned_by_common_repository(self) -> None:
        common_git_dir = Path("/repo/.git")

        target = cargo_cache.shared_target_for(common_git_dir)

        self.assertEqual(target, Path("/repo/build/codex-cargo-target"))

    def test_agent_environment_is_isolated_and_preserves_explicit_target(self) -> None:
        parent = {
            "CARGO_INCREMENTAL": "1",
            "CARGO_TARGET_DIR": "/caller/target",
            "UNCHANGED": "yes",
        }

        child = cargo_cache.agent_environment(parent, Path("/repo/build/codex-cargo-target"))

        self.assertEqual(child["CARGO_INCREMENTAL"], "0")
        self.assertEqual(child["CARGO_TARGET_DIR"], "/caller/target")
        self.assertEqual(child["UNCHANGED"], "yes")
        self.assertEqual(parent["CARGO_INCREMENTAL"], "1")

    def test_measure_target_reports_profiles_and_incremental_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            target = Path(temp_dir) / "target"
            (target / "debug" / "deps").mkdir(parents=True)
            (target / "debug" / "incremental" / "unit").mkdir(parents=True)
            (target / "release").mkdir()
            (target / "debug" / "deps" / "lib.rlib").write_bytes(b"d" * 7)
            (target / "debug" / "incremental" / "unit" / "state.bin").write_bytes(
                b"i" * 11
            )
            (target / "release" / "app").write_bytes(b"r" * 13)

            report = cargo_cache.measure_target(target)

        self.assertEqual(report["bytes"], 31)
        self.assertEqual(
            report["profiles"],
            [
                {"name": "debug", "bytes": 18, "incremental_bytes": 11},
                {"name": "release", "bytes": 13, "incremental_bytes": 0},
            ],
        )

    def test_cleanup_defaults_to_dry_run(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            worktree = Path(temp_dir) / "worktree"
            target = worktree / "target"
            target.mkdir(parents=True)
            artifact = target / "artifact"
            artifact.write_text("keep", encoding="utf-8")

            removed = cargo_cache.clean_target(worktree, target, incremental_only=False, apply=False)

            self.assertEqual(removed, [target.resolve()])
            self.assertTrue(artifact.exists())

    def test_incremental_cleanup_removes_only_incremental_directories(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            worktree = Path(temp_dir) / "worktree"
            target = worktree / "target"
            incremental = target / "debug" / "incremental"
            deps = target / "debug" / "deps"
            incremental.mkdir(parents=True)
            deps.mkdir()
            (incremental / "state").write_text("delete", encoding="utf-8")
            (deps / "lib").write_text("keep", encoding="utf-8")

            removed = cargo_cache.clean_target(worktree, target, incremental_only=True, apply=True)

            self.assertEqual(removed, [incremental.resolve()])
            self.assertFalse(incremental.exists())
            self.assertTrue((deps / "lib").exists())

    def test_cleanup_rejects_target_outside_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            worktree = root / "worktree"
            outside = root / "outside" / "target"
            worktree.mkdir()
            outside.mkdir(parents=True)

            with self.assertRaisesRegex(ValueError, "exact worktree target"):
                cargo_cache.clean_target(
                    worktree, outside, incremental_only=False, apply=True
                )

            self.assertTrue(outside.exists())

    @unittest.skipUnless(os.name != "nt" or hasattr(os, "symlink"), "symlinks unavailable")
    def test_cleanup_rejects_target_symlink_escape(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            worktree = root / "worktree"
            outside = root / "outside"
            worktree.mkdir()
            outside.mkdir()
            try:
                (worktree / "target").symlink_to(outside, target_is_directory=True)
            except OSError as error:
                self.skipTest(f"symlink creation unavailable: {error}")

            with self.assertRaisesRegex(ValueError, "escapes worktree"):
                cargo_cache.clean_target(
                    worktree,
                    worktree / "target",
                    incremental_only=False,
                    apply=True,
                )

            self.assertTrue(outside.exists())


if __name__ == "__main__":
    unittest.main()

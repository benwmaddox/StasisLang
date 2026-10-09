import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from tools import cargo_cache
from tools.ci import verify_staged_live_ttt_fixtures


ROOT = Path(__file__).resolve().parents[2]


class CargoCacheTests(unittest.TestCase):
    def test_local_automation_replaces_inherited_hook_without_skipping_signing(self):
        parent = {
            "STASIS_AOT_SIGN_TOOL": "C:/personal tools/sign.cmd",
            "STASIS_SIGNING_CERT_THUMBPRINT": "ABC",
        }
        child = cargo_cache.agent_environment(parent, Path("target"), windows=True)
        self.assertNotIn("STASIS_AOT_SIGN_TOOL", child)
        self.assertEqual(child["STASIS_REQUIRE_SIGNED_EXECUTION"], "1")
        self.assertEqual(child["STASIS_SIGNING_CERT_THUMBPRINT"], "ABC")
        self.assertIn("STASIS_AOT_SIGN_TOOL", parent)

    def test_hook_only_configuration_still_requires_signing(self):
        child = cargo_cache.agent_environment(
            {"STASIS_AOT_SIGN_TOOL": "old.cmd"}, Path("target"), windows=True
        )
        self.assertTrue(cargo_cache._signing_is_configured(child))

    def test_non_windows_automation_preserves_hook(self):
        child = cargo_cache.agent_environment(
            {"STASIS_AOT_SIGN_TOOL": "sign.sh"}, Path("target"), windows=False
        )
        self.assertEqual(child["STASIS_AOT_SIGN_TOOL"], "sign.sh")

    def test_production_automation_preserves_explicit_signer(self):
        for setting in ("STASIS_SIGNING_MODE", "STASIS_SIGNING_PROFILE"):
            parent = {setting: "production", "STASIS_AOT_SIGN_TOOL": "release.cmd"}
            child = cargo_cache.agent_environment(parent, Path("target"), windows=True)
            self.assertEqual(child["STASIS_AOT_SIGN_TOOL"], "release.cmd")
            self.assertNotIn("STASIS_REQUIRE_SIGNED_EXECUTION", child)

    def test_repository_validation_routes_cargo_through_shared_policy(self) -> None:
        validation = (ROOT / "tools" / "validate_repo.sh").read_text(encoding="utf-8")

        self.assertIn(
            '"$PYTHON" tools/cargo_cache.py run -- cargo test --workspace --all-targets',
            validation,
        )

    def test_precommit_routes_cargo_through_shared_policy(self) -> None:
        hook = (ROOT / ".githooks" / "pre-commit.ps1").read_text(encoding="utf-8")

        self.assertEqual(hook.count("$cargoPolicy run -- cargo"), 1)
        self.assertIn("cargo run --quiet -p stasis -- format", hook)
        self.assertIn("verify_staged_live_ttt_fixtures.py", hook)
        self.assertIn("Where-Object { $_ -notin $frozenStasis }", hook)
        self.assertIn('$env:VCToolsInstallDir', hook)
        self.assertIn('bin\\Hostx64\\x64', hook)
        self.assertIn('Test-Path -LiteralPath $msvcLinkExe -PathType Leaf', hook)
        self.assertIn('$env:PATH = "$msvcLinkDirectory;$env:PATH"', hook)
        self.assertLess(
            hook.index('$env:PATH = "$msvcLinkDirectory;$env:PATH"'),
            hook.index('& python $cargoPolicy run -- cargo'),
        )
        self.assertLess(
            hook.index("verify_staged_live_ttt_fixtures.py"),
            hook.index("git diff --cached --name-only"),
        )
        self.assertNotIn("cargo test", hook)
        self.assertNotIn("staged_repository_stasis_sources_are_formatted", hook)
        self.assertNotIn("& cargo ", hook)

    def test_staged_fixture_pin_accepts_only_exact_provenance_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._initialize_fixture_index(repo)

            validated = verify_staged_live_ttt_fixtures.verify_staged_fixtures(repo)

            self.assertEqual(validated, list(verify_staged_live_ttt_fixtures.SOURCE_HASHES))

    def test_staged_fixture_change_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._initialize_fixture_index(repo)
            source = next(iter(verify_staged_live_ttt_fixtures.SOURCE_HASHES))
            source_path = repo / Path(source)
            source_path.write_bytes(source_path.read_bytes() + b"\r\n")
            self._git(repo, "add", source)

            with self.assertRaisesRegex(
                verify_staged_live_ttt_fixtures.VerificationError,
                "staged fixture bytes differ",
            ):
                verify_staged_live_ttt_fixtures.verify_staged_fixtures(repo)

    def test_staged_provenance_change_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._initialize_fixture_index(repo)
            manifest_path = repo / verify_staged_live_ttt_fixtures.MANIFEST_PATH
            manifest_path.write_bytes(manifest_path.read_bytes().replace(b"cb43e41a", b"00000000"))
            self._git(repo, "add", verify_staged_live_ttt_fixtures.MANIFEST_PATH)

            with self.assertRaisesRegex(
                verify_staged_live_ttt_fixtures.VerificationError,
                "manifest differs from the trusted pin",
            ):
                verify_staged_live_ttt_fixtures.verify_staged_fixtures(repo)

    def test_staged_fixture_attribute_change_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._initialize_fixture_index(repo)
            attributes_path = repo / verify_staged_live_ttt_fixtures.ATTRIBUTES_PATH
            attributes_path.write_bytes(attributes_path.read_bytes().replace(b"-text", b"text", 1))
            self._git(repo, "add", verify_staged_live_ttt_fixtures.ATTRIBUTES_PATH)

            with self.assertRaisesRegex(
                verify_staged_live_ttt_fixtures.VerificationError,
                "attributes differ from the trusted pin",
            ):
                verify_staged_live_ttt_fixtures.verify_staged_fixtures(repo)

    def test_staged_rename_of_pinned_fixture_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = Path(temp_dir)
            self._initialize_fixture_index(repo)
            self._git(repo, "commit", "-m", "fixture baseline")
            source = next(iter(verify_staged_live_ttt_fixtures.SOURCE_HASHES))
            destination = source.removesuffix("protocol.stasis") + "renamed_protocol.stasis"
            self._git(repo, "mv", source, destination)
            default_rename_paths = subprocess.run(
                ["git", "diff", "--cached", "--name-only", "--diff-filter=ACMRD", "-z"],
                cwd=repo,
                check=True,
                stdout=subprocess.PIPE,
            ).stdout.split(b"\0")
            self.assertEqual(
                {path.decode("utf-8") for path in default_rename_paths if path},
                {destination},
            )

            with self.assertRaises(verify_staged_live_ttt_fixtures.VerificationError):
                verify_staged_live_ttt_fixtures.verify_staged_fixtures(repo)

    @staticmethod
    def _git(repo: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=repo, check=True, stdout=subprocess.PIPE)

    @classmethod
    def _initialize_fixture_index(cls, repo: Path) -> None:
        cls._git(repo, "init", "-q")
        cls._git(repo, "config", "user.email", "codex-test@example.invalid")
        cls._git(repo, "config", "user.name", "Codex Test")
        cls._git(repo, "config", "core.autocrlf", "false")
        fixture_paths = [
            verify_staged_live_ttt_fixtures.MANIFEST_PATH,
            verify_staged_live_ttt_fixtures.ATTRIBUTES_PATH,
            *verify_staged_live_ttt_fixtures.SOURCE_HASHES,
        ]
        for relative in fixture_paths:
            source = ROOT / Path(relative)
            destination = repo / Path(relative)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        cls._git(repo, "add", "--", *fixture_paths)

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

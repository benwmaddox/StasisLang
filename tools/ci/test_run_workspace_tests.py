import subprocess
import sys
import tempfile
import time
import unittest
from collections import Counter
from pathlib import Path
from unittest import mock

from tools.ci import run_workspace_tests as workspace_tests


ROOT = Path(__file__).resolve().parents[2]


def target(name, *kind):
    return {"name": name, "kind": list(kind)}


def metadata(*packages):
    return {
        "workspace_members": [package["id"] for package in packages],
        "packages": list(packages),
    }


def package(name, *targets):
    return {"id": f"workspace#{name}", "name": name, "targets": list(targets)}


class WorkspaceTestPlanTests(unittest.TestCase):
    def setUp(self):
        self.metadata = metadata(
            package(
                "stasis",
                target("stasis", "lib"),
                target("stasis", "bin"),
                target("preview", "example"),
                target("desktop_display", "test"),
                target("windows_game_launch", "test"),
                target("build-script-build", "custom-build"),
            ),
            package(
                "stasis_compiler",
                target("stasis_compiler", "lib"),
                target("compiler_bench", "bench"),
                target("compiler_contract", "test"),
            ),
            package(
                "stasis_network",
                target("stasis_network", "rlib", "staticlib", "cdylib"),
                target("supervise", "bin"),
                target("native_client", "test"),
            ),
        )

    def test_plan_covers_every_selectable_target_once_in_four_semantic_shards(self):
        shards = workspace_tests.plan_workspace_shards(self.metadata)

        self.assertEqual(
            [
                "workspace-core",
                "stasis-integrations",
                "windows-game-launch",
                "other-integrations",
            ],
            [shard.name for shard in shards],
        )
        covered = [item for shard in shards for item in shard.targets]
        self.assertEqual(len(covered), len(set(covered)))
        self.assertEqual(
            Counter(
                (package["name"], item["name"])
                for package in self.metadata["packages"]
                for item in package["targets"]
                if item["kind"] != ["custom-build"]
            ),
            Counter((package, target) for package, target, _ in covered),
        )
        self.assertIn("--benches", shards[0].cargo_arguments)
        for shard in shards:
            self.assertIn("--workspace", shard.cargo_arguments)
            self.assertNotIn("-p", shard.cargo_arguments)
            self.assertNotIn("--exclude", shard.cargo_arguments)

    def test_duplicate_integration_target_names_fail_before_global_selection(self):
        self.metadata["packages"][0]["targets"].append(
            target("compiler_contract", "test")
        )

        with self.assertRaisesRegex(
            ValueError, "duplicate integration target name compiler_contract"
        ):
            workspace_tests.plan_workspace_shards(self.metadata)

    def test_every_cargo_command_uses_cache_and_serial_test_execution(self):
        for shard in workspace_tests.plan_workspace_shards(self.metadata):
            command = shard.command(ROOT)
            self.assertEqual(sys.executable, command[0])
            self.assertEqual(str(ROOT / "tools/cargo_cache.py"), command[1])
            self.assertEqual(["run", "--", "cargo", "test"], command[2:6])
            self.assertEqual(["--", "--test-threads=1"], command[-2:])
        self.assertIn(
            "--benches",
            workspace_tests.plan_workspace_shards(self.metadata)[0].cargo_arguments,
        )

    @mock.patch.object(workspace_tests.subprocess, "Popen")
    def test_metadata_inventory_also_uses_the_cache_wrapper(self, popen):
        process = popen.return_value
        process.communicate.return_value = (
            'signing diagnostic\n{"packages": [], "workspace_members": []}\n',
            "",
        )
        process.returncode = 0

        workspace_tests.load_metadata(ROOT)

        command = popen.call_args.args[0]
        self.assertEqual(
            [
                sys.executable,
                str(ROOT / "tools/cargo_cache.py"),
                "run",
                "--",
                "cargo",
                "metadata",
            ],
            command[:6],
        )

    @mock.patch.object(workspace_tests, "terminate_process_tree")
    @mock.patch.object(workspace_tests.subprocess, "Popen")
    def test_interrupted_metadata_inventory_terminates_its_tree(self, popen, terminate):
        process = popen.return_value
        process.communicate.side_effect = KeyboardInterrupt

        with self.assertRaises(KeyboardInterrupt):
            workspace_tests.load_metadata(ROOT)

        terminate.assert_called_once_with(process)

    def test_unknown_target_kind_fails_instead_of_losing_coverage(self):
        self.metadata["packages"][2]["targets"].append(target("mystery", "future-kind"))

        with self.assertRaisesRegex(ValueError, "future-kind.*stasis_network:mystery"):
            workspace_tests.plan_workspace_shards(self.metadata)

    def test_missing_calibrated_windows_shard_fails_actionably(self):
        self.metadata["packages"][0]["targets"] = [
            item
            for item in self.metadata["packages"][0]["targets"]
            if item["name"] != "windows_game_launch"
        ]

        with self.assertRaisesRegex(ValueError, "windows_game_launch"):
            workspace_tests.plan_workspace_shards(self.metadata)

    @mock.patch.object(workspace_tests, "terminate_process_tree")
    @mock.patch.object(workspace_tests.subprocess, "Popen")
    def test_timed_out_shard_terminates_its_owned_process_tree(self, popen, terminate):
        process = popen.return_value
        process.wait.side_effect = subprocess.TimeoutExpired(["cargo"], 900)

        result = workspace_tests.run_bounded(["cargo"], timeout_seconds=900)

        self.assertEqual(124, result.exit_code)
        self.assertTrue(result.timed_out)
        terminate.assert_called_once_with(process)

    @mock.patch.object(workspace_tests, "terminate_process_tree")
    @mock.patch.object(workspace_tests.subprocess, "Popen")
    def test_nonzero_shard_exit_is_returned_without_timeout_cleanup(self, popen, terminate):
        process = popen.return_value
        process.wait.return_value = 17

        result = workspace_tests.run_bounded(["cargo"], timeout_seconds=900)

        self.assertEqual(17, result.exit_code)
        self.assertFalse(result.timed_out)
        terminate.assert_not_called()

    def test_process_tree_termination_stops_a_real_child(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_path = Path(directory) / "pids.txt"
            child_script = "import time; time.sleep(60)"
            parent_script = (
                "import os, pathlib, subprocess, sys, time; "
                f"child = subprocess.Popen([sys.executable, '-c', {child_script!r}]); "
                f"pid_path = pathlib.Path({str(pid_path)!r}); "
                "staging = pid_path.with_suffix('.tmp'); "
                "staging.write_text(f'{os.getpid()} {child.pid}', encoding='utf-8'); "
                "staging.replace(pid_path); "
                "time.sleep(60)"
            )
            parent = subprocess.Popen(
                [sys.executable, "-c", parent_script],
                **workspace_tests._popen_options(),
            )
            child_pid = None
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not pid_path.is_file():
                    time.sleep(0.01)
                self.assertTrue(pid_path.is_file(), "parent-child PID handshake timed out")
                parent_pid, child_pid = map(
                    int, pid_path.read_text(encoding="utf-8").split()
                )
                self.assertEqual(parent.pid, parent_pid)

                workspace_tests.terminate_process_tree(parent)

                self.assertIsNotNone(parent.poll())
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and self._process_is_running(child_pid):
                    time.sleep(0.05)
                self.assertFalse(self._process_is_running(child_pid))
            finally:
                if parent.poll() is None:
                    workspace_tests.terminate_process_tree(parent)
                if child_pid is not None and self._process_is_running(child_pid):
                    if sys.platform == "win32":
                        subprocess.run(
                            ["taskkill", "/PID", str(child_pid), "/T", "/F"],
                            check=False,
                            capture_output=True,
                        )
                    else:
                        import os
                        import signal

                        os.kill(child_pid, signal.SIGKILL)

    @staticmethod
    def _process_is_running(pid):
        if sys.platform == "win32":
            result = subprocess.run(
                ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
                check=False,
                capture_output=True,
                text=True,
            )
            return result.returncode == 0 and not result.stdout.lstrip().startswith("INFO:")
        import os

        stat = Path(f"/proc/{pid}/stat")
        if stat.is_file():
            try:
                if stat.read_text(encoding="utf-8").split()[2] == "Z":
                    return False
            except (OSError, IndexError):
                pass
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        return True


if __name__ == "__main__":
    unittest.main()

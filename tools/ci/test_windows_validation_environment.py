from __future__ import annotations

import io
import json
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import Mock

from tools.windows import prepare_validation


ROOT = Path(__file__).resolve().parents[2]
TEST_TEMP_ROOT = ROOT / "target" / "windows-validation-test-tmp"
MSVC_TOOLCHAIN = "stable-x86_64-pc-windows-msvc"
MSVC_TARGET = "x86_64-pc-windows-msvc"
GNU_TARGET = "x86_64-pc-windows-gnu"


def rustup_runner(
    *,
    active: str,
    hosts: dict[str, str],
    targets: dict[str, set[str]] | None = None,
    missing: set[str] | None = None,
):
    targets = targets or {}
    missing = missing or set()

    def run(arguments, **_kwargs):
        if arguments[0] != "rustup.exe":
            raise AssertionError(f"unexpected command: {arguments}")
        args = arguments[1:]
        if args == ["show", "active-toolchain"]:
            return subprocess.CompletedProcess(arguments, 0, f"{active} (default)\n", "")
        if len(args) == 4 and args[0] == "run" and args[2:] == ["rustc", "-vV"]:
            toolchain = args[1]
            if toolchain in missing or toolchain not in hosts:
                return subprocess.CompletedProcess(
                    arguments, 1, "", f"toolchain '{toolchain}' is not installed\n"
                )
            output = f"rustc 1.92.0\nhost: {hosts[toolchain]}\n"
            return subprocess.CompletedProcess(arguments, 0, output, "")
        if len(args) >= 4 and args[:3] == ["target", "list", "--installed"]:
            toolchain = args[args.index("--toolchain") + 1]
            if toolchain in missing or toolchain not in hosts:
                return subprocess.CompletedProcess(
                    arguments, 1, "", f"toolchain '{toolchain}' is not installed\n"
                )
            installed = targets.get(toolchain)
            if installed is None:
                host = hosts[toolchain]
                installed = {host} if host.endswith("-windows-msvc") else set()
            output = "".join(f"{target}\n" for target in sorted(installed))
            return subprocess.CompletedProcess(arguments, 0, output, "")
        raise AssertionError(f"unexpected rustup command: {args}")

    return run


def vswhere_fixture(root: Path, records: list[dict[str, str]], *, vcvars: set[str] | None = None):
    program_files = root / "Program Files (x86)"
    vswhere = program_files / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    vswhere.parent.mkdir(parents=True)
    vswhere.touch()
    vcvars = vcvars if vcvars is not None else {record["installationPath"] for record in records}
    for path in vcvars:
        vcvars_path = Path(path) / "VC" / "Auxiliary" / "Build" / "vcvars64.bat"
        vcvars_path.parent.mkdir(parents=True, exist_ok=True)
        vcvars_path.touch()
    return {"ProgramFiles(x86)": str(program_files)}, vswhere


class WindowsValidationEnvironmentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        TEST_TEMP_ROOT.mkdir(parents=True, exist_ok=True)

    def test_active_msvc_keeps_host_target_unset(self):
        run = rustup_runner(
            active=MSVC_TOOLCHAIN,
            hosts={MSVC_TOOLCHAIN: MSVC_TARGET},
        )

        selection = prepare_validation.resolve_rust_selection(
            {}, run=run, rustup_executable="rustup.exe"
        )

        self.assertEqual(selection.toolchain, MSVC_TOOLCHAIN)
        self.assertIsNone(selection.target)
        self.assertEqual(selection.target_mode, "unset")

    def test_pinned_msvc_survives_gnu_build_target_normalization(self):
        pinned = "1.92.0-x86_64-pc-windows-msvc"
        run = rustup_runner(
            active=MSVC_TOOLCHAIN,
            hosts={pinned: MSVC_TARGET, MSVC_TOOLCHAIN: MSVC_TARGET},
            targets={pinned: {MSVC_TARGET}},
        )

        selection = prepare_validation.resolve_rust_selection(
            {"RUSTUP_TOOLCHAIN": pinned, "CARGO_BUILD_TARGET": GNU_TARGET},
            run=run,
            rustup_executable="rustup.exe",
        )

        self.assertEqual(selection.toolchain, pinned)
        self.assertEqual(selection.target, MSVC_TARGET)
        self.assertEqual(selection.target_mode, "set")
        self.assertTrue(selection.target_was_normalized)

    def test_gnu_toolchain_selects_installed_stable_msvc_without_forcing_target(self):
        gnu = "stable-x86_64-pc-windows-gnu"
        run = rustup_runner(
            active=gnu,
            hosts={gnu: GNU_TARGET, MSVC_TOOLCHAIN: MSVC_TARGET},
        )

        selection = prepare_validation.resolve_rust_selection(
            {}, run=run, rustup_executable="rustup.exe"
        )

        self.assertEqual(selection.toolchain, MSVC_TOOLCHAIN)
        self.assertIsNone(selection.target)
        self.assertEqual(selection.target_mode, "unset")

    def test_explicit_msvc_target_is_preserved(self):
        run = rustup_runner(
            active=MSVC_TOOLCHAIN,
            hosts={MSVC_TOOLCHAIN: MSVC_TARGET},
            targets={MSVC_TOOLCHAIN: {MSVC_TARGET}},
        )

        selection = prepare_validation.resolve_rust_selection(
            {"CARGO_BUILD_TARGET": MSVC_TARGET},
            run=run,
            rustup_executable="rustup.exe",
        )

        self.assertEqual(selection.target, MSVC_TARGET)
        self.assertFalse(selection.target_was_normalized)

    def test_gnu_build_target_uses_pinned_msvc_toolchain_when_available(self):
        pinned = "1.91.1-x86_64-pc-windows-msvc"
        gnu = "stable-x86_64-pc-windows-gnu"
        run = rustup_runner(
            active=gnu,
            hosts={pinned: MSVC_TARGET, gnu: GNU_TARGET, MSVC_TOOLCHAIN: MSVC_TARGET},
            targets={pinned: {MSVC_TARGET}},
        )

        selection = prepare_validation.resolve_rust_selection(
            {"RUSTUP_TOOLCHAIN": pinned, "CARGO_BUILD_TARGET": GNU_TARGET},
            run=run,
            rustup_executable="rustup.exe",
        )

        self.assertEqual(selection.toolchain, pinned)
        self.assertEqual(selection.target, MSVC_TARGET)

    def test_missing_msvc_target_component_fails_actionably(self):
        def run(arguments, **_kwargs):
            self.assertEqual(
                arguments,
                [
                    "rustup.exe",
                    "target",
                    "list",
                    "--installed",
                    "--toolchain",
                    MSVC_TOOLCHAIN,
                ],
            )
            return subprocess.CompletedProcess(arguments, 0, "", "")

        with self.assertRaisesRegex(
            prepare_validation.ValidationSetupError,
            r"missing required target.*rustup target add.*x86_64-pc-windows-msvc",
        ):
            prepare_validation._require_msvc_target(
                MSVC_TOOLCHAIN, {}, run, "rustup.exe"
            )

    def test_missing_explicit_msvc_toolchain_fails_without_downgrade(self):
        pinned = "1.92.0-x86_64-pc-windows-msvc"
        run = rustup_runner(active=MSVC_TOOLCHAIN, hosts={}, missing={pinned})

        with self.assertRaisesRegex(
            prepare_validation.ValidationSetupError,
            r"RUSTUP_TOOLCHAIN.*rustup toolchain install",
        ):
            prepare_validation.resolve_rust_selection(
                {"RUSTUP_TOOLCHAIN": pinned},
                run=run,
                rustup_executable="rustup.exe",
            )

    def test_missing_stable_fallback_fails_actionably(self):
        gnu = "stable-x86_64-pc-windows-gnu"
        run = rustup_runner(
            active=gnu,
            hosts={gnu: GNU_TARGET},
            missing={MSVC_TOOLCHAIN},
        )

        with self.assertRaisesRegex(
            prepare_validation.ValidationSetupError,
            r"rustup toolchain install stable-x86_64-pc-windows-msvc",
        ):
            prepare_validation.resolve_rust_selection(
                {}, run=run, rustup_executable="rustup.exe"
            )

    def test_non_x64_windows_target_is_rejected_without_rewrite(self):
        with self.assertRaisesRegex(
            prepare_validation.ValidationSetupError,
            "only x86_64 Windows validation is supported",
        ):
            prepare_validation.resolve_rust_selection(
                {"CARGO_BUILD_TARGET": "aarch64-pc-windows-msvc"},
                run=Mock(),
                rustup_executable="rustup.exe",
            )

    def test_visual_studio_selection_uses_latest_supported_instance_with_spaces(self):
        with tempfile.TemporaryDirectory(prefix="VS fixture ", dir=TEST_TEMP_ROOT) as temp:
            root = Path(temp)
            old = root / "Microsoft Visual Studio" / "2022 Build Tools"
            latest = root / "Microsoft Visual Studio" / "2026 Build Tools"
            unsupported = root / "Microsoft Visual Studio" / "Preview"
            records = [
                {"installationPath": str(old), "installationVersion": "17.14.3"},
                {"installationPath": str(latest), "installationVersion": "18.0.1"},
                {"installationPath": str(unsupported), "installationVersion": "19.0.0"},
            ]
            env, vswhere = vswhere_fixture(root, records)

            def run(arguments, **_kwargs):
                self.assertEqual(Path(arguments[0]), vswhere)
                self.assertIn("Microsoft.VisualStudio.Component.VC.Tools.x86.x64", arguments)
                self.assertNotIn("Microsoft.Component.MSBuild", arguments)
                return subprocess.CompletedProcess(arguments, 0, json.dumps(records), "")

            selected = prepare_validation.discover_visual_studio(env, run=run)

            self.assertEqual(selected.version, "18.0.1")
            self.assertEqual(selected.installation_path, latest)
            self.assertEqual(
                selected.vcvars64,
                latest / "VC" / "Auxiliary" / "Build" / "vcvars64.bat",
            )

    def test_visual_studio_selection_skips_instance_without_vcvars(self):
        with tempfile.TemporaryDirectory(prefix="VS fixture ", dir=TEST_TEMP_ROOT) as temp:
            root = Path(temp)
            broken = root / "VS 2026"
            working = root / "VS 2022"
            records = [
                {"installationPath": str(broken), "installationVersion": "18.1.0"},
                {"installationPath": str(working), "installationVersion": "17.14.3"},
            ]
            env, _ = vswhere_fixture(root, records, vcvars={str(working)})
            run = lambda arguments, **_kwargs: subprocess.CompletedProcess(
                arguments, 0, json.dumps(records), ""
            )

            selected = prepare_validation.discover_visual_studio(env, run=run)

            self.assertEqual(selected.installation_path, working)

    def test_missing_visual_studio_tools_report_install_requirement(self):
        with tempfile.TemporaryDirectory(prefix="VS fixture ", dir=TEST_TEMP_ROOT) as temp:
            root = Path(temp)
            env, _ = vswhere_fixture(root, [])
            run = lambda arguments, **_kwargs: subprocess.CompletedProcess(
                arguments, 0, "[]", ""
            )

            with self.assertRaisesRegex(
                prepare_validation.ValidationSetupError,
                "Visual Studio 2022 or 2026.*VC x64 build tools",
            ):
                prepare_validation.discover_visual_studio(env, run=run)

    def test_missing_vswhere_is_actionable(self):
        with tempfile.TemporaryDirectory(prefix="VS fixture ", dir=TEST_TEMP_ROOT) as temp:
            with self.assertRaisesRegex(
                prepare_validation.ValidationSetupError,
                "vswhere.exe was not found",
            ):
                prepare_validation.discover_visual_studio(
                    {"ProgramFiles(x86)": temp}, run=Mock()
                )

    def test_bridge_quotes_paths_checks_tools_and_propagates_shell_status(self):
        batch = prepare_validation.bridge_batch_contents()

        self.assertIn('call "%STASIS_VALIDATE_VCVARS64%"', batch)
        self.assertIn('"%STASIS_VALIDATE_BASH_EXE%" -l "%STASIS_VALIDATE_SCRIPT%"', batch)
        self.assertIn("%VCToolsInstallDir%\\bin\\Hostx64\\x64", batch)
        self.assertIn('if not exist "%STASIS_VALIDATE_MSVC_BIN%\\cl.exe"', batch)
        self.assertIn('if not exist "%STASIS_VALIDATE_MSVC_BIN%\\link.exe"', batch)
        self.assertIn("exit /b %ERRORLEVEL%", batch)

        command = prepare_validation.build_cmd_invocation(
            Path(r"C:\Windows\System32\cmd.exe"),
            Path(r"C:\Users\Ben\AppData\Local\Temp\validation bridge.cmd"),
        )
        self.assertEqual(
            command,
            '"C:\\Windows\\System32\\cmd.exe" /d /s /c ""C:\\Users\\Ben\\AppData\\Local\\Temp\\validation bridge.cmd""',
        )

    def test_reentry_verifies_exact_cl_and_link_paths(self):
        expected = r"C:\Program Files\Microsoft Visual Studio\VC\Tools\MSVC\14.44\bin\Hostx64\x64"
        env = {"STASIS_VALIDATE_MSVC_BIN": expected}
        which = Mock(side_effect=lambda tool, path=None: str(Path(expected) / tool))

        prepare_validation.verify_msvc_tools(env, which=which)

        bad_which = Mock(
            side_effect=lambda tool, path=None: rf"C:\Program Files\Git\usr\bin\{tool}"
            if tool == "link.exe"
            else str(Path(expected) / tool)
        )
        with self.assertRaisesRegex(
            prepare_validation.ValidationSetupError,
            r"link.exe resolved to .* expected",
        ):
            prepare_validation.verify_msvc_tools(env, which=bad_which)

    def test_reentry_requires_selected_msvc_rust_toolchain_and_target(self):
        with tempfile.TemporaryDirectory(prefix="MSVC tools ", dir=TEST_TEMP_ROOT) as temp:
            tools = Path(temp)
            for executable in ("cl.exe", "link.exe"):
                path = tools / executable
                path.touch()
                path.chmod(path.stat().st_mode | 0o111)
            env = {
                "PATH": str(tools),
                "RUSTUP_TOOLCHAIN": "1.92.0-x86_64-pc-windows-msvc",
                "STASIS_VALIDATE_RUSTUP_TOOLCHAIN": "1.92.0-x86_64-pc-windows-msvc",
                "CARGO_BUILD_TARGET": MSVC_TARGET,
                "STASIS_VALIDATE_TARGET_MODE": "set",
                "STASIS_VALIDATE_CARGO_BUILD_TARGET": MSVC_TARGET,
                "STASIS_VALIDATE_MSVC_BIN": str(tools),
            }

            prepare_validation.verify_reentry(env)

            env["CARGO_BUILD_TARGET"] = GNU_TARGET
            with self.assertRaisesRegex(
                prepare_validation.ValidationSetupError,
                "CARGO_BUILD_TARGET selection was not restored",
            ):
                prepare_validation.verify_reentry(env)

            env["CARGO_BUILD_TARGET"] = MSVC_TARGET
            env["RUSTUP_TOOLCHAIN"] = "stable-x86_64-pc-windows-gnu"
            with self.assertRaisesRegex(
                prepare_validation.ValidationSetupError,
                "Rust toolchain selection was not restored",
            ):
                prepare_validation.verify_reentry(env)

            env["RUSTUP_TOOLCHAIN"] = env["STASIS_VALIDATE_RUSTUP_TOOLCHAIN"]
            env["STASIS_VALIDATE_TARGET_MODE"] = "unset"
            env["STASIS_VALIDATE_CARGO_BUILD_TARGET"] = ""
            env.pop("CARGO_BUILD_TARGET")
            prepare_validation.verify_reentry(env)

            env["CARGO_BUILD_TARGET"] = MSVC_TARGET
            with self.assertRaisesRegex(
                prepare_validation.ValidationSetupError,
                "CARGO_BUILD_TARGET should be unset",
            ):
                prepare_validation.verify_reentry(env)

    def test_windows_shell_reenters_after_login_and_cargo_env(self):
        validation = (ROOT / "tools" / "validate_repo.sh").read_text(encoding="utf-8")
        cargo_env = validation.index('source "$HOME/.cargo/env"')
        windows_gate = validation.index('if [[ "${OS:-}" == Windows_NT ]]')
        restore = validation.index('export RUSTUP_TOOLCHAIN=', windows_gate)
        prepend = validation.index('export PATH=', restore)
        verify = validation.index("--verify-reentry", prepend)

        self.assertLess(cargo_env, windows_gate)
        self.assertLess(windows_gate, restore)
        self.assertLess(restore, prepend)
        self.assertLess(prepend, verify)
        self.assertIn('exec "$PYTHON" tools/windows/prepare_validation.py --launch', validation)
        self.assertIn('"$PYTHON" tools/ci/run_workspace_tests.py', validation)

    def test_windows_launcher_preserves_parent_and_process_exit_code(self):
        with tempfile.TemporaryDirectory(prefix="validation fixture ", dir=TEST_TEMP_ROOT) as temp:
            root = Path(temp)
            vs = root / "Visual Studio Build Tools"
            records = [{"installationPath": str(vs), "installationVersion": "17.14.3"}]
            env, _ = vswhere_fixture(root, records)
            system_root = root / "Windows"
            cmd = system_root / "System32" / "cmd.exe"
            cmd.parent.mkdir(parents=True)
            cmd.touch()
            env.update(
                {
                    "SystemRoot": str(system_root),
                    "CARGO_BUILD_TARGET": GNU_TARGET,
                    "STASIS_SIGNING_CERT_THUMBPRINT": "ABC123",
                    "PATH": "original path",
                }
            )
            original = dict(env)
            gnu_toolchain = "stable-x86_64-pc-windows-gnu"
            run = rustup_runner(
                active=gnu_toolchain,
                hosts={gnu_toolchain: GNU_TARGET, MSVC_TOOLCHAIN: MSVC_TARGET},
            )
            called = []

            def process_run(arguments, **kwargs):
                called.append((arguments, kwargs))
                if isinstance(arguments, list):
                    return subprocess.CompletedProcess(arguments, 0, json.dumps(records), "")
                return subprocess.CompletedProcess(arguments, 23, "", "")

            def combined_run(arguments, **kwargs):
                if arguments and arguments[0] == "rustup.exe":
                    return run(arguments, **kwargs)
                return process_run(arguments, **kwargs)

            output = io.StringIO()
            with redirect_stdout(output):
                result = prepare_validation.launch_windows_validation(
                    env,
                    r"C:\Program Files\Git\bin\bash.exe",
                    r"D:\code\StasisLang\tools\validate_repo.sh",
                    run=combined_run,
                    rustup_executable="rustup.exe",
                    temp_root=root,
                )

            self.assertEqual(result, 23)
            self.assertEqual(env, original)
            command, kwargs = called[-1]
            self.assertEqual(
                kwargs["env"]["STASIS_VALIDATE_BASH_EXE"],
                r"C:\Program Files\Git\bin\bash.exe",
            )
            self.assertEqual(
                kwargs["env"]["STASIS_VALIDATE_RUSTUP_TOOLCHAIN"], MSVC_TOOLCHAIN
            )
            self.assertEqual(
                kwargs["env"]["STASIS_VALIDATE_CARGO_BUILD_TARGET"], MSVC_TARGET
            )
            self.assertEqual(
                kwargs["env"]["STASIS_SIGNING_CERT_THUMBPRINT"], "ABC123"
            )
            self.assertEqual(kwargs["env"]["PATH"], "original path")
            self.assertNotIn("capture_output", kwargs)
            self.assertNotIn("stdout", kwargs)
            self.assertNotIn("stderr", kwargs)
            self.assertIn("run-validation.cmd", command)
            self.assertIn(
                "replacing active Rust toolchain "
                f"{gnu_toolchain} with installed MSVC toolchain {MSVC_TOOLCHAIN}",
                output.getvalue(),
            )
            self.assertIn(
                f"normalizing CARGO_BUILD_TARGET {GNU_TARGET} -> {MSVC_TARGET}",
                output.getvalue(),
            )


if __name__ == "__main__":
    unittest.main()

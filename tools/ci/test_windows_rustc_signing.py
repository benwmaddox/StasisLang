import importlib.util
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools import cargo_cache
from tools.windows import test_signing_acceptance as acceptance


ROOT = Path(__file__).resolve().parents[2]
WRAPPER_PATH = ROOT / "tools" / "windows" / "stasis-rustc-wrapper.py"
SPEC = importlib.util.spec_from_file_location("stasis_rustc_wrapper", WRAPPER_PATH)
assert SPEC and SPEC.loader
wrapper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wrapper)


class WindowsRustcSigningTests(unittest.TestCase):
    def test_cargo_policy_installs_wrapper_only_for_windows_signing(self) -> None:
        unsigned = {"UNCHANGED": "yes"}
        cargo_cache.configure_windows_rustc_wrapper(unsigned, ROOT, windows=True)
        self.assertNotIn("RUSTC_WRAPPER", unsigned)

        optional = {"STASIS_SIGNING_MODE": "optional"}
        cargo_cache.configure_windows_rustc_wrapper(optional, ROOT, windows=True)
        self.assertNotIn("RUSTC_WRAPPER", optional)

        non_windows = {"STASIS_REQUIRE_SIGNED_EXECUTION": "1"}
        cargo_cache.configure_windows_rustc_wrapper(non_windows, ROOT, windows=False)
        self.assertNotIn("RUSTC_WRAPPER", non_windows)

        with tempfile.TemporaryDirectory() as directory:
            calls: list[list[str]] = []

            def process(command, **_kwargs):
                calls.append(command)
                if command[0] == "rustc":
                    Path(command[-1]).write_bytes(b"MZ launcher")
                return mock.Mock(returncode=0)

            signed = {
                "STASIS_REQUIRE_SIGNED_EXECUTION": "1",
                "CARGO_TARGET_DIR": directory,
            }
            cargo_cache.configure_windows_rustc_wrapper(
                signed, ROOT, windows=True, process_run=process
            )
            launcher = Path(signed["RUSTC_WRAPPER"])
            self.assertEqual(launcher.parent.parent, Path(directory) / "stasis-signing-wrapper")
            self.assertEqual(
                Path(signed["STASIS_RUSTC_SIGNING_POLICY"]),
                (ROOT / "tools/windows/stasis-signing.ps1").resolve(),
            )
            self.assertTrue(signed["STASIS_RUSTC_WRAPPER_PYTHON"])
            self.assertEqual(
                Path(signed["STASIS_RUSTC_WRAPPER_SCRIPT"]), WRAPPER_PATH
            )
            self.assertEqual(calls[0][0], "rustc")
            self.assertEqual(calls[1][-2], "-Artifact")
            self.assertEqual(Path(calls[1][-1]), launcher)
            self.assertTrue(launcher.is_file())

            calls.clear()
            uppercase_required = {
                "STASIS_SIGNING_MODE": "REQUIRED",
                "CARGO_TARGET_DIR": directory,
            }
            cargo_cache.configure_windows_rustc_wrapper(
                uppercase_required, ROOT, windows=True, process_run=process
            )
            other_launcher = Path(uppercase_required["RUSTC_WRAPPER"])
            self.assertEqual(len(calls), 2)
            self.assertEqual(calls[1][-3], "sign")
            self.assertNotEqual(launcher, other_launcher)
            cargo_cache.cleanup_windows_rustc_wrapper(other_launcher)
            self.assertFalse(other_launcher.exists())
            self.assertTrue(launcher.is_file())
            cargo_cache.cleanup_windows_rustc_wrapper(launcher)
            self.assertFalse(launcher.parent.exists())

    def test_cargo_policy_rejects_conflicting_wrapper(self) -> None:
        environment = {
            "STASIS_SIGNING_CERT_THUMBPRINT": "ABCDEF",
            "RUSTC_WRAPPER": "C:/other/cache-wrapper.exe",
            "CARGO_TARGET_DIR": "C:/target",
        }
        with self.assertRaisesRegex(ValueError, "unset it before running signed Cargo"):
            cargo_cache.configure_windows_rustc_wrapper(
                environment, ROOT, windows=True
            )

    def test_cargo_does_not_start_when_native_wrapper_signing_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            environment = {
                "STASIS_REQUIRE_SIGNED_EXECUTION": "1",
                "CARGO_TARGET_DIR": directory,
            }

            def process(command, **_kwargs):
                if command[0] == "rustc":
                    output = Path(command[-1])
                    output.write_bytes(b"MZ launcher")
                    output.with_suffix(".pdb").write_bytes(b"symbols")
                    return mock.Mock(returncode=0)
                return mock.Mock(returncode=7)

            with self.assertRaisesRegex(ValueError, "before Cargo launch"):
                cargo_cache.configure_windows_rustc_wrapper(
                    environment, ROOT, windows=True, process_run=process
                )

            build_dir = Path(directory) / "stasis-signing-wrapper"
            self.assertEqual(list(build_dir.iterdir()), [])
            self.assertNotIn("RUSTC_WRAPPER", environment)

    def test_local_record_override_and_production_suppress_default_record(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            local_app_data = Path(directory)
            default = (
                local_app_data
                / "Stasis"
                / "signing"
                / "development-thumbprint.txt"
            )
            default.parent.mkdir(parents=True)
            default.write_text("ABCDEF", encoding="ascii")

            self.assertFalse(
                wrapper.signing_is_configured(
                    {
                        "LOCALAPPDATA": str(local_app_data),
                        "STASIS_SIGNING_LOCAL_RECORD": str(
                            local_app_data / "missing.txt"
                        ),
                    }
                )
            )
            self.assertTrue(
                wrapper.signing_is_configured({"STASIS_SIGNING_MODE": "REQUIRED"})
            )
            self.assertFalse(
                wrapper.signing_is_configured(
                    {
                        "LOCALAPPDATA": str(local_app_data),
                        "STASIS_SIGNING_PROFILE": "production",
                    }
                )
            )

    def test_build_script_is_signed_after_successful_rustc(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "deps with spaces"
            output.mkdir()
            artifact = output / "build_script_build-a1b2.exe"
            policy = Path(directory) / "policy with spaces.ps1"
            calls: list[list[str]] = []

            def process(command):
                calls.append(command)
                if command[0] == "rustc.exe":
                    artifact.write_bytes(b"MZ")
                return mock.Mock(returncode=0)

            result = wrapper.run(
                [
                    "rustc.exe",
                    "build.rs",
                    "--crate-name",
                    "build_script_build",
                    "--crate-type",
                    "bin",
                    "--out-dir",
                    str(output),
                    "-C",
                    "extra-filename=-a1b2",
                    "--emit=dep-info,link",
                ],
                {"STASIS_RUSTC_SIGNING_POLICY": str(policy)},
                process_run=process,
            )

            self.assertEqual(result, 0)
            self.assertEqual(calls[0][0], "rustc.exe")
            self.assertEqual(calls[1], wrapper.signing_command(policy, artifact))

    def test_proc_macro_dll_is_signed_and_cross_compiled_so_is_skipped(self) -> None:
        windows = wrapper.emitted_windows_artifacts(
            [
                "lib.rs",
                "--crate-name=derive_thing",
                "--crate-type=proc-macro",
                "--out-dir=C:/target/deps",
                "-Cextra-filename=-1234",
            ]
        )
        linux = wrapper.emitted_windows_artifacts(
            [
                "lib.rs",
                "--crate-name=derive_thing",
                "--crate-type=proc-macro",
                "--out-dir=C:/target/deps",
                "--target=x86_64-unknown-linux-gnu",
                "-Cextra-filename=-1234",
            ]
        )
        self.assertEqual(windows, [Path("C:/target/deps/derive_thing-1234.dll")])
        self.assertEqual(linux, [])

    def test_custom_json_target_is_rejected_before_rustc(self) -> None:
        process = mock.Mock()
        result = wrapper.run(
            [
                "rustc.exe",
                "main.rs",
                "--crate-name=custom",
                "--out-dir=target",
                "--target=targets/vendor-pe.json",
            ],
            {"STASIS_RUSTC_SIGNING_POLICY": "policy.ps1"},
            process_run=process,
        )
        self.assertEqual(result, 2)
        process.assert_not_called()

        with self.assertRaisesRegex(ValueError, "custom JSON rustc targets"):
            wrapper.emitted_windows_artifacts(
                [
                    "main.rs",
                    "--target=targets/vendor-pe.json",
                    "-o",
                    "opaque-output.bin",
                ]
            )

    def test_test_harness_and_default_crate_type_emit_executables(self) -> None:
        common = ["lib.rs", "--crate-name=unit", "--out-dir=C:/target/deps"]
        self.assertEqual(
            wrapper.emitted_windows_artifacts(
                [*common, "--test", "--crate-type=lib", "-Cextra-filename=-1"]
            ),
            [Path("C:/target/deps/unit-1.exe")],
        )
        self.assertEqual(
            wrapper.emitted_windows_artifacts(common),
            [Path("C:/target/deps/unit.exe")],
        )
        self.assertEqual(
            wrapper.emitted_windows_artifacts(
                [
                    *common,
                    "--test",
                    "--crate-type=proc-macro,dylib",
                    "-Cextra-filename=-2",
                ]
            ),
            [Path("C:/target/deps/unit-2.exe")],
        )

    def test_rustc_information_probes_do_not_expect_artifacts(self) -> None:
        for probe in ("--print=file-names", "--print", "-vV", "--version"):
            arguments = [
                "--crate-name=probe",
                "--out-dir=target",
                probe,
            ]
            if probe == "--print":
                arguments.append("file-names")
            with self.subTest(probe=probe):
                self.assertEqual(wrapper.emitted_windows_artifacts(arguments), [])

    def test_print_link_args_still_requires_signing_the_emitted_binary(self) -> None:
        self.assertEqual(
            wrapper.emitted_windows_artifacts(
                [
                    "main.rs",
                    "--crate-name=linked",
                    "--out-dir=target",
                    "--print=link-args",
                ]
            ),
            [Path("target/linked.exe")],
        )

    def test_native_launcher_uses_process_api_without_cmd(self) -> None:
        source = (ROOT / "tools/windows/stasis-rustc-wrapper.rs").read_text(
            encoding="utf-8"
        )
        self.assertIn("Command::new(python)", source)
        self.assertIn("env::args_os().skip(1)", source)
        self.assertNotIn("cmd.exe", source.casefold())

    def test_live_acceptance_signs_before_executing_long_argument(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            commands: list[list[str]] = []

            def process(command, *, env, check, timeout):
                self.assertEqual(timeout, 900)
                self.assertFalse(check)
                commands.append(command)
                if command[0] == "rustc":
                    Path(command[-1]).write_bytes(b"MZ launcher")
                elif command[0].endswith("stasis-rustc-wrapper-acceptance.exe"):
                    Path(env["STASIS_TEST_MARKER"]).write_text(
                        str(len(command[1])), encoding="ascii"
                    )
                return mock.Mock(returncode=0)

            with (
                mock.patch.object(acceptance.sys, "platform", "win32"),
                mock.patch.object(acceptance.subprocess, "run", side_effect=process),
                mock.patch.object(acceptance, "run_cargo_acceptance") as cargo_acceptance,
            ):
                acceptance.run_acceptance(Path(directory), {})
                cargo_acceptance.assert_called_once()

            self.assertEqual(commands[0][0], "rustc")
            self.assertEqual(commands[1][-2], "-Artifact")
            self.assertEqual(commands[1][-3], "sign")
            self.assertEqual(commands[2][-3], "verify")
            self.assertTrue(
                commands[3][0].endswith("stasis-rustc-wrapper-acceptance.exe")
            )
            self.assertEqual(len(commands[3][1]), 9000)

    def test_explicit_output_does_not_scan_stale_neighbor(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            current = output / "current.exe"
            stale = output / "stale.exe"
            stale.write_bytes(b"MZ")
            calls: list[list[str]] = []

            def process(command):
                calls.append(command)
                if command[0] == "rustc.exe":
                    current.write_bytes(b"MZ")
                return mock.Mock(returncode=0)

            result = wrapper.run(
                ["rustc.exe", "main.rs", "-o", str(current)],
                {"STASIS_RUSTC_SIGNING_POLICY": str(output / "policy.ps1")},
                process_run=process,
            )
            self.assertEqual(result, 0)
            self.assertEqual(len(calls), 2)
            self.assertEqual(Path(calls[1][-1]), current)
            self.assertNotIn(str(stale), calls[1])

    def test_explicit_artifact_cli_uses_same_configured_policy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "native test.exe"
            artifact.write_bytes(b"MZ")
            process = mock.Mock(return_value=mock.Mock(returncode=0))
            environment = {"STASIS_SIGNING_CERT_THUMBPRINT": "ABCDEF"}

            result = wrapper.sign_explicit_artifact(
                artifact,
                environment,
                windows=True,
                process_run=process,
            )

            self.assertEqual(result, 0)
            command = process.call_args.args[0]
            self.assertEqual(command[-2:], ["-Artifact", str(artifact.resolve())])

            process.reset_mock()
            result = wrapper.sign_explicit_artifact(
                artifact, {}, windows=True, process_run=process
            )
            self.assertEqual(result, 0)
            process.assert_not_called()

    def test_rustc_failure_is_preserved_without_signing(self) -> None:
        process = mock.Mock(return_value=mock.Mock(returncode=37))
        result = wrapper.run(
            ["rustc.exe", "main.rs", "-o", "failed.exe"],
            {"STASIS_RUSTC_SIGNING_POLICY": "policy.ps1"},
            process_run=process,
        )
        self.assertEqual(result, 37)
        process.assert_called_once()

    def test_metadata_probe_has_no_artifact_to_sign(self) -> None:
        self.assertEqual(
            wrapper.emitted_windows_artifacts(
                [
                    "lib.rs",
                    "--crate-name",
                    "probe",
                    "--crate-type",
                    "bin",
                    "--out-dir",
                    "target",
                    "--emit=metadata",
                ]
            ),
            [],
        )


if __name__ == "__main__":
    unittest.main()

import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

from tools.generate_release_provenance import RUNTIME_DIRS, RUNTIME_FILES, render_contract_version
from tools.verify_package_provenance import verify_asset_package_identities


ROOT = pathlib.Path(__file__).resolve().parents[2]
VERIFY = ROOT / "tools" / "verify_package_provenance.py"


class ReleaseProvenanceTests(unittest.TestCase):
    def test_asset_package_identity_binds_exact_manifest_bytes(self):
        class Parser:
            @staticmethod
            def error(message):
                raise ValueError(message)

        with tempfile.TemporaryDirectory() as temporary:
            package = pathlib.Path(temporary)
            (package / "assets").mkdir()
            manifest = b'{"schema":"stasis-assets","version":2,"assets":[]}'
            (package / "assets/manifest.json").write_bytes(manifest)
            identity = {
                "schema": "stasis.asset_package",
                "version": 1,
                "manifest_path": "assets/manifest.json",
                "manifest_sha256": hashlib.sha256(manifest).hexdigest(),
            }
            (package / "stasis_asset_package.json").write_text(
                json.dumps(identity), encoding="utf-8"
            )
            verify_asset_package_identities(Parser(), package)
            (package / "assets/manifest.json").write_bytes(manifest + b"\n")
            with self.assertRaisesRegex(ValueError, "manifest hash mismatch"):
                verify_asset_package_identities(Parser(), package)

    def test_render_contract_version_resolves_current_header_alias(self):
        self.assertEqual(6, render_contract_version(ROOT))

    def test_render_contract_version_rejects_missing_or_non_numeric_alias(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            runtime = root / "runtime"
            runtime.mkdir()
            header = runtime / "stasis_render_contract.h"
            header.write_text(
                "#define STASIS_RENDER_CURRENT_VERSION STASIS_RENDER_V5_VERSION\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "missing STASIS_RENDER_V5_VERSION"):
                render_contract_version(root)
            header.write_text(
                "#define STASIS_RENDER_V5_VERSION STASIS_RENDER_V6_VERSION\n"
                "#define STASIS_RENDER_V6_VERSION 6\n"
                "#define STASIS_RENDER_CURRENT_VERSION STASIS_RENDER_V5_VERSION\n",
                encoding="utf-8",
            )
            self.assertEqual(6, render_contract_version(root))
            header.write_text(
                "#define STASIS_RENDER_CURRENT_VERSION STASIS_RENDER_V5_VERSION\n"
                "#define STASIS_RENDER_V5_VERSION not_numeric\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "not a numeric alias"):
                render_contract_version(root)

    def test_audio_asset_decoder_is_part_of_release_provenance(self):
        self.assertIn("MINIMP3-LICENSE.txt", RUNTIME_FILES)
        self.assertIn("minimp3.h", RUNTIME_FILES)
        self.assertIn("minimp3_ex.h", RUNTIME_FILES)
        self.assertIn("stasis_audio_assets.c", RUNTIME_FILES)
        self.assertIn("stasis_audio_assets.h", RUNTIME_FILES)

    def test_mobile_preference_host_is_part_of_release_provenance(self):
        self.assertIn("stasis_platform_storage.c", RUNTIME_FILES)
        self.assertIn("stasis_platform_storage.h", RUNTIME_FILES)

    def test_thorvg_is_part_of_release_provenance(self):
        self.assertIn("stasis_svg.cpp", RUNTIME_FILES)
        self.assertIn("stasis_svg.h", RUNTIME_FILES)
        self.assertIn("third_party/thorvg", RUNTIME_DIRS)

    def test_thorvg_source_closure_is_wired_for_desktop_android_and_ios(self):
        thorvg = ROOT / "runtime/third_party/thorvg"
        cmake = (thorvg / "CMakeLists.txt").read_text(encoding="utf-8")
        sources = set(re.findall(r"^\s+(src/[^\s]+\.cpp)$", cmake, re.MULTILINE))
        self.assertGreater(len(sources), 30)
        for source in sources:
            self.assertTrue((thorvg / source).is_file(), source)

        runtime_cmake = (ROOT / "runtime/CMakeLists.txt").read_text(encoding="utf-8")
        self.assertIn("stasis_graphics.c stasis_image_writer.c", runtime_cmake)
        self.assertIn("stasis_audio_assets.c stasis_svg.cpp", runtime_cmake)
        self.assertIn("stasis_thorvg", runtime_cmake)

        android_cmake = (
            ROOT / "mobile/android/app/src/main/cpp/CMakeLists.txt"
        ).read_text(encoding="utf-8")
        self.assertIn("runtime/stasis_svg.cpp", android_cmake)
        self.assertIn("runtime/third_party/thorvg", android_cmake)
        self.assertIn("stasis_thorvg", android_cmake)

        ios_project = (
            ROOT / "mobile/shells/ios/StasisMobile.xcodeproj/project.pbxproj"
        ).read_text(encoding="utf-8")
        self.assertIn("../runtime/stasis_svg.cpp", ios_project)
        for source in sources:
            self.assertIn(f"../runtime/third_party/thorvg/{source}", ios_project)

        self.assertFalse((ROOT / "runtime/nanosvg.h").exists())
        self.assertFalse((ROOT / "runtime/nanosvgrast.h").exists())

    def test_windows_dpi_manifest_is_part_of_release_provenance(self):
        self.assertIn("stasis_runner.manifest", RUNTIME_FILES)

    def test_image_writer_sources_are_part_of_release_provenance(self):
        self.assertIn("stasis_image_writer.c", RUNTIME_FILES)
        self.assertIn("stasis_image_writer.h", RUNTIME_FILES)

    def test_macos_retina_plist_is_part_of_release_provenance(self):
        self.assertIn("stasis_runner_macos.plist.in", RUNTIME_FILES)

    def test_release_workflows_assemble_every_provenance_runtime_file(self):
        for workflow_name in (
            ".github/workflows/nightly-release.yml",
            ".github/workflows/bootstrap-artifacts.yml",
        ):
            workflow = (ROOT / workflow_name).read_text(encoding="utf-8")
            unix_matches = re.findall(
                r'^\s+cp (?P<files>runtime/[^\n]+) "\$\{out\}/runtime/"\s*$',
                workflow,
                re.MULTILINE,
            )
            windows_matches = re.findall(
                r"^\s+@\((?P<files>[^\n]+)\) \| ForEach-Object \{ Copy-Item \"runtime/\$_\" \"\$out/runtime/\" -Force \}\s*$",
                workflow,
                re.MULTILINE,
            )
            self.assertEqual(1, len(unix_matches), workflow_name)
            self.assertEqual(1, len(windows_matches), workflow_name)

            unix_files = {pathlib.Path(path).name for path in unix_matches[0].split()}
            windows_files = set(re.findall(r"'([^']+)'", windows_matches[0]))
            for filename in RUNTIME_FILES:
                self.assertIn(
                    filename,
                    unix_files,
                    f"{filename} missing from Unix assembly in {workflow_name}",
                )
                self.assertIn(
                    filename,
                    windows_files,
                    f"{filename} missing from Windows assembly in {workflow_name}",
                )
            self.assertIn("cp -R runtime/third_party", workflow)
            self.assertIn('Copy-Item "runtime/third_party"', workflow)

    def test_release_workflows_select_platform_smoke_executable(self):
        for workflow_name in (
            ".github/workflows/nightly-release.yml",
            ".github/workflows/bootstrap-artifacts.yml",
        ):
            workflow = (ROOT / workflow_name).read_text(encoding="utf-8")
            smoke_start = workflow.index("      - name: Smoke test bundled CLI (unix)")
            next_step = workflow.find("\n      - name:", smoke_start + 1)
            smoke_block = workflow[smoke_start:next_step if next_step != -1 else None]
            self.assertIn(
                'smoke_executable="./cli-smoke/build/ci_smoke"',
                smoke_block,
                workflow_name,
            )
            self.assertIn(
                'if [[ "${{ runner.os }}" == "macOS" ]]; then',
                smoke_block,
                workflow_name,
            )
            self.assertIn(
                'smoke_executable="./cli-smoke/build/ci_smoke.app/Contents/MacOS/ci_smoke"',
                smoke_block,
                workflow_name,
            )
            self.assertIn('"${smoke_executable}"', smoke_block, workflow_name)
            self.assertNotRegex(
                smoke_block,
                r"(?m)^\s+\./cli-smoke/build/ci_smoke\s*$",
                workflow_name,
            )

    def test_release_workflows_bound_graphical_smoke_processes(self):
        for workflow_name in (
            ".github/workflows/nightly-release.yml",
            ".github/workflows/bootstrap-artifacts.yml",
        ):
            workflow = (ROOT / workflow_name).read_text(encoding="utf-8")
            windows_start = workflow.index(
                "      - name: Smoke test bundled graphics runtime (windows)"
            )
            unix_start = workflow.index(
                "      - name: Smoke test bundled CLI (unix)"
            )
            windows_block = workflow[windows_start:unix_start]
            unix_end = workflow.find("\n      - name:", unix_start + 1)
            unix_block = workflow[unix_start:unix_end if unix_end != -1 else None]

            self.assertIn(
                'Start-Process -FilePath ".\\cli-smoke\\build\\ci_smoke.exe" -PassThru',
                windows_block,
                workflow_name,
            )
            self.assertIn("WaitForExit(5000)", windows_block, workflow_name)
            self.assertIn("$smokeProcess.ExitCode -ne 0", windows_block, workflow_name)
            self.assertIn("Stop-Process -Id $smokeProcess.Id -Force", windows_block, workflow_name)
            self.assertIn("$smokeProcess.WaitForExit()", windows_block, workflow_name)
            self.assertNotRegex(
                windows_block,
                r"(?m)^\s+\.\\cli-smoke\\build\\ci_smoke\.exe\s*$",
                workflow_name,
            )

            self.assertIn('python3 - "${smoke_executable}" <<\'PY\'', unix_block, workflow_name)
            self.assertIn("process = subprocess.Popen([sys.argv[1]])", unix_block, workflow_name)
            self.assertIn("process.wait(timeout=5)", unix_block, workflow_name)
            self.assertIn("if return_code != 0:", unix_block, workflow_name)
            self.assertIn("process.terminate()", unix_block, workflow_name)
            self.assertIn("process.kill()", unix_block, workflow_name)
            self.assertIn("process.wait()", unix_block, workflow_name)
            self.assertNotRegex(
                unix_block,
                r"(?m)^\s+\./cli-smoke/build/ci_smoke\s*$",
                workflow_name,
            )

    def test_windows_graphics_smoke_requires_monolithic_package_payload(self):
        for workflow_name in (
            ".github/workflows/nightly-release.yml",
            ".github/workflows/bootstrap-artifacts.yml",
        ):
            workflow = (ROOT / workflow_name).read_text(encoding="utf-8")
            windows_start = workflow.index(
                "      - name: Smoke test bundled graphics runtime (windows)"
            )
            unix_start = workflow.index(
                "      - name: Smoke test bundled CLI (unix)"
            )
            windows_block = workflow[windows_start:unix_start]

            self.assertRegex(
                windows_block,
                r'if \(-not \(Test-Path "[^"\n]+/app/stasis\.json"\)\) \{ throw "game package manifest missing" \}',
                workflow_name,
            )
            self.assertRegex(
                windows_block,
                r'if \(-not \(Test-Path "[^"\n]+/app/stasis_provenance\.json"\)\) \{ throw "game package provenance missing" \}',
                workflow_name,
            )
            self.assertRegex(
                windows_block,
                r'if \(Test-Path "[^"\n]+/app/ci_smoke\.dll"\) \{ throw "obsolete game package library present; monolithic Windows package must not contain app/ci_smoke\.dll" \}',
                workflow_name,
            )
            self.assertNotRegex(
                windows_block,
                r'if \(-not \(Test-Path "[^"\n]+/app/ci_smoke\.dll"\)\)',
                workflow_name,
            )

    def test_nightly_release_filters_top_level_regular_assets(self):
        workflow = (ROOT / ".github/workflows/nightly-release.yml").read_text(
            encoding="utf-8"
        )
        release_start = workflow.index("      - name: Create GitHub prerelease")
        release_block = workflow[release_start:]
        self.assertIn(
            "mapfile -d '' -t release_assets < <(find dist -maxdepth 1 -type f -print0 | sort -z)",
            release_block,
        )
        self.assertIn("if ((${#release_assets[@]} == 0)); then", release_block)
        self.assertIn(
            'echo "No regular release assets found directly under dist" >&2',
            release_block,
        )
        self.assertIn(
            'gh release create "${NIGHTLY_TAG}" "${release_assets[@]}"',
            release_block,
        )
        self.assertNotRegex(
            release_block,
            r'gh release create .*dist/\*',
        )

    def test_package_verifier_detects_runtime_substitution(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            release = root / "release"
            package = root / "package"
            (release / "runtime").mkdir(parents=True)
            (package / "runtime").mkdir(parents=True)
            (release / "mobile/shells/common").mkdir(parents=True)
            (release / "mobile/shells/android").mkdir(parents=True)
            (package / "common").mkdir(parents=True)
            (package / "android").mkdir(parents=True)
            runtime = b"official renderer\n"
            expected = hashlib.sha256(runtime).hexdigest()
            common_shell = b"common\n"
            android_shell = (
                b"@STASIS_APP_NAME@ @STASIS_PACKAGE_ID@ "
                b"@STASIS_JNI_PACKAGE@ @STASIS_ANDROID_ORIENTATION@ "
                b"@STASIS_ANDROID_VERSION_CODE@ @STASIS_ANDROID_VERSION_NAME@\n"
                b"@STASIS_ANDROID_ABI@\n"
            )
            manifest = {
                "schema": "stasis.release_provenance.v1",
                "release_tag": "v1.0.0",
                "source_commit": "0123456789012345678901234567890123456789",
                "development_build": False,
                "dirty_state": False,
                "command_buffer": {"name": "gfx_cmd", "version": 6},
                "runtime_sources": {"runtime/stasis_graphics.c": expected},
                "mobile_shell_sources": {
                    "mobile/shells/common/main.c": hashlib.sha256(common_shell).hexdigest(),
                    "mobile/shells/android/main.c": hashlib.sha256(android_shell).hexdigest(),
                },
            }
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (package / "runtime/stasis_graphics.c").write_bytes(runtime)
            (release / "mobile/shells/common/main.c").write_bytes(common_shell)
            (release / "mobile/shells/android/main.c").write_bytes(android_shell)
            (package / "common/main.c").write_bytes(common_shell)
            (package / "android/main.c").write_bytes(
                b"Demo App com.example.demo com_example_demo sensorPortrait 7 2.1.0\n"
                b"arm64-v8a\n"
            )
            (package / "stasis_mobile_package.json").write_text(
                json.dumps(
                    {
                        "target": "android-arm64",
                        "name": "demo",
                        "app_name": "Demo App",
                        "package_id": "com.example.demo",
                        "android_orientation": "sensorPortrait",
                        "android_version_code": "7",
                        "android_version_name": "2.1.0",
                    }
                ),
                encoding="utf-8",
            )
            (package / "common/stasis_package_provenance.h").write_bytes(
                ("#ifndef STASIS_PACKAGE_PROVENANCE_H\n"
                "#define STASIS_PACKAGE_PROVENANCE_H\n"
                '#define STASIS_PACKAGE_RELEASE_TAG "v1.0.0"\n'
                '#define STASIS_PACKAGE_SOURCE_COMMIT "0123456789012345678901234567890123456789"\n'
                '#define STASIS_PACKAGE_BUILD_LABEL "official release"\n'
                "#endif\n").encode("utf-8")
            )

            command = [
                sys.executable,
                str(VERIFY),
                "--release-root",
                str(release),
                "--package-root",
                str(package),
                "--expect-runtime-sources",
            ]
            self.assertEqual(subprocess.run(command, check=False).returncode, 0)
            legacy = dict(manifest)
            legacy["command_buffer"] = {"name": "gfx_cmd", "version": 4}
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(legacy), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(legacy), encoding="utf-8"
            )
            self.assertEqual(
                subprocess.run(command, check=False).returncode,
                0,
                "official legacy gfx_cmd schema 4 must remain accepted",
            )
            unsupported = dict(legacy)
            unsupported["command_buffer"] = {"name": "other_cmd", "version": 9}
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(unsupported), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(unsupported), encoding="utf-8"
            )
            contract_failed = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertNotEqual(contract_failed.returncode, 0)
            self.assertIn("command_buffer family must be gfx_cmd", contract_failed.stderr)
            numeric_type = dict(manifest)
            numeric_type["command_buffer"] = {"name": "gfx_cmd", "version": 6.0}
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(numeric_type), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(numeric_type), encoding="utf-8"
            )
            numeric_failed = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertNotEqual(numeric_failed.returncode, 0)
            self.assertIn("unsupported gfx_cmd command_buffer schema", numeric_failed.stderr)
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(manifest), encoding="utf-8"
            )
            (release / "mobile/shells/android/main.c").write_bytes(b"substituted shell\n")
            shell_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(shell_failed.returncode, 0)
            self.assertIn("shell tree does not match", shell_failed.stderr)
            (release / "mobile/shells/android/main.c").write_bytes(android_shell)
            (package / "runtime/stasis_graphics.c").write_bytes(b"local worktree\n")
            failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("packaged runtime hash mismatch", failed.stderr)
            (package / "runtime/stasis_graphics.c").write_bytes(runtime)
            (package / "android/untracked.java").write_text("class Untracked {}\n", encoding="utf-8")
            extra_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(extra_failed.returncode, 0)
            self.assertIn("source tree differs", extra_failed.stderr)
            (package / "android/untracked.java").unlink()
            (package / "stasis_mobile_package.json").write_text(
                json.dumps({"target": "windows-arm64", "name": "demo"}), encoding="utf-8"
            )
            target_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(target_failed.returncode, 0)
            self.assertIn("unsupported mobile package target", target_failed.stderr)


if __name__ == "__main__":
    unittest.main()

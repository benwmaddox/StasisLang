import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

from tools.generate_release_provenance import (
    DESKTOP_NETWORK_LIBRARIES,
    DESKTOP_NETWORK_HEADER,
    RUNTIME_DIRS,
    RUNTIME_FILES,
    desktop_network_artifact_hashes,
    render_contract_version,
)
from tools.verify_package_provenance import (
    verify_asset_package_identities,
    verify_mobile_shells,
    verify_network_guest_bundles,
)


ROOT = pathlib.Path(__file__).resolve().parents[2]
VERIFY = ROOT / "tools" / "verify_package_provenance.py"


class ReleaseProvenanceTests(unittest.TestCase):
    def test_android_network_client_shell_provenance(self):
        class Parser:
            @staticmethod
            def error(message):
                raise ValueError(message)

        template = (
            ROOT / "mobile/shells/android/app/src/main/AndroidManifest.xml"
        ).read_bytes()
        for mode in ("offline", "network", "network_client"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temporary:
                root = pathlib.Path(temporary)
                release = root / "release"
                package = root / "package"
                relative = "android/app/src/main/AndroidManifest.xml"
                source = release / "mobile/shells" / relative
                destination = package / relative
                source.parent.mkdir(parents=True)
                destination.parent.mkdir(parents=True)
                source.write_bytes(template)
                receipt = {
                    "target": "android-arm64",
                    "name": "demo",
                    "package_id": "com.example.demo",
                    "network": mode == "network",
                    "network_client": mode == "network_client",
                }
                (package / "stasis_mobile_package.json").write_text(
                    json.dumps(receipt), encoding="utf-8"
                )
                expected = template.decode("utf-8").replace("@STASIS_APP_NAME@", "demo")
                expected = expected.replace(
                    "@STASIS_ANDROID_ORIENTATION@", "sensorLandscape"
                )
                expected = expected.replace(
                    "@STASIS_NETWORK_PERMISSION@",
                    '    <uses-permission android:name="android.permission.INTERNET" />\n'
                    if mode != "offline" else "",
                ).replace(
                    "@STASIS_NETWORK_CLIENT_PERMISSION@",
                    '    <permission android:name="com.example.demo.permission.PROVISION_NETWORK_CLIENT" '
                    'android:protectionLevel="signature" />'
                    if mode == "network_client" else "",
                ).replace(
                    "@STASIS_NETWORK_CLIENT_ALIAS@",
                    '        <activity-alias android:name=".NetworkJoin" '
                    'android:targetActivity=".MainActivity" android:exported="true" '
                    'android:permission="com.example.demo.permission.PROVISION_NETWORK_CLIENT" />'
                    if mode == "network_client" else "",
                )
                destination.write_bytes(expected.encode("utf-8"))
                activity_source = source.parent / "client.txt"
                activity_source.write_bytes(
                    b"@STASIS_NETWORK_ENABLED@ @STASIS_NETWORK_CLIENT_ENABLED@"
                )
                (destination.parent / "client.txt").write_bytes(
                    {"offline": b"0 0", "network": b"1 0", "network_client": b"0 1"}[mode]
                )
                if mode != "offline":
                    network = package / "android/app/src/main/cpp/network"
                    (network / "include").mkdir(parents=True)
                    (network / "libstasis_network.a").write_bytes(b"library")
                    (network / "include/stasis_network.h").write_bytes(b"header")
                (package / "common").mkdir()
                (package / "common/stasis_package_provenance.h").write_bytes(
                    b"#ifndef STASIS_PACKAGE_PROVENANCE_H\n#define STASIS_PACKAGE_PROVENANCE_H\n"
                    b'#define STASIS_PACKAGE_RELEASE_TAG "development"\n'
                    b'#define STASIS_PACKAGE_SOURCE_COMMIT "unknown"\n'
                    b'#define STASIS_PACKAGE_BUILD_LABEL "non-release development build"\n#endif\n'
                )
                manifest = {
                    "development_build": True,
                    "mobile_shell_sources": {
                        path.relative_to(release).as_posix(): hashlib.sha256(
                            path.read_bytes()
                        ).hexdigest()
                        for path in (source, activity_source)
                    },
                }
                verify_mobile_shells(Parser(), release, package, manifest)
                destination.write_bytes(
                    expected.replace(
                        'android:exported="true"', 'android:exported="false"'
                    ).encode("utf-8")
                )
                with self.assertRaisesRegex(ValueError, "does not match release transform"):
                    verify_mobile_shells(Parser(), release, package, manifest)

    def test_desktop_network_artifact_hashes_are_exact_and_complete(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            self.assertEqual({}, desktop_network_artifact_hashes(root))

            library = root / DESKTOP_NETWORK_LIBRARIES[0]
            header = root / DESKTOP_NETWORK_HEADER
            library.parent.mkdir(parents=True)
            library.write_bytes(b"network library")
            with self.assertRaisesRegex(ValueError, "incomplete"):
                desktop_network_artifact_hashes(root)

            header.parent.mkdir(parents=True)
            header.write_bytes(b"network header")
            self.assertEqual(
                {
                    DESKTOP_NETWORK_LIBRARIES[0]: hashlib.sha256(
                        b"network library"
                    ).hexdigest(),
                    DESKTOP_NETWORK_HEADER: hashlib.sha256(
                        b"network header"
                    ).hexdigest(),
                },
                desktop_network_artifact_hashes(root),
            )

    def test_desktop_network_native_archives_require_exactly_one_target(self):
        for library in DESKTOP_NETWORK_LIBRARIES:
            with self.subTest(library=library), tempfile.TemporaryDirectory() as temporary:
                root = pathlib.Path(temporary)
                header = root / DESKTOP_NETWORK_HEADER
                header.parent.mkdir(parents=True)
                header.write_bytes(b"header")
                with self.assertRaisesRegex(ValueError, "incomplete"):
                    desktop_network_artifact_hashes(root)
                native = root / library
                native.parent.mkdir(parents=True)
                native.write_bytes(b"native")
                self.assertEqual(
                    {library: hashlib.sha256(b"native").hexdigest(),
                     DESKTOP_NETWORK_HEADER: hashlib.sha256(b"header").hexdigest()},
                    desktop_network_artifact_hashes(root),
                )
                other = next(name for name in DESKTOP_NETWORK_LIBRARIES if name != library)
                other_path = root / other
                other_path.parent.mkdir(parents=True, exist_ok=True)
                other_path.write_bytes(b"wrong target")
                with self.assertRaisesRegex(ValueError, "exactly one"):
                    desktop_network_artifact_hashes(root)
                other_path.unlink()
                (native.parent / "unexpected.a").write_bytes(b"untracked payload")
                with self.assertRaisesRegex(ValueError, "unsupported"):
                    desktop_network_artifact_hashes(root)

    def test_network_guest_bundle_identity_rejects_corruption_and_missing_pairs(self):
        class Parser:
            @staticmethod
            def error(message):
                raise ValueError(message)

        with tempfile.TemporaryDirectory() as temporary:
            package = pathlib.Path(temporary)
            bundle = package / "network_guest.bundle"
            receipt = package / "network_guest.bundle.json"
            payload = b"SGB1 test payload"
            identity = {
                "format": "stasis.static_bundle.v1",
                "path": "network_guest.bundle",
                "length": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
            verify_network_guest_bundles(Parser(), package)
            bundle.write_bytes(payload)
            with self.assertRaisesRegex(ValueError, "identity is missing"):
                verify_network_guest_bundles(Parser(), package)
            receipt.write_text(json.dumps(identity), encoding="utf-8")
            verify_network_guest_bundles(Parser(), package)
            bundle.write_bytes(b"X" + payload[1:])
            with self.assertRaisesRegex(ValueError, "hash mismatch"):
                verify_network_guest_bundles(Parser(), package)
            bundle.write_bytes(payload + b"X")
            with self.assertRaisesRegex(ValueError, "length mismatch"):
                verify_network_guest_bundles(Parser(), package)
            bundle.unlink()
            with self.assertRaisesRegex(ValueError, "bundle is missing"):
                verify_network_guest_bundles(Parser(), package)
            bundle.write_bytes(payload)
            for path in ("../network_guest.bundle", "/network_guest.bundle", "other.bundle"):
                identity["path"] = path
                receipt.write_text(json.dumps(identity), encoding="utf-8")
                with self.assertRaisesRegex(ValueError, "unsafe"):
                    verify_network_guest_bundles(Parser(), package)

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
            identity_path = package / "stasis_asset_package.json"
            identity_path.write_text(
                json.dumps(identity), encoding="utf-8"
            )
            verify_asset_package_identities(Parser(), package)
            identity_path.unlink()
            with self.assertRaisesRegex(ValueError, "identity is missing"):
                verify_asset_package_identities(Parser(), package)
            identity_path.write_text(json.dumps(identity), encoding="utf-8")
            (package / "assets/manifest.json").write_bytes(manifest + b"\n")
            with self.assertRaisesRegex(ValueError, "manifest hash mismatch"):
                verify_asset_package_identities(Parser(), package)

    def test_render_contract_version_reads_current_header_constant(self):
        self.assertEqual(7, render_contract_version(ROOT))

    def test_render_contract_version_rejects_missing_or_non_numeric_constant(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            runtime = root / "runtime"
            runtime.mkdir()
            header = runtime / "stasis_render_contract.h"
            header.write_text(
                "#define STASIS_RENDER_CURRENT_VERSION 6\n",
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "missing STASIS_RENDER_VERSION"):
                render_contract_version(root)
            header.write_text(
                "#define STASIS_RENDER_VERSION 6\n",
                encoding="utf-8",
            )
            self.assertEqual(6, render_contract_version(root))
            header.write_text(
                "#define STASIS_RENDER_VERSION not_numeric\n",
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
        graphics_sources = re.search(
            r"target_sources\(\$\{target\}\s+PRIVATE\s+([^)]*)\)", runtime_cmake
        )
        self.assertIsNotNone(graphics_sources)
        self.assertTrue({
            "stasis_graphics.c", "stasis_image_writer.c", "stasis_audio_assets.c",
            "stasis_platform_services.c", "stasis_svg.cpp",
        }.issubset(set(graphics_sources.group(1).split())))
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

    def test_canonical_runtime_files_close_over_quoted_local_includes(self):
        runtime = ROOT / "runtime"
        canonical_files = set(RUNTIME_FILES)
        canonical_directories = tuple(pathlib.PurePosixPath(path) for path in RUNTIME_DIRS)
        source_suffixes = {".c", ".cc", ".cpp", ".h", ".hpp"}

        for source_name in RUNTIME_FILES:
            source = runtime / source_name
            if source.suffix not in source_suffixes:
                continue
            includes = re.findall(
                r'^\s*#\s*include\s+"([^"]+)"',
                source.read_text(encoding="utf-8"),
                re.MULTILINE,
            )
            for include_name in includes:
                include_path = pathlib.PurePosixPath(include_name.replace("\\", "/"))
                direct = source.parent / pathlib.Path(*include_path.parts)
                candidates = [direct] if direct.is_file() else []
                if not candidates:
                    for directory in RUNTIME_DIRS:
                        subtree = runtime / directory
                        candidates.extend(
                            path
                            for path in subtree.rglob(include_path.name)
                            if pathlib.PurePosixPath(path.relative_to(subtree).as_posix()).as_posix().endswith(
                                include_path.as_posix()
                            )
                        )
                self.assertTrue(
                    candidates,
                    f'{source_name} quoted include "{include_name}" does not resolve inside the release runtime closure',
                )
                for candidate in candidates:
                    relative = pathlib.PurePosixPath(candidate.relative_to(runtime).as_posix())
                    self.assertTrue(
                        relative.as_posix() in canonical_files
                        or any(relative.is_relative_to(directory) for directory in canonical_directories),
                        f'{source_name} quoted include "{include_name}" resolves to uncatalogued runtime file {relative}',
                    )

    def test_mobile_runtime_closure_matches_release_provenance(self):
        toolchain = (ROOT / "apps/stasis/src/toolchain_cli.rs").read_text(encoding="utf-8")

        def rust_string_slice(name):
            match = re.search(
                rf"const {name}: &\[&str\]\s*=\s*&\[(.*?)\];",
                toolchain,
                re.DOTALL,
            )
            self.assertIsNotNone(match, name)
            return tuple(re.findall(r'"([^"]+)"', match.group(1)))

        self.assertEqual(RUNTIME_FILES, rust_string_slice("MOBILE_RUNTIME_FILES"))
        self.assertEqual(RUNTIME_DIRS, rust_string_slice("MOBILE_RUNTIME_DIRS"))

    def test_release_workflows_include_complete_knowledge_library(self):
        toolchain = (ROOT / "apps/stasis/src/toolchain_cli.rs").read_text(
            encoding="utf-8"
        )
        knowledge_match = re.search(
            r"const KNOWLEDGE_FILES: &\[&str\]\s*=\s*&\[(.*?)\];",
            toolchain,
            re.DOTALL,
        )
        self.assertIsNotNone(knowledge_match, "KNOWLEDGE_FILES")
        knowledge_files = tuple(re.findall(r'"([^"]+)"', knowledge_match.group(1)))
        self.assertTrue(knowledge_files)

        knowledge_root = ROOT / "docs/knowledge"
        for relative in knowledge_files:
            source = knowledge_root.joinpath(*pathlib.PurePosixPath(relative).parts)
            self.assertTrue(source.is_file(), relative)

        for workflow_name in (
            ".github/workflows/nightly-release.yml",
            ".github/workflows/bootstrap-artifacts.yml",
        ):
            workflow = (ROOT / workflow_name).read_text(encoding="utf-8")
            unix_copy = re.findall(
                r'^\s+cp -R docs/knowledge "\$\{out\}/docs/"\s*$',
                workflow,
                re.MULTILINE,
            )
            windows_copy = re.findall(
                r'^\s+Copy-Item docs/knowledge "\$out/docs/knowledge" -Recurse -Force\s*$',
                workflow,
                re.MULTILINE,
            )
            self.assertEqual(1, len(unix_copy), workflow_name)
            self.assertEqual(1, len(windows_copy), workflow_name)

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
            self.assertEqual(set(RUNTIME_FILES), unix_files, workflow_name)
            self.assertEqual(set(RUNTIME_FILES), windows_files, workflow_name)
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
        self.assertIn('--target "${GITHUB_SHA}"', release_block)
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
                b"@STASIS_ANDROID_ABI@ @STASIS_NETWORK_ENABLED@ "
                b"@STASIS_NETWORK_CLIENT_ENABLED@\n"
                b"@STASIS_NETWORK_PERMISSION@"
                b"@STASIS_NETWORK_CLIENT_PERMISSION@\n"
                b"@STASIS_NETWORK_CLIENT_ALIAS@\n"
            )
            manifest = {
                "schema": "stasis.release_provenance.v1",
                "release_tag": "v1.0.0",
                "source_commit": "0123456789012345678901234567890123456789",
                "development_build": False,
                "dirty_state": False,
                "command_buffer": {"name": "gfx_cmd", "version": 7},
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
                b"arm64-v8a 0 0\n\n\n"
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
            network_client_receipt = {
                "target": "android-arm64",
                "name": "demo",
                "app_name": "Demo App",
                "package_id": "com.example.demo",
                "android_orientation": "sensorPortrait",
                "android_version_code": "7",
                "android_version_name": "2.1.0",
                "network_client": True,
            }
            (package / "stasis_mobile_package.json").write_text(
                json.dumps(network_client_receipt), encoding="utf-8"
            )
            (package / "android/main.c").write_bytes(
                b"Demo App com.example.demo com_example_demo sensorPortrait 7 2.1.0\n"
                b"arm64-v8a 0 1\n"
                b'    <uses-permission android:name="android.permission.INTERNET" />\n'
                b'    <permission android:name="com.example.demo.permission.PROVISION_NETWORK_CLIENT" android:protectionLevel="signature" />\n'
                b'        <activity-alias android:name=".NetworkJoin" android:targetActivity=".MainActivity" android:exported="true" android:permission="com.example.demo.permission.PROVISION_NETWORK_CLIENT" />\n'
            )
            network = package / "android/app/src/main/cpp/network"
            (network / "include").mkdir(parents=True)
            (network / "libstasis_network.a").write_bytes(b"library")
            (network / "include/stasis_network.h").write_bytes(b"header")
            self.assertEqual(subprocess.run(command, check=False).returncode, 0)
            network_client_receipt["network_client"] = False
            (package / "stasis_mobile_package.json").write_text(
                json.dumps(network_client_receipt), encoding="utf-8"
            )
            client_mismatch = subprocess.run(
                command, check=False, capture_output=True, text=True
            )
            self.assertNotEqual(client_mismatch.returncode, 0)
            self.assertIn("release transform", client_mismatch.stderr)
            (package / "stasis_mobile_package.json").write_text(
                json.dumps(network_client_receipt | {"network_client": True}),
                encoding="utf-8",
            )
            legacy = dict(manifest)
            legacy["command_buffer"] = {"name": "gfx_cmd", "version": 4}
            (release / "stasis_release_provenance.json").write_text(
                json.dumps(legacy), encoding="utf-8"
            )
            (package / "stasis_provenance.json").write_text(
                json.dumps(legacy), encoding="utf-8"
            )
            legacy_failed = subprocess.run(command, check=False, capture_output=True, text=True)
            self.assertNotEqual(legacy_failed.returncode, 0)
            self.assertIn("expected current 7", legacy_failed.stderr)
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
            numeric_type["command_buffer"] = {"name": "gfx_cmd", "version": 7.0}
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

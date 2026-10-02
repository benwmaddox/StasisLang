import json
import pathlib
import tempfile
import textwrap
import unittest
from unittest.mock import patch

from tools.audit_release_bundle import required_files
from tools.desktop_network_target import network_target


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/nightly-release.yml"
ANDROID_SHELL = (
    ROOT
    / "mobile/shells/android/app/src/main/java/com/stasislang/game/MainActivity.java"
)


class NightlyNetworkSupportContractTests(unittest.TestCase):
    def test_bootstrap_native_runner_architectures(self):
        for system, architecture, expected in (
            ("macOS", "X64", "macos-x86_64"),
            ("macOS", "ARM64", "macos-arm64"),
            ("Linux", "X64", "linux-x86_64"),
            ("Linux", "ARM64", "linux-arm64"),
        ):
            with self.subTest(system=system, architecture=architecture):
                self.assertEqual(network_target(system, architecture), expected)
        for system, architecture in (("Linux", "X86"), ("macOS", "unknown"), ("unknown", "X64")):
            with self.subTest(system=system, architecture=architecture):
                with self.assertRaises(ValueError):
                    network_target(system, architecture)

    def test_windows_supervisor_is_shipped_and_release_gated(self):
        for name in ("nightly-release.yml", "bootstrap-artifacts.yml"):
            workflow = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            self.assertIn('/stasis-network-supervise.exe" "$out/" -ErrorAction Stop', workflow)
            self.assertIn("docs/network_supervision.md", workflow)
            self.assertIn("--features supervision-cli", workflow)
        manifest = (ROOT / "crates/stasis_network/Cargo.toml").read_text(encoding="utf-8")
        self.assertIn('required-features = ["supervision-cli"]', manifest)
        self.assertIn("name: Verify extracted Windows editor toolchain", self.workflow)
        self.assertIn("-InstalledToolchain", self.workflow)
        self.assertIn("-File tools/ci/test_network_supervision.ps1", self.workflow)
        self.assertIn('Expand-Archive -LiteralPath "dist/${{ matrix.archive }}.${{ matrix.ext }}"', self.workflow)
        self.assertIn('-Toolchain "$validationRoot/stasis.exe"', self.workflow)
        self.assertIn('-Supervisor "$validationRoot/stasis-network-supervise.exe"', self.workflow)

    def test_windows_archive_ships_matching_desktop_network_support(self):
        self.assertIn("name: Build desktop network support (windows)", self.workflow)
        self.assertIn("RUSTFLAGS: -C target-feature=+crt-static", self.workflow)
        self.assertIn(
            "python tools/cargo_cache.py run -- cargo build -p stasis_network --release --target ${{ matrix.rust_target }}",
            self.workflow,
        )
        self.assertIn('"$out/desktop/network/windows-x86_64/"', self.workflow)
        self.assertIn('"$out/desktop/network/include/"', self.workflow)
        self.assertIn('Copy-Item tools/diagnose_desktop_network.ps1', self.workflow)
        self.assertIn('runtime/stasis_network_join_card.h', self.workflow)

    def test_windows_nightly_qualifies_relocated_native_client_package(self):
        step = "name: Qualify extracted Windows native client package"
        hide_source = "Move-Item $repoNetworkSource $repoNetworkBackup -Force"
        package_probe = "test_windows_desktop_network_client_package.ps1"
        self.assertIn(step, self.workflow)
        qualification = self.workflow.index(step)
        self.assertLess(self.workflow.index(hide_source, qualification), self.workflow.index(package_probe, qualification))
        self.assertIn('-Toolchain "build/network-supervision-release/stasis.exe"', self.workflow)
        self.assertIn("-InstalledToolchain", self.workflow[qualification:])

    def test_bootstrap_windows_network_support_is_staged_before_provenance(self):
        workflow = (ROOT / ".github/workflows/bootstrap-artifacts.yml").read_text(encoding="utf-8")
        self.assertIn("name: Build desktop network support (windows)", workflow)
        self.assertIn("RUSTFLAGS: -C target-feature=+crt-static", workflow)
        self.assertIn("python tools/cargo_cache.py run -- cargo build -p stasis_network --release", workflow)
        library_copy = 'Copy-Item -Path "build/codex-cargo-target/release/stasis_network.dll", "build/codex-cargo-target/release/stasis_network.dll.lib" -Destination "$out/desktop/network/windows-x86_64/" -ErrorAction Stop'
        header_copy = 'Copy-Item crates/stasis_network/include/stasis_network.h "$out/desktop/network/include/" -ErrorAction Stop'
        for copied in (library_copy, header_copy):
            self.assertIn(copied, workflow)
            self.assertLess(workflow.index(copied), workflow.index("generate_release_provenance.py", workflow.index(copied)))
        self.assertIn('Copy-Item tools/diagnose_desktop_network.ps1', workflow)

    def test_unix_archives_stage_native_network_support_before_provenance(self):
        for name, target_directory in (
            ("nightly-release.yml", "${{ matrix.rust_target }}/"),
            ("bootstrap-artifacts.yml", ""),
        ):
            with self.subTest(workflow=name):
                workflow = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
                self.assertIn("name: Build desktop network support (unix)", workflow)
                if name == "bootstrap-artifacts.yml":
                    self.assertIn('tools/desktop_network_target.py "${RUNNER_OS}" "${RUNNER_ARCH}"', workflow)
                    self.assertIn("libstasis_network.dylib", workflow)
                    library_copy = 'cp "build/codex-cargo-target/release/libstasis_network.so" "${out}/desktop/network/${network_target}/"'
                else:
                    self.assertIn('network_target=macos-arm64', workflow)
                    self.assertIn('network_target=linux-x86_64', workflow)
                    library_copy = (
                        'cp "build/codex-cargo-target/' + target_directory
                        + 'release/${network_shared}" "${out}/desktop/network/${network_target}/"'
                    )
                header_copy = 'cp crates/stasis_network/include/stasis_network.h "${out}/desktop/network/include/"'
                provenance = workflow.index("generate_release_provenance.py")
                self.assertLess(workflow.index(library_copy), provenance)
                self.assertLess(workflow.index(header_copy), provenance)

    def test_unix_network_package_smoke_uses_relocated_release_support(self):
        hide_source = 'mv "${repo_root}/crates/stasis_network" "${network_source_backup}/"'
        package = './bin/stasis --workspace cli-smoke package --target desktop --out dist/network-desktop'
        self.assertLess(self.workflow.index(hide_source), self.workflow.index(package))
        self.assertIn('test -f cli-smoke/dist/network-desktop/network_guest.bundle', self.workflow)
        self.assertIn('test -f cli-smoke/dist/network-desktop/network_guest.bundle.json', self.workflow)
        self.assertIn('--release-root . --package-root cli-smoke/dist/network-desktop', self.workflow)
        self.assertIn("tools/ci/test_linux_desktop_network_package.py", self.workflow)
        self.assertIn("--executable cli-smoke/dist/network-desktop/ci_smoke", self.workflow)

    def test_unix_ci_covers_native_link_lifecycle_and_packages(self):
        workflow = (ROOT / ".github/workflows/network-browser-acceptance.yml").read_text(encoding="utf-8")
        unix = workflow.split("  unix-desktop-package:", 1)[1].split("  apple-network-artifacts:", 1)[0]
        self.assertIn("runs-on: ubuntu-latest", unix)
        self.assertNotIn("macos-15", unix)
        self.assertIn("timeout-minutes: 15", unix)
        self.assertIn("libx11-dev", unix)
        self.assertIn("libxkbcommon-dev", unix)
        self.assertLess(unix.index("apt-get install"), unix.index("cargo test -p stasis --lib"))
        self.assertIn("bash tools/ci/test_desktop_network_link.sh", unix)
        self.assertIn("--target stasis_mobile_runtime_network_test stasis_network_join_card_test", unix)
        self.assertIn("mobile_runtime_network_lifecycle|network_join_card_contract", unix)
        self.assertIn("--no-tests=error", unix)
        self.assertIn("--lib macos_packaged_runner", unix)
        self.assertIn("cargo test -p stasis --bin stasis network", unix)
        self.assertIn("bash tools/ci/test_unix_desktop_network_package.sh", unix)
        script = (ROOT / "tools/ci/test_desktop_network_link.sh").read_text(encoding="utf-8")
        self.assertIn("tools/cargo_cache.py run -- cargo build -p stasis_network --release", script)
        self.assertIn("stasis_network_link_test stasis_network_client_link_test", script)
        self.assertIn("timeout=60", script)

    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.android_shell = ANDROID_SHELL.read_text(encoding="utf-8")

    def test_support_job_is_fail_closed_and_uses_pinned_targets(self):
        self.assertIn("mobile_network_support:", self.workflow)
        self.assertIn(
            "needs: [detect, mobile_network_support, android_runtime_support]",
            self.workflow,
        )
        self.assertIn("runs-on: ubuntu-latest", self.workflow)
        self.assertIn("macos-15", self.workflow)
        self.assertIn('ndk;27.0.12077973', self.workflow)
        self.assertIn("aarch64-linux-android", self.workflow)
        self.assertIn("x86_64-linux-android", self.workflow)
        self.assertIn('"platforms;android-26"', self.workflow)
        self.assertIn('api_target="${target}26"', self.workflow)
        self.assertIn("-Wl,-soname,libstasis_network_v1.so", self.workflow)
        self.assertIn(
            "network-artifact/android-arm64/libstasis_network_v1.so",
            self.workflow,
        )
        self.assertNotIn("libstasis_network.a", self.workflow)
        self.assertNotIn("release/stasis_network.lib", self.workflow)
        self.assertIn("--network-enabled --readelf", self.workflow)
        self.assertNotIn("xcrun", self.workflow)
        self.assertNotIn("aarch64-apple-ios", self.workflow)
        self.assertIn("actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a", self.workflow)
        self.assertIn("name: mobile-network-support-android", self.workflow)

    def test_apple_network_acceptance_is_scoped_to_the_network_workflow(self):
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            workflow = path.read_text(encoding="utf-8")
            with self.subTest(workflow=path.name):
                if path.name == "network-browser-acceptance.yml":
                    self.assertIn("apple-network-artifacts:", workflow)
                    self.assertIn("runs-on: macos-15", workflow)
                    self.assertIn("aarch64-apple-ios-sim", workflow)
                    package_script = (
                        ROOT / "tools/ci/test_unix_desktop_network_package.sh"
                    ).read_text(encoding="utf-8")
                    signer = (
                        ROOT / "tools/ci/macos_ad_hoc_sign.sh"
                    ).read_text(encoding="utf-8")
                    self.assertIn("STASIS_AOT_SIGN_TOOL", package_script)
                    self.assertIn(
                        "not has_suffix(order[-2], dylib_suffix)", package_script
                    )
                    self.assertIn(
                        "not has_suffix(order[-1], app_suffix)", package_script
                    )
                    self.assertIn("executable_suffix", package_script)
                    self.assertIn("codesign --verify --strict", package_script)
                    self.assertNotIn("codesign --force --sign -", package_script)
                    self.assertIn("codesign --force --sign -", signer)
                elif path.name != "nightly-release.yml":
                    self.assertNotRegex(
                        workflow,
                        r"(?im)^\s*(?:runs-on|os):\s*[^\n]*\bmacos(?:-[\w]+)?\b",
                    )
                self.assertNotRegex(workflow, r"(?im)^\s*kind:\s*ios\b")
        for marker in ("ios-package-link:", "ios-generics-simulator:"):
            self.assertNotIn(marker, (ROOT / ".github/workflows/pr-ci.yml").read_text(encoding="utf-8"))
        for archive in ("stasis-nightly-linux-x64", "stasis-nightly-win-x64", "stasis-nightly-osx-arm64"):
            self.assertIn(f"archive: {archive}", self.workflow)
        self.assertIn("vsce_target: darwin-arm64", self.workflow)
        self.assertIn("name: stasis-nightly-osx-arm64", self.workflow)
        self.assertIn("python3 tools/audit_release_bundle.py", self.workflow)
        self.assertIn("python tools/audit_release_bundle.py", self.workflow)
        self.assertIn("mobile/network/android-arm64/libstasis_network_v1.so", required_files("linux"))
        self.assertIn("mobile/network/android-x86_64/libstasis_network_v1.so", required_files("windows"))
        self.assertIn("desktop/network/macos-arm64/libstasis_network.dylib", required_files("macos"))

    def test_archive_layout_is_copied_before_provenance(self):
        for path in (
            "mobile/network/android-arm64/libstasis_network_v1.so",
            "mobile/network/android-x86_64/libstasis_network_v1.so",
            "mobile/network/include/stasis_network.h",
        ):
            self.assertIn(path, self.workflow)
        for platform in ("windows", "linux", "macos"):
            self.assertIn("mobile/network/android-arm64/libstasis_network_v1.so", required_files(platform))
            self.assertIn("mobile/network/android-x86_64/libstasis_network_v1.so", required_files(platform))
            self.assertNotIn("mobile/network/ios-arm64/libstasis_network.a", required_files(platform))
            self.assertNotIn("mobile/network/ios-simulator-arm64/libstasis_network.a", required_files(platform))
        self.assertNotIn("mobile/network/ios-arm64/libstasis_network.a", self.workflow)
        self.assertNotIn("mobile/network/ios-simulator-arm64/libstasis_network.a", self.workflow)
        self.assertIn('cp -R mobile/network "${out}/mobile/"', self.workflow)
        self.assertIn('Copy-Item mobile/network "$out/mobile/"', self.workflow)
        self.assertGreaterEqual(
            self.workflow.count("generate_release_provenance.py"), 2
        )

    def test_relocated_network_smoke_keeps_android_and_rejects_unavailable_ios_archive(self):
        self.assertIn('"capabilities"] = {"network": True}', self.workflow)
        self.assertIn('"stasis.network": {"features": ["host"]}', self.workflow)
        self.assertIn('"web"] = {"entry": "src/main.stasis"}', self.workflow)
        self.assertIn("dist/network-android", self.workflow)
        self.assertIn("network_guest.bundle", self.workflow)
        self.assertNotIn("ios/network/libstasis_network.a", self.workflow)
        self.assertIn("crates/stasis_network", self.workflow)

    def test_android_acceptance_upgrades_fixture_before_explicit_libraries(self):
        job = self.workflow.split("  android_prebuilt_acceptance:", 1)[1].split(
            "  vscode_extension:", 1
        )[0]
        self.assertEqual(2, job.count('value["manifest_version"] = 3'))
        host_version = job.index('value["manifest_version"] = 3')
        host_library = job.index('"stasis.network": {"features": ["host"]}')
        client_version = job.index('value["manifest_version"] = 3', host_version + 1)
        client_library = job.index('"stasis.network": {"features": ["client"]}')
        self.assertLess(host_version, host_library)
        self.assertLess(client_version, client_library)
        self.assertIn("stasis-network-source-backup", self.workflow)
        self.assertIn("requires a macOS host with Xcode", self.workflow)
        self.assertIn("verify_package_provenance.py", self.workflow)

    def test_legacy_android_acceptance_preserves_host_guest_entry(self):
        legacy_loop = self.workflow.split(
            "for legacy_variant in legacy-v1-host legacy-v2-client; do", 1
        )[1]
        script = textwrap.dedent(
            legacy_loop.split("<<'PY'\n", 1)[1].split("          PY", 1)[0]
        )
        for variant, version, capability in (
            ("legacy-v1-host", 1, "network"),
            ("legacy-v2-client", 2, "network_client"),
        ):
            with self.subTest(variant=variant), tempfile.TemporaryDirectory() as directory:
                manifest_path = pathlib.Path(directory) / "stasis.json"
                manifest_path.write_text(
                    json.dumps({
                        "manifest_version": 3,
                        "entry": "src/main.stasis",
                        "libraries": {"selections": {}},
                        "web": {"entry": "old.stasis"},
                    }),
                    encoding="utf-8",
                )
                with patch("sys.argv", ["fixture", str(manifest_path), variant]):
                    exec(compile(script, "nightly legacy fixture", "exec"), {})
                manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                self.assertEqual(version, manifest["manifest_version"])
                self.assertEqual({capability: True}, manifest["capabilities"])
                self.assertNotIn("libraries", manifest)
                if capability == "network":
                    self.assertEqual({"entry": "src/main.stasis"}, manifest["web"])
                else:
                    self.assertNotIn("web", manifest)

    def test_relocated_smoke_hides_checkout_source_and_restores_it(self):
        windows_root = '$checkoutRoot = (Get-Location).Path'
        windows_push = 'Push-Location "dist/${{ matrix.archive }}"'
        self.assertIn(windows_root, self.workflow)
        self.assertLess(self.workflow.index(windows_root), self.workflow.index(windows_push))
        self.assertIn(
            '$repoNetworkSource = Join-Path $checkoutRoot "crates/stasis_network"',
            self.workflow,
        )
        self.assertIn('try {', self.workflow)
        self.assertIn('} finally {', self.workflow)
        self.assertIn('Move-Item $repoNetworkBackup $repoNetworkSource -Force', self.workflow)
        self.assertNotIn(
            '$repoNetworkSource = Join-Path (Get-Location) "crates/stasis_network"',
            self.workflow,
        )

        unix_root = 'repo_root="$(pwd)"'
        unix_push = 'pushd "dist/${{ matrix.archive }}"'
        self.assertLess(self.workflow.index(unix_root), self.workflow.index(unix_push))
        self.assertIn('trap restore_network_source EXIT', self.workflow)
        self.assertIn('mv "${repo_root}/crates/stasis_network" "${network_source_backup}/"', self.workflow)
        self.assertIn('restore_network_source\n          trap - EXIT', self.workflow)

    def test_generic_android_shell_has_no_product_specific_copy(self):
        self.assertNotIn("Maddox", self.android_shell)
        self.assertIn('Manual network join URL', self.android_shell)
        self.assertIn('Stasis join URL', self.android_shell)


if __name__ == "__main__":
    unittest.main()

import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/nightly-release.yml"
ANDROID_SHELL = (
    ROOT
    / "mobile/shells/android/app/src/main/java/com/stasislang/game/MainActivity.java"
)


class NightlyNetworkSupportContractTests(unittest.TestCase):
    def test_windows_archive_ships_matching_desktop_network_support(self):
        self.assertIn("name: Build desktop network support (windows)", self.workflow)
        self.assertIn("RUSTFLAGS: -C target-feature=+crt-static", self.workflow)
        self.assertIn(
            "python tools/cargo_cache.py run -- cargo build -p stasis_network --release --target ${{ matrix.rust_target }} --target-dir target/desktop-network",
            self.workflow,
        )
        self.assertIn('"$out/desktop/network/windows-x86_64/"', self.workflow)
        self.assertIn('"$out/desktop/network/include/"', self.workflow)
        self.assertIn('Copy-Item tools/diagnose_desktop_network.ps1', self.workflow)
        self.assertIn('runtime/stasis_network_join_card.h', self.workflow)

    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text(encoding="utf-8")
        cls.android_shell = ANDROID_SHELL.read_text(encoding="utf-8")

    def test_support_job_is_fail_closed_and_uses_pinned_targets(self):
        self.assertIn("mobile_network_support:", self.workflow)
        self.assertIn("needs: [detect, mobile_network_support]", self.workflow)
        self.assertIn("os: ubuntu-latest", self.workflow)
        self.assertIn("os: macos-15", self.workflow)
        self.assertIn('ndk;27.0.12077973', self.workflow)
        self.assertIn("aarch64-linux-android", self.workflow)
        self.assertIn("x86_64-linux-android", self.workflow)
        self.assertIn("aarch64-apple-ios", self.workflow)
        self.assertIn('"platforms;android-26"', self.workflow)
        self.assertIn('api_target="${target}26"', self.workflow)
        self.assertIn("xcrun --sdk iphoneos --find clang", self.workflow)
        self.assertIn("actions/upload-artifact@v4", self.workflow)
        self.assertIn("name: mobile-network-support-${{ matrix.kind }}", self.workflow)

    def test_archive_layout_is_copied_before_provenance(self):
        for path in (
            "mobile/network/android-arm64/libstasis_network.a",
            "mobile/network/android-x86_64/libstasis_network.a",
            "mobile/network/ios-arm64/libstasis_network.a",
            "mobile/network/include/stasis_network.h",
        ):
            self.assertIn(path, self.workflow)
        self.assertIn('cp -R mobile/network "${out}/mobile/"', self.workflow)
        self.assertIn('Copy-Item mobile/network "$out/mobile/"', self.workflow)
        self.assertGreaterEqual(
            self.workflow.count("generate_release_provenance.py"), 2
        )

    def test_relocated_network_smoke_is_present_for_android_and_ios(self):
        self.assertIn('"capabilities"] = {"network": True}', self.workflow)
        self.assertIn('"web"] = {"entry": "src/main.stasis"}', self.workflow)
        self.assertIn("dist/network-android", self.workflow)
        self.assertIn("network_guest.bundle", self.workflow)
        self.assertIn("ios/network/libstasis_network.a", self.workflow)
        self.assertIn("crates/stasis_network", self.workflow)
        self.assertIn("stasis-network-source-backup", self.workflow)
        self.assertIn("requires a macOS host with Xcode", self.workflow)
        self.assertIn("verify_package_provenance.py", self.workflow)

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

"""Deterministic source-contract checks for the Windows local installer."""

from pathlib import Path
import re
import unittest
from tools.compute_toolchain_fingerprint import fingerprint
from tools.generate_release_provenance import RUNTIME_DIRS, RUNTIME_FILES


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = (ROOT / "scripts" / "install_local_toolchain.ps1").read_text(encoding="ascii")


class LocalToolchainInstallTests(unittest.TestCase):
    def test_fingerprint_helper_is_deterministic(self):
        value = fingerprint("abc123", "local-test")
        self.assertEqual(value, fingerprint("abc123", "local-test"))
        self.assertEqual(len(value), 64)
        self.assertNotEqual(value, fingerprint("abc124", "local-test"))

    def test_builds_both_halves_from_one_fingerprint(self):
        self.assertIn('STASIS_BUILD_FINGERPRINT', SCRIPT)
        self.assertIn('-DSTASIS_BUILD_FINGERPRINT=$fingerprint', SCRIPT)
        self.assertIn('cargo_cache.py", "run", "--", "cargo", "build"', SCRIPT)
        self.assertIn('"--path-format=absolute", "--git-common-dir"', SCRIPT)
        self.assertIn('Join-Path $cargoTarget "release/stasis.exe"', SCRIPT)
        self.assertIn('editor-info', SCRIPT)
        self.assertIn('$editorInfo.result.build_fingerprint', SCRIPT)
        self.assertIn('windows_launch_smoke', SCRIPT)

    def test_uses_one_cached_release_build_for_both_packages(self):
        build_start = SCRIPT.index('if (-not $SkipBuild)')
        build_end = SCRIPT.index('$vcpkgRoot', build_start)
        build_section = re.sub(r'\s+', ' ', SCRIPT[build_start:build_end])
        cargo_builds = re.findall(
            r'Invoke-Bounded .*?tools/cargo_cache\.py.*?"cargo", "build".*?\| Out-Null',
            build_section,
        )
        self.assertEqual(len(cargo_builds), 1)
        self.assertRegex(cargo_builds[0], r'"-p", "stasis"')
        self.assertRegex(cargo_builds[0], r'"-p", "stasis_dynload"')
        self.assertRegex(cargo_builds[0], r'"--release"')

    def test_stages_complete_dynamic_toolchain(self):
        for required in (
            'stasis_dynload.dll',
            'stasis_dynload.dll.lib',
            'stasis_graphics.dll',
            'stasis_runner.exe',
            'Get-ChildItem -LiteralPath (Split-Path -Parent $runtime) -Filter "*.dll"',
            'Join-Path $Destination "runtime"',
            'Join-Path $staging "mobile"',
            'Join-Path $staging "tools/windows"',
            '$signingArtifacts',
            'configured local signer failed',
        ):
            self.assertIn(required, SCRIPT)

    def test_runtime_staging_excludes_generated_build_outputs(self):
        files_match = re.search(
            r"\$runtimeSourceFiles\s*=\s*@\((.*?)\n\)", SCRIPT, re.DOTALL
        )
        self.assertIsNotNone(files_match)
        runtime_files = set(re.findall(r'"([^"]+)"', files_match.group(1)))
        directories_match = re.search(
            r"\$runtimeSourceDirectories\s*=\s*@\((.*?)\)", SCRIPT, re.DOTALL
        )
        self.assertIsNotNone(directories_match)
        runtime_directories = set(
            re.findall(r'"([^"]+)"', directories_match.group(1))
        )

        self.assertEqual(set(RUNTIME_FILES), runtime_files)
        self.assertEqual(set(RUNTIME_DIRS), runtime_directories)
        self.assertIn("third_party/thorvg", runtime_directories)
        self.assertNotIn("build", runtime_directories)
        self.assertNotIn("build_ci", runtime_directories)
        self.assertNotIn("stasis_graphics.dll", runtime_files)
        self.assertNotIn("stasis_runner.exe", runtime_files)
        self.assertIn("foreach ($relative in $runtimeSourceFiles)", SCRIPT)
        self.assertIn("foreach ($relative in $runtimeSourceDirectories)", SCRIPT)
        self.assertIn(
            "Copy-RuntimeSources -SourceRoot $repoRoot -Destination $staging", SCRIPT
        )
        self.assertNotIn(
            'Copy-Item -LiteralPath (Join-Path $repoRoot "runtime") '
            '-Destination (Join-Path $staging "runtime") -Recurse',
            SCRIPT,
        )

    def test_promotion_is_staged_and_rolls_back(self):
        self.assertIn('Move-Item -LiteralPath $Destination -Destination $backup', SCRIPT)
        self.assertIn('Move-Item -LiteralPath $Staging -Destination $Destination', SCRIPT)
        self.assertIn('[Parameter(Mandatory)] [scriptblock]$PostActivationValidation', SCRIPT)
        self.assertIn('& $PostActivationValidation', SCRIPT)
        self.assertIn('InjectBackupCleanupFailure', SCRIPT)
        self.assertIn('Write-Warning "installed toolchain, but could not remove backup', SCRIPT)
        self.assertIn('previous bin was restored', SCRIPT)
        self.assertIn('TestInjectPromotionFailure', SCRIPT)
        self.assertIn('TestInjectValidationFailure', SCRIPT)
        self.assertIn('test-injected post-activation validation failure', SCRIPT)
        self.assertIn('STASIS_TEST_MODE -ne "1"', SCRIPT)
        self.assertIn('required build output is missing', SCRIPT)

    def test_post_activation_checks_use_stable_bin(self):
        self.assertNotRegex(
            SCRIPT,
            r'Invoke-Bounded\s+-FilePath\s+\(Join-Path \$staging\s+"stasis\.exe"\)',
        )
        self.assertGreaterEqual(
            len(re.findall(r'Invoke-Bounded\s+-FilePath \$installedExecutable', SCRIPT)),
            2,
        )
        self.assertIn('Join-Path $binRoot "stasis.exe"', SCRIPT)
        self.assertIn('-WorkingDirectory $binRoot', SCRIPT)

    def test_backup_lives_through_post_activation_validation(self):
        promotion_start = SCRIPT.index('function Promote-ToolchainDirectory')
        promotion_end = SCRIPT.index('\nfunction Copy-RuntimeSources', promotion_start)
        promotion = SCRIPT[promotion_start:promotion_end]
        backup_move = promotion.index('Move-Item -LiteralPath $Destination -Destination $backup')
        validation = promotion.index('& $PostActivationValidation')
        transaction_end = promotion.index('\n  } catch {', validation)
        backup_cleanup = promotion.index('if (Test-Path -LiteralPath $backup)', transaction_end)
        self.assertLess(backup_move, validation)
        self.assertLess(validation, transaction_end)
        self.assertLess(transaction_end, backup_cleanup)
        self.assertIn('try {', promotion[backup_cleanup:])
        self.assertIn('Write-Warning', promotion[backup_cleanup:])

    def test_post_activation_rollback_removes_candidate_and_staging(self):
        promotion_start = SCRIPT.index('function Promote-ToolchainDirectory')
        promotion_end = SCRIPT.index('\nfunction Copy-RuntimeSources', promotion_start)
        promotion = SCRIPT[promotion_start:promotion_end]
        self.assertIn('Remove-Item -LiteralPath $Destination -Recurse -Force', promotion)
        self.assertIn('Move-Item -LiteralPath $backup -Destination $Destination -Force', promotion)
        self.assertIn('Remove-Item -LiteralPath $Staging -Recurse -Force', promotion)
        self.assertIn('Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction Stop', promotion)

    def test_cleanup_failure_is_best_effort_after_transaction(self):
        promotion_start = SCRIPT.index('function Promote-ToolchainDirectory')
        promotion_end = SCRIPT.index('\nfunction Copy-RuntimeSources', promotion_start)
        promotion = SCRIPT[promotion_start:promotion_end]
        catch_end = promotion.index('\n  }\n  if (Test-Path -LiteralPath $backup)', promotion.index('  } catch {'))
        cleanup = promotion[catch_end:]
        self.assertIn('InjectBackupCleanupFailure', cleanup)
        self.assertIn('Write-Warning', cleanup)
        self.assertLess(promotion.index('& $PostActivationValidation'), catch_end)
        self.assertNotIn('Move-Item -LiteralPath $backup -Destination $Destination -Force', cleanup)

    def test_no_development_fingerprint_fallback(self):
        self.assertNotIn('STASIS_BUILD_FINGERPRINT = "development"', SCRIPT)
        self.assertIn('clean source revision', SCRIPT)


if __name__ == "__main__":
    unittest.main()

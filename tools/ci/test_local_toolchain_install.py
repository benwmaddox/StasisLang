"""Source and focused behavior checks for the Windows local installer."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import uuid
import unittest
from unittest.mock import patch
from tools.compute_toolchain_fingerprint import fingerprint
from tools.generate_release_provenance import RUNTIME_DIRS, RUNTIME_FILES


ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = ROOT / "scripts" / "install_local_toolchain.ps1"
SCRIPT = SCRIPT_PATH.read_text(encoding="ascii")
SMOKE_WORKSPACE = ROOT / "samples" / "windows_launch_smoke"
SMOKE_CACHE = SMOKE_WORKSPACE / ".stasis_cache"


def cache_snapshot(path: Path | None = None) -> dict[str, str]:
    """Map every regular file below the smoke cache to its SHA-256 digest."""
    if path is None:
        path = SMOKE_CACHE
    if not path.exists():
        return {}
    snapshot: dict[str, str] = {}
    for current, directories, files in os.walk(path, followlinks=False):
        directories[:] = [name for name in directories if not (Path(current) / name).is_symlink()]
        for name in files:
            file_path = Path(current) / name
            relative = file_path.relative_to(path).as_posix()
            digest = hashlib.sha256(file_path.read_bytes()).hexdigest()
            snapshot[relative] = digest
    return dict(sorted(snapshot.items()))


def cache_state(path: Path | None = None) -> dict[str, object]:
    """Capture both cache presence and bytes so absent and empty caches differ."""
    if path is None:
        path = SMOKE_CACHE
    return {"exists": path.exists(), "files": cache_snapshot(path)}


def remove_owned_target_tree(path: Path, *, prefix: str) -> None:
    """Remove one new direct child of target after checking its resolved parent."""
    if not path.exists():
        return
    target = (ROOT / "target").resolve(strict=True)
    if path.is_symlink():
        raise RuntimeError(f"refusing to remove a symlink as an owned target tree: {path}")
    resolved = path.resolve(strict=True)
    if resolved.parent != target or not resolved.name.startswith(prefix):
        raise RuntimeError(f"refusing to remove a path outside the owned target root: {resolved}")
    shutil.rmtree(resolved)


def sanitized_toolchain_environment(*, test_mode: bool = False) -> dict[str, str]:
    """Keep signing configuration but discard ambient runtime pairing overrides."""
    environment = os.environ.copy()
    for key in tuple(environment):
        if key.upper().startswith("STASIS_RUNTIME_"):
            environment.pop(key, None)
    environment.pop("STASIS_TEST_MODE", None)
    if test_mode:
        environment["STASIS_TEST_MODE"] = "1"
    return environment


def write_process_log(path: Path, command: list[str], result: subprocess.CompletedProcess[str]) -> None:
    path.write_text(
        "command: " + subprocess.list2cmdline(command) + "\n"
        + f"exit code: {result.returncode}\n\n"
        + "--- stdout ---\n" + result.stdout
        + "\n--- stderr ---\n" + result.stderr,
        encoding="utf-8",
    )


def run_logged_process(
    command: list[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    log_path: Path,
    timeout_seconds: int = 900,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout_seconds,
        check=False,
    )
    write_process_log(log_path, command, result)
    return result


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def editor_info_identity(executable: Path, bin_root: Path, evidence_log: Path) -> dict[str, str]:
    result = run_logged_process(
        [str(executable), "--json", "editor-info"],
        cwd=bin_root,
        environment=sanitized_toolchain_environment(),
        log_path=evidence_log,
        timeout_seconds=120,
    )
    if result.returncode != 0:
        raise AssertionError(f"restored editor-info failed; see {evidence_log}")
    try:
        payload = json.loads(result.stdout.strip())
        result_payload = payload["result"]
        return {
            "build_fingerprint": str(result_payload["build_fingerprint"]),
            "graphics_runtime_build_fingerprint": str(
                result_payload["graphics_runtime"]["build_fingerprint"]
            ),
        }
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise AssertionError(f"editor-info returned an unexpected payload; see {evidence_log}") from error


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def run_real_windows_integration(evidence_directory: Path) -> None:
    """Exercise actual install success and transactional failure on a clean Windows candidate."""
    if platform.system() != "Windows":
        raise RuntimeError("the real installer integration is Windows-only; it is never silently skipped")
    evidence_directory = evidence_directory.expanduser().resolve(strict=False)
    if evidence_directory == ROOT.resolve() or ROOT.resolve() in evidence_directory.parents:
        raise ValueError("integration evidence must be outside the Stasis repository")
    evidence_directory.mkdir(parents=True, exist_ok=False)

    target = ROOT / "target"
    target.mkdir(parents=True, exist_ok=True)
    integration_root = target / f"local-it-{uuid.uuid4().hex[:8]}"
    if integration_root.exists():
        raise FileExistsError(integration_root)
    bin_root = integration_root / "bin"
    smoke_output = target / "local-editor-record-smoke"
    sample_cache_before = cache_state()
    write_json(evidence_directory / "sample-cache-before.json", sample_cache_before)
    smoke_roots_before = {
        path.name for path in target.glob("local-toolchain-smoke-*") if path.is_dir()
    }
    shell = shutil.which("powershell.exe") or shutil.which("powershell") or shutil.which("pwsh")
    if not shell:
        raise RuntimeError("PowerShell is required for the real Windows installer integration")

    try:
        success_command = [
            shell,
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(SCRIPT_PATH),
            "-BinRoot",
            str(bin_root),
        ]
        success = run_logged_process(
            success_command,
            cwd=ROOT,
            environment=sanitized_toolchain_environment(),
            log_path=evidence_directory / "install-success.log",
        )
        if success.returncode != 0:
            raise AssertionError(f"normal installer smoke failed; see {evidence_directory / 'install-success.log'}")

        pngs = sorted(smoke_output.glob("*.png")) if smoke_output.is_dir() else []
        if len(pngs) != 1:
            raise AssertionError(f"expected one real recorded PNG frame, found {len(pngs)}")
        png_bytes = pngs[0].read_bytes()
        if len(png_bytes) < 24 or png_bytes[:8] != b"\x89PNG\r\n\x1a\n":
            raise AssertionError("installer record output is not a PNG")
        width, height = struct.unpack(">II", png_bytes[16:24])
        if (width, height) != (320, 180):
            raise AssertionError(f"installer PNG dimensions were {width}x{height}, expected 320x180")
        (evidence_directory / "successful-record-frame.png").write_bytes(png_bytes)

        cache_after_success = cache_state()
        write_json(evidence_directory / "sample-cache-after-success.json", cache_after_success)
        if cache_after_success != sample_cache_before:
            raise AssertionError("successful install changed the canonical sample cache")

        executable = bin_root / "stasis.exe"
        if not executable.is_file():
            raise AssertionError("successful installer did not leave stasis.exe in BinRoot")
        before_directory_id = bin_root.stat().st_ino
        if before_directory_id <= 0:
            raise AssertionError("Windows filesystem did not provide a usable BinRoot file ID")
        before_executable_hash = file_sha256(executable)
        before_identity = editor_info_identity(
            executable, bin_root, evidence_directory / "editor-info-before-failure.log"
        )

        failure_command = [
            shell,
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(SCRIPT_PATH),
            "-BinRoot",
            str(bin_root),
            "-SkipBuild",
            "-TestInjectRecordFailure",
        ]
        failure = run_logged_process(
            failure_command,
            cwd=ROOT,
            environment=sanitized_toolchain_environment(test_mode=True),
            log_path=evidence_directory / "install-record-failure.log",
        )
        if failure.returncode == 0:
            raise AssertionError("actual CLI record --frames 0 unexpectedly succeeded")
        failure_output = (failure.stdout + "\n" + failure.stderr).lower()
        prepare_marker = "prepared the selected toolchain stdlib in .stasis_cache/toolchain/src"
        record_marker = "recording frame count must be between 1"
        prepare_at = failure_output.find(prepare_marker)
        record_at = failure_output.find(record_marker)
        if prepare_at < 0 or record_at < 0 or record_at <= prepare_at:
            raise AssertionError(
                "failure log did not show successful prepare followed by the actual record frames rejection; "
                f"see {evidence_directory / 'install-record-failure.log'}"
            )
        if smoke_output.exists():
            raise AssertionError("failed record left target/local-editor-record-smoke output behind")

        cache_after_failure = cache_state()
        write_json(evidence_directory / "sample-cache-after-failure.json", cache_after_failure)
        if cache_after_failure != sample_cache_before:
            raise AssertionError("failed record changed the canonical sample cache")

        after_directory_id = bin_root.stat().st_ino
        after_executable_hash = file_sha256(executable)
        if after_directory_id <= 0 or after_directory_id != before_directory_id:
            raise AssertionError("rollback did not restore the original NTFS BinRoot directory identity")
        if after_executable_hash != before_executable_hash:
            raise AssertionError("rollback did not restore the prior stasis.exe bytes")
        after_identity = editor_info_identity(
            executable, bin_root, evidence_directory / "editor-info-after-failure.log"
        )
        if after_identity != before_identity:
            raise AssertionError("restored editor-info identity changed after rollback")

        leftovers = list(bin_root.parent.glob(f"{bin_root.name}.staging-*"))
        leftovers += list(bin_root.parent.glob(f"{bin_root.name}.backup-*"))
        if leftovers:
            raise AssertionError(f"installer left staging or backup paths behind: {leftovers}")
        smoke_roots_after = {
            path.name for path in target.glob("local-toolchain-smoke-*") if path.is_dir()
        }
        if smoke_roots_after != smoke_roots_before:
            raise AssertionError("installer left a new disposable smoke workspace behind")

        write_json(
            evidence_directory / "integration-summary.json",
            {
                "sample_cache_was_present": sample_cache_before["exists"],
                "sample_cache_file_count": len(sample_cache_before["files"]),
                "sample_cache_unchanged_after_success": cache_after_success == sample_cache_before,
                "sample_cache_unchanged_after_failure": cache_after_failure == sample_cache_before,
                "png": {"path": "successful-record-frame.png", "width": width, "height": height},
                "failure_log_confirms_prepare_before_record_rejection": True,
                "bin_directory_file_id_before": before_directory_id,
                "bin_directory_file_id_after": after_directory_id,
                "stasis_exe_sha256_before": before_executable_hash,
                "stasis_exe_sha256_after": after_executable_hash,
                "editor_info_identity_before": before_identity,
                "editor_info_identity_after": after_identity,
                "record_failure_return_code": failure.returncode,
                "remaining_record_smoke_output_paths": 0,
                "remaining_owned_staging_or_backup_paths": 0,
            },
        )
    finally:
        remove_owned_target_tree(integration_root, prefix="local-it-")


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
            'Copy-TrackedTree -SourceRoot $repoRoot -Destination $staging -RelativeRoot "mobile"',
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

    def test_mobile_staging_copies_only_versioned_inputs(self):
        self.assertIn("function Copy-TrackedTree", SCRIPT)
        self.assertIn('"ls-files", "--", $normalizedRoot', SCRIPT)
        self.assertIn(
            'Copy-TrackedTree -SourceRoot $repoRoot -Destination $staging '
            '-RelativeRoot "mobile"',
            SCRIPT,
        )
        self.assertNotIn(
            'Copy-Item -LiteralPath (Join-Path $repoRoot "mobile") '
            '-Destination (Join-Path $staging "mobile") -Recurse',
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

    def test_smoke_prepares_activated_toolchain_snapshot_before_record(self):
        validation = SCRIPT[SCRIPT.index('$postActivationValidation = {'):]
        identity_check = validation.index('$editorInfo.result.build_fingerprint')
        prepare = validation.index('"prepare"')
        record = validation.index('"record"')
        self.assertLess(identity_check, prepare)
        self.assertLess(prepare, record)
        preparation = validation[identity_check:record]
        self.assertIn('Invoke-Bounded -FilePath $installedExecutable', preparation)
        self.assertIn('-WorkingDirectory $binRoot', preparation)

    def test_smoke_uses_a_disposable_workspace(self):
        validation = SCRIPT[SCRIPT.index('$postActivationValidation = {'):]
        self.assertNotIn(
            'Join-Path $repoRoot "samples/windows_launch_smoke"', validation,
            "the smoke still prepares the canonical sample and can mutate its cache",
        )
        self.assertIn("Invoke-SmokeInDisposableWorkspace", validation)
        helper_start = SCRIPT.index("function Invoke-SmokeInDisposableWorkspace")
        helper_end = SCRIPT.index("\nfunction ", helper_start + 1)
        helper = SCRIPT[helper_start:helper_end]
        self.assertIn('Copy-TrackedTree -SourceRoot $repoRoot', helper)
        self.assertIn('finally {', helper)
        self.assertIn('Remove-OwnedSmokeWorkspace', helper)

    def test_record_failure_hook_reaches_real_cli_after_prepare(self):
        validation = SCRIPT[SCRIPT.index('$postActivationValidation = {'):]
        prepare = validation.index('"prepare"')
        record = validation.index('"record"')
        frames = validation.index('"--frames", $recordFrames')
        record_return = validation.index(') -WorkingDirectory $binRoot | Out-Null', record)
        unexpected_success = validation.index('throw "actual record --frames 0 unexpectedly succeeded"', record_return)
        png_assertion = validation.index('throw "bounded record smoke did not produce a PNG frame"', unexpected_success)
        self.assertLess(prepare, record)
        self.assertLess(record, frames)
        self.assertLess(record_return, unexpected_success)
        self.assertLess(unexpected_success, png_assertion)
        self.assertIn('if ($TestInjectRecordFailure -and $env:STASIS_TEST_MODE -eq "1") { $recordFrames = "0" }', validation)
        self.assertIn('if ($TestInjectRecordFailure -and $env:STASIS_TEST_MODE -ne "1")', SCRIPT)

    def test_smoke_workspace_success_and_failure_cleanup(self):
        self._run_smoke_workspace_behavior(cache_path=SMOKE_CACHE)

    def test_smoke_workspace_accepts_an_absent_original_cache(self):
        target = ROOT / "target"
        target.mkdir(parents=True, exist_ok=True)
        absent_root = target / f"local-toolchain-absent-cache-{uuid.uuid4().hex}"
        absent_root.mkdir()
        absent_cache = absent_root / ".stasis_cache"
        try:
            self.assertEqual(cache_state(absent_cache), {"exists": False, "files": {}})
            self._run_smoke_workspace_behavior(cache_path=absent_cache)
            self.assertFalse(absent_cache.exists(), "the smoke workspace test created the absent original cache")
        finally:
            remove_owned_target_tree(absent_root, prefix="local-toolchain-absent-cache-")

    def _run_smoke_workspace_behavior(self, *, cache_path: Path):
        shell = shutil.which("pwsh") or shutil.which("powershell") or shutil.which("powershell.exe")
        self.assertIsNotNone(shell, "the fast smoke workspace regression requires PowerShell Core or Windows PowerShell")
        target = ROOT / "target"
        target.mkdir(parents=True, exist_ok=True)
        owned_root = target / f"local-toolchain-smoke-test-{uuid.uuid4().hex}"
        failed_root = Path(f"{owned_root}-failure")
        self.assertFalse(owned_root.exists())
        self.assertFalse(failed_root.exists())
        before_cache = cache_state(cache_path)
        environment = sanitized_toolchain_environment(test_mode=True)
        command = [
            shell,
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(SCRIPT_PATH),
            "-TestSmokeWorkspaceOnly",
            "-TestSmokeWorkspaceRoot",
            str(owned_root),
        ]
        try:
            result = subprocess.run(
                command,
                cwd=ROOT,
                env=environment,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=120,
                check=False,
            )
            self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
            self.assertIn("Disposable smoke workspace success and failure cleanup tests passed", result.stdout)
            self.assertFalse(owned_root.exists(), "the PowerShell success finally block left its owned smoke root")
            self.assertFalse(failed_root.exists(), "the PowerShell failure finally block left its owned smoke root")
            self.assertEqual(cache_state(cache_path), before_cache, "the original sample cache changed")
        finally:
            remove_owned_target_tree(owned_root, prefix="local-toolchain-smoke-test-")
            remove_owned_target_tree(failed_root, prefix="local-toolchain-smoke-test-")

    def test_integration_environment_clears_runtime_overrides_and_keeps_signing(self):
        ambient = {
            "STASIS_RUNTIME_LIBRARY_PATH": "ambient-library.dll",
            "STASIS_RUNTIME_DLL_PATH": "ambient-runtime.dll",
            "STASIS_RUNTIME_RUNNER_PATH": "ambient-runner.exe",
            "STASIS_RUNTIME_IMPORT_LIBRARY_PATH": "ambient-import-library.dll",
            "STASIS_RUNTIME_FUTURE_OVERRIDE": "ambient-future-runtime-value",
            "stasis_runtime_lowercase_future": "ambient-lowercase-runtime-value",
            "STASIS_REQUIRE_SIGNED_EXECUTION": "1",
            "STASIS_AOT_SIGN_TOOL": "repository-signer.cmd",
            "STASIS_TEST_MODE": "ambient-test-mode",
        }
        with patch.dict(os.environ, ambient, clear=True):
            environment = sanitized_toolchain_environment()
            test_environment = sanitized_toolchain_environment(test_mode=True)
        self.assertFalse(
            any(key.upper().startswith("STASIS_RUNTIME_") for key in environment),
            "all ambient runtime pairing overrides must be removed",
        )
        self.assertNotIn("STASIS_TEST_MODE", environment)
        self.assertEqual(environment["STASIS_REQUIRE_SIGNED_EXECUTION"], "1")
        self.assertEqual(environment["STASIS_AOT_SIGN_TOOL"], "repository-signer.cmd")
        self.assertEqual(test_environment["STASIS_TEST_MODE"], "1")

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
    if len(sys.argv) > 1 and sys.argv[1] == "--real-install-integration":
        parser = argparse.ArgumentParser(description="Run the real Windows local toolchain installer integration")
        parser.add_argument("--real-install-integration", action="store_true", required=True)
        parser.add_argument("--evidence-dir", type=Path, required=True)
        arguments = parser.parse_args()
        run_real_windows_integration(arguments.evidence_dir)
        print(f"Real Windows installer integration passed; evidence: {arguments.evidence_dir}")
    else:
        unittest.main()

import hashlib
import json
import subprocess
import os
import shutil
import tempfile
import unittest
import zipfile
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

from tools import android_release


class AndroidReleaseBoundaryTest(unittest.TestCase):
    def test_keytool_certificate_chain_selects_leaf_not_issuer(self):
        output = f"Signer #1:\nCertificate #1:\n SHA256: {'12' * 32}\nCertificate #2:\n SHA256: {'34' * 32}\n"
        self.assertEqual(android_release.certificate_digest(output, certificate_chain=True), "12" * 32)
        with self.assertRaisesRegex(android_release.ReleaseError, "multiple signer"):
            android_release.certificate_digest(output + f"Signer #2:\n SHA256: {'56' * 32}\n", certificate_chain=True)

    def test_receipt_publication_failure_removes_artifact_and_sidecars(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest = root / "package.json"
            manifest.write_text(json.dumps({
                "development_build": True,
                "android_version_code": "1",
                "android_version_name": "1.0",
                "package_id": "com.example.game",
                "target": "android-arm64",
                "provenance": "provenance.json",
            }))
            provenance = root / "provenance.json"
            provenance.write_text("{}")
            mapping = root / "mapping.txt"
            mapping.write_text("mapping")
            output = root / "artifacts/stasis-debug.apk"
            receipt = root / "artifacts/stasis-debug.receipt.json"
            args = android_release.parser().parse_args([
                "finalize", "--source", str(source), "--output", str(output),
                "--receipt", str(receipt), "--package-manifest", str(manifest),
                "--provenance", str(provenance), "--variant", "debug",
                "--format", "apk", "--abi", "arm64-v8a", "--sidecar", f"mapping.txt={mapping}",
            ])
            replace = os.replace

            def fail_receipt(source_path, destination):
                if Path(destination) == receipt:
                    raise OSError("injected receipt publication failure")
                replace(source_path, destination)

            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32), patch(
                "tools.android_release.os.replace", side_effect=fail_receipt
            ):
                with self.assertRaisesRegex(OSError, "injected receipt"):
                    android_release.finalize(args)
            self.assertFalse(output.exists())
            self.assertFalse(receipt.exists())
            self.assertFalse((output.parent / "mapping.txt").exists())

    def test_aab_accepts_self_signed_chain_but_rejects_invalid_signature(self):
        digest = "12" * 32
        certificate = subprocess.CompletedProcess(["keytool"], 0, f"SHA256: {digest}", "")
        verified = subprocess.CompletedProcess(
            ["jarsigner"], 4,
            "jar verified, with signer errors.\nThis jar contains entries whose certificate chain is invalid.\n",
            "",
        )
        with patch("tools.android_release.subprocess.run", return_value=verified), patch(
            "tools.android_release.run_tool", return_value=certificate
        ):
            self.assertEqual(android_release.verify_aab_signer(Path("game.aab"), "jarsigner", "keytool"), digest)
        invalid = subprocess.CompletedProcess(["jarsigner"], 1, "", "SHA-256 digest error for base/classes.dex")
        with patch("tools.android_release.subprocess.run", return_value=invalid):
            with self.assertRaisesRegex(android_release.ReleaseError, "jarsigner"):
                android_release.verify_aab_signer(Path("game.aab"), "jarsigner", "keytool")

    def test_legacy_signing_names_remain_compatible(self):
        with tempfile.TemporaryDirectory() as directory:
            keystore = Path(directory) / "release.jks"
            keystore.write_bytes(b"fixture")
            inputs = android_release.resolve_signing_inputs(
                {
                    "ANDROID_KEYSTORE_PATH": str(keystore),
                    "ANDROID_SIGNING_KEY_ALIAS": "release",
                    "ANDROID_KEYSTORE_PASSWORD": "store secret",
                    "ANDROID_KEY_PASSWORD": "key secret",
                }
            )
            self.assertEqual(inputs.keystore, keystore.resolve())
            self.assertEqual(inputs.key_alias, "release")

    def test_missing_signing_inputs_name_only_canonical_variables(self):
        with self.assertRaisesRegex(
            android_release.ReleaseError,
            "STASIS_ANDROID_KEYSTORE.*STASIS_ANDROID_KEY_PASSWORD",
        ):
            android_release.resolve_signing_inputs({})

    def test_unsigned_release_is_explicit_and_cannot_install(self):
        arguments = Namespace(
            development_build=False,
            unsigned_release=True,
            install=False,
            keystore="",
            key_alias="",
            expected_signer_sha256="",
            keytool="",
            jarsigner="",
            forbidden_root=[],
        )
        self.assertEqual(android_release.preflight(arguments)["handoff"], "unsigned")
        arguments.install = True
        with self.assertRaisesRegex(android_release.ReleaseError, "cannot be installed"):
            android_release.preflight(arguments)

    def test_release_finalize_rejects_emulator_abi_before_artifact_work(self):
        arguments = Namespace(
            unsigned_release=True, install=False, variant="release", abi="x86_64"
        )
        with self.assertRaisesRegex(android_release.ReleaseError, "arm64-v8a"):
            android_release.finalize(arguments)

    def test_tool_errors_redact_both_passwords(self):
        signing = android_release.SigningInputs(
            Path("private-location.jks"), "private-alias", "store secret", "key secret"
        )
        failure = subprocess.CompletedProcess(
            ["signer"],
            1,
            "",
            "bad store secret key secret private-alias private-location.jks",
        )
        with patch("tools.android_release.subprocess.run", return_value=failure):
            with self.assertRaises(android_release.ReleaseError) as raised:
                android_release.run_tool(["signer"], signing)
        self.assertNotIn("store secret", str(raised.exception))
        self.assertNotIn("key secret", str(raised.exception))
        self.assertNotIn("private-location.jks", str(raised.exception))
        self.assertNotIn("private-alias", str(raised.exception))
        self.assertIn("<redacted>", str(raised.exception))

    def test_private_key_preflight_uses_environment_password_references(self):
        signing = android_release.SigningInputs(
            Path("release.jks"), "release", "store secret", "key secret"
        )
        with patch("tools.android_release.run_tool") as run:
            android_release.verify_private_key(signing, "jarsigner")
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("-sigfile") + 1], "STASIS")
        self.assertIn("-storepass:env", command)
        self.assertIn("STASIS_ANDROID_STORE_PASSWORD", command)
        self.assertIn("-keypass:env", command)
        self.assertIn("STASIS_ANDROID_KEY_PASSWORD", command)
        self.assertNotIn("store secret", command)
        self.assertNotIn("key secret", command)

    def test_unsigned_apk_rejects_an_unrelated_verifier_failure(self):
        failure = subprocess.CompletedProcess(
            ["apksigner"], 1, "", "ERROR: malformed ZIP directory"
        )
        with patch("tools.android_release.subprocess.run", return_value=failure):
            with self.assertRaisesRegex(android_release.ReleaseError, "could not be proven"):
                android_release.ensure_unsigned(Path("artifact.apk"), "apk", "apksigner")

    def test_sidecars_reject_duplicates_and_artifact_name_collisions(self):
        with tempfile.TemporaryDirectory() as directory:
            first = Path(directory) / "a"
            second = Path(directory) / "b"
            first.write_text("a", encoding="utf-8")
            second.write_text("b", encoding="utf-8")
            with self.assertRaisesRegex(android_release.ReleaseError, "duplicate"):
                android_release.parse_sidecars(
                    [f"mapping.txt={first}", f"mapping.txt={second}"],
                    {"stasis-release.apk"},
                )
        with self.assertRaisesRegex(android_release.ReleaseError, "sidecar name"):
            android_release.parse_sidecars(
                ["stasis-release.apk=secret"], {"stasis-release.apk"}
            )
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(android_release.ReleaseError, "source was not found"):
                android_release.parse_sidecars(
                    [f"mapping.txt={Path(directory) / 'missing.txt'}"], set()
                )

    def test_certificate_digest_accepts_colon_fingerprint(self):
        digest = "12" * 32
        rendered = ":".join(digest[index : index + 2] for index in range(0, 64, 2))
        self.assertEqual(
            android_release.certificate_digest(f"SHA256: {rendered}"),
            digest,
        )

    def test_apksigner_digest_ignores_public_key_digest(self):
        certificate = "11" * 32
        public_key = "22" * 32
        output = (
            f"Signer #1 certificate SHA-256 digest: {certificate}\n"
            f"Signer #1 public key SHA-256 digest: {public_key}\n"
        )
        self.assertEqual(android_release.certificate_digest(output), certificate)

    def test_digest_rejects_non_separator_characters_and_multiple_signers(self):
        with self.assertRaisesRegex(android_release.ReleaseError, "only hexadecimal"):
            android_release.normalize_digest("AA-" + "11" * 31)
        with self.assertRaisesRegex(android_release.ReleaseError, "multiple signer"):
            android_release.certificate_digest(
                f"SHA256: {'11' * 32}\nSHA256: {'22' * 32}"
            )

    def test_debug_finalize_emits_atomic_receipt_and_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "app-debug.apk"
            package_manifest = root / "stasis_mobile_package.json"
            package_manifest.write_text(
                json.dumps({
                    "development_build": True,
                    "android_version_code": 7,
                    "android_version_name": "1.2.3",
                    "package_id": "com.example.game",
                    "target": "android-arm64",
                    "provenance": "stasis_provenance.json",
                }),
                encoding="utf-8",
            )
            provenance = root / "stasis_provenance.json"
            provenance.write_text('{"identity":"fixture"}\n', encoding="utf-8")
            self.write_package(source, provenance.read_bytes())
            sidecar = root / "symbols.zip"
            sidecar.write_bytes(b"symbols")
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            arguments = Namespace(
                source=str(source),
                output=str(output),
                receipt=str(receipt),
                package_manifest=str(package_manifest),
                provenance=str(provenance),
                format="apk",
                variant="debug",
                abi="arm64-v8a",
                required_asset="assets/ball.svg",
                unsigned_release=False,
                install=False,
                serial="",
                keystore="",
                key_alias="",
                expected_signer_sha256="",
                apksigner="",
                aapt="",
                adb="",
                jarsigner="",
                keytool="",
                bundletool="",
                java="",
                sidecar=[f"native-debug-symbols.zip={sidecar}"],
                forbidden_root=[],
            )
            signer = "AB" * 32
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "7", "1.2.3", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value=signer):
                result = android_release.finalize(arguments)
            self.assertTrue(output.is_file())
            self.assertTrue((output.parent / "native-debug-symbols.zip").is_file())
            saved = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertEqual(saved["variant"], "debug")
            self.assertEqual(saved["abi"], "arm64-v8a")
            self.assertTrue(saved["debuggable"])
            self.assertEqual(saved["version"], {"code": "7", "name": "1.2.3"})
            self.assertEqual(saved["signer_sha256"], signer)
            self.assertEqual(result["artifact_path"], str(output.resolve()))
            self.assertEqual(saved["artifact_sha256"], hashlib.sha256(output.read_bytes()).hexdigest())
            self.assertEqual(saved["signing_mode"], "debug")
            self.assertEqual(saved["install_status"], "not-requested")
            self.assertEqual(saved["package"], "com.example.game")
            self.assertEqual(
                saved["provenance_identity"],
                hashlib.sha256(provenance.read_bytes()).hexdigest(),
            )

    def test_publication_failure_restores_previous_artifact_receipt_and_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest = root / "package.json"
            manifest.write_text(json.dumps({
                "development_build": True,
                "android_version_code": "1",
                "android_version_name": "1.0",
                "package_id": "com.example.game",
                "target": "android-arm64",
                "provenance": "provenance.json",
            }))
            provenance = root / "provenance.json"
            provenance.write_text("{}", encoding="utf-8")
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            sidecar_source = root / "mapping.new.txt"
            sidecar_source.write_text("new mapping", encoding="utf-8")
            output.parent.mkdir()
            output.write_bytes(b"last good artifact")
            receipt.write_text('{"status":"last-good"}', encoding="utf-8")
            (output.parent / "mapping.txt").write_text("last good mapping", encoding="utf-8")
            args = self.finalize_args(
                source, manifest, provenance, output, receipt,
                sidecar=[f"mapping.txt={sidecar_source}"],
            )
            original_replace = os.replace

            def fail_receipt(source_path, destination):
                if Path(destination) == receipt:
                    raise OSError("injected receipt publication failure")
                original_replace(source_path, destination)

            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32), patch(
                "tools.android_release.os.replace", side_effect=fail_receipt
            ):
                with self.assertRaisesRegex(OSError, "injected receipt"):
                    android_release.finalize(args)
            self.assertEqual(output.read_bytes(), b"last good artifact")
            self.assertEqual(receipt.read_text(encoding="utf-8"), '{"status":"last-good"}')
            self.assertEqual(
                (output.parent / "mapping.txt").read_text(encoding="utf-8"),
                "last good mapping",
            )

    def test_incomplete_rollback_keeps_recovery_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            staged = root / "staged.apk"
            destination = root / "artifact.apk"
            staged.write_bytes(b"new")
            destination.write_bytes(b"old")
            original_copy2 = shutil.copy2

            def fail_restore(source, target, *args, **kwargs):
                if Path(source).name == "0.bak":
                    raise OSError("injected rollback failure")
                return original_copy2(source, target, *args, **kwargs)

            with patch("tools.android_release.os.replace", side_effect=OSError("injected publish failure")), patch(
                "tools.android_release.shutil.copy2", side_effect=fail_restore
            ):
                with self.assertRaisesRegex(android_release.ReleaseError, "recovery directory") as raised:
                    android_release._publish_staged([(staged, destination)], root)
            self.assertEqual(destination.read_bytes(), b"old")
            recovery_text = str(raised.exception).split("recovery directory: ", 1)[1]
            recovery = Path(recovery_text.split(";", 1)[0])
            self.assertTrue((recovery / "0.bak").is_file())
            shutil.rmtree(recovery, ignore_errors=True)

    def test_install_runs_after_publication_and_updates_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(root)
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(
                source, manifest, provenance, output, receipt, install=True
            )
            commands = []

            def install(command, signing=None):
                commands.append(list(command))
                self.assertTrue(output.is_file())
                pending = json.loads(receipt.read_text(encoding="utf-8"))
                self.assertEqual(pending["install_status"], "pending")
                return subprocess.CompletedProcess(command, 0, "", "")

            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32), patch(
                "tools.android_release.run_tool", side_effect=install
            ):
                result = android_release.finalize(args)
            self.assertEqual(commands[0][-2:], ["-r", str(output)])
            self.assertTrue(result["installed"])
            self.assertEqual(result["install_status"], "passed")
            self.assertEqual(json.loads(receipt.read_text())["install_status"], "passed")

    def test_install_failure_is_recorded_after_publication(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(root)
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(
                source, manifest, provenance, output, receipt, install=True
            )

            def fail_install(command, signing=None):
                self.assertTrue(output.is_file())
                self.assertEqual(
                    json.loads(receipt.read_text(encoding="utf-8"))["install_status"],
                    "pending",
                )
                raise android_release.ReleaseError("adb failed with private secret")

            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32), patch(
                "tools.android_release.run_tool", side_effect=fail_install
            ):
                with self.assertRaisesRegex(android_release.ReleaseError, "installation failed"):
                    android_release.finalize(args)
            saved = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertFalse(saved["installed"])
            self.assertEqual(saved["install_status"], "failed")
            self.assertIn("adb failed", saved["install_error"])

    def test_missing_adb_is_recorded_as_install_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(root)
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(
                source, manifest, provenance, output, receipt, install=True
            )

            def find_tool(name, explicit=""):
                if name == "adb":
                    raise android_release.ReleaseError("adb was not found")
                return "tool"

            with patch("tools.android_release.find_sdk_tool", side_effect=find_tool), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32):
                with self.assertRaisesRegex(android_release.ReleaseError, "installation failed"):
                    android_release.finalize(args)
            saved = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertEqual(saved["install_status"], "failed")
            self.assertIn("adb was not found", saved["install_error"])

    def test_finalize_rejects_stale_embedded_provenance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source, b"stale")
            manifest, provenance = self.write_manifest(root, provenance_bytes=b"fresh")
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(source, manifest, provenance, output, receipt)
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32):
                with self.assertRaisesRegex(android_release.ReleaseError, "embedded provenance"):
                    android_release.finalize(args)
            self.assertFalse(output.exists())

    def test_release_missing_compiled_launcher_aborts_before_publish(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(root, development_build=False)
            output = root / "artifacts" / "stasis-release.apk"
            receipt = root / "artifacts" / "stasis-release.receipt.json"
            args = self.finalize_args(
                source, manifest, provenance, output, receipt,
                variant="release", unsigned_release=True,
            )
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(False, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.ensure_unsigned"), patch(
                "tools.android_release.verify_compiled_launcher",
                side_effect=ValueError("compiled Android application is missing android:icon"),
            ):
                with self.assertRaisesRegex(ValueError, "missing android:icon"):
                    android_release.finalize(args)
            self.assertFalse(output.exists())
            self.assertFalse(receipt.exists())

    def test_unsigned_release_finalize_is_explicit_handoff(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(
                root, development_build=False
            )
            output = root / "artifacts" / "stasis-release.apk"
            receipt = root / "artifacts" / "stasis-release.receipt.json"
            args = self.finalize_args(
                source,
                manifest,
                provenance,
                output,
                receipt,
                variant="release",
                unsigned_release=True,
            )
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(False, "1", "1.0", "com.example.game"),
            ), patch("tools.android_release.ensure_unsigned") as unsigned, patch(
                "tools.android_release.verify_compiled_launcher"
            ) as launcher:
                result = android_release.finalize(args)
            launcher.assert_called_once()
            unsigned.assert_called_once()
            self.assertEqual(result["signing_mode"], "unsigned-release")
            self.assertIsNone(result["signer_sha256"])
            self.assertFalse(result["installed"])

    def test_finalize_rejects_artifact_package_identity_mismatch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest, provenance = self.write_manifest(root)
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(source, manifest, provenance, output, receipt)
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release._inspect_apk_details",
                return_value=(True, "1", "1.0", "com.other.game"),
            ), patch("tools.android_release.verify_apk_signer", return_value="12" * 32):
                with self.assertRaisesRegex(android_release.ReleaseError, "package ID"):
                    android_release.finalize(args)
            self.assertFalse(output.exists())

    def test_finalize_rejects_malformed_package_manifest_before_publication(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            self.write_package(source)
            manifest = root / "stasis_mobile_package.json"
            manifest.write_text("{ malformed", encoding="utf-8")
            provenance = root / "stasis_provenance.json"
            provenance.write_bytes(b"{}")
            output = root / "artifacts" / "stasis-debug.apk"
            receipt = root / "artifacts" / "stasis-debug.receipt.json"
            args = self.finalize_args(source, manifest, provenance, output, receipt)
            with self.assertRaisesRegex(android_release.ReleaseError, "could not be read"):
                android_release.finalize(args)
            self.assertFalse(output.exists())

    def test_finalize_rejects_artifact_and_receipt_path_collision(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source.apk"
            source.write_bytes(b"source")
            same = root / "artifact.apk"
            with self.assertRaisesRegex(android_release.ReleaseError, "paths must differ"):
                android_release.finalize(Namespace(
                    source=str(source),
                    output=str(same),
                    receipt=str(same),
                    variant="debug",
                    abi="arm64-v8a",
                    unsigned_release=False,
                    install=False,
                ))

    def test_verify_rejects_receipt_artifact_digest_drift(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            apk = root / "stasis-release.apk"
            self.write_package(apk)
            signer = "AB" * 32
            (root / "stasis-release.receipt.json").write_text(
                json.dumps({
                    "variant": "release",
                    "signing_mode": "signed-release",
                    "abi": "arm64-v8a",
                    "artifact_sha256": "00" * 32,
                    "signer_sha256": signer,
                    "version": {"code": "7", "name": "1.2.3"},
                }),
                encoding="utf-8",
            )
            arguments = Namespace(
                apk=str(apk), required_asset="assets/ball.svg",
                expected_signer_sha256="", aapt="", apksigner="",
            )
            badging = subprocess.CompletedProcess(
                ["aapt"], 0, "package: name='com.example.game' versionCode='7' versionName='1.2.3'\n", ""
            )
            with patch("tools.android_release.find_sdk_tool", return_value="tool"), patch(
                "tools.android_release.run_tool", return_value=badging
            ), patch(
                "tools.android_release.inspect_apk", return_value=(False, "7", "1.2.3")
            ), patch("tools.android_release.verify_apk_signer", return_value=signer), patch(
                "tools.android_release.verify_compiled_launcher"
            ):
                with self.assertRaisesRegex(android_release.ReleaseError, "artifact digest"):
                    android_release.verify(arguments)

    @staticmethod
    def finalize_args(
        source: Path,
        manifest: Path,
        provenance: Path,
        output: Path,
        receipt: Path,
        *,
        install: bool = False,
        sidecar: list[str] | None = None,
        variant: str = "debug",
        unsigned_release: bool = False,
    ) -> Namespace:
        return Namespace(
            source=str(source),
            output=str(output),
            receipt=str(receipt),
            package_manifest=str(manifest),
            provenance=str(provenance),
            format="apk",
            variant=variant,
            abi="arm64-v8a",
            required_asset="assets/ball.svg",
            unsigned_release=unsigned_release,
            install=install,
            serial="",
            keystore="",
            key_alias="",
            package_id="",
            expected_signer_sha256="",
            apksigner="",
            aapt="",
            adb="",
            jarsigner="",
            keytool="",
            bundletool="",
            java="",
            sidecar=sidecar or [],
            forbidden_root=[],
        )

    @staticmethod
    def write_manifest(
        root: Path,
        provenance_bytes: bytes = b"{}",
        *,
        development_build: bool = True,
        target: str = "android-arm64",
    ) -> tuple[Path, Path]:
        manifest = root / "stasis_mobile_package.json"
        provenance = root / "stasis_provenance.json"
        provenance.write_bytes(provenance_bytes)
        manifest.write_text(json.dumps({
            "development_build": development_build,
            "android_version_code": "1",
            "android_version_name": "1.0",
            "package_id": "com.example.game",
            "target": target,
            "provenance": provenance.name,
        }), encoding="utf-8")
        return manifest, provenance

    @staticmethod
    def write_package(path: Path, embedded_provenance: bytes = b"{}") -> None:
        asset = b"asset"
        manifest = {
            "schema": "stasis-assets",
            "version": 1,
            "assets": [
                {
                    "id": "ball",
                    "path": "assets/ball.svg",
                    "content_sha256": hashlib.sha256(asset).hexdigest(),
                }
            ],
        }
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("AndroidManifest.xml", b"manifest")
            archive.writestr(
                "assets/stasis_game/assets/manifest.json", json.dumps(manifest)
            )
            archive.writestr("assets/stasis_game/assets/ball.svg", asset)
            archive.writestr(
                "assets/stasis_game/stasis_provenance.json", embedded_provenance
            )
            archive.writestr("lib/arm64-v8a/libmain.so", b"native")


if __name__ == "__main__":
    unittest.main()

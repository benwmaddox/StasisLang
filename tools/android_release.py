#!/usr/bin/env python3
"""Sign, audit, and publish one deterministic Stasis Android artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping, Sequence

if __package__ in {None, ""}:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.ci.check_android_release_package import validate as validate_contents
from tools.ci.check_android_launcher_icon import verify_compiled_launcher

REPOSITORY_ROOT = Path(__file__).resolve().parent.parent


class ReleaseError(ValueError):
    pass


SECRET_NAMES = {
    "store_password": ("STASIS_ANDROID_STORE_PASSWORD", "ANDROID_STORE_PASSWORD", "ANDROID_KEYSTORE_PASSWORD"),
    "key_password": ("STASIS_ANDROID_KEY_PASSWORD", "ANDROID_KEY_PASSWORD"),
}
VALUE_NAMES = {
    "keystore": ("STASIS_ANDROID_KEYSTORE", "ANDROID_KEYSTORE", "ANDROID_KEYSTORE_PATH"),
    "key_alias": ("STASIS_ANDROID_KEY_ALIAS", "ANDROID_KEY_ALIAS", "ANDROID_SIGNING_KEY_ALIAS"),
}


@dataclass(frozen=True)
class SigningInputs:
    keystore: Path
    key_alias: str
    store_password: str
    key_password: str

    def child_environment(self, source: Mapping[str, str]) -> dict[str, str]:
        result = dict(source)
        result["STASIS_ANDROID_STORE_PASSWORD"] = self.store_password
        result["STASIS_ANDROID_KEY_PASSWORD"] = self.key_password
        return result


def _first(environment: Mapping[str, str], names: Sequence[str]) -> str:
    return next((str(environment[name]) for name in names if environment.get(name)), "")


def resolve_signing_inputs(
    environment: Mapping[str, str],
    keystore: str = "",
    key_alias: str = "",
    forbidden_roots: Sequence[str] = (),
) -> SigningInputs:
    values = {
        "keystore": keystore or _first(environment, VALUE_NAMES["keystore"]),
        "key_alias": key_alias or _first(environment, VALUE_NAMES["key_alias"]),
        "store_password": _first(environment, SECRET_NAMES["store_password"]),
        "key_password": _first(environment, SECRET_NAMES["key_password"]),
    }
    missing = [
        canonical
        for field, canonical in (
            ("keystore", "STASIS_ANDROID_KEYSTORE"),
            ("key_alias", "STASIS_ANDROID_KEY_ALIAS"),
            ("store_password", "STASIS_ANDROID_STORE_PASSWORD"),
            ("key_password", "STASIS_ANDROID_KEY_PASSWORD"),
        )
        if not values[field]
    ]
    if missing:
        raise ReleaseError(
            "signed Android release requires complete signing inputs; missing "
            + ", ".join(missing)
        )
    resolved_keystore = Path(values["keystore"]).expanduser().resolve()
    if not resolved_keystore.is_file():
        raise ReleaseError(f"Android signing keystore was not found: {resolved_keystore}")
    for root in forbidden_roots:
        resolved_root = Path(root).resolve()
        if resolved_keystore.is_relative_to(resolved_root):
            raise ReleaseError(
                "Android signing keystore must remain outside the repository, project, "
                f"and generated output: {resolved_root}"
            )
    return SigningInputs(
        resolved_keystore,
        values["key_alias"],
        values["store_password"],
        values["key_password"],
    )


def normalize_digest(value: str) -> str:
    if not re.fullmatch(r"[0-9A-Fa-f: ]+", value):
        raise ReleaseError("expected signer SHA-256 may contain only hexadecimal digits and separators")
    normalized = value.replace(":", "").replace(" ", "").upper()
    if len(normalized) != 64:
        raise ReleaseError("expected signer SHA-256 must contain exactly 64 hexadecimal digits")
    return normalized


def _redact(text: str, signing: SigningInputs | None) -> str:
    if signing is None:
        return text
    for secret in (
        signing.store_password,
        signing.key_password,
        signing.key_alias,
        str(signing.keystore),
    ):
        if secret:
            text = text.replace(secret, "<redacted>")
    return text


def run_tool(
    command: Sequence[str], signing: SigningInputs | None = None
) -> subprocess.CompletedProcess[str]:
    environment = signing.child_environment(os.environ) if signing else None
    try:
        result = subprocess.run(
            list(command), capture_output=True, text=True, env=environment,
            check=False, timeout=900,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ReleaseError(
            f"Android release tool could not complete ({Path(command[0]).name}): "
            + _redact(str(error), signing)
        ) from error
    if result.returncode:
        detail = _redact((result.stderr or result.stdout).strip(), signing)
        if len(detail) > 2000:
            detail = detail[-2000:]
        raise ReleaseError(f"Android release tool failed ({Path(command[0]).name}): {detail}")
    return result


def find_sdk_tool(name: str, explicit: str = "") -> str:
    if explicit:
        path = Path(explicit).resolve()
        if path.is_file():
            return str(path)
        raise ReleaseError(f"Android tool was not found: {path}")
    found = shutil.which(name) or shutil.which(f"{name}.bat") or shutil.which(f"{name}.exe")
    if found:
        return found
    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if sdk:
        candidates = sorted(
            (Path(sdk) / "build-tools").glob(f"*/{name}*"), reverse=True
        )
        executable = next(
            (path for path in candidates if path.name in {name, f"{name}.bat", f"{name}.exe"}),
            None,
        )
        if executable:
            return str(executable)
        command_line = Path(sdk) / "cmdline-tools" / "latest" / "bin"
        for candidate in (command_line / name, command_line / f"{name}.bat", command_line / f"{name}.exe"):
            if candidate.is_file():
                return str(candidate)
    raise ReleaseError(f"{name} was not found; pass its explicit path or configure the Android SDK")


def certificate_digest(output: str, *, certificate_chain: bool = False) -> str:
    matches = re.findall(
        r"(?:certificate SHA[- ]?256 digest|^\s*SHA256):\s*"
        r"([0-9A-Fa-f: ]{64,96})",
        output,
        re.I | re.M,
    )
    digests = {normalize_digest(match.strip()) for match in matches}
    if not digests:
        raise ReleaseError("signer verification did not report a SHA-256 certificate digest")
    if certificate_chain:
        # keytool prints the leaf first, followed by its issuing certificates.
        signers = set(re.findall(r"^Signer #(\d+):", output, re.M))
        if len(signers) > 1:
            raise ReleaseError("Android artifact contains multiple signer certificates")
        return normalize_digest(matches[0].strip())
    if len(digests) != 1:
        raise ReleaseError("Android artifact contains multiple signer certificates")
    return digests.pop()


def verify_keystore(
    signing: SigningInputs, keytool: str, expected_digest: str = ""
) -> str:
    result = run_tool(
        [
            keytool,
            "-J-Duser.language=en",
            "-list",
            "-v",
            "-keystore",
            str(signing.keystore),
            "-alias",
            signing.key_alias,
            "-storepass:env",
            "STASIS_ANDROID_STORE_PASSWORD",
        ],
        signing,
    )
    output = result.stdout + result.stderr
    if re.search(r"Owner:.*CN=Android Debug", output, re.I):
        raise ReleaseError("production release cannot use the Android debug certificate")
    digest = certificate_digest(output, certificate_chain=True)
    if expected_digest and digest != normalize_digest(expected_digest):
        raise ReleaseError(
            f"Android signer SHA-256 mismatch: expected {normalize_digest(expected_digest)}, got {digest}"
        )
    return digest


def verify_private_key(signing: SigningInputs, jarsigner: str) -> None:
    """Exercise the selected private key without retaining a signed artifact."""
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "preflight.jar"
        signed = Path(directory) / "preflight-signed.jar"
        import zipfile

        with zipfile.ZipFile(source, "w") as archive:
            archive.writestr("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\r\n\r\n")
            archive.writestr("stasis-preflight", b"signing boundary probe")
        run_tool(
            [
                jarsigner,
                "-J-Duser.language=en",
                "-sigfile",
                "STASIS",
                "-signedjar",
                str(signed),
                "-keystore",
                str(signing.keystore),
                "-storepass:env",
                "STASIS_ANDROID_STORE_PASSWORD",
                "-keypass:env",
                "STASIS_ANDROID_KEY_PASSWORD",
                str(source),
                signing.key_alias,
            ],
            signing,
        )


def inspect_apk(artifact: Path, aapt: str) -> tuple[bool, str, str]:
    debuggable, version_code, version_name, _ = _inspect_apk_details(artifact, aapt)
    return debuggable, version_code, version_name


def _inspect_apk_details(artifact: Path, aapt: str) -> tuple[bool, str, str, str]:
    output = run_tool([aapt, "dump", "badging", str(artifact)]).stdout
    package = re.search(
        r"^package:\s+name='([^']+)'.*versionCode='([^']+)'.*versionName='([^']*)'",
        output,
        re.M,
    )
    if not package:
        raise ReleaseError("aapt did not report the APK version")
    return (
        "application-debuggable" in output,
        package.group(2),
        package.group(3),
        package.group(1),
    )


def inspect_aab(artifact: Path, bundletool: str, java: str) -> tuple[bool, str, str]:
    debuggable, version_code, version_name, _ = _inspect_aab_details(
        artifact, bundletool, java
    )
    return debuggable, version_code, version_name


def _inspect_aab_details(
    artifact: Path, bundletool: str, java: str
) -> tuple[bool, str, str, str]:
    output = run_tool(
        [java, "-jar", bundletool, "dump", "manifest", "--bundle", str(artifact)]
    ).stdout
    package = re.search(r"\bpackage=\"([^\"]+)\"", output)
    version_code = re.search(r"android:versionCode=\"([^\"]+)\"", output)
    version_name = re.search(r"android:versionName=\"([^\"]*)\"", output)
    if not package or not version_code or not version_name:
        raise ReleaseError("bundletool did not report the app bundle identity and version")
    debuggable = bool(re.search(r"android:debuggable=\"true\"", output, re.I))
    return debuggable, version_code.group(1), version_name.group(1), package.group(1)


def verify_apk_signer(artifact: Path, apksigner: str, reject_debug: bool = False) -> str:
    result = run_tool([apksigner, "verify", "--verbose", "--print-certs", str(artifact)])
    output = result.stdout + result.stderr
    signers = set(re.findall(r"Signer #(\d+) certificate SHA-256 digest", output, re.I))
    if len(signers) != 1:
        raise ReleaseError("Android APK must contain exactly one signer certificate")
    if reject_debug and re.search(r"certificate DN:.*CN=Android Debug", output, re.I):
        raise ReleaseError("production release cannot use the Android debug certificate")
    return certificate_digest(output)


def verify_aab_signer(artifact: Path, jarsigner: str, keytool: str) -> str:
    try:
        verification = subprocess.run(
            [jarsigner, "-J-Duser.language=en", "-verify", "-strict", str(artifact)],
            capture_output=True, text=True, check=False, timeout=900,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ReleaseError(f"jarsigner verification could not complete: {error}") from error
    verification_output = verification.stdout + verification.stderr
    verification_lower = verification_output.lower()
    if (
        verification.returncode not in {0, 4}
        or not any(message in verification_lower for message in (
            "jar verified.", "jar verified, with signer errors."
        ))
        or "certificate has expired" in verification_lower
        or "certificate is not yet valid" in verification_lower
    ):
        raise ReleaseError(
            "Android release tool failed (jarsigner): "
            + verification_output.strip()[-2000:]
        )
    result = run_tool(
        [keytool, "-J-Duser.language=en", "-printcert", "-jarfile", str(artifact)]
    )
    output = result.stdout + result.stderr
    if re.search(r"Owner:.*CN=Android Debug", output, re.I):
        raise ReleaseError("production release cannot use the Android debug certificate")
    return certificate_digest(output, certificate_chain=True)


def ensure_unsigned(artifact: Path, artifact_format: str, verifier: str) -> None:
    command = (
        [verifier, "verify", str(artifact)]
        if artifact_format == "apk"
        else [verifier, "-J-Duser.language=en", "-verify", str(artifact)]
    )
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, check=False, timeout=900
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ReleaseError(
            f"unsigned Android artifact verification could not complete: {error}"
        ) from error
    output = (result.stdout + result.stderr).lower()
    if artifact_format == "apk":
        if result.returncode == 0:
            raise ReleaseError("unsigned release APK unexpectedly contains a valid signer")
        unsigned_diagnostics = (
            "missing meta-inf/manifest.mf",
            "no signatures",
            "not signed",
        )
        if "does not verify" not in output or not any(
            diagnostic in output for diagnostic in unsigned_diagnostics
        ):
            raise ReleaseError("unsigned release APK could not be proven unsigned")
    if artifact_format == "aab" and "jar is unsigned" not in output:
        raise ReleaseError("unsigned release AAB could not be proven unsigned")


def write_json_atomic(destination: Path, value: object) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=destination.parent, delete=False, newline="\n"
    ) as temporary:
        json.dump(value, temporary, indent=2, sort_keys=True)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    try:
        os.replace(temporary_path, destination)
    finally:
        temporary_path.unlink(missing_ok=True)


def parse_sidecar(value: str) -> tuple[str, Path]:
    name, separator, path = value.partition("=")
    if not separator or not name or not path or Path(name).name != name:
        raise ReleaseError("sidecar must use NAME=PATH with a plain file name")
    if name not in {"mapping.txt", "native-debug-symbols.zip"}:
        raise ReleaseError(
            "Android sidecar name must be mapping.txt or native-debug-symbols.zip"
        )
    return name, Path(path).resolve()


def parse_sidecars(
    values: Sequence[str], reserved_names: set[str]
) -> list[tuple[str, Path]]:
    parsed: list[tuple[str, Path]] = []
    names: set[str] = set()
    for value in values:
        name, path = parse_sidecar(value)
        if name in names or name in reserved_names:
            raise ReleaseError(f"duplicate or colliding Android sidecar name: {name}")
        names.add(name)
        if not path.is_file():
            raise ReleaseError(f"Android sidecar source was not found: {path}")
        parsed.append((name, path))
    return parsed


def _read_json_object(path: Path, label: str) -> dict[str, object]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"{label} could not be read from {path}: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError(f"{label} must be a JSON object: {path}")
    return value


def _embedded_provenance_digest(artifact: Path, artifact_format: str) -> str:
    entry = (
        "base/assets/stasis_game/stasis_provenance.json"
        if artifact_format == "aab"
        else "assets/stasis_game/stasis_provenance.json"
    )
    try:
        with zipfile.ZipFile(artifact) as archive:
            embedded = archive.read(entry)
    except (OSError, KeyError, zipfile.BadZipFile) as error:
        raise ReleaseError(
            f"Android artifact is missing embedded provenance: {entry}"
        ) from error
    return hashlib.sha256(embedded).hexdigest()


def _verify_embedded_provenance(
    artifact: Path, provenance_path: Path, artifact_format: str
) -> str:
    expected = hashlib.sha256(provenance_path.read_bytes()).hexdigest()
    actual = _embedded_provenance_digest(artifact, artifact_format)
    if actual != expected:
        raise ReleaseError(
            "Android artifact embedded provenance does not match the generated provenance"
        )
    return actual


def _publish_staged(
    staged_files: Sequence[tuple[Path, Path]], destination_parent: Path
) -> None:
    """Replace a set of files while retaining the previous set on failure."""
    backup_directory = Path(
        tempfile.mkdtemp(prefix=".stasis-android-publish-", dir=destination_parent)
    )
    backups: dict[Path, Path] = {}
    keep_backup = False
    try:
        for index, (_, destination) in enumerate(staged_files):
            if destination.exists():
                backup = backup_directory / f"{index}.bak"
                shutil.copy2(destination, backup)
                backups[destination] = backup
        published: list[Path] = []
        try:
            for staged, destination in staged_files:
                os.replace(staged, destination)
                published.append(destination)
        except BaseException as publication_error:
            rollback_errors: list[str] = []
            for _, destination in reversed(staged_files):
                backup = backups.get(destination)
                try:
                    if backup is not None and backup.is_file():
                        shutil.copy2(backup, destination)
                    elif destination in published:
                        destination.unlink(missing_ok=True)
                except OSError as rollback_error:
                    rollback_errors.append(f"{destination}: {rollback_error}")
            if rollback_errors:
                keep_backup = True
                detail = "; ".join(rollback_errors)
                raise ReleaseError(
                    "Android publication failed and rollback was incomplete; "
                    f"recovery directory: {backup_directory}; {detail}"
                ) from publication_error
            raise
    finally:
        if not keep_backup:
            shutil.rmtree(backup_directory, ignore_errors=True)


def _install_receipt_update(
    receipt_path: Path,
    receipt: dict[str, object],
    *,
    status: str,
    installed: bool,
    error: str | None = None,
) -> dict[str, object]:
    updated = dict(receipt)
    updated["installed"] = installed
    updated["install_status"] = status
    if error is None:
        updated.pop("install_error", None)
    else:
        updated["install_error"] = error[:2000]
    write_json_atomic(receipt_path, updated)
    return updated


def preflight(args: argparse.Namespace) -> dict[str, object]:
    if args.development_build:
        if args.unsigned_release:
            raise ReleaseError("-UnsignedRelease is only valid for a production release handoff")
        return {"variant": "debug", "signed": False}
    if args.unsigned_release:
        if args.install:
            raise ReleaseError("an unsigned release cannot be installed or published")
        return {"variant": "release", "signed": False, "handoff": "unsigned"}
    signing = resolve_signing_inputs(
        os.environ,
        args.keystore,
        args.key_alias,
        [*args.forbidden_root, str(REPOSITORY_ROOT)],
    )
    keytool = find_sdk_tool("keytool", args.keytool)
    digest = verify_keystore(signing, keytool, args.expected_signer_sha256)
    verify_private_key(signing, find_sdk_tool("jarsigner", args.jarsigner))
    return {"variant": "release", "signed": True, "signer_sha256": digest}


def finalize(args: argparse.Namespace) -> dict[str, object]:
    unsigned_release = bool(getattr(args, "unsigned_release", False))
    install_requested = bool(getattr(args, "install", False))
    if unsigned_release and (install_requested or args.variant == "debug"):
        raise ReleaseError("unsigned release mode cannot be installed or used for debug builds")
    if unsigned_release and getattr(args, "expected_signer_sha256", ""):
        raise ReleaseError("an unsigned release cannot verify an expected signer digest")
    if args.variant == "release" and args.abi != "arm64-v8a":
        raise ReleaseError("production Android releases require the arm64-v8a ABI")
    if install_requested and getattr(args, "format", "") != "apk":
        raise ReleaseError("installation requires an APK artifact")
    source = Path(args.source).resolve()
    if not source.is_file():
        raise ReleaseError(f"built Android artifact was not found: {source}")
    output = Path(args.output).resolve()
    receipt_path = Path(args.receipt).resolve()
    if receipt_path.parent != output.parent:
        raise ReleaseError("final Android artifact and receipt must share one output directory")
    if os.path.normcase(str(output)) == os.path.normcase(str(receipt_path)):
        raise ReleaseError("final Android artifact and receipt paths must differ")
    package_manifest_path = Path(args.package_manifest).resolve()
    provenance_path = Path(args.provenance).resolve()
    package_manifest = _read_json_object(package_manifest_path, "mobile package manifest")
    expected_development = args.variant == "debug"
    if package_manifest.get("development_build") is not expected_development:
        raise ReleaseError("generated package provenance does not match the requested Android variant")
    declared_package_id = package_manifest.get("package_id", "")
    if not isinstance(declared_package_id, str):
        raise ReleaseError("generated package manifest package_id must be a string")
    requested_package_id = str(getattr(args, "package_id", "") or "")
    if requested_package_id and declared_package_id and requested_package_id != declared_package_id:
        raise ReleaseError("requested Android package ID does not match generated package provenance")
    expected_package_id = requested_package_id or declared_package_id
    if not expected_package_id:
        raise ReleaseError("generated package manifest is missing the Android package ID")
    declared_target = package_manifest.get("target")
    expected_target = {
        "arm64-v8a": "android-arm64",
        "x86_64": "android-x86_64",
    }.get(args.abi)
    if declared_target is not None and declared_target != expected_target:
        raise ReleaseError("generated package target does not match the requested Android ABI")
    expected_code_value = package_manifest.get("android_version_code")
    expected_name_value = package_manifest.get("android_version_name")
    if isinstance(expected_code_value, bool) or not isinstance(
        expected_code_value, (str, int)
    ) or not str(expected_code_value):
        raise ReleaseError("generated package manifest is missing a valid Android version code")
    if not isinstance(expected_name_value, str) or not expected_name_value:
        raise ReleaseError("generated package manifest is missing a valid Android version name")
    if not provenance_path.is_file():
        raise ReleaseError(f"Android provenance file was not found: {provenance_path}")
    manifest_provenance = package_manifest.get("provenance")
    if manifest_provenance is not None and manifest_provenance != provenance_path.name:
        raise ReleaseError("Android provenance path does not match generated package manifest")
    parsed_sidecars = parse_sidecars(
        getattr(args, "sidecar", ()), {output.name, receipt_path.name}
    )

    signing = None
    signed_release = args.variant == "release" and not unsigned_release
    if signed_release:
        signing = resolve_signing_inputs(
            os.environ,
            args.keystore,
            args.key_alias,
            [
                *args.forbidden_root,
                str(REPOSITORY_ROOT),
                str(package_manifest_path.parent),
            ],
        )
        selected_signer_digest = verify_keystore(
            signing, find_sdk_tool("keytool", args.keytool), args.expected_signer_sha256
        )
    else:
        selected_signer_digest = None

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent) as directory:
        staged = Path(directory) / output.name
        if signed_release and args.format == "apk":
            run_tool(
                [
                    find_sdk_tool("apksigner", args.apksigner), "sign",
                    "--ks", str(signing.keystore), "--ks-key-alias", signing.key_alias,
                    "--ks-pass", "env:STASIS_ANDROID_STORE_PASSWORD",
                    "--key-pass", "env:STASIS_ANDROID_KEY_PASSWORD",
                    "--v1-signer-name", "STASIS",
                    "--out", str(staged), str(source),
                ],
                signing,
            )
        else:
            shutil.copy2(source, staged)
            if signed_release:
                run_tool(
                    [
                        find_sdk_tool("jarsigner", args.jarsigner),
                        "-J-Duser.language=en",
                        "-sigfile", "STASIS",
                        "-keystore", str(signing.keystore),
                        "-storepass:env", "STASIS_ANDROID_STORE_PASSWORD",
                        "-keypass:env", "STASIS_ANDROID_KEY_PASSWORD",
                        str(staged), signing.key_alias,
                    ],
                    signing,
                )

        validate_contents(staged, args.abi, args.required_asset)
        embedded_provenance_identity = _verify_embedded_provenance(
            staged, provenance_path, args.format
        )
        if args.format == "apk":
            debuggable, version_code, version_name, package_id = _inspect_apk_details(
                staged, find_sdk_tool("aapt", args.aapt)
            )
            apksigner = find_sdk_tool("apksigner", args.apksigner)
            if unsigned_release:
                ensure_unsigned(staged, "apk", apksigner)
                signer_digest = None
            else:
                signer_digest = verify_apk_signer(
                    staged, apksigner, reject_debug=signed_release
                )
        else:
            bundletool = args.bundletool or os.environ.get("BUNDLETOOL_PATH", "")
            if not bundletool or not Path(bundletool).is_file():
                raise ReleaseError(
                    "AAB inspection requires -BundletoolPath or BUNDLETOOL_PATH "
                    "pointing to the official bundletool-all JAR"
                )
            debuggable, version_code, version_name, package_id = _inspect_aab_details(
                staged, bundletool, find_sdk_tool("java", args.java)
            )
            jarsigner = find_sdk_tool("jarsigner", args.jarsigner)
            if unsigned_release:
                ensure_unsigned(staged, "aab", jarsigner)
                signer_digest = None
            else:
                signer_digest = verify_aab_signer(
                    staged, jarsigner, find_sdk_tool("keytool", args.keytool)
                )
        if not expected_development:
            verify_compiled_launcher(
                staged, aapt=args.aapt, bundletool=args.bundletool, java=args.java
            )
        if debuggable is not expected_development:
            raise ReleaseError(
                f"{args.variant} artifact reported debuggable={str(debuggable).lower()}"
            )
        if package_id != expected_package_id:
            raise ReleaseError(
                "Android artifact package ID does not match generated package provenance"
            )
        if args.expected_signer_sha256 and signer_digest != normalize_digest(args.expected_signer_sha256):
            raise ReleaseError("final Android artifact signer does not match the expected SHA-256")
        if signed_release and signer_digest is None:
            raise ReleaseError("signed release artifact did not contain a signer certificate")
        if signed_release and signer_digest != selected_signer_digest:
            raise ReleaseError("final Android artifact signer differs from the selected keystore certificate")
        expected_code = str(expected_code_value)
        expected_name = expected_name_value
        if version_code != expected_code or version_name != expected_name:
            raise ReleaseError(
                "Android artifact version does not match the generated package receipt"
            )

        artifact_sha256 = hashlib.sha256(staged.read_bytes()).hexdigest()
        sidecar_pairs: list[tuple[Path, Path]] = []
        sidecars: list[str] = []
        for name, source_sidecar in parsed_sidecars:
            staged_sidecar = Path(directory) / name
            shutil.copy2(source_sidecar, staged_sidecar)
            destination = output.parent / name
            sidecar_pairs.append((staged_sidecar, destination))
            sidecars.append(str(destination.resolve()))

        provenance_identity = hashlib.sha256(provenance_path.read_bytes()).hexdigest()
        receipt = {
            "schema": "stasis.android_artifact.v1",
            "variant": args.variant,
            "abi": args.abi,
            "debuggable": debuggable,
            "package": package_id,
            "version": {"code": version_code, "name": version_name},
            "signer_sha256": signer_digest,
            "provenance_identity": provenance_identity,
            "embedded_provenance_sha256": embedded_provenance_identity,
            "artifact_path": str(output.resolve()),
            "artifact_sha256": artifact_sha256,
            "format": args.format,
            "unsigned_release": unsigned_release,
            "signing_mode": (
                "unsigned-release" if unsigned_release else
                "debug" if args.variant == "debug" else "signed-release"
            ),
            "install_requested": install_requested,
            "installed": False,
            "install_status": "pending" if install_requested else "not-requested",
            "sidecars": sidecars,
        }
        staged_receipt = Path(directory) / receipt_path.name
        write_json_atomic(staged_receipt, receipt)
        _publish_staged(
            [(staged, output), *sidecar_pairs, (staged_receipt, receipt_path)],
            output.parent,
        )

    if install_requested:
        try:
            command = [find_sdk_tool("adb", getattr(args, "adb", ""))]
            if getattr(args, "serial", ""):
                command.extend(["-s", args.serial])
            command.extend(["install", "-r", str(output)])
            run_tool(command)
        except (OSError, ReleaseError) as error:
            redacted = _redact(str(error), signing)
            try:
                receipt = _install_receipt_update(
                    receipt_path,
                    receipt,
                    status="failed",
                    installed=False,
                    error=redacted,
                )
            except OSError as receipt_error:
                raise ReleaseError(
                    "Android artifact was published but installation failed and "
                    f"the failure receipt could not be updated: {_redact(str(receipt_error), signing)}"
                ) from error
            raise ReleaseError(
                f"Android artifact was published but installation failed: {redacted}"
            ) from error
        try:
            receipt = _install_receipt_update(
                receipt_path, receipt, status="passed", installed=True
            )
        except OSError as error:
            raise ReleaseError(
                "Android artifact was installed but the success receipt could not "
                f"be updated: {_redact(str(error), signing)}"
            ) from error
    return {**receipt, "receipt_path": str(receipt_path.resolve())}


def verify(args: argparse.Namespace) -> dict[str, object]:
    artifact = Path(args.apk).resolve()
    receipt_path = artifact.with_name("stasis-release.receipt.json")
    if not receipt_path.is_file():
        raise ReleaseError(f"Android release receipt was not found: {receipt_path}")
    receipt = _read_json_object(receipt_path, "Android release receipt")
    if receipt.get("variant") != "release" or receipt.get("signing_mode") != "signed-release":
        raise ReleaseError("Android release receipt does not describe a signed production release")
    abi = receipt.get("abi")
    if not isinstance(abi, str) or not abi:
        raise ReleaseError("Android release receipt does not contain an ABI")
    if abi != "arm64-v8a":
        raise ReleaseError("production Android releases require the arm64-v8a ABI")
    validate_contents(artifact, abi, args.required_asset)
    aapt = find_sdk_tool("aapt", args.aapt)
    verify_compiled_launcher(artifact, aapt=aapt)
    badging = run_tool([aapt, "dump", "badging", str(artifact)]).stdout
    package_match = re.search(r"^package:\s+name='([^']+)'", badging, re.M)
    if not package_match:
        raise ReleaseError("aapt did not report the APK application ID")
    debuggable, version_code, version_name = inspect_apk(artifact, aapt)
    if debuggable:
        raise ReleaseError("production release APK is marked debuggable")
    signer = verify_apk_signer(
        artifact, find_sdk_tool("apksigner", args.apksigner), reject_debug=True
    )
    if args.expected_signer_sha256 and signer != normalize_digest(args.expected_signer_sha256):
        raise ReleaseError("Android release APK signer does not match the expected SHA-256")
    artifact_sha256 = hashlib.sha256(artifact.read_bytes()).hexdigest()
    if receipt.get("artifact_sha256", "").lower() != artifact_sha256:
        raise ReleaseError("Android release receipt artifact digest does not match the APK")
    if receipt.get("signer_sha256") != signer:
        raise ReleaseError("Android release receipt signer does not match the APK")
    embedded_provenance = _embedded_provenance_digest(artifact, "apk")
    expected_provenance = receipt.get("embedded_provenance_sha256") or receipt.get(
        "provenance_identity"
    )
    if expected_provenance and str(expected_provenance).lower() != embedded_provenance:
        raise ReleaseError("Android release receipt provenance does not match the APK")
    if receipt.get("version") != {"code": version_code, "name": version_name}:
        raise ReleaseError("Android release receipt version does not match the APK")
    receipt_package = receipt.get("package")
    if receipt_package is not None and receipt_package != package_match.group(1):
        raise ReleaseError("Android release receipt package ID does not match the APK")
    return {
        **receipt,
        "artifact_sha256": artifact_sha256,
        "signer_sha256": signer,
        "debuggable": False,
        "package": package_match.group(1),
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    subparsers = result.add_subparsers(dest="command", required=True)
    before = subparsers.add_parser("preflight")
    before.add_argument("--development-build", action="store_true")
    before.add_argument("--unsigned-release", action="store_true")
    before.add_argument("--install", action="store_true")
    before.add_argument("--keystore", default="")
    before.add_argument("--key-alias", default="")
    before.add_argument("--expected-signer-sha256", default="")
    before.add_argument("--keytool", default="")
    before.add_argument("--jarsigner", default="")
    before.add_argument("--forbidden-root", action="append", default=[])

    after = subparsers.add_parser("finalize")
    after.add_argument("--source", required=True)
    after.add_argument("--output", required=True)
    after.add_argument("--receipt", required=True)
    after.add_argument("--package-manifest", required=True)
    after.add_argument("--provenance", required=True)
    after.add_argument("--format", choices=("apk", "aab"), required=True)
    after.add_argument("--variant", choices=("debug", "release"), required=True)
    after.add_argument("--abi", required=True)
    after.add_argument("--required-asset", default="")
    after.add_argument("--unsigned-release", action="store_true")
    after.add_argument("--install", action="store_true")
    after.add_argument("--serial", default="")
    after.add_argument("--keystore", default="")
    after.add_argument("--key-alias", default="")
    after.add_argument("--package-id", default="")
    after.add_argument("--expected-signer-sha256", default="")
    after.add_argument("--apksigner", default="")
    after.add_argument("--aapt", default="")
    after.add_argument("--adb", default="")
    after.add_argument("--jarsigner", default="")
    after.add_argument("--keytool", default="")
    after.add_argument("--bundletool", default="")
    after.add_argument("--java", default="")
    after.add_argument("--sidecar", action="append", default=[])
    after.add_argument("--forbidden-root", action="append", default=[])

    check = subparsers.add_parser("verify")
    check.add_argument("--apk", required=True)
    check.add_argument("--expected-signer-sha256", default="")
    check.add_argument("--required-asset", default="")
    check.add_argument("--aapt", default="")
    check.add_argument("--apksigner", default="")
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        result = (
            preflight(args)
            if args.command == "preflight"
            else verify(args)
            if args.command == "verify"
            else finalize(args)
        )
    except (OSError, ValueError) as error:
        print(f"Android release boundary failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Validate the versioned cross-host registry against production sources."""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.ci import check_runtime_abi_contract as abi

REGISTRY = Path("contracts/v1/host_runtime.json")
LIFECYCLE = Path("runtime/stasis_renderer_lifecycle.h")
JAVA_LIFECYCLE = Path(
    "mobile/android/app/src/main/java/com/stasislang/workshop/RendererResourceLifecycle.java"
)
COMPILER = Path("crates/stasis_compiler/src/lib.rs")
ASSETS = Path("crates/stasis_assets/src/lib.rs")
SWAP_CONTRACTS = Path("crates/stasis_runner/src/swap/contracts.rs")
DEVELOPMENT_SWAP = Path("crates/stasis_compiler/src/backend/development_swap.rs")
DYNLOAD = Path("crates/stasis_dynload/src/lib.rs")
MOBILE = Path("runtime/stasis_mobile_runtime.h")
WEB = Path("runtime/web/game.js")

EXPECTED_TOP_LEVEL = {
    "schema", "version", "host_frame", "render_command", "renderer_lifecycle",
    "guest_entrypoints", "diagnostics", "compile_transaction", "development_swap",
    "asset_package",
}


@dataclass(frozen=True)
class Failure:
    field: str
    source: str
    expected: object
    actual: object

    def __str__(self) -> str:
        return (
            f"host contract mismatch: field={self.field} source={self.source} "
            f"expected={self.expected!r} actual={self.actual!r}"
        )


def _read(path: Path, overlays: dict[Path, str]) -> str:
    return overlays.get(path, (ROOT / path).read_text(encoding="utf-8"))


def _compare_map(
    prefix: str, expected: dict[str, int], actual: dict[str, int], source: Path
) -> list[Failure]:
    return [
        Failure(f"{prefix}.{name}", source.as_posix(), value, actual.get(name, "missing"))
        for name, value in expected.items()
        if actual.get(name) != value
    ]


def _enum_constants(text: str, prefix: str) -> dict[str, int]:
    return {
        name: int(value)
        for name, value in re.findall(rf"\b({prefix}[A-Z0-9_]+)\s*=\s*(-?\d+)", text)
    }


def _struct_fields(text: str, name: str) -> list[str]:
    match = re.search(rf"pub struct {re.escape(name)}\s*\{{(?P<body>.*?)\n\}}", text, re.S)
    if not match:
        return []
    return re.findall(r"^\s*pub\s+([a-z][a-z0-9_]*):", match.group("body"), re.M)


def validate_envelope(value: object) -> str | None:
    if not isinstance(value, dict):
        return "envelope must be an object"
    if value.get("schema") != "stasis.host_runtime_contract":
        return "unsupported contract schema"
    if value.get("version") != 1:
        return "unsupported contract version"
    return None


def check(
    registry: dict[str, object] | None = None,
    overlays: dict[Path, str] | None = None,
) -> tuple[list[Failure], dict[str, object]]:
    overlays = overlays or {}
    if registry is None:
        registry = json.loads(_read(REGISTRY, overlays))
    failures: list[Failure] = []
    if set(registry) != EXPECTED_TOP_LEVEL:
        failures.append(Failure("registry.fields", REGISTRY.as_posix(), sorted(EXPECTED_TOP_LEVEL), sorted(registry)))
        return failures, {"checks": 1, "status": "failed"}
    envelope_error = validate_envelope(registry)
    if envelope_error:
        failures.append(Failure("registry.envelope", REGISTRY.as_posix(), "schema/version v1", envelope_error))
        return failures, {"checks": 1, "status": "failed"}

    host = registry["host_frame"]
    render = registry["render_command"]
    failures += _compare_map(
        "host_frame.constants",
        host["constants"],
        abi.stasis_constants(_read(abi.HOST_FRAME, overlays)),
        abi.HOST_FRAME,
    )
    render_actual = abi.c_constants(_read(abi.RENDER_HEADER, overlays))
    failures += _compare_map(
        "render_command.constants", render["constants"], render_actual, abi.RENDER_HEADER
    )
    accepted = [render_actual[f"STASIS_RENDER_V{version}_VERSION"] for version in range(2, 7)]
    if render["accepted_versions"] != accepted:
        failures.append(Failure("render_command.accepted_versions", abi.RENDER_HEADER.as_posix(), accepted, render["accepted_versions"]))

    lifecycle_text = _read(LIFECYCLE, overlays)
    lifecycle = registry["renderer_lifecycle"]
    failures += _compare_map("renderer_lifecycle.states", lifecycle["states"], _enum_constants(lifecycle_text, "STASIS_RENDERER_"), LIFECYCLE)
    failures += _compare_map("renderer_lifecycle.reasons", lifecycle["reasons"], _enum_constants(lifecycle_text, "STASIS_RENDERER_REASON_"), LIFECYCLE)
    lifecycle_struct = re.search(r"typedef struct \{(?P<body>.*?)\} StasisRendererLifecycle;", lifecycle_text, re.S)
    fields = re.findall(r"\b([a-z][a-z0-9_]*)\s*;", lifecycle_struct.group("body")) if lifecycle_struct else []
    if lifecycle["snapshot_fields"] != fields:
        failures.append(Failure("renderer_lifecycle.snapshot_fields", LIFECYCLE.as_posix(), fields, lifecycle["snapshot_fields"]))
    java_text = _read(JAVA_LIFECYCLE, overlays)
    for name in lifecycle["states"]:
        java_name = name.removeprefix("STASIS_RENDERER_")
        if not re.search(rf"\b{re.escape(java_name)}\b", java_text):
            failures.append(Failure(f"renderer_lifecycle.java.{java_name}", JAVA_LIFECYCLE.as_posix(), "present", "missing"))

    compiler_text = _read(COMPILER, overlays)
    source_codes = sorted(set(re.findall(r'"(stasis\.[A-Za-z]+)"', compiler_text)))
    if sorted(registry["diagnostics"]["source_codes"]) != source_codes:
        failures.append(Failure("diagnostics.source_codes", COMPILER.as_posix(), source_codes, sorted(registry["diagnostics"]["source_codes"])))
    asset_codes = sorted(set(re.findall(r'"(asset_[a-z0-9_]+)"', _read(ASSETS, overlays))))
    if sorted(registry["diagnostics"]["asset_codes"]) != asset_codes:
        failures.append(Failure("diagnostics.asset_codes", ASSETS.as_posix(), asset_codes, sorted(registry["diagnostics"]["asset_codes"])))

    swap_text = _read(SWAP_CONTRACTS, overlays)
    version = re.search(r"pub const CONTRACT_VERSION: u16 = (\d+);", swap_text)
    actual_version = int(version.group(1)) if version else "missing"
    transaction = registry["compile_transaction"]
    if transaction["version"] != actual_version:
        failures.append(Failure("compile_transaction.version", SWAP_CONTRACTS.as_posix(), actual_version, transaction["version"]))
    for registry_name, struct_name in (("compile_result_fields", "CompileResult"), ("activation_result_fields", "SwapCommitResult")):
        actual_fields = _struct_fields(swap_text, struct_name)
        if transaction[registry_name] != actual_fields:
            failures.append(Failure(f"compile_transaction.{registry_name}", SWAP_CONTRACTS.as_posix(), actual_fields, transaction[registry_name]))

    development_text = _read(DEVELOPMENT_SWAP, overlays)
    development = registry["development_swap"]
    development_version = re.search(
        r"pub const DEVELOPMENT_SWAP_RECEIPT_SCHEMA_VERSION: u16 = (\d+);",
        development_text,
    )
    actual_development_version = (
        int(development_version.group(1)) if development_version else "missing"
    )
    if development["version"] != actual_development_version:
        failures.append(
            Failure(
                "development_swap.version",
                DEVELOPMENT_SWAP.as_posix(),
                actual_development_version,
                development["version"],
            )
        )
    actual_receipt_fields = _struct_fields(development_text, "DevelopmentSwapReceipt")
    if development["receipt_fields"] != actual_receipt_fields:
        failures.append(
            Failure(
                "development_swap.receipt_fields",
                DEVELOPMENT_SWAP.as_posix(),
                actual_receipt_fields,
                development["receipt_fields"],
            )
        )
    status_match = re.search(
        r"pub enum DevelopmentSwapStatus\s*\{(?P<body>.*?)\n\}",
        development_text,
        re.S,
    )
    actual_status_tags = (
        [name.lower() for name in re.findall(r"^\s*([A-Z][A-Za-z0-9_]*)\s*,", status_match.group("body"), re.M)]
        if status_match
        else []
    )
    if development["status_tags"] != actual_status_tags:
        failures.append(
            Failure(
                "development_swap.status_tags",
                DEVELOPMENT_SWAP.as_posix(),
                actual_status_tags,
                development["status_tags"],
            )
        )

    entrypoints = registry["guest_entrypoints"]
    source_patterns = {
        "main": ((DYNLOAD, "stasis_jit_host_main_trampoline"), (MOBILE, "main_entry"), (WEB, "instance.exports.main")),
        "tick": ((DYNLOAD, "stasis_jit_host_tick_trampoline"), (MOBILE, "tick_entry"), (WEB, "instance.exports.tick")),
        "render": ((DYNLOAD, "stasis_jit_host_render_trampoline"), (MOBILE, "render_entry"), (WEB, "instance.exports.render")),
        "on_code_swap": ((DYNLOAD, "stasis_jit_host_on_code_swap_trampoline"),),
    }
    for entrypoint, patterns in source_patterns.items():
        if entrypoints[entrypoint]["symbol"] != entrypoint:
            failures.append(Failure(f"guest_entrypoints.{entrypoint}.symbol", REGISTRY.as_posix(), entrypoint, entrypoints[entrypoint]["symbol"]))
        for path, pattern in patterns:
            if pattern not in _read(path, overlays):
                failures.append(Failure(f"guest_entrypoints.{entrypoint}", path.as_posix(), pattern, "missing"))

    asset = registry["asset_package"]
    expected_asset = {"schema": "stasis.asset_package", "version": 1, "identity_path": "stasis_asset_package.json", "manifest_path": "assets/manifest.json", "hash_algorithm": "sha256"}
    if asset != expected_asset:
        failures.append(Failure("asset_package", REGISTRY.as_posix(), expected_asset, asset))
    assets_text = _read(ASSETS, overlays)
    asset_source_values: dict[str, object] = {}
    for field, constant in (
        ("schema", "ASSET_PACKAGE_IDENTITY_SCHEMA"),
        ("identity_path", "ASSET_PACKAGE_IDENTITY_PATH"),
        ("manifest_path", "DEFAULT_ASSET_MANIFEST_PATH"),
        ("hash_algorithm", "ASSET_PACKAGE_IDENTITY_HASH_ALGORITHM"),
    ):
        match = re.search(
            rf'pub const {constant}: &str = "([^"]+)";', assets_text
        )
        asset_source_values[field] = match.group(1) if match else "missing"
    version_match = re.search(
        r"pub const ASSET_PACKAGE_IDENTITY_VERSION: u32 = (\d+);", assets_text
    )
    asset_source_values["version"] = (
        int(version_match.group(1)) if version_match else "missing"
    )
    for field, expected in asset.items():
        if asset_source_values.get(field) != expected:
            failures.append(
                Failure(
                    f"asset_package.{field}",
                    ASSETS.as_posix(),
                    expected,
                    asset_source_values.get(field, "missing"),
                )
            )
    provenance_text = _read(abi.PACKAGE_PROVENANCE, overlays)
    provenance_values: dict[str, object] = {}
    for field, constant in (
        ("schema", "ASSET_PACKAGE_IDENTITY_SCHEMA"),
        ("identity_path", "ASSET_PACKAGE_IDENTITY_NAME"),
        ("version", "ASSET_PACKAGE_IDENTITY_VERSION"),
    ):
        pattern = (
            rf'{constant} = "([^"]+)"'
            if field in ("schema", "identity_path")
            else rf"{constant} = (\d+)"
        )
        match = re.search(pattern, provenance_text)
        provenance_values[field] = (
            (int(match.group(1)) if field == "version" else match.group(1))
            if match
            else "missing"
        )
    for field in ("schema", "identity_path", "version"):
        if provenance_values[field] != asset[field]:
            failures.append(
                Failure(
                    f"asset_package.provenance.{field}",
                    abi.PACKAGE_PROVENANCE.as_posix(),
                    asset[field],
                    provenance_values[field],
                )
            )
    toolchain_text = _read(abi.TOOLCHAIN, overlays)
    for pattern in (
        "write_asset_package_identity",
        'runtime_config["asset_package"]',
    ):
        if pattern not in toolchain_text:
            failures.append(
                Failure(
                    "asset_package.toolchain_consumer",
                    abi.TOOLCHAIN.as_posix(),
                    pattern,
                    "missing",
                )
            )

    abi_failures, abi_evidence = abi.check(overlays={path: text for path, text in overlays.items() if path in abi.REQUIRED})
    for failure in abi_failures:
        failures.append(Failure(f"existing_abi.{failure.field}", failure.consumer, failure.expected, failure.actual))
    checks = len(host["constants"]) + len(render["constants"]) + len(lifecycle["states"]) + len(lifecycle["reasons"]) + len(source_codes) + len(asset_codes) + len(actual_receipt_fields) + len(actual_status_tags) + 1 + int(abi_evidence.get("checks", 0))
    return failures, {"schema": "stasis.host_contract.evidence.v1", "checks": checks, "status": "failed" if failures else "passed"}


def main() -> int:
    failures, evidence = check()
    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    print(f"host runtime contract passed ({evidence['checks']} comparisons)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

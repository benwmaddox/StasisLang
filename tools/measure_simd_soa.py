#!/usr/bin/env python3
"""Reproduce the static ARM64 SIMD characterization for Maddox task #735."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys
from typing import Any


INSTRUCTION = re.compile(r"^\s*[0-9a-f]+:\s+([a-z0-9.]+)\s*(.*)$", re.IGNORECASE)
VECTOR_REGISTER = re.compile(r"\bv\d+(?:\.\d+[bhsd])?\b", re.IGNORECASE)
Q_REGISTER = re.compile(r"\bq\d+\b", re.IGNORECASE)


def run(command: list[str], root: Path, env: dict[str, str] | None = None) -> str:
    completed = subprocess.run(
        command,
        cwd=root,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return completed.stdout


def find_ndk() -> Path:
    configured = os.environ.get("ANDROID_NDK_ROOT") or os.environ.get("ANDROID_NDK_HOME")
    if configured:
        return Path(configured).resolve()
    sdk = Path(os.environ.get("ANDROID_SDK_ROOT", r"C:\Android\Sdk"))
    candidates = sorted((sdk / "ndk").glob("*"), reverse=True)
    if not candidates:
        raise RuntimeError("Android NDK not found; set ANDROID_NDK_ROOT")
    return candidates[0].resolve()


def llvm_tool(ndk: Path, name: str) -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    prebuilt = ndk / "toolchains" / "llvm" / "prebuilt"
    hosts = sorted(path for path in prebuilt.glob("*") if path.is_dir())
    if not hosts:
        raise RuntimeError(f"NDK LLVM prebuilt host directory missing under {prebuilt}")
    path = hosts[0] / "bin" / f"{name}{suffix}"
    if not path.is_file():
        raise RuntimeError(f"required LLVM tool missing: {path}")
    return path


def disassembly_metrics(text: str) -> dict[str, Any]:
    instructions: list[tuple[str, str]] = []
    for line in text.splitlines():
        match = INSTRUCTION.match(line)
        if match:
            instructions.append((match.group(1).lower(), match.group(2).lower()))
    vector = [
        (mnemonic, operands)
        for mnemonic, operands in instructions
        if VECTOR_REGISTER.search(operands) or Q_REGISTER.search(operands)
    ]
    packed = [
        (mnemonic, operands)
        for mnemonic, operands in vector
        if Q_REGISTER.search(operands) or mnemonic not in {"movi", "fmov"}
    ]

    def is_branch(mnemonic: str) -> bool:
        return mnemonic in {"b", "cbz", "cbnz", "tbz", "tbnz"} or mnemonic.startswith("b.")

    return {
        "branch_instructions": sum(is_branch(mnemonic) for mnemonic, _ in instructions),
        "instruction_count": len(instructions),
        "load_instructions": sum(mnemonic.startswith("ld") for mnemonic, _ in instructions),
        "store_instructions": sum(mnemonic.startswith("st") for mnemonic, _ in instructions),
        "packed_lane_instruction_count": len(packed),
        "packed_lane_mnemonics": sorted({mnemonic for mnemonic, _ in packed}),
        "vector_encoding_instruction_count": len(vector),
        "vector_encoding_mnemonics": sorted({mnemonic for mnemonic, _ in vector}),
    }


def text_size(llvm_size: Path, object_path: Path, root: Path) -> int:
    output = run([str(llvm_size), str(object_path)], root)
    for line in output.splitlines()[1:]:
        columns = line.split()
        if columns and columns[0].isdigit():
            return int(columns[0])
    raise RuntimeError(f"could not parse llvm-size output for {object_path}:\n{output}")


def compile_prototype(output: Path, clang: Path, objdump: Path, llvm_size: Path, root: Path) -> dict[str, Any]:
    sources = {
        "scalar": r"""
#include <stddef.h>
void position_scalar(float *restrict x, float *restrict y,
                     const float *restrict vx, const float *restrict vy,
                     size_t count) {
  for (size_t i = 0; i < count; ++i) {
    x[i] += vx[i];
    y[i] += vy[i];
  }
}
""",
        "neon": r"""
#include <arm_neon.h>
#include <stddef.h>
void position_neon(float *restrict x, float *restrict y,
                   const float *restrict vx, const float *restrict vy,
                   size_t count) {
  size_t i = 0;
  for (; i + 4 <= count; i += 4) {
    vst1q_f32(x + i, vaddq_f32(vld1q_f32(x + i), vld1q_f32(vx + i)));
    vst1q_f32(y + i, vaddq_f32(vld1q_f32(y + i), vld1q_f32(vy + i)));
  }
  for (; i < count; ++i) {
    x[i] += vx[i];
    y[i] += vy[i];
  }
}
""",
    }
    result: dict[str, Any] = {
        "contract": "scratch-only static position update; count is dynamic and scalar remainder preserves 1-3 tail lanes",
        "runtime_state_parity": "unresolved without ARM64 execution",
    }
    for label, source in sources.items():
        source_path = output / f"position_{label}.c"
        object_path = output / f"position_{label}.o"
        source_path.write_text(source.strip() + "\n", encoding="ascii")
        flags = [
            "--target=aarch64-linux-android26",
            "-O3",
            "-ffreestanding",
            "-fno-vectorize",
            "-fno-slp-vectorize",
        ]
        run([str(clang), *flags, "-c", str(source_path), "-o", str(object_path)], root)
        disassembly = run([str(objdump), "--disassemble", "--no-show-raw-insn", str(object_path)], root)
        (output / f"position_{label}.disasm.txt").write_text(disassembly, encoding="utf-8")
        result[label] = {
            **disassembly_metrics(disassembly),
            "text_bytes": text_size(llvm_size, object_path, root),
        }
    return result


def device_inventory(root: Path) -> list[dict[str, str]]:
    sdk = Path(os.environ.get("ANDROID_SDK_ROOT", r"C:\Android\Sdk"))
    adb = sdk / "platform-tools" / ("adb.exe" if os.name == "nt" else "adb")
    if not adb.is_file():
        return []
    lines = run([str(adb), "devices", "-l"], root).splitlines()[1:]
    devices = []
    for line in lines:
        columns = line.split()
        if len(columns) < 2 or columns[1] != "device":
            continue
        serial = columns[0]
        abi = run([str(adb), "-s", serial, "shell", "getprop", "ro.product.cpu.abi"], root).strip()
        model = next((part.removeprefix("model:") for part in columns if part.startswith("model:")), "unknown")
        devices.append({"abi": abi, "model": model, "serial": serial})
    return devices


def distribution(samples: list[int]) -> dict[str, Any]:
    median = statistics.median(samples)
    return {
        "mad_us": statistics.median(abs(value - median) for value in samples),
        "max_us": max(samples),
        "median_us": median,
        "min_us": min(samples),
        "raw_us": samples,
    }


def host_details(root: Path, ndk: Path, clang: Path) -> dict[str, Any]:
    details: dict[str, Any] = {
        "cargo": run(["cargo", "--version"], root).strip(),
        "cpu": platform.processor() or os.environ.get("PROCESSOR_IDENTIFIER", "unknown"),
        "git_dirty": bool(run(["git", "status", "--porcelain"], root).strip()),
        "git_revision": run(["git", "rev-parse", "HEAD"], root).strip(),
        "machine": platform.machine(),
        "ndk": str(ndk),
        "ndk_clang": run([str(clang), "--version"], root).splitlines()[0],
        "os": platform.platform(),
        "python": platform.python_version(),
        "rustc": run(["rustc", "--version"], root).strip(),
    }
    if os.name == "nt":
        query = (
            "$cpu=(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name);"
            "$system=Get-CimInstance Win32_ComputerSystem;"
            "[pscustomobject]@{cpu=$cpu;manufacturer=$system.Manufacturer;"
            "model=$system.Model;memory_bytes=[uint64]$system.TotalPhysicalMemory}"
            " | ConvertTo-Json -Compress"
        )
        try:
            details.update(json.loads(run(["powershell", "-NoProfile", "-Command", query], root)))
        except (OSError, subprocess.CalledProcessError, json.JSONDecodeError):
            pass
    return details


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True, help="new directory for generated evidence")
    parser.add_argument("--snapshot", type=Path, help="optional checked-in JSON snapshot path")
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.warmups < 0 or args.repetitions < 1:
        parser.error("warmups must be >= 0 and repetitions must be >= 1")

    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if output.exists():
        raise RuntimeError(f"output directory already exists: {output}")
    output.mkdir(parents=True)

    ndk = find_ndk()
    objdump = llvm_tool(ndk, "llvm-objdump")
    clang = llvm_tool(ndk, "clang")
    llvm_size = llvm_tool(ndk, "llvm-size")

    env = os.environ.copy()
    env["STASIS_SIMD_EVIDENCE_DIR"] = str(output)
    env["STASIS_SIMD_COMPILE_WARMUPS"] = str(args.warmups)
    env["STASIS_SIMD_COMPILE_REPETITIONS"] = str(args.repetitions)
    test_command = [
        sys.executable,
        "tools/cargo_cache.py",
        "run",
        "--",
        "cargo",
        "test",
        "-p",
        "stasis_compiler",
        "--lib",
        "simd_soa_characterization_emits_scalar_arm64_evidence",
        "--",
        "--nocapture",
        "--test-threads=1",
    ]
    test_output = run(test_command, root, env)
    (output / "cargo-test-output.txt").write_text(test_output, encoding="utf-8")

    manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
    for case in manifest["cases"]:
        object_path = output / case["tick_object"]
        disassembly = run([str(objdump), "--disassemble", "--no-show-raw-insn", str(object_path)], root)
        disassembly_path = output / case["case"] / "tick.arm64.disasm.txt"
        disassembly_path.write_text(disassembly, encoding="utf-8")
        case["arm64"] = disassembly_metrics(disassembly)
        case["compile_distribution"] = distribution(case.pop("aot_compile_total_us"))
        if case["arm64"]["packed_lane_instruction_count"] != 0:
            raise RuntimeError(f"production case unexpectedly contains packed-lane work: {case['case']}")

    devices = device_inventory(root)
    manifest["arm64_runtime"] = {
        "available": any(device["abi"] in {"arm64-v8a", "aarch64"} for device in devices),
        "devices": devices,
        "timing_claim": "none; static evidence only" if not any(
            device["abi"] in {"arm64-v8a", "aarch64"} for device in devices
        ) else "device present; this script still does not execute the objects",
    }
    manifest["explicit_simd_prototype"] = compile_prototype(output, clang, objdump, llvm_size, root)
    manifest["host"] = host_details(root, ndk, clang)
    manifest["reproduce"] = {
        "command": f"python tools/measure_simd_soa.py --output <new-dir> --warmups {args.warmups} --repetitions {args.repetitions}",
        "test_command": test_command,
    }
    serialized = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    (output / "analysis.json").write_text(serialized, encoding="utf-8")
    if args.snapshot:
        snapshot = args.snapshot if args.snapshot.is_absolute() else root / args.snapshot
        snapshot.parent.mkdir(parents=True, exist_ok=True)
        snapshot.write_text(serialized, encoding="utf-8")
    print(output / "analysis.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

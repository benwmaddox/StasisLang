#!/usr/bin/env python3
"""Build a fresh native client ABI probe and run it on an Android device."""

import argparse
import os
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[2]


def run(command, *, env=None, capture=False, timeout=900):
    return subprocess.run(
        [str(value) for value in command], cwd=ROOT, env=env,
        check=True, text=True, capture_output=capture, timeout=timeout,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--ndk", type=Path, required=True)
    parser.add_argument("--adb", default="adb")
    args = parser.parse_args()
    adb = [args.adb, "-s", args.serial]
    abi = run(adb + ["shell", "getprop", "ro.product.cpu.abi"], capture=True, timeout=30).stdout.strip()
    targets = {"arm64-v8a": "aarch64-linux-android", "x86_64": "x86_64-linux-android"}
    if abi not in targets:
        raise SystemExit("native client probe requires arm64-v8a or x86_64 Android")
    target = targets[abi]
    platform = "windows-x86_64" if os.name == "nt" else "linux-x86_64"
    toolchain = args.ndk.resolve() / "toolchains" / "llvm" / "prebuilt" / platform
    compiler = toolchain / "bin" / (target + "26-clang" + (".cmd" if os.name == "nt" else ""))
    clang = toolchain / "bin" / ("clang.exe" if os.name == "nt" else "clang")
    if not compiler.is_file() or not clang.is_file():
        raise SystemExit("Android NDK compiler is unavailable")
    env = dict(os.environ)
    # Keep generated artifacts inside the selected worktree.
    cargo_target = ROOT / "target" / "android-network-client"
    env["CARGO_TARGET_DIR"] = str(cargo_target)
    env["CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"] = str(compiler)
    run([sys.executable, "tools/cargo_cache.py", "run", "--", "cargo", "build",
         "-p", "stasis_network", "--release", "--target", target], env=env)
    executable = cargo_target / "native_client_probe"
    library = cargo_target / target / "release" / "libstasis_network.a"
    run([clang, "--target=" + target + "26", "--sysroot=" + str(toolchain / "sysroot"),
         "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-Wall", "-Wextra", "-Werror",
         "-I", ROOT / "crates/stasis_network/include",
         ROOT / "runtime/tests/stasis_network_client_link_test.c", library,
         "-ldl", "-llog", "-lm", "-o", executable], env=env)
    bridge = cargo_target / "native_client_bridge_probe"
    run([clang, "--target=" + target + "26", "--sysroot=" + str(toolchain / "sysroot"),
         "-std=c11", "-D_POSIX_C_SOURCE=200809L", "-DSTASIS_NETWORK_CLIENT_ENABLED=1",
         "-I", ROOT / "crates/stasis_network/include", "-I", ROOT / "runtime",
         ROOT / "runtime/tests/stasis_mobile_aot_runtime_test.c",
         ROOT / "runtime/stasis_mobile_aot_runtime.c",
         ROOT / "runtime/stasis_platform_services.c", "-lm", "-o", bridge], env=env)
    remote = "/data/local/tmp/stasis_native_client_probe"
    evidence = "Android ABI: " + abi + "\n"
    try:
        for probe, expected in (
            (executable, "background resume passed"),
            (bridge, "stasis_mobile_aot_runtime_test: ok"),
        ):
            run(adb + ["push", probe, remote], timeout=30)
            run(adb + ["shell", "chmod", "700", remote], timeout=30)
            result = run(adb + ["shell", remote], capture=True, timeout=90)
            print(result.stdout, end="")
            if expected not in result.stdout:
                raise SystemExit("Android native client probe did not report acceptance")
            evidence += result.stdout
        (cargo_target / "result.txt").write_text(
            evidence, encoding="utf-8")
    finally:
        subprocess.run(adb + ["shell", "rm", "-f", remote], check=False, timeout=30)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Run the backend-neutral local fixed-array source and packaged-Web oracle."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


EXPECTED = 146


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def json_result(output: str) -> dict[str, object]:
    for line in reversed(output.splitlines()):
        if line.startswith("{"):
            return json.loads(line)
    raise RuntimeError(f"command produced no JSON result: {output}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stasis", required=True, type=Path)
    parser.add_argument(
        "--workspace",
        type=Path,
        default=Path(__file__).resolve().parents[2]
        / "tests"
        / "fixtures"
        / "local_fixed_arrays",
    )
    args = parser.parse_args()

    repository = Path(__file__).resolve().parents[2]
    workspace = args.workspace.resolve()
    stasis = args.stasis.resolve()
    if not stasis.is_file():
        raise FileNotFoundError(f"Stasis CLI not found: {stasis}")

    test = json_result(
        run(
            [str(stasis), "--workspace", str(workspace), "test", "--json"],
            repository,
        ).stdout
    )
    if not test.get("ok") or test.get("result", {}).get("tests_passed") != 1:
        raise RuntimeError(f"pure Stasis test failed: {test}")

    with tempfile.TemporaryDirectory(prefix="stasis-local-fixed-arrays-") as temporary:
        temporary_root = Path(temporary)
        temporary_workspace = temporary_root / "workspace"
        shutil.copytree(workspace, temporary_workspace)
        output_name = Path("package")
        output = temporary_workspace / output_name
        package = json_result(
            run(
                [
                    str(stasis),
                    "--workspace",
                    str(temporary_workspace),
                    "package",
                    "--target",
                    "web",
                    "--development-build",
                    "--out",
                    str(output_name),
                    "--json",
                ],
                repository,
            ).stdout
        )
        if not package.get("ok"):
            raise RuntimeError(f"Web package failed: {package}")
        packaged_output = Path(str(package["result"]["output"]))
        if packaged_output.resolve() != output.resolve():
            raise RuntimeError(
                f"package escaped requested temporary output: {packaged_output}"
            )
        wasm = packaged_output / str(package["result"]["wasm"])
        oracle = (
            "const fs=require('node:fs');"
            "WebAssembly.instantiate(fs.readFileSync(process.argv[1]),{}).then(({instance})=>{"
            "const e=instance.exports;const trap=i=>{try{e.render(i);return false}catch(x){"
            "if(!(x instanceof WebAssembly.RuntimeError))throw x;return true}};"
            "const actual=[e.main(),e.tick(),trap(-1),trap(9)];"
            "process.stdout.write(JSON.stringify(actual));"
            f"if(JSON.stringify(actual)!==JSON.stringify([{EXPECTED},{EXPECTED},true,true]))process.exit(2)"
            "}).catch(error=>{console.error(error);process.exit(1)})"
        )
        node = run(["node", "-e", oracle, str(wasm)], repository)
        actual = json.loads(node.stdout)
        print(
            json.dumps(
                {
                    "ok": True,
                    "pure_tests_passed": 1,
                    "packaged_wasm": str(wasm),
                    "actual": actual,
                    "expected": [EXPECTED, EXPECTED, True, True],
                },
                sort_keys=True,
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

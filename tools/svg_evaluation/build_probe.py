"""Build the real SVG bridge probe without SDL or Cargo dependencies."""
import argparse
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--build", default="output/svg-evaluation-build")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    # MSBuild rejects inherited Path/PATH duplicates on some Windows workers.
    env = {k.upper(): v for k, v in os.environ.items()} if os.name == "nt" else dict(os.environ)
    commands = [
        ["cmake", "-S", "runtime", "-B", args.build,
         "-DSTASIS_GRAPHICS_BUILD_SHARED=OFF", "-DSTASIS_GRAPHICS_BUILD_STATIC=OFF",
         "-DSTASIS_BUILD_RUNNER=OFF", "-DSTASIS_BUILD_SYS=OFF", "-DSTASIS_SVG_BUILD_TESTS=ON",
         "-DCMAKE_BUILD_TYPE=Release"],
        ["cmake", "--build", args.build, "--config", "Release", "--parallel", "4"],
        ["ctest", "--test-dir", args.build, "-C", "Release", "--output-on-failure", "--timeout", "120"],
    ]
    for command in commands:
        subprocess.run(command, cwd=root, env=env, check=True, timeout=900)


if __name__ == "__main__":
    main()

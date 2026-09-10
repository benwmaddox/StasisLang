"""Resolve the native desktop network archive directory for a CI runner."""

import argparse


def network_target(system: str, architecture: str) -> str:
    systems = {"Linux": "linux", "macOS": "macos"}
    architectures = {"X64": "x86_64", "ARM64": "arm64"}
    if system not in systems or architecture not in architectures:
        raise ValueError("unsupported native desktop network runner")
    return f"{systems[system]}-{architectures[architecture]}"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("system")
    parser.add_argument("architecture")
    args = parser.parse_args()
    try:
        print(network_target(args.system, args.architecture))
    except ValueError as error:
        parser.error(str(error))

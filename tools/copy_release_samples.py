"""Copy the release sample set without its largest sample."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path


OMITTED_SAMPLE = "brickout_defense"
RELEASE_README_NAME = "README.release.md"
RELEASE_README = """# Release samples

The full `brickout_defense` sample is omitted from this release archive to keep
the bundled sample set compact. Its complete source is available in the
[StasisLang repository](https://github.com/benwmaddox/StasisLang/tree/main/samples/brickout_defense).
"""


def copy_release_samples(source: Path | str, destination: Path | str) -> None:
    source_path = Path(source)
    if source_path.is_symlink() or not source_path.is_dir():
        raise ValueError(f"source must be a real directory: {source_path}")

    destination_path = Path(destination)
    if destination_path.exists() or destination_path.is_symlink():
        raise FileExistsError(f"destination already exists: {destination_path}")

    if (source_path / RELEASE_README_NAME).exists() or (
        source_path / RELEASE_README_NAME
    ).is_symlink():
        raise ValueError(f"source already contains reserved {RELEASE_README_NAME}")

    source_root = source_path.resolve()
    try:
        destination_path.resolve().relative_to(source_root)
    except ValueError:
        pass
    else:
        raise ValueError("destination must be outside the source directory")

    def omit_heavy_sample(directory: str, names: list[str]) -> list[str]:
        if Path(directory).resolve() == source_root and OMITTED_SAMPLE in names:
            return [OMITTED_SAMPLE]
        return []

    destination_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(
        source_path,
        destination_path,
        symlinks=True,
        ignore=omit_heavy_sample,
    )
    (destination_path / RELEASE_README_NAME).write_bytes(RELEASE_README.encode("utf-8"))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True, help="source samples directory")
    parser.add_argument(
        "--destination", type=Path, required=True, help="new release samples directory"
    )
    args = parser.parse_args(argv)
    try:
        copy_release_samples(args.source, args.destination)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

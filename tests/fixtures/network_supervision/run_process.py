"""Run one acceptance child with a case-normalized Windows environment."""

from __future__ import annotations

import os
import subprocess
import sys


def main() -> int:
    if len(sys.argv) < 3:
        return 2
    return subprocess.run(sys.argv[2:], cwd=sys.argv[1], env=dict(os.environ)).returncode


if __name__ == "__main__":
    raise SystemExit(main())

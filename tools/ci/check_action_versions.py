"""Audit reviewed action pins and runtime-sensitive inputs without dependencies."""

import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
PINS = json.loads((ROOT / "tools/ci/action_versions.json").read_text())


def validate(text: str) -> list[str]:
    errors = []
    for match in re.finditer(r"(?m)^\s*(?:- )?uses:\s*(\S+)([^\n]*)", text):
        ref, comment = match.groups()
        if ref.startswith("./.github/workflows/"):
            continue
        action, _, sha = ref.partition("@")
        pin = PINS.get(action)
        if not pin or sha != pin["sha"] or comment.strip() != "# " + pin["version"]:
            errors.append(f"Unreviewed action: {ref}")
            continue
        # Inputs belong to this step, up to the next step or job.
        tail = text[match.end():]
        block = re.split(r"\n\s*- (?:name|uses):|\n  [\w-]+:", tail, maxsplit=1)[0]
        if action == "actions/setup-node":
            for required in ('node-version: "24"', 'package-manager-cache: false'):
                if required not in block:
                    errors.append(f"{action} requires {required}")
        if action == "actions/upload-artifact" and "archive: true" not in block:
            errors.append("Artifact uploads must preserve named zip archives")
    for bypass in ("ACTIONS_ALLOW_USE_UNSECURE_NODE_VERSION", "ACTIONS_RUNNER_FORCE_ACTIONS_NODE_VERSION", "NODE_NO_WARNINGS"):
        if bypass in text:
            errors.append(f"Runtime warning bypass: {bypass}")
    return errors


def main() -> None:
    errors = []
    for path in sorted((ROOT / ".github/workflows").glob("*.y*ml")):
        errors.extend(f"{path.name}: {error}" for error in validate(path.read_text()))
    if errors:
        raise SystemExit("\n".join(errors))
    print("GitHub Actions version policy passed")


if __name__ == "__main__":
    main()

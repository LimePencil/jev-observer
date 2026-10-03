#!/usr/bin/env python3
"""Validate release metadata before any native build or publication."""
import argparse
import json
import os
from pathlib import Path
import re
import tomllib


def metadata(root, ref=""):
    package = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    version = package["version"]
    match = re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", version)
    if not match:
        raise ValueError("Cargo version must be a semantic version")
    if match[4] and any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in match[4].split(".")):
        raise ValueError("Numeric prerelease identifiers must not have leading zeros")
    tag = "v" + version
    if ref.startswith("refs/tags/") and ref != "refs/tags/" + tag:
        raise ValueError(f"Release tag must be {tag}")
    locked = tomllib.loads((root / "Cargo.lock").read_text())["package"]
    if [entry["version"] for entry in locked if entry["name"] == package["name"]] != [version]:
        raise ValueError("Cargo.lock package version must match Cargo.toml")
    ui = json.loads((root / "ui/package.json").read_text())
    lock = json.loads((root / "ui/package-lock.json").read_text())
    if any(found != version for found in (ui["version"], lock["version"], lock["packages"][""]["version"])):
        raise ValueError("UI manifest and lockfile versions must match Cargo.toml")
    notes = root / "docs/releases" / (version + ".md")
    if not notes.is_file() or not notes.read_text().strip():
        raise ValueError(f"Nonempty release notes are required at docs/releases/{version}.md")
    prerelease = match[4] is not None
    return {"tag": tag, "prerelease": str(prerelease).lower(), "latest": str(not prerelease).lower()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--ref", default=os.environ.get("GITHUB_REF", ""))
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    try:
        result = metadata(args.root, args.ref)
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Release preflight failed: {error}\n")
    if args.github_output:
        with args.github_output.open("a") as output:
            for key, value in result.items():
                output.write(f"{key}={value}\n")
    print(json.dumps(result))


if __name__ == "__main__":
    main()

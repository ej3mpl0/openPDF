#!/usr/bin/env python3
"""Fail if packaging/flatpak/cargo-sources.json is missing any Cargo.lock crate."""

from __future__ import annotations

import json
import sys
from pathlib import Path


def lock_registry_packages(lock_text: str) -> list[tuple[str, str, str]]:
    packages: list[tuple[str, str, str]] = []
    name = version = checksum = None
    for raw in lock_text.splitlines():
        line = raw.strip()
        if line == "[[package]]":
            if name and version and checksum:
                packages.append((name, version, checksum))
            name = version = checksum = None
            continue
        if line.startswith("name = "):
            name = line.split("=", 1)[1].strip().strip('"')
        elif line.startswith("version = "):
            version = line.split("=", 1)[1].strip().strip('"')
        elif line.startswith("checksum = "):
            checksum = line.split("=", 1)[1].strip().strip('"')
    if name and version and checksum:
        packages.append((name, version, checksum))
    return packages


def main() -> int:
    root = Path(__file__).resolve().parents[2]
    lock_path = root / "Cargo.lock"
    sources_path = root / "packaging" / "flatpak" / "cargo-sources.json"
    packages = lock_registry_packages(lock_path.read_text(encoding="utf-8"))
    sources = json.loads(sources_path.read_text(encoding="utf-8"))
    have = {entry["sha256"] for entry in sources if isinstance(entry, dict) and "sha256" in entry}
    missing = [(name, version, sha) for name, version, sha in packages if sha not in have]
    if missing:
        print("cargo-sources.json is out of sync with Cargo.lock", file=sys.stderr)
        print("Missing crates (run packaging/flatpak/generate-cargo-sources.sh):", file=sys.stderr)
        for name, version, sha in missing:
            print(f"  {name} {version} {sha}", file=sys.stderr)
        return 1
    print(f"cargo-sources.json covers {len(packages)} crates from Cargo.lock")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

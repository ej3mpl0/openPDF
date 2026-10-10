#!/usr/bin/env python3
"""Fail if any workspace member does not inherit [workspace.package] rust-version.

Usage: cargo metadata --no-deps --format-version 1 | python3 check-rust-version.py
"""
import json, sys, tomllib
d = json.load(sys.stdin)
with open(d["workspace_root"] + "/Cargo.toml", "rb") as f:
    want = tomllib.load(f).get("workspace", {}).get("package", {}).get("rust-version")
if not want:
    sys.exit("::error::[workspace.package] rust-version is not set")
bad = [f'  {p["name"]} ({p["manifest_path"]}): rust_version={p.get("rust_version")}'
       for p in d["packages"] if p.get("rust_version") != want]
if bad:
    print(f"::error::workspace members without rust-version = {want} (add rust-version.workspace = true):")
    print("\n".join(bad))
    sys.exit(1)
print(f"all {len(d['packages'])} workspace members have rust-version {want}")

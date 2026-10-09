#!/usr/bin/env bash
# Generate packaging/flatpak/cargo-sources.json from Cargo.lock.
# CI runs packaging/flatpak/check-cargo-sources.py and fails if this file is stale.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/packaging/flatpak/cargo-sources.json"
CACHE="$ROOT/packaging/flatpak/.cache"
GENERATOR="$CACHE/flatpak-cargo-generator.py"
GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"

mkdir -p "$CACHE"
if [[ ! -f "$GENERATOR" ]]; then
  curl -fsSL -o "$GENERATOR" "$GENERATOR_URL"
fi

if ! python3 -c 'import aiohttp, tomlkit, yaml' 2>/dev/null; then
  python3 -m pip install --user --quiet 'aiohttp>=3.9.5' 'tomlkit>=0.13.3' 'PyYAML>=6.0.2'
fi

python3 "$GENERATOR" "$ROOT/Cargo.lock" -o "$OUT"
echo "wrote $OUT"

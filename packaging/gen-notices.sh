#!/usr/bin/env bash
# Generate THIRD-PARTY-NOTICES: header + Rust crates (cargo-about, pinned) +
# packaging/third-party-non-rust.md (PDFium, OpenSC, Adwaita).
# Usage: packaging/gen-notices.sh [output]   (default: $ROOT/THIRD-PARTY-NOTICES)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=config.sh
source "$ROOT/packaging/config.sh"
out="${1:-$ROOT/$NOTICES_FILE}"
have="$(cargo about --version 2>/dev/null || true)"
if [[ "$have" != "cargo-about $CARGO_ABOUT_VERSION" ]]; then
  echo "gen-notices: need cargo-about $CARGO_ABOUT_VERSION (have: ${have:-none});" \
    "run .github/scripts/install-cargo-tool.sh cargo-about" >&2
  exit 1
fi
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
(cd "$ROOT" && cargo about generate --locked --fail \
  -c about.toml packaging/notices/about.hbs -o "$tmp")
ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
{
  echo "openPDF ${ver} - third-party notices"
  echo
  echo "openPDF itself is licensed under the Mozilla Public License 2.0 (MPL-2.0)."
  echo "It includes or loads the third-party components listed below, under"
  echo "their own licenses."
  echo
  cat "$tmp"
  echo
  cat "$ROOT/packaging/third-party-non-rust.md"
} > "$out"
for must in "Part 1: Rust crates" "Part 2: Non-Rust components" "PDFium" "OpenSC" "Adwaita"; do
  grep -q "$must" "$out" || { echo "gen-notices: '$must' missing from $out" >&2; exit 1; }
done
echo "gen-notices: wrote $out ($(wc -l < "$out") lines)"

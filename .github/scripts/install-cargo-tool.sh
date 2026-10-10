#!/usr/bin/env bash
# Install a pinned cargo tool: install-cargo-tool.sh cargo-about|cargo-deny
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../../packaging/config.sh
source "$ROOT/packaging/config.sh"
case "$1" in
  cargo-about) ver="$CARGO_ABOUT_VERSION"; extra=(--features cli) ;;
  cargo-deny)  ver="$CARGO_DENY_VERSION";  extra=() ;;
  *) echo "unknown tool $1" >&2; exit 2 ;;
esac
sub="${1#cargo-}"
if have="$(cargo "$sub" --version 2>/dev/null)" && [[ "$have" == "$1 $ver" ]]; then
  echo "$have already installed"
  exit 0
fi
cargo install --locked --version "$ver" "${extra[@]}" "$1"
cargo "$sub" --version

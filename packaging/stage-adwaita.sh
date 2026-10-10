#!/usr/bin/env bash
# Stage the runtime part of an extracted adwaita-icon-theme release tarball
# (pinned in config.sh) the way its meson.build installs it:
#   <src>/index.theme        -> <dest>/index.theme
#   <src>/Adwaita/{scalable,symbolic,16x16,...} -> <dest>/  (cursors excluded)
# Usage: packaging/stage-adwaita.sh <extracted-src-dir> <dest Adwaita dir>
set -euo pipefail
[[ $# -eq 2 ]] || { echo "usage: $0 <src> <dest>" >&2; exit 2; }
src="$1"; dest="$2"
for p in "$src/index.theme" "$src/Adwaita/scalable" "$src/Adwaita/symbolic"; do
  [[ -e "$p" ]] || { echo "stage-adwaita: unexpected tarball layout, missing $p" >&2; exit 1; }
done
rm -rf "$dest"
mkdir -p "$dest"
cp -a "$src/index.theme" "$dest/"
for d in "$src/Adwaita"/*/; do
  [[ "$(basename "$d")" == cursors ]] && continue
  cp -a "$d" "$dest/"
done

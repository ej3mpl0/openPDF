#!/usr/bin/env bash
# Fail if any icon name used by the GTK app is not provided by the given icon
# theme roots (e.g. AppDir/usr/share/icons) or by the app's own GResource icons.
# Usage: packaging/check-icons.sh <icons-root> [<icons-root>...]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ $# -ge 1 ]] || { echo "usage: $0 <icons-root>..." >&2; exit 2; }

src=()
for d in "$ROOT/crates/openpdf-gtk" "$ROOT/crates/openpdf-app"; do
  [[ -d "$d" ]] && src+=("$d")
done
if [[ ${#src[@]} -eq 0 ]]; then
  echo "check-icons: no GTK crates in this checkout; nothing to check"
  exit 0
fi

# Literal names passed to from_icon_name / set_icon_name / icon_name / icon-name
# in Rust, .ui and .blp files.
mapfile -t names < <(
  grep -rhoE --include='*.rs' --include='*.ui' --include='*.blp' \
    '(icon_name|icon-name)[^"]{0,40}"[A-Za-z0-9_.-]+"' "${src[@]}" \
    | grep -oE '"[A-Za-z0-9_.-]+"$' | tr -d '"' | sort -u
)
if [[ ${#names[@]} -eq 0 ]]; then
  echo "check-icons: no icon names found"
  exit 0
fi

missing=0
for n in "${names[@]}"; do
  found=""
  for r in "$@" "$ROOT/crates/openpdf-gtk/data/icons"; do
    [[ -d "$r" ]] || continue
    if find -L "$r" \( -name "$n.svg" -o -name "$n.png" \) -print -quit | grep -q .; then
      found=1; break
    fi
  done
  if [[ -z "$found" ]]; then
    echo "MISSING icon: $n" >&2
    missing=1
  else
    echo "ok: $n"
  fi
done
exit "$missing"

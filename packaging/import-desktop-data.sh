#!/usr/bin/env bash
# Import data/*.desktop and data/*.metainfo.xml from the gtk branch when
# this checkout does not have them yet (PR #1 not merged).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=config.sh
source "$ROOT/packaging/config.sh"

if [[ -f "$ROOT/$DESKTOP_FILE" && -f "$ROOT/$METAINFO_FILE" ]]; then
  exit 0
fi

echo "data/ desktop metadata missing; importing from origin/gtk (PR #1)" >&2
git -C "$ROOT" fetch --depth=1 origin gtk
git -C "$ROOT" checkout FETCH_HEAD -- data
if [[ ! -f "$ROOT/$DESKTOP_FILE" || ! -f "$ROOT/$METAINFO_FILE" ]]; then
  echo "expected $DESKTOP_FILE and $METAINFO_FILE after importing gtk data/" >&2
  exit 1
fi

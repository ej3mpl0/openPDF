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

# A release must ship only reviewed files from the tagged ref itself.
if [[ "${GITHUB_REF:-}" == refs/tags/* || "${RELEASE:-0}" == "1" ]]; then
  echo "data/ desktop metadata ($DESKTOP_FILE, $METAINFO_FILE) is not on the" \
    "checked-out ref (${GITHUB_REF:-RELEASE=1}); refusing to import it from gtk for a release" >&2
  exit 1
fi

echo "data/ desktop metadata missing; importing from origin/gtk (PR #1)" >&2
# Prefer the ref already fetched by actions/checkout (fetch-depth: 0), so CI
# works with persist-credentials: false; fall back to a fetch locally.
if git -C "$ROOT" rev-parse --verify -q refs/remotes/origin/gtk >/dev/null; then
  src=refs/remotes/origin/gtk
else
  git -C "$ROOT" fetch --depth=1 origin gtk
  src=FETCH_HEAD
fi
git -C "$ROOT" checkout "$src" -- data
if [[ ! -f "$ROOT/$DESKTOP_FILE" || ! -f "$ROOT/$METAINFO_FILE" ]]; then
  echo "expected $DESKTOP_FILE and $METAINFO_FILE after importing gtk data/" >&2
  exit 1
fi

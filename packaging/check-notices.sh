#!/usr/bin/env bash
# Fail unless a built package contains a non-empty THIRD-PARTY-NOTICES.
# Usage: packaging/check-notices.sh appimage <file.AppImage>
#        packaging/check-notices.sh flatpak  <ostree-repo> [app-id]
#        packaging/check-notices.sh zip|tar  <archive>   (file at archive root dir)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=config.sh
source "$ROOT/packaging/config.sh"
kind="$1"; pkg="$2"
ok() { echo "check-notices: OK $kind $pkg -> $1"; }
die() { echo "check-notices: FAIL $kind $pkg: $*" >&2; exit 1; }
case "$kind" in
  appimage)
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    (cd "$tmp" && "$(realpath "$pkg")" --appimage-extract "usr/$NOTICES_REL" >/dev/null) \
      || die "--appimage-extract failed"
    [[ -s "$tmp/squashfs-root/usr/$NOTICES_REL" ]] || die "usr/$NOTICES_REL missing or empty"
    grep -q "Part 2: Non-Rust components" "$tmp/squashfs-root/usr/$NOTICES_REL" || die "incomplete notices"
    ok "usr/$NOTICES_REL" ;;
  flatpak)
    id="${3:-$APP_ID}"
    ref="$(ostree --repo="$pkg" refs | grep -E "^app/${id}/" | head -n1)"
    [[ -n "$ref" ]] || die "no app/${id}/ ref in repo"
    size="$(ostree --repo="$pkg" ls "$ref" "/files/$NOTICES_REL" | awk '{print $4}')" \
      || die "/app/$NOTICES_REL missing in $ref"
    [[ "${size:-0}" -gt 0 ]] || die "/app/$NOTICES_REL empty"
    ok "/app/$NOTICES_REL ($ref, $size bytes)" ;;
  zip)
    python3 - "$pkg" "$NOTICES_FILE" <<'PY' || die "no non-empty <dir>/$NOTICES_FILE in zip"
import sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
hits = [i for i in z.infolist() if i.filename.count("/") == 1
        and i.filename.endswith("/" + sys.argv[2]) and i.file_size > 0]
sys.exit(0 if hits else 1)
PY
    ok "<root>/$NOTICES_FILE" ;;
  tar)
    python3 - "$pkg" "$NOTICES_FILE" <<'PY' || die "no non-empty <dir>/$NOTICES_FILE in tar"
import sys, tarfile
t = tarfile.open(sys.argv[1])
hits = [m for m in t.getmembers() if m.isfile() and m.size > 0
        and m.name.lstrip("./").count("/") == 1
        and m.name.endswith("/" + sys.argv[2])]
sys.exit(0 if hits else 1)
PY
    ok "<root>/$NOTICES_FILE" ;;
  *) die "unknown kind" ;;
esac

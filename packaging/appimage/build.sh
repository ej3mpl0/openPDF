#!/usr/bin/env bash
# Build an x86_64 AppImage: GUI + CLI + libpdfium + GTK4/libadwaita.
# The CLI (CLI_BIN) is a separate binary and must not load GTK.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../config.sh
source "$ROOT/packaging/config.sh"

if [[ "$(uname -m)" != "x86_64" ]]; then
  echo "AppImage packaging is pinned to pdfium-linux-x64 (same as CI)" >&2
  exit 1
fi

"$ROOT/packaging/import-desktop-data.sh"

ARCH="x86_64"
BUILD_DIR="${OPENPDF_APPIMAGE_BUILD_DIR:-$ROOT/packaging/appimage/build}"
APPDIR="$BUILD_DIR/AppDir"
DIST_DIR="${OPENPDF_APPIMAGE_DIST_DIR:-$ROOT/packaging/appimage/dist}"
TOOLS_DIR="${OPENPDF_APPIMAGE_TOOLS_DIR:-$BUILD_DIR/tools}"
PDFIUM_DIR="$BUILD_DIR/pdfium"

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${ARCH}.AppImage"
GTK_PLUGIN_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"

export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH

mkdir -p "$BUILD_DIR" "$DIST_DIR" "$TOOLS_DIR" "$PDFIUM_DIR"

download() {
  local url="$1" dest="$2"
  if [[ ! -f "$dest" ]]; then
    curl -fsSL -o "$dest" "$url"
  fi
}

download "$LINUXDEPLOY_URL" "$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage"
download "$GTK_PLUGIN_URL" "$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"
chmod +x "$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage" "$TOOLS_DIR/linuxdeploy-plugin-gtk.sh"

if [[ ! -f "$PDFIUM_DIR/lib/libpdfium.so" ]]; then
  archive="$BUILD_DIR/$PDFIUM_ARCHIVE"
  download "$PDFIUM_URL" "$archive"
  got="$(sha256sum "$archive" | awk '{print $1}')"
  if [[ "$got" != "$PDFIUM_SHA256" ]]; then
    echo "PDFium checksum mismatch: expected $PDFIUM_SHA256 got $got" >&2
    exit 1
  fi
  tar -xzf "$archive" -C "$PDFIUM_DIR"
fi

cargo build --release -p "$APP_BIN" -p "$CLI_CRATE" --manifest-path "$ROOT/Cargo.toml"
BIN_PATH="$ROOT/target/release/$APP_BIN"
CLI_PATH="$ROOT/target/release/$CLI_BIN"
if [[ ! -x "$BIN_PATH" ]]; then
  echo "release GUI binary not found: $BIN_PATH" >&2
  exit 1
fi
if [[ ! -x "$CLI_PATH" ]]; then
  echo "release CLI binary not found: $CLI_PATH" >&2
  exit 1
fi
if ldd "$CLI_PATH" | grep -E 'libgtk|libadwaita'; then
  echo "${CLI_BIN} must not link GTK/libadwaita" >&2
  exit 1
fi

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo" "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
  "$APPDIR/apprun-hooks"

install -Dm0755 "$BIN_PATH" "$APPDIR/usr/bin/$APP_BIN"
install -Dm0755 "$CLI_PATH" "$APPDIR/usr/bin/$CLI_BIN"
install -Dm0755 "$PDFIUM_DIR/lib/libpdfium.so" "$APPDIR/usr/lib/libpdfium.so"
install -Dm0644 "$ROOT/$DESKTOP_FILE" "$APPDIR/usr/share/applications/${APP_ID}.desktop"
install -Dm0644 "$ROOT/$METAINFO_FILE" "$APPDIR/usr/share/metainfo/${APP_ID}.metainfo.xml"

icon_src=""
for candidate in \
  "$ROOT/data/icons/hicolor/scalable/apps/${ICON_NAME}.svg" \
  "$ROOT/data/icons/hicolor/scalable/apps/${APP_ID}.svg" \
  "$ROOT/data/${ICON_NAME}.svg" \
  "$ROOT/packaging/icons/${ICON_NAME}.svg"
do
  if [[ -f "$candidate" ]]; then
    icon_src="$candidate"
    break
  fi
done
if [[ -z "$icon_src" ]]; then
  echo "no scalable SVG icon found under data/ or packaging/icons/" >&2
  exit 1
fi
install -Dm0644 "$icon_src" "$APPDIR/usr/share/icons/hicolor/scalable/apps/${ICON_NAME}.svg"
install -Dm0644 "$icon_src" "$APPDIR/usr/share/icons/hicolor/scalable/apps/${APP_ID}.svg"

cat > "$APPDIR/apprun-hooks/openpdf-pdfium.sh" <<'EOF'
# Bundled PDFium; same search order as openpdf-core honors OPENPDF_PDFIUM first.
export OPENPDF_PDFIUM="${APPDIR}/usr/lib/libpdfium.so"
EOF

# Force GTK4 even when this checkout's binary is still a stub (gtk PR not merged).
export DEPLOY_GTK_VERSION=4
export LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage"
export PATH="$TOOLS_DIR:$PATH"
export OUTPUT="${DIST_DIR}/${APP_NAME}-${ARCH}.AppImage"
export VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)}"

extra_libs=()
for spec in gtk4 libadwaita-1; do
  if ! pkg-config --exists "$spec"; then
    echo "pkg-config module $spec is required to bundle GTK4/libadwaita" >&2
    exit 1
  fi
  libdir="$(pkg-config --variable=libdir "$spec")"
  case "$spec" in
    gtk4) pattern="libgtk-4.so*" ;;
    libadwaita-1) pattern="libadwaita-1.so*" ;;
  esac
  while IFS= read -r -d '' so; do
    extra_libs+=(--library="$so")
  done < <(find "$libdir" -maxdepth 1 \( -type f -o -type l \) -name "$pattern" -print0)
done

"$LINUXDEPLOY" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/$APP_BIN" \
  --executable "$APPDIR/usr/bin/$CLI_BIN" \
  --desktop-file "$APPDIR/usr/share/applications/${APP_ID}.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/scalable/apps/${ICON_NAME}.svg" \
  "${extra_libs[@]}" \
  --plugin gtk \
  --output appimage

echo "AppImage: $OUTPUT"
ls -lh "$OUTPUT"

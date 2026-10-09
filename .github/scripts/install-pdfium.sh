#!/usr/bin/env bash
# Install bblanchon/pdfium-binaries (chromium/8035) for the current OS/arch.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=../../packaging/config.sh
source "$ROOT/packaging/config.sh"

DEST="${PDFIUM_PREFIX:-$HOME/pdfium}"
mkdir -p "$DEST"

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Linux)
    archive="pdfium-linux-x64.tgz"
    sha="$PDFIUM_SHA256_LINUX_X64"
    lib_name="libpdfium.so"
    ;;
  Darwin)
    if [[ "$arch" == "arm64" ]]; then
      archive="pdfium-mac-arm64.tgz"
      sha="$PDFIUM_SHA256_MAC_ARM64"
    else
      archive="pdfium-mac-x64.tgz"
      sha="$PDFIUM_SHA256_MAC_X64"
    fi
    lib_name="libpdfium.dylib"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    archive="pdfium-win-x64.tgz"
    sha="$PDFIUM_SHA256_WIN_X64"
    lib_name="pdfium.dll"
    ;;
  *)
    echo "unsupported OS: $os $arch" >&2
    exit 1
    ;;
esac

url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8035/${archive}"
tgz="$DEST/$archive"
curl -fsSL -o "$tgz" "$url"
if command -v sha256sum >/dev/null; then
  got="$(sha256sum "$tgz" | awk '{print $1}')"
else
  got="$(shasum -a 256 "$tgz" | awk '{print $1}')"
fi
if [[ "$got" != "$sha" ]]; then
  echo "PDFium checksum mismatch for $archive: expected $sha got $got" >&2
  exit 1
fi
tar -xzf "$tgz" -C "$DEST"

lib="$(find "$DEST" -name "$lib_name" \( -type f -o -type l \) | head -n1)"
if [[ -z "$lib" ]]; then
  echo "did not find $lib_name in $DEST" >&2
  find "$DEST" -maxdepth 3 -print >&2
  exit 1
fi

if [[ "$os" == "Linux" ]] && command -v sudo >/dev/null && [[ -w /usr/local/lib || -d /usr/local/lib ]]; then
  sudo cp "$lib" /usr/local/lib/libpdfium.so
  sudo ldconfig || true
  lib=/usr/local/lib/libpdfium.so
fi

echo "OPENPDF_PDFIUM=$lib"
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "OPENPDF_PDFIUM=$lib" >> "$GITHUB_ENV"
  echo "PDFIUM_LIB=$lib" >> "$GITHUB_ENV"
fi

#!/usr/bin/env bash
# Build openpdf-macos (SwiftPM) against libopenpdf_ffi.dylib. macOS only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build -p openpdf-ffi --release
LIB="$ROOT/target/release/libopenpdf_ffi.dylib"
if [[ ! -f "$LIB" ]]; then
  echo "libopenpdf_ffi.dylib missing; this script is macOS-only" >&2
  exit 1
fi

GEN="$ROOT/apps/macos/.generated"
mkdir -p "$GEN" \
  "$ROOT/apps/macos/Sources/OpenPDFApp/Generated" \
  "$ROOT/apps/macos/Sources/openpdfFFI/include" \
  "$ROOT/apps/macos/lib"

cargo run -p openpdf-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$LIB" --language swift --out-dir "$GEN"

cp "$GEN/openpdf.swift" "$ROOT/apps/macos/Sources/OpenPDFApp/Generated/openpdf.swift"
cp "$GEN/openpdfFFI.h" "$ROOT/apps/macos/Sources/openpdfFFI/include/openpdfFFI.h"
cat > "$ROOT/apps/macos/Sources/openpdfFFI/include/module.modulemap" <<'EOF'
module openpdfFFI {
    header "openpdfFFI.h"
    export *
}
EOF
cp "$ROOT/apps/macos/Sources/openpdfFFI/include/module.modulemap" \
  "$ROOT/apps/macos/Sources/openpdfFFI/module.modulemap"

cp "$LIB" "$ROOT/apps/macos/lib/libopenpdf_ffi.dylib"
install_name_tool -id @rpath/libopenpdf_ffi.dylib \
  "$ROOT/apps/macos/lib/libopenpdf_ffi.dylib" || true

if [[ -n "${OPENPDF_PDFIUM:-}" && -f "${OPENPDF_PDFIUM}" ]]; then
  cp "$OPENPDF_PDFIUM" "$ROOT/apps/macos/lib/libpdfium.dylib"
  install_name_tool -id @rpath/libpdfium.dylib \
    "$ROOT/apps/macos/lib/libpdfium.dylib" || true
fi

python3 "$ROOT/apps/macos/scripts/po-to-strings.py" \
  --po "$ROOT/po" \
  --out "$ROOT/apps/macos/Sources/OpenPDFApp/Resources"

cd "$ROOT/apps/macos"
swift build -c release \
  -Xlinker -L"$ROOT/apps/macos/lib" \
  -Xlinker -lopenpdf_ffi \
  -Xlinker -rpath -Xlinker "$ROOT/apps/macos/lib" \
  -Xlinker -rpath -Xlinker @executable_path

BIN="$(swift build -c release --show-bin-path)/openpdf-macos"
cp "$ROOT/apps/macos/lib/"*.dylib "$(dirname "$BIN")/" || true
echo "built $BIN"

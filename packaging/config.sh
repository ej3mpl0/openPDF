# Shared packaging settings. Scripts source this file.
#
# GUI binary (Flatpak command + .desktop Exec). Keep this a single variable.
APP_BIN="${APP_BIN:-openpdf-gtk}"
# CLI binary from crate openpdf-cli. Separate from the GUI; must not load GTK.
CLI_BIN="${CLI_BIN:-openpdf}"
CLI_CRATE="${CLI_CRATE:-openpdf-cli}"

# App id and desktop/metainfo paths match PR #1 (branch `gtk`) under data/.
APP_ID="io.github.ej3mpl0.openpdf"
APP_NAME="openPDF"
ICON_NAME="openpdf"
DESKTOP_FILE="data/${APP_ID}.desktop"
METAINFO_FILE="data/${APP_ID}.metainfo.xml"

# Same chromium/8035 build as .github/workflows/ci.yml
PDFIUM_TAG="chromium/8035"
PDFIUM_ARCHIVE="pdfium-linux-x64.tgz"
PDFIUM_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8035/pdfium-linux-x64.tgz"
PDFIUM_SHA256="2e6db042dd2cff2d5247023dbec6c7ebb800042ce83c835d6468d45229669bd4"

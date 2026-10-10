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
PDFIUM_SHA256_LINUX_X64="$PDFIUM_SHA256"
PDFIUM_SHA256_LINUX_ARM64="10bb7728a5268593a31c2024e0e57ea26b73b7a54195054f43d2ad0869d55a48"
PDFIUM_SHA256_WIN_X64="61513d611ad200a383456140739be77d156f1e3a2eef22bd89f6c3bda79bdd41"
PDFIUM_SHA256_MAC_X64="9170dd3bb0f14a712369dd8a1978e77e0b5a05c4371aca2ee49727daabf3201a"
PDFIUM_SHA256_MAC_ARM64="308fd9c6eff1be5b7bde62e7a9a42f525075901314a2a50058ae0b6ea0ff30a2"

# AppImage tools: versioned release / commit, never moving tags.
LINUXDEPLOY_TAG="1-alpha-20251107-1"
LINUXDEPLOY_SHA256_X86_64="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
LINUXDEPLOY_PLUGIN_GTK_COMMIT="3b67a1d1c1b0c8268f57f2bce40fe2d33d409cea"
LINUXDEPLOY_PLUGIN_GTK_SHA256="b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171"

# Adwaita icon theme bundled into the AppImage (non-GNOME hosts may lack it).
# 51.0 matches the org.gnome.Platform 51 runtime used by the Flatpak manifest.
ADWAITA_ICON_THEME_VERSION="51.0"
ADWAITA_ICON_THEME_URL="https://download.gnome.org/sources/adwaita-icon-theme/51/adwaita-icon-theme-51.0.tar.xz"
ADWAITA_ICON_THEME_SHA256="ba561cf3c96305a47179fa5605856ad695c9238ea07f7093aebd9ba80c4e323b"

# openPDF

Native Linux PDF app. Rust + GTK4/libadwaita, render with pdfium-render (BSD). MuPDF is not used.

- `openpdf-core`: document, tiles, text, undo, merge/split, `Signer`, `PinProvider`, sign UI strings
- `openpdf-cli`: `openpdf info|render|text|merge|split` plus organize (`insert`, `extract`, `replace`, `crop`), `optimize`, `protect`/`unprotect`/`sanitize`, `watermark`/`header`/`footer`/`number`, `from-images`/`to-images`
- `openpdf-gtk`: UI library (depends only on core)
- `openpdf-sign`: PAdES-B-B backends (PKCS#11 on all OSes, Windows CNG, macOS Keychain; depends only on core)
- `openpdf-app`: GTK application binary named `openpdf-gtk` (depends on gtk + sign)

Rule: gtk and sign depend on core, never on each other. The `openpdf-gtk`
binary (package `openpdf-app`) constructs `Box<dyn Signer>` from
`openpdf-sign` (with `GtkPinProvider`) and hands that trait object to the
UI library. PKCS#11 still asks for a PIN through `PinProvider`.
Windows and macOS native stores let the OS show any smart-card or
Keychain prompt. The CLI binary is `openpdf` (`openpdf-cli`) and stays GTK-free.

PDFium is loaded at runtime (`OPENPDF_PDFIUM`, next to the binary, or the system).

## Localization

User-facing strings are gettext-compatible. English `msgid`s live in source; translations live under `po/`.

- `po/openpdf.pot` — template (English msgids, empty `msgstr`)
- `po/es.po` — Spanish
- `po/LINGUAS` — installed languages

Locale follows GNU gettext: `LANGUAGE` (colon-separated priority list) first, then `LC_ALL` / `LC_MESSAGES` / `LANG`. `LANGUAGE` is ignored when the effective LC locale is `C` or `POSIX`. Missing keys fall back to English.

```rust
openpdf_core::init_i18n();
openpdf_core::tr("document has no pages");
openpdf_core::tr_args("pages: {count}", &[("count", "2")]);
```

The GTK UI matches `Error`, `InvalidKind`, and `SignedOp` instead of parsing `Display`. Adding a language is a new `po/{lang}.po` plus a line in `po/LINGUAS`. Preferences may pin `en` / `es` or use `auto` (follow `detect_locale()`).

## openpdf-gtk

GTK4 + libadwaita (no webview). Opens documents and asks `openpdf-core::Engine` for tiles.
Does not depend on `openpdf-sign`; signing goes through `Signer::list_certificates` and
`SignRequest.cert_id`. Signed documents (`Document::is_signed`) are read-only. The PIN
is asked by the token via `PinProvider` (UI dialog), never as a field on `SignRequest`.

```
sudo apt install libgtk-4-dev libadwaita-1-dev pkg-config
export OPENPDF_PDFIUM=/usr/local/lib/libpdfium.so
cargo run --bin openpdf-gtk
```

Application id, desktop file, AppStream id, and icon name are
`io.github.ej3mpl0.openpdf` (see `data/`). Settings stay in
`~/.config/openpdf/settings.toml` (or `$XDG_CONFIG_HOME/openpdf`).
The open dialog uses `GtkFileDialog` (xdg portals, Flatpak-ready).

CI opens the real window under Xvfb (`xvfb-run … openpdf-gtk --measure 32`)
and fails if startup or peak RSS exceed the limits in
`crates/openpdf-gtk/src/measure.rs` (8 s / 250 MiB). Measured under xvfb+cairo:
about 150 ms startup and 135 MiB peak RSS (32 pages, 16 visible tiles + 3 thumbnails).
`--measure` without a display exits non-zero.

```
xvfb-run -a --server-args="-screen 0 1280x800x24" \
  env GDK_BACKEND=x11 GSK_RENDERER=cairo \
  cargo run --bin openpdf-gtk --release -- --measure 32
```

# openPDF

One repository. Native UI per platform (Linux GTK now; Windows and macOS later)
sharing `openpdf-core`, `openpdf-cli`, and signing.

- `openpdf-core`: document, tiles, text, undo, merge/split, `Signer`, `OcrEngine`, `PinProvider`, sign UI strings
- `openpdf-cli`: `openpdf info|render|text|merge|split` plus organize (`insert`, `extract`, `replace`, `crop`), `optimize`, `protect`/`unprotect`/`sanitize`, `watermark`/`header`/`footer`/`number`, `from-images`/`to-images`, `ocr`, `redact`, `compare`
- `openpdf-gtk`: Linux UI (depends only on core)
- `openpdf-sign`: PKCS#11 / PAdES backend (depends only on core)

Rule: gtk and sign depend on core, never on each other. The application
binary constructs `Box<dyn Signer>` from `openpdf-sign` (with a UI
`PinProvider`) and `Box<dyn OcrEngine>` and hands those trait objects to
the GTK UI.

Settings and recovery use the `directories` crate: XDG on Linux,
`%APPDATA%` / `%LOCALAPPDATA%` on Windows, `~/Library` on macOS.

PDFium is loaded at runtime (`OPENPDF_PDFIUM`, next to the binary, macOS
app-bundle `Frameworks`, or the system). Library name is `libpdfium.so` /
`pdfium.dll` / `libpdfium.dylib`.

OCR recognition is `OcrEngine` (capability, languages, recognize raster to
words + boxes). Text-layer writing stays in core. Linux default is Tesseract
only if it is present at runtime (`tesseract` on `PATH`, `libtesseract` via
dlopen, `OPENPDF_TESSERACT`, or a Flatpak extension under `/app/extensions`).
It is never linked. Windows.Media.Ocr and macOS Vision plug in from platform
crates later; `UnavailableOcr` is the stub until then. `ocr_capability()` is
the UI probe.

Measure open time, first tile, and peak RSS:

```
cargo run -p openpdf-core --release --bin openpdf-measure
```

Budgets: `OPENPDF_BENCH_MAX_OPEN_MS`, `OPENPDF_BENCH_MAX_TILE_MS`,
`OPENPDF_BENCH_MAX_RSS_BYTES`. The process exits non-zero when a set limit
is exceeded.

## Localization

User-facing strings are gettext-compatible. English `msgid`s live in source; translations live under `po/`.

- `po/openpdf.pot` — template (English msgids, empty `msgstr`)
- `po/es.po` — Spanish
- `po/LINGUAS` — installed languages

Locale follows GNU gettext: `LANGUAGE` (colon-separated priority list) first, then `LC_ALL` / `LC_MESSAGES` / `LANG`. `LANGUAGE` is ignored when the effective LC locale is `C` or `POSIX`. When those variables are unset (typical on Windows and macOS), the OS locale from `sys-locale` is used. Missing keys fall back to English.

```rust
openpdf_core::init_i18n();
openpdf_core::tr("document has no pages");
openpdf_core::tr_args("pages: {count}", &[("count", "2")]);
```

The GTK UI can match `Error`, `InvalidKind`, and `SignedOp` instead of parsing `Display`. Adding a language is a new `po/{lang}.po` plus a line in `po/LINGUAS`.

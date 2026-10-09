# openPDF

Native Linux PDF app. Rust + GTK4/libadwaita, render with pdfium-render (BSD). MuPDF is not used.

- `openpdf-core`: document, tiles, text, undo, merge/split, `Signer`
- `openpdf-cli`: `openpdf info|render|text|merge|split`
- `openpdf-gtk`: UI
- `openpdf-sign`: PKCS#11 signing (DNIe, FNMT)

Rule: gtk and sign depend on core, never on each other.

PDFium is loaded at runtime (`OPENPDF_PDFIUM`, next to the binary, or the system).

## Localization

User-facing strings are gettext-compatible. English `msgid`s live in source; translations live under `po/`.

- `po/openpdf.pot` — template (English msgids, empty `msgstr`)
- `po/es.po` — Spanish
- `po/LINGUAS` — installed languages

Locale comes from `LC_ALL` / `LC_MESSAGES` / `LANG` / `LANGUAGE`. Missing keys and `C`/`POSIX` fall back to English.

```rust
openpdf_core::init_i18n();
openpdf_core::tr("document has no pages");
openpdf_core::tr_args("pages: {count}", &[("count", "2")]);
```

The GTK UI can match `Error`, `InvalidKind`, and `SignedOp` instead of parsing `Display`. Adding a language is a new `po/{lang}.po` plus a line in `po/LINGUAS`.

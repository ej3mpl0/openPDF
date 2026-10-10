# openPDF

Native Linux PDF app. Rust + GTK4/libadwaita, render with pdfium-render (BSD). MuPDF is not used.

- `openpdf-core`: document, tiles, text, undo, merge/split, `Signer`, `PinProvider`, sign UI strings
- `openpdf-cli`: `openpdf info|render|text|merge|split` plus organize (`insert`, `extract`, `replace`, `crop`), `optimize`, `protect`/`unprotect`/`sanitize`, `watermark`/`header`/`footer`/`number`, `from-images`/`to-images`
- `openpdf-gtk`: UI (depends only on core)
- `openpdf-sign`: PKCS#11 / PAdES backend (depends only on core)
- `openpdf-ffi`: UniFFI entry point for native macOS (Swift) and Windows (C#) UIs; owned-data APIs, signing stays in Rust
- `apps/windows`: WinUI 3 viewer (`OpenPDF`); tiled swap chain, PIN dialog, `--measure`

Rule: gtk and sign depend on core, never on each other. The application
binary constructs `Box<dyn Signer>` from `openpdf-sign` (with a UI
`PinProvider`) and hands that trait object to the GTK UI.

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

The GTK UI can match `Error`, `InvalidKind`, and `SignedOp` instead of parsing `Display`. Adding a language is a new `po/{lang}.po` plus a line in `po/LINGUAS`.

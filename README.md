# openPDF

Native PDF tools for Linux, written in Rust. Rendering uses PDFium
(`pdfium-render`), loaded at runtime. MuPDF is not used.

On `main`, **openpdf-core**, the `openpdf` CLI, and **openpdf-sign**
(PKCS#11 PAdES) are available.

**Binaries:** `openpdf` is the CLI. It does not link GTK and works headless.
The GUI will be a separate `openpdf-gtk` binary (still in development,
[PR #1](https://github.com/ej3mpl0/openPDF/pull/1)).

> **Author of `openpdf-core`:** Jian Yang.
> The document model, tile engine, structure operations (rotate, delete, move,
> merge, split), and incremental save are his work.

License: [MPL-2.0](https://www.mozilla.org/MPL/2.0/).

## Building

Rust *stable* is required (`rust-toolchain.toml`). From the workspace root:

```bash
cargo build --release
```

CLI only:

```bash
cargo build -p openpdf-cli --release
```

The binary is written to `target/release/openpdf`.

`info`, `merge`, `split`, and the organize / optimize / protect / overlay
commands use `lopdf` and do not need PDFium. `render`, `text`, and
`to-images` do: `libpdfium.so` must be locatable (see
[Loading PDFium](#loading-pdfium)).

Tests:

```bash
cargo test --workspace
```

## CLI

`openpdf` is a headless CLI (no GTK). Page numbers are **1-based**. There is
no `sign` or `verify` subcommand; signing is a library API (`openpdf-sign`).

Every operation that **edits or rewrites** a document refuses a signed PDF
(`Error::Signed`). That includes insert, replace, crop, optimize, protect,
unprotect, sanitize, watermark, header, footer, and number. `extract` from a
signed file writes a new unsigned PDF. `to-images` only renders. `save` is a
full rewrite, so these tools write `-o` through `Document::save`.

### `openpdf info`

Page count, size in points, rotation, and outline:

```bash
openpdf info document.pdf
```

Output (fields match the code):

```text
file: document.pdf
pages: 2
page 1: 612.0 x 792.0 pt  rot 0
page 2: 612.0 x 792.0 pt  rot 0
outline:
  Chapter 1 -> 1
  Chapter 2 -> 2
```

If the PDF has no bookmarks: `outline: (none)`.

### `openpdf render`

Rasterize a page to PNG (`--page` defaults to 1, `--zoom` defaults to 1.0):

```bash
openpdf render document.pdf --page 1 -o page.png
openpdf render document.pdf --page 1 --zoom 2.0 -o page.png
```

### `openpdf text`

Extract text. Without `--page`, every page is printed (separated by `-----`):

```bash
openpdf text document.pdf
openpdf text document.pdf --page 1
```

### `openpdf merge`

Concatenate **at least two** PDFs. The destination is required (`-o`):

```bash
openpdf merge a.pdf b.pdf -o c.pdf
```

More than two inputs are allowed. Merge rewrites the file with `lopdf`.

### `openpdf split`

Without `--pages`, split the document into one PDF per page. `-o` is a
directory; files are named `{stem}-1.pdf`, `{stem}-2.pdf`, …

```bash
openpdf split document.pdf -o pages/
```

With `--pages`, use 1-based ranges, for example `1-3,5`:

```bash
openpdf split document.pdf --pages 1-3,5 -o parts/
```

If the range produces **one** document and `-o` ends in `.pdf`, that file is
written (not a directory):

```bash
openpdf split document.pdf --pages 1-3 -o extract.pdf
```

### `openpdf insert`

Insert pages from a source PDF into the target at a 1-based `--at` position.
`--pages` selects pages from the source (default `1` if omitted):

```bash
openpdf insert two_page.pdf hello.pdf --at 2 -o inserted.pdf
```

### `openpdf extract`

Extract `--pages` (required) into a new PDF:

```bash
openpdf extract document.pdf --pages 2 -o one.pdf
```

### `openpdf replace`

Replace `--count` pages of the target starting at `--at` (1-based) with
`--pages` from the source (source pages default to `1`, `--count` defaults
to `1`):

```bash
openpdf replace document.pdf other.pdf --at 2 --count 1 --pages 1 -o out.pdf
```

### `openpdf crop`

Set `/CropBox` as `llx,lly,urx,ury` in points. Without `--pages`, every page
is cropped:

```bash
openpdf crop document.pdf --box 0,0,612,792 --pages 1-2 -o cropped.pdf
```

### `openpdf optimize`

Recompress images (JPEG `--quality` 1-100, default 75), drop unused objects,
and write a linearized hint unless `--no-linearize`:

```bash
openpdf optimize document.pdf --quality 75 -o small.pdf
```

### `openpdf protect`

Encrypt with AES-256. `--user` is required; `--owner` defaults to the user
password. `--permit` is a comma list:
`print,modify,copy,annotate,fill,assemble,print-high` (or `all`):

```bash
openpdf protect document.pdf --user secret --permit print,copy -o locked.pdf
```

### `openpdf unprotect`

Remove password protection with the user or owner password:

```bash
openpdf unprotect locked.pdf --password secret -o plain.pdf
```

### `openpdf sanitize`

Remove metadata, JavaScript, and embedded-file names:

```bash
openpdf sanitize document.pdf -o clean.pdf
```

### `openpdf watermark`

Stamp overlay `--text` (optional `--pages`). `{page}` and `{total}` expand:

```bash
openpdf watermark document.pdf --text DRAFT -o marked.pdf
```

### `openpdf header`

Add a header:

```bash
openpdf header document.pdf --text "Confidential" -o headed.pdf
```

### `openpdf footer`

Add a footer:

```bash
openpdf footer document.pdf --text "Company" --pages 1-3 -o footed.pdf
```

### `openpdf number`

Add page numbers. `--format` defaults to `{page} / {total}`:

```bash
openpdf number document.pdf -o numbered.pdf
```

### `openpdf from-images`

Build a PDF from one or more image files (one page per image, US Letter):

```bash
openpdf from-images photo.png scan.jpg -o album.pdf
```

### `openpdf to-images`

Render pages to PNG (`--zoom` default 1.0). `-o` is a directory
(`{stem}-1.png`, …) unless there is one page and `-o` ends in `.png`:

```bash
openpdf to-images document.pdf --zoom 1.0 -o pages/
```

## Loading PDFium

`libpdfium` is resolved in this order (`bind_pdfium` / `Engine::new`):

1. `OPENPDF_PDFIUM` environment variable (path to the `.so` or to a directory).
2. `libpdfium.so` next to the binary (`current_exe()`, or a bundled path if
   `Engine::with_bundled_library` is used).
3. The system library (`/usr/local/lib`, `/usr/lib`,
   `/usr/lib/x86_64-linux-gnu`, and finally `Pdfium::bind_to_system_library()`).

Example:

```bash
export OPENPDF_PDFIUM=/usr/local/lib/libpdfium.so
openpdf render document.pdf -o page.png
```

## Language

Core and CLI strings are gettext-compatible. English `msgid`s live in source;
translations live in the shared `po/` directory:

- `po/openpdf.pot` — template (English msgids, empty `msgstr`)
- `po/es.po` — Spanish catalog
- `po/LINGUAS` — installed languages (`es`; English is the msgid fallback)

Supported languages on `main`: **English** and **Spanish**. Catalogs are
embedded at build time (`openpdf-core` `build.rs`). The CLI calls
`init_i18n()` on startup. Missing keys fall back to the English msgid.

Locale selection follows GNU gettext (`detect_locale` in `openpdf-core`):

1. `LANGUAGE` — colon-separated list, first available catalog wins
   (for example `es:en` or `fr:es`). **Ignored** when the effective LC
   locale is `C` or `POSIX`.
2. `LC_ALL`
3. `LC_MESSAGES`
4. `LANG`

If none of those select a catalog, the fallback is English.

```bash
LANGUAGE=es openpdf info file.pdf
LANGUAGE=es:en openpdf --help
LC_ALL=es_ES.UTF-8 openpdf info file.pdf
```

`LANGUAGE=es:en` with `LANG=C` stays English: `LANGUAGE` is ignored for the
`C` / `POSIX` locale.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for workspace layout, build and test
commands, i18n rules (including the locale test lock), structured errors,
signing architecture, and append-only handling of signed files.

## XDG paths

`openpdf-core` exposes `settings_dir`, `recovery_dir`, and `ensure_app_dirs`
(creates the directories if they do not exist). They honor the XDG variables:

| Use        | Default                      | Variable          |
|------------|------------------------------|-------------------|
| Settings   | `~/.config/openpdf`          | `XDG_CONFIG_HOME` |
| Recovery   | `~/.local/state/openpdf`     | `XDG_STATE_HOME`  |

## `openpdf-core`

Document, render, structure-ops, and signing traits. **Author: Jian Yang.**
`openpdf-gtk` and `openpdf-sign` depend only on this crate, never on each other.

- **Tiles.** Single-thread render (`openpdf-render`). 256 px RGBA tiles, priority
  queue (`Visible`, `Adjacent`, `Thumbnail`) that drops stale work on a frame
  change.
- **LRU cache.** Default cap of **64 MiB** of decoded RGBA
  (`DEFAULT_CACHE_BYTES`). Invalidated per document.
- **Search.** `Engine::search` walks each page’s text (case-insensitive) and
  returns `SearchHit` (page, range, context).
- **Outline.** `Document::outline()` reads the PDF bookmarks
  (`OutlineItem`: title, page, children).
- **Rotate, delete, and move**, with **undo / redo** (`Command` /
  `History`: `Rotate`, `Delete`, `Move`).
- **Incremental save.** `save_incremental` / `to_incremental_bytes` keep the
  original bytes as a prefix and append an update. `save` rewrites the whole
  file.
- **Merge / split.** `merge` / `merge_paths` and `split` / `split_all` via
  `lopdf` (no PDFium).
- **Signing traits.** `Signer` and `PinProvider` (plus `SecretPin`,
  `SignRequest`, certificate / verify helpers). Implementations live in
  `openpdf-sign`.

## Signing

`openpdf-sign` implements **PAdES-B-B** (`ETSI.CAdES.detached`, SHA-256,
detached CAdES-BES with `signing-certificate-v2`) over PKCS#11. Tokens
documented in the loader: **DNIe** (OpenSC), **FNMT** (p11-kit / NSS
softoken), and SoftHSM.

### PKCS#11 modules

`Pkcs11Signer::new` resolves modules in this order (`resolve_module_paths`):

1. `Pkcs11Config.module_path` when set (that path only).
2. `OPENPDF_PKCS11_MODULE` when set.
3. Built-in defaults: OpenSC (`opensc-pkcs11.so`), p11-kit
   (`p11-kit-proxy.so`), NSS (`libsoftokn3.so`), SoftHSM2
   (`libsofthsm2.so`) under `/usr/lib`, `/usr/lib64`, and
   `/usr/lib/x86_64-linux-gnu` (including `pkcs11/` and `softhsm/`
   subdirs).

Every path that exists and initializes is loaded (several modules can be
active at once). `list_certificates` does not ask for a PIN.

### PIN

The PIN is requested only when `Signer::sign` logs in to the token, through
`PinProvider::request_pin`. It is never a field on `SignRequest`, `Signer`,
or `Document`. The value lives in `SecretPin` (`zeroize::Zeroizing<String>`)
and is wiped when that wrapper is dropped (after login).

### Incremental signatures and locks

`pades::sign_incremental` appends a PDF update. The original bytes stay a
prefix. A second signature is also incremental. After a signature,
`Document::is_signed()` / `is_read_only()` are true: rotate, delete, move,
undo, redo, and a full rewrite (`save` / `to_bytes`) return `Error::Signed`.
Only incremental / append-only writes are allowed.

### Verification

`Signer::verify` and `verify_pdf` (no PKCS#11 module required) return
`SignatureInfo` (`name`, `reason`, `location`, `valid`). Core helpers
`format_verify_result` / `format_verify_results` localize those lines.
The CLI has no verify command.

## Architecture

- `Signer` and `PinProvider` live in **openpdf-core**.
- **openpdf-sign** implements PKCS#11 login and CMS/PAdES
  (`Pkcs11Signer`, `cades`, `pades`).
- The **application binary** (not the GTK crate) constructs
  `Box<dyn Signer>` and hands it to UI code that only sees core:

```rust
let pin = /* UI type that implements PinProvider */;
let signer: Box<dyn openpdf_core::Signer> = Box::new(
    openpdf_sign::Pkcs11Signer::new(openpdf_sign::Pkcs11Config::default(), pin)?,
);
```

- **openpdf-gtk must never depend on openpdf-sign** (enforced by
  `gtk_manifest_does_not_depend_on_sign`).

## Status

- The **GTK4 UI** is in development ([PR #1](https://github.com/ej3mpl0/openPDF/pull/1)).
- **Digital signing is available** in `openpdf-sign` (PAdES / PKCS#11). There
  is no CLI `sign` / `verify` command and no GTK signing UI yet.
- **Core and CLI translations** (English and Spanish) are done. **GUI
  translations** are still in progress.

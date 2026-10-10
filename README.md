# openPDF

Native PDF tools for Linux, Windows, and macOS, written in Rust.
Rendering uses PDFium (`pdfium-render`), loaded at runtime. MuPDF is
not used. Core and CLI paths are portable on
[PR #10](https://github.com/ej3mpl0/openPDF/pull/10). The GTK UI is
Linux-only.

**Binaries:** `openpdf` is the headless CLI (crate `openpdf-cli`, no GTK).
`openpdf-gtk` is the Linux GUI (package `openpdf-app`, GTK4/libadwaita).
The UI library crate is `openpdf-gtk`; it never depends on `openpdf-sign`.
The app binary constructs `Box<dyn Signer>` from `openpdf-sign`.

> **Author of `openpdf-core`:** Jian Yang.
> The document model, tile engine, structure operations (rotate, delete, move,
> merge, split), and incremental save are his work.

License: [MPL-2.0](LICENSE).

## Installation

A push of a `v*` tag runs `.github/workflows/release.yml`. That workflow
attaches the files below to the GitHub release, plus a `SHA256SUMS` list.
**Flathub is not available yet.** There is no Flathub package and no
`flatpak install flathub ...` command.

### Linux Flatpak (`x86_64` and `aarch64`)

The release file is **`io.github.ej3mpl0.openpdf.flatpak`** (`bundle:` in
`release.yml`). The Flatpak manifest
(`packaging/flatpak/io.github.ej3mpl0.openpdf.yml`) ships PDFium for
`x86_64` and `aarch64` (`only-arches`). The app id is
`io.github.ej3mpl0.openpdf`; the GUI command is `openpdf-gtk`; the CLI
`openpdf` is in the same bundle.

```bash
flatpak install --user io.github.ej3mpl0.openpdf.flatpak
flatpak run io.github.ej3mpl0.openpdf
```

Permissions from `finish-args` (do not grant a home filesystem):

- **Files:** the XDG document portal. The manifest does not set
  `--filesystem=home`.
- **Smart cards / DNIe:** `--socket=pcsc` talks to host `pcscd`. OpenSC
  is bundled and `OPENPDF_PKCS11_MODULE` is
  `/app/lib/pkcs11/opensc-pkcs11.so`.

### Linux AppImage (`x86_64`)

`packaging/appimage/build.sh` writes **`openPDF-x86_64.AppImage`**
(`APP_NAME`-`ARCH`). AppImage packaging is x86_64 only. The image
contains `openpdf-gtk` and `openpdf`.

```bash
chmod +x openPDF-x86_64.AppImage
./openPDF-x86_64.AppImage
```

### Windows CLI (`x64`)

Every release ships **`openpdf-<version>-windows-x64.zip`**, where
`<version>` is the tag without the leading `v` (`GITHUB_REF_NAME#v` in
`release.yml`). The zip holds `openpdf.exe` and `pdfium.dll` in a folder
of the same name.

```text
openpdf-<version>-windows-x64/openpdf.exe
openpdf-<version>-windows-x64/pdfium.dll
```

Unzip and run `openpdf.exe` from that folder (`pdfium.dll` must stay
beside the exe).

### macOS CLI (arm64 and Intel)

The `macos-cli` matrix (`macos-latest` and `macos-13`) packs one
`tar.gz` per runner. `uname -m` is mapped to `arm64` or `x64`:

- **`openpdf-<version>-macos-arm64.tar.gz`** — Apple Silicon (`arm64`)
- **`openpdf-<version>-macos-x64.tar.gz`** — Intel (`x86_64`)

Each archive holds `openpdf` and `libpdfium.dylib`. Extract and run
`./openpdf` from that folder.

### Verify downloads

`SHA256SUMS` is attached next to the artifacts. Check a download:

```bash
sha256sum -c SHA256SUMS
```

On macOS, `shasum -a 256 -c SHA256SUMS` is the equivalent.

Each release job also writes a GitHub artifact attestation
(`actions/attest-build-provenance@v2`; the publish job attests every
file in `out/`, including `SHA256SUMS`). Verify a file with the GitHub
CLI:

```bash
gh attestation verify io.github.ej3mpl0.openpdf.flatpak --repo ej3mpl0/openPDF
gh attestation verify openPDF-x86_64.AppImage --repo ej3mpl0/openPDF
gh attestation verify openpdf-<version>-windows-x64.zip --repo ej3mpl0/openPDF
gh attestation verify openpdf-<version>-macos-arm64.tar.gz --repo ej3mpl0/openPDF
```

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

`info`, `merge`, `split`, organize / optimize / protect / overlay, `ocr`
(after Tesseract is found), `redact`, and `sanitize` use `lopdf`. `render`,
`text`, `to-images`, `ocr`, and `compare` also need the platform PDFium
library (`libpdfium.so` / `pdfium.dll` / `libpdfium.dylib`; see
[Loading PDFium](#loading-pdfium)). The GUI needs GTK4, libadwaita, and
PDFium.

Tests:

```bash
cargo test --workspace
```

## CLI

`openpdf` is a headless CLI (no GTK). Page numbers are **1-based**. There is
no `sign` or `verify` subcommand; signing is a library API and the GTK
certificate picker.

Every operation that **edits or rewrites** a document refuses a signed PDF
(`Error::Signed`). That includes insert, replace, crop, optimize, protect,
unprotect, sanitize, watermark, header, footer, number, `ocr`, and `redact`.
`extract` from a signed file writes a new unsigned PDF. `to-images` and
`compare` only read. `save` is a full rewrite, so these tools write `-o`
through `Document::save`.

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

### `openpdf ocr`

Add an invisible searchable text layer. Recognition is `OcrEngine`.
Tesseract is the **Linux** default (`TesseractOcr` / `resolve_engine`).
It is **optional** and never linked: if it is missing, every other
command still works. Windows and macOS default to `UnavailableOcr`
until a platform crate supplies Windows.Media.Ocr or Vision.

Tesseract lookup (`tesseract_search_paths`) uses **absolute paths
only**: no current directory, no bare `tesseract` / `tesseract.exe`,
no relative `PATH` entries.

1. `OPENPDF_TESSERACT` if it is an absolute path (binary or library).
2. Fixed bin candidates: Linux `/usr/bin/tesseract`,
   `/usr/local/bin/tesseract`, `/app/bin/tesseract`; macOS
   `/opt/homebrew/bin/tesseract`, `/usr/local/bin/tesseract`; Windows
   `%PROGRAMFILES%\Tesseract-OCR\tesseract.exe`,
   `%PROGRAMFILES(X86)%\Tesseract-OCR\tesseract.exe`,
   `%LOCALAPPDATA%\Programs\Tesseract-OCR\tesseract.exe`.
3. Linux: `tesseract` on `PATH`, but only directories that are
   absolute (`which_absolute`).
4. Library roots + names: Linux `/app/extensions`,
   `/usr/lib/x86_64-linux-gnu`, `/usr/lib`, `/usr/local/lib`
   (`libtesseract.so.5` / `.so.4` / `.so`); macOS `/opt/homebrew/lib`,
   `/usr/local/lib` (`libtesseract.dylib` / `.5.dylib` / `.4.dylib`);
   Windows `%PROGRAMFILES%\Tesseract-OCR` (`libtesseract-5.dll`,
   `libtesseract-4.dll`, `tesseract54.dll`, `tesseract.dll`).

`--lang` defaults to `eng`. `--list-langs` prints installed languages and
exits. `--pages` is optional. Needs PDFium to rasterize.

```bash
openpdf ocr document.pdf --lang eng -o searchable.pdf
openpdf ocr --list-langs
```

### `openpdf redact`

True redaction: permanently remove content in one or more boxes
(`--box page,llx,lly,urx,ury`, page 1-based, repeatable) and **rewrite**
the file. Irreversible. Refuses signed documents. Drops text in the box
(including an OCR text layer), punches matching image pixels, removes
overlapping annotations, page thumbnails, and metadata that still holds
the removed strings.

```bash
openpdf redact document.pdf --box 1,0,0,612,792 -o redacted.pdf
```

### `openpdf compare`

Compare two PDFs (PDFium text + per-page pixels). `--zoom` defaults to
1.0. Read-only; signed files are allowed. Prints `identical` or `differ`.

```bash
openpdf compare a.pdf b.pdf
```

## Linux GUI (`openpdf-gtk`)

Native GTK4 + libadwaita reader (GNOME HIG). Binary name `openpdf-gtk`
from crate `openpdf-app`. The `openpdf-gtk` crate is the UI library and
**must not** depend on `openpdf-sign`.

Features: home (recent files / open), tabs, tiled viewer with thumbnail
sidebar, bookmarks, find, zoom / page layout, rotate / delete / undo,
preferences (language, theme, tile cache), about. Certificate picker
(`app.sign` / Sign with certificate): lists `Signer::list_certificates()`,
disables non-signing certs, never stores a PIN — `GtkPinProvider` asks
the token via `SecretPin`. A signed document is **read-only**: banner
text *Signed document — read-only. Rotate, delete and reorder are
disabled.* Core refuses those ops.

```bash
cargo build -p openpdf-app --release
./target/release/openpdf-gtk
./target/release/openpdf-gtk document.pdf
```

`--measure [PAGES]` opens the real libadwaita window (needs a display or
`xvfb-run`), loads a multi-page PDF, waits for visible tiles and
thumbnails, prints `startup_ms` and `peak_rss_bytes`, and exits
non-zero when over its limit:

```bash
xvfb-run -a ./target/release/openpdf-gtk --measure
```

## Loading PDFium

Library name (`pdfium_library_name`):

| OS | File |
|----|------|
| Linux | `libpdfium.so` |
| Windows | `pdfium.dll` |
| macOS | `libpdfium.dylib` |

`bind_pdfium` / `pdfium_search_paths` try **absolute paths only**.
Relative paths, `.`, bare names, and relative `PATH` entries are
dropped (`dll::require_absolute`). There is no
`Pdfium::bind_to_system_library()` fallback.

Order:

1. `OPENPDF_PDFIUM` if it is an absolute path (file or directory).
2. An absolute bundled path when `Engine::with_bundled_library` is used.
3. The executable directory (`current_exe()` parent) and
   `<exe-dir>/<library-name>`.
4. **macOS only:** `<exe-dir>/Frameworks`,
   `<exe-dir>/Frameworks/<library-name>`, and the same under
   `Contents/Frameworks` (parent of the exe dir).
5. **Linux system:** `/usr/local/lib`, `/usr/lib`,
   `/usr/lib/x86_64-linux-gnu`, `/usr/lib/aarch64-linux-gnu`
   (directory and `<dir>/<library-name>`).
6. **macOS system:** `/usr/local/lib`, `/opt/homebrew/lib`.
7. **Windows system:** `%WINDIR%\System32\pdfium.dll` when `WINDIR` is
   an absolute path.

On Windows, `dll::harden()` calls `SetDefaultDllDirectories` with
`LOAD_LIBRARY_SEARCH_DEFAULT_DIRS` before any load (no CWD DLL
search). The bind also uses
`LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR`.

Example (absolute path required):

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

Locale selection (`detect_locale`) works on Linux, Windows, and macOS.

1. GNU gettext env vars, when any of them is set:
   `LANGUAGE` (colon-separated list, first available catalog wins,
   for example `es:en` or `fr:es`) — **ignored** when the effective LC
   locale is `C` or `POSIX` — then `LC_ALL`, `LC_MESSAGES`, `LANG`.
2. If **none** of those variables are set (typical on Windows and
   macOS), `sys-locale` (`sys_locale::get_locale()`). Hyphen tags
   such as `en-US` become `en_US`.
3. English if nothing matches.

`LANGUAGE=es:en` with `LANG=C` stays English: `LANGUAGE` is ignored for
the `C` / `POSIX` locale.

```bash
LANGUAGE=es openpdf info file.pdf
LANGUAGE=es:en openpdf --help
LC_ALL=es_ES.UTF-8 openpdf info file.pdf
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for workspace layout, build and test
commands, i18n rules (including the locale test lock), structured errors,
signing architecture, and append-only handling of signed files.

## Config and state directories

`openpdf-core` (`paths.rs`) uses the `directories` crate
(`ProjectDirs::from("", "", "openpdf")`). `settings_dir`,
`recovery_dir`, and `ensure_app_dirs` (creates the directories if they
do not exist):

| Use | Linux | Windows | macOS |
|-----|-------|---------|-------|
| Settings | `~/.config/openpdf` (`XDG_CONFIG_HOME`) | `%APPDATA%\openpdf` | `~/Library/Application Support/openpdf` |
| Recovery | `~/.local/state/openpdf` (`XDG_STATE_HOME`) | `%LOCALAPPDATA%\openpdf` | `~/Library/Caches/openpdf` |

`state_dir` is Linux-only in `directories`. Recovery on Windows is
`data_local_dir`; on macOS it is `cache_dir`.

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
detached CAdES-BES with `signing-certificate-v2`). CMS / ByteRange /
incremental-append is shared. Backends only list certificates and sign
opaque bytes. Native Windows and macOS stores are on
[PR #11](https://github.com/ej3mpl0/openPDF/pull/11).

### Backends per platform

`default_signer(pin)` builds a `CompositeSigner` (`openpdf-sign` `backend.rs`):

| Platform | Native store | PKCS#11 |
|----------|--------------|---------|
| Linux | none | `Pkcs11Signer` (OpenSC **DNIe**, p11-kit / NSS **FNMT**, SoftHSM) |
| Windows | `WindowsStoreSigner`: CurrentUser `MY`, CNG / `NCryptSignHash` only | yes |
| macOS | `KeychainSigner`: Keychain `SecIdentity` + `SecKeyCreateSignature` | yes |

When a native store is compiled in and PKCS#11 also loads, PKCS#11
certificate ids get a `pkcs11:` prefix. If PKCS#11 fails to load and a
native store is present, listing continues from the store.

### Which certificates can sign

One gate on every backend (`can_use_for_pades`): empty KeyUsage, or
KeyUsage **`nonRepudiation`** (`CertificateUsage::Signing`).
`digitalSignature` without `nonRepudiation` is authentication-only
(the DNIe authentication certificate). Those rows stay in
`list_certificates` as `CertUnusable::AuthenticationOnly`; `sign()`
refuses them.

Expired and not-yet-valid certificates are also listed, with
`unusable` set. `sign()` refuses them (`check_validity` /
`describe_unusable`). `Signer::sign` refuses an unusable id even if
the picker did not disable the row.

### Keys and curves

`cert::key_kind` accepts:

- RSA
- ECDSA **P-256** (`secp256r1`) with SHA-256
- ECDSA **P-384** (`secp384r1`) with SHA-384
- ECDSA **P-521** (`secp521r1`) with SHA-512

Any other algorithm or **unknown curve** is `None`:
`CertUnusable::UnsupportedKey`. `sign()` returns
`unsupported public key algorithm`. There is no SHA-256 fallback.

### `CertUnusable` in the picker

`CertificateInfo.unusable: Option<CertUnusable>`. `Some` means the
picker must disable that row. `describe_unusable()` (English msgid):

| Variant | Text |
|---------|------|
| `AuthenticationOnly` | `Authentication only` |
| `Expired { not_after }` | `Certificate has expired (valid until {not_after})` |
| `NotYetValid { not_before }` | `Certificate is not yet valid (valid from {not_before})` |
| `UnsupportedKey` | `Unsupported key algorithm` |

`CertUnusable` is `#[non_exhaustive]` and **may grow** (a later
`Revoked` is called out in the code). Downstream `match`es need a
wildcard arm.

### PKCS#11 modules

`Pkcs11Signer::new` resolves modules in this order (`resolve_module_paths`):

1. `Pkcs11Config.module_path` when set (that path only).
2. `OPENPDF_PKCS11_MODULE` when set.
3. Built-in defaults for the OS:
   - Linux: OpenSC (`opensc-pkcs11.so`), p11-kit (`p11-kit-proxy.so`),
     NSS (`libsoftokn3.so`), SoftHSM2 (`libsofthsm2.so`) under
     `/usr/lib`, `/usr/lib64`, and `/usr/lib/x86_64-linux-gnu`
     (including `pkcs11/` and `softhsm/` subdirs).
   - Windows: OpenSC `opensc-pkcs11.dll` under `C:\Program Files\OpenSC Project\OpenSC\pkcs11\`,
     `C:\Program Files (x86)\OpenSC Project\OpenSC\pkcs11\`,
     `C:\Windows\System32\`, and `C:\Windows\SysWOW64\`.
   - macOS: OpenSC `opensc-pkcs11.so` under `/opt/homebrew/lib`,
     `/usr/local/lib`, `/Library/OpenSC/lib` (and `pkcs11/` subdirs);
     p11-kit `p11-kit-proxy.dylib` under `/opt/homebrew/lib/pkcs11`
     and `/usr/local/lib/pkcs11`.

Every path that exists and initializes is loaded (several modules can be
active at once). `list_certificates` does not ask for a PIN.

### PIN

The PIN is never a field on `SignRequest`, `Signer`, or `Document`.
Do not put a PIN in an environment variable. There is no user-facing
PIN variable (including any `OPENPDF_PKCS11_PIN`).

- **PKCS#11:** `login_user` in `token.rs` runs only from `Signer::sign`.
  `PinProvider::request_pin` supplies a `SecretPin`
  (`zeroize::Zeroizing<String>`), wiped after login.
- **Windows store:** the OS owns any PIN / smart-card dialog. The
  backend never collects a PIN.
- **macOS Keychain:** the OS owns Keychain / token prompts. The
  backend never collects a PIN.

Empty, whitespace-only, missing, or cancelled PINs never reach the
card: no `C_Login`, so no retry is spent.

- Empty or whitespace-only (`trim` is empty) → `Error::PinRequired`
  (`PIN is required`) before `C_Login`.
- Cancelled dialog (`PinProvider` returns `Error::PinCancelled`) →
  `PIN entry was cancelled`; the token is not touched.
- Locked token (`CKF_USER_PIN_LOCKED`) → `Error::PinLocked`
  (`Token PIN is locked`) and no `C_Login`.
- Pinpad reader (`CKF_PROTECTED_AUTHENTICATION_PATH`): the **reader**
  asks for the PIN. The app logs in with a NULL PIN and never prompts
  or sends `""`.

`CertificateInfo.pin_status` (`Option<TokenPinStatus>`) carries the
PKCS#11 flags. Native stores leave it `None`.
`describe_pin_status` (first match) warns the picker:

| Flag | Meaning | Text |
|------|---------|------|
| `locked` | `CKF_USER_PIN_LOCKED` | `Token PIN is locked` |
| `final_try` | `CKF_USER_PIN_FINAL_TRY` | `Last PIN attempt` |
| `count_low` | `CKF_USER_PIN_COUNT_LOW` | `Few PIN attempts remaining` |

`TokenPinStatus` is `#[non_exhaustive]` and may grow.

**DNIe:** the card locks after **3** wrong PINs. A locked DNIe must be
unlocked at a police station. Treat `Last PIN attempt` as the last try
before that lock.

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
- **openpdf-sign** implements PKCS#11, Windows CNG, and macOS Keychain
  login plus CMS/PAdES (`default_signer`, `Pkcs11Signer`,
  `WindowsStoreSigner`, `KeychainSigner`, `cades`, `pades`).
- The **application binary** (not the GTK crate) constructs
  `Box<dyn Signer>` and hands it to UI code that only sees core:

```rust
let pin = /* UI type that implements PinProvider */;
let signer: Box<dyn openpdf_core::Signer> = openpdf_sign::default_signer(pin)?;
```

- **openpdf-gtk must never depend on openpdf-sign** (enforced by
  `gtk_manifest_does_not_depend_on_sign`).

### Roadmap (planned, not available)

**In progress / not yet available.** The first release is meant to cover
Linux, Windows, and macOS.

- **Core and CLI** stay platform-neutral.
- **OCR** behind an `OcrEngine` trait: Tesseract on Linux,
  `Windows.Media.Ocr` on Windows, Vision on macOS.
- **Signing** stays in Rust in `openpdf-sign`. PKCS#11 on all three
  (DNIe everywhere). Windows CurrentUser `MY` and macOS Keychain are
  implemented on [PR #11](https://github.com/ej3mpl0/openPDF/pull/11).
  UIs only see `list_certificates()` and `sign()`.
- **Native UIs** share core through an `openpdf-ffi` crate (UniFFI):
  GTK4/libadwaita on Linux, SwiftUI/AppKit in `apps/macos`, WinUI 3 + C#
  in `apps/windows` (MSIX).
- **CI** matrix on the three platforms.
- **Releases:** one package per platform.

## Status

- **GTK4 UI** (`openpdf-gtk`) is implemented on [PR #1](https://github.com/ej3mpl0/openPDF/pull/1)
  (this docs PR stacks it; not on `main` until #1 merges).
- **OCR, redaction, and compare** are implemented on
  [PR #8](https://github.com/ej3mpl0/openPDF/pull/8) (this docs PR stacks
  that branch).
- **Digital signing** is available in `openpdf-sign` (PAdES / PKCS#11).
  Windows CNG and macOS Keychain backends, plus `CertUnusable` in the
  picker, are on [PR #11](https://github.com/ej3mpl0/openPDF/pull/11).
  The GTK picker is on PR #1. There is no CLI `sign` / `verify`.
- **Core and CLI translations** (English and Spanish) are done. GUI
  catalogs ship with the GTK crate.
- **Release packages** (Flatpak, AppImage, Windows CLI zip, macOS CLI
  tarballs) are produced by [PR #6](https://github.com/ej3mpl0/openPDF/pull/6).
  Flathub is not published.
- **Portable core/CLI** (per-OS config/state dirs, absolute PDFium and
  Tesseract lookup, `sys-locale` + gettext) is on
  [PR #10](https://github.com/ej3mpl0/openPDF/pull/10).

# Contributing

Rules below match what is on `main`. Available crates: **openpdf-core**, the
**openpdf** CLI, and **openpdf-sign** (PKCS#11 PAdES). The GTK UI is not
available.

## Workspace

```text
crates/openpdf-core   document model, tiles, structure ops, i18n, Signer, PinProvider
crates/openpdf-cli    headless `openpdf` CLI (no GTK)
crates/openpdf-sign   PAdES-B-B: PKCS#11, Windows CNG, macOS Keychain
crates/openpdf-gtk    GTK4/libadwaita UI library (never depends on sign)
crates/openpdf-app    `openpdf-gtk` binary (wires UI + default_signer)
po/                   shared gettext catalogs
corpus/               fixture PDFs for tests
```

`openpdf-gtk` and `openpdf-sign` may depend on `openpdf-core` only. They must
never depend on each other. The application binary constructs
`Box<dyn Signer>` from `openpdf-sign` and passes it to UI code. **openpdf-gtk
must never depend on openpdf-sign.**

Rust *stable* (`rust-toolchain.toml`), edition 2021, license MPL-2.0.

## Build and test

From the workspace root (same commands as CI):

```bash
cargo build --workspace --release
cargo test --workspace
```

CLI only:

```bash
cargo build -p openpdf-cli --release
```

`render` / `text` tests need the platform PDFium library
(`libpdfium.so` / `pdfium.dll` / `libpdfium.dylib`; see README). PKCS#11 sign+verify
tests need SoftHSM2 (`libsofthsm2.so`); CI installs `softhsm2` and sets
`OPENPDF_PDFIUM` and `CI=true`.

## i18n

User-facing strings go through `tr()` / `tr_args()` after `init_i18n()`.
English `msgid`s live in source. Translations live only in `po/*.po`.

| File | Role |
|------|------|
| `po/openpdf.pot` | Template: English msgids, empty `msgstr` |
| `po/es.po` | Spanish catalog |
| `po/LINGUAS` | Installed languages (`es`) |
| `po/POTFILES.in` | Sources that contribute msgids (core `error.rs`, `i18n.rs`, `signer.rs`, `document.rs`, `outline.rs`, `ops.rs`, `save.rs`, `render/engine.rs`; CLI `main.rs`; sign `token.rs`, `pades.rs`) |

`openpdf-core` `build.rs` embeds every `po/{lang}.po` (not `openpdf.po`).
Rebuild after catalog edits.

### No Spanish in source

`rust_sources_have_no_spanish_literals` walks `crates/`, `README.md`,
`po/openpdf.pot`, `po/POTFILES.in`, and `po/LINGUAS`. It fails if those files
contain Spanish letters (accented Latin letters or inverted punctuation).
Spanish belongs only in `po/es.po`.

```bash
cargo test -p openpdf-core --lib rust_sources_have_no_spanish_literals
```

### Update `openpdf.pot` and `es.po`

There is no extract script on `main`. Keep the catalogs in lockstep with
`tr()` / `tr_args()` in the files listed by `POTFILES.in`:

1. Write the new English msgid in Rust (`tr("…")` or `tr_args("…", …)`).
2. Add the same msgid to `po/openpdf.pot` with an empty `msgstr`.
3. Add that msgid to `po/es.po` with a Spanish `msgstr`.
4. If you introduce a new source file of user-facing strings, append it to
   `POTFILES.in`.
5. A new language is `po/{lang}.po` plus a line in `po/LINGUAS`.

`es_po_has_msgstr_for_every_msgid` requires a non-empty `msgstr` for every
entry. The only msgid allowed to keep the English text as `msgstr` is
`openpdf: {error}`.

```bash
cargo test -p openpdf-core --lib es_po_has_msgstr_for_every_msgid
```

### Locale lock in tests

`set_locale` / `tr` / `tr_args` share process-wide catalog state. Tests in
the same binary run in parallel, so every test that changes or asserts the
locale must take the shared lock in core:

```rust
openpdf_core::test_support::with_locale("es", || {
    // set_locale + assertions; English is restored afterwards
});
```

Or hold `locale_test_guard()` around a sequence of `set_locale` calls.
Dependents enable the `test-support` feature on `openpdf-core` (CLI and
`openpdf-sign` already do in `[dev-dependencies]`).

## Structured errors

`Error`, `InvalidKind`, and `SignedOp` are matchable types. Callers must match
the variant, not parse `Display`. `Display` / `to_locale_string()` go through
the gettext catalog.

```rust
assert!(matches!(
    err,
    Error::Signed { op: SignedOp::Rotate }
));
```

## Signing and append-only documents

`openpdf-sign` implements `Signer` (PAdES-B-B). Linux uses PKCS#11.
Windows adds CurrentUser `MY` (CNG); macOS adds the Keychain.
`default_signer` combines the native store with PKCS#11.

PKCS#11 collects the PIN only through `PinProvider` into `SecretPin`
(zeroized on drop). Native Windows and macOS backends let the OS show
the PIN or Keychain dialog; they never collect a PIN. It must never
appear on `SignRequest`, `Document`, any stored field, or an
environment variable. There is no user-facing PIN variable. Tests use
`StaticPinProvider` / a test `PinProvider`, never `OPENPDF_PKCS11_PIN`.

`login_user` never calls `C_Login` with an empty, whitespace-only,
missing, or cancelled PIN (`Error::PinRequired` / `PinCancelled`), and
never logs into a locked token (`Error::PinLocked`). A pinpad
(`CKF_PROTECTED_AUTHENTICATION_PATH`) logs in with a NULL PIN so the
reader asks. `CertificateInfo.pin_status` / `describe_pin_status`
surface `CKF_USER_PIN_COUNT_LOW`, `FINAL_TRY`, and `LOCKED`. The DNIe
locks after 3 wrong PINs and must be unlocked at a police station.

`CertificateInfo.unusable` (`CertUnusable`, `#[non_exhaustive]`) marks
rows the picker must disable. Variants today: `AuthenticationOnly`
(DNIe authentication / no `nonRepudiation`), `Expired`, `NotYetValid`,
`UnsupportedKey` (unknown curve or algorithm). The enum may grow;
`match` needs a wildcard. `sign()` still refuses an unusable cert.

A document with a signature dictionary (`/Type /Sig` or `/FT /Sig`) is
locked: `is_signed()` / `is_read_only()` are true. Core refuses rotate,
delete, move, undo, redo, and a full rewrite (`save` / `to_bytes`) with
`Error::Signed`. New signatures and saves are incremental / append-only
(`sign_incremental`, `save_incremental`, `to_incremental_bytes`, `write_to`).
The original bytes stay a prefix of the output.

The CLI has no `sign` or `verify` command. Verification is
`Signer::verify` or `openpdf_sign::verify_pdf`.

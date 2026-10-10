# Security policy

## Reporting a vulnerability

Report security bugs **privately**. Do **not** open a public GitHub
issue, discussion, or pull request for a vulnerability.

Use GitHub private vulnerability reporting / security advisories for
[`ej3mpl0/openPDF`](https://github.com/ej3mpl0/openPDF):

1. Open the repository
   [Security advisories](https://github.com/ej3mpl0/openPDF/security/advisories)
   page.
2. Choose **Report a vulnerability**.
3. Describe the issue, affected code, and how to reproduce it.

GitHub notifies the maintainers. Prefer a temporary private fork on
the advisory if you have a fix.

There is no security-contact email. Do not send reports to addresses
you find in git history or package metadata.

## Supported versions

There is **no stable release** yet. Only the latest `main` branch is
in scope. Tagged builds, if any, are not a supported channel until a
stable version is published.

## Scope

Security reports are welcome for defects that let an attacker read,
alter, or forge documents, steal secrets, or run code. Emphasis:

### Signing

PKCS#11 PAdES signing (`openpdf-sign`): DNIe (OpenSC), FNMT (p11-kit /
NSS), and other tokens; PIN handling (`PinProvider`, `SecretPin` —
never on `SignRequest` or stored on disk); certificate validation
(usage / `nonRepudiation`, expiry, key type); bypasses of incremental
signature locks.

### Redaction

True redaction must permanently remove content. Reports that recovered
text, images, or metadata after `redact` are in scope.

### Password encryption

`protect` / `unprotect` (AES-256). Bypass of the password, leakage of
the password, or encryption that does not actually protect the file.

### Library loading (DLL planting)

PDFium and Tesseract are loaded at runtime. Reports that a relative
path, the current directory, or a planted `pdfium.dll` /
`libpdfium.so` / `libpdfium.dylib` (or Tesseract) is loaded are in
scope. Windows must not search CWD (`SetDefaultDllDirectories`).

### Release artifacts

GitHub Releases attach binaries plus `SHA256SUMS` and artifact
attestations. Reports about a missing or forgeable checksum,
attestation bypass, or a substituted artifact are in scope.

Out of scope unless they have a security impact: feature requests,
docs typos, and crashes that do not leak data or execute code.

## Test credentials

SoftHSM tokens, PINs, and certificates used in tests and CI are
**test-only**. Do not use them in production. Finding that a test PIN
or SoftHSM label works in the test suite is not a vulnerability.

## License

The project is MPL-2.0 (see [LICENSE](LICENSE)).

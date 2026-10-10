# openpdf-ffi

UniFFI crate for native macOS (Swift) and Windows (C#, via `uniffi-bindgen-cs`) UIs.

Owned-data APIs only. Signing (`list_certificates`, `sign`) is implemented in Rust by `openpdf-sign`. Foreign code does not implement `Signer`. PIN is a UniFFI callback (`PinProvider`) that returns `SecretPin`; cancel/empty is `FfiError::PinCancelled` (mapped to core `Error::PinCancelled`, never `Error::Sign`). UIs treat that variant as a silent cancel. `translate` / `pin_prompt` / `describe_pin_status` expose the shared gettext catalog. `OPENPDF_PKCS11_PIN` exists only in `#[cfg(test)]`. Passwords are Zeroizing and never logged.

## Generate bindings

UniFFI **0.29.4** + `uniffi-bindgen-cs` **v0.10.0+v0.29.4**. Generation works on Linux; compiling Swift/C# is not required here.

```bash
cargo build -p openpdf-ffi --release
cargo run -p openpdf-ffi --features bindgen --bin uniffi-bindgen -- \
    generate --library target/release/libopenpdf_ffi.so \
    --language swift --out-dir bindings/swift
cargo install uniffi-bindgen-cs \
    --git https://github.com/NordSecurity/uniffi-bindgen-cs \
    --tag v0.10.0+v0.29.4
uniffi-bindgen-cs --library --out-dir bindings/csharp \
    target/release/libopenpdf_ffi.so
```

On macOS the library is `target/release/libopenpdf_ffi.dylib`; on Windows, `openpdf_ffi.dll`.

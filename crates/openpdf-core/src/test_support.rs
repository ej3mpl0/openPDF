//! Helpers for tests in this crate and dependents (`test-support` feature).
//!
//! `set_locale` / `tr` share process-wide catalog state. Hold
//! [`locale_test_guard`] around any test that changes or asserts the locale.

use std::sync::{Mutex, MutexGuard};

use crate::i18n::set_locale;

static LOCALE_LOCK: Mutex<()> = Mutex::new(());

/// Crate-wide lock for tests that call [`set_locale`] or read `tr` / `tr_args`.
pub fn locale_test_guard() -> MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Set `tag`, run `f`, then restore English. Serializes against other locale tests.
pub fn with_locale(tag: &str, f: impl FnOnce()) {
    let _g = locale_test_guard();
    set_locale(tag);
    f();
    set_locale("en");
}

#[cfg(test)]
pub fn corpus(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name)
}

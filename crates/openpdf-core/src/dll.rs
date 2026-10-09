//! Process-wide dynamic-library search hardening.
//!
//! Windows loads DLLs from the current directory by default (DLL planting).
//! Call [`harden`] once at startup, before any PDFium / Tesseract load.

use std::env;
use std::path::{Path, PathBuf};

/// Drop relative paths. Canonicalize when the file exists.
pub(crate) fn require_absolute(path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return None;
    }
    match std::fs::canonicalize(path) {
        Ok(c) if c.is_absolute() => Some(c),
        _ => Some(path.to_path_buf()),
    }
}

/// `VAR` if it names an absolute path (canonicalized when possible).
pub(crate) fn env_absolute(var: &str) -> Option<PathBuf> {
    let raw = env::var(var).ok()?;
    if raw.is_empty() {
        return None;
    }
    require_absolute(PathBuf::from(raw))
}

/// Absolute directory containing the running executable.
pub(crate) fn exe_dir() -> Option<PathBuf> {
    let exe = env::current_exe().ok()?;
    let exe = require_absolute(exe)?;
    exe.parent().and_then(require_absolute)
}

/// Restrict Windows DLL search to default dirs (no CWD). Idempotent.
pub fn harden() {
    #[cfg(windows)]
    {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;
            extern "system" {
                fn SetDefaultDllDirectories(directory_flags: u32) -> i32;
            }
            unsafe {
                SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_relative_and_bare_names() {
        assert!(require_absolute(Path::new(".")).is_none());
        assert!(require_absolute(Path::new("pdfium.dll")).is_none());
        assert!(require_absolute(Path::new("libpdfium.so")).is_none());
        assert!(require_absolute(Path::new("libpdfium.dylib")).is_none());
        assert!(require_absolute(Path::new("tesseract")).is_none());
        assert!(require_absolute(Path::new("./libpdfium.so")).is_none());
    }

    #[test]
    fn keeps_absolute_paths() {
        #[cfg(unix)]
        {
            let p = require_absolute(Path::new("/usr/lib/libpdfium.so")).unwrap();
            assert!(p.is_absolute());
        }
        #[cfg(windows)]
        {
            let p = require_absolute(Path::new(r"C:\Windows\System32\pdfium.dll")).unwrap();
            assert!(p.is_absolute());
        }
    }
}

//! Password input for encrypted PDFs.
//!
//! Passwords are never taken as command-line values (they would leak through
//! shell history and `ps`). They are read from the terminal without echo or,
//! with `--password-stdin`, one per line from standard input. Every copy we
//! hold is a [`Zeroizing`] string, and none is ever printed.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use openpdf_core::{tr, tr_args, Document, Error, Zeroizing};

static FROM_STDIN: AtomicBool = AtomicBool::new(false);

/// Passwords read from stdin, consumed in order. Each line is `take()`n out
/// when used, so it is dropped (and zeroized) right after the caller is done
/// with it; [`clear`] drops whatever is left before the process exits.
static STDIN_LINES: Mutex<Option<VecDeque<Zeroizing<String>>>> = Mutex::new(None);

pub(crate) fn set_from_stdin(enabled: bool) {
    FROM_STDIN.store(enabled, Ordering::Relaxed);
}

/// Split stdin into password lines. The raw buffer is a `Zeroizing<String>`
/// owned by the caller and wiped when it is dropped.
fn split_lines(all: &Zeroizing<String>) -> VecDeque<Zeroizing<String>> {
    all.lines()
        .map(|l| Zeroizing::new(l.trim_end_matches('\r').to_string()))
        .collect()
}

/// Next unused stdin line (stdin is read once on first use).
fn next_stdin_line() -> Result<Option<Zeroizing<String>>> {
    let mut guard = STDIN_LINES.lock().map_err(|_| anyhow!("stdin lock"))?;
    if guard.is_none() {
        let mut all = Zeroizing::new(String::new());
        std::io::stdin().read_to_string(&mut all)?;
        *guard = Some(split_lines(&all));
    }
    Ok(guard.as_mut().and_then(VecDeque::pop_front))
}

/// Drop (zeroize) any stdin passwords that were never used.
pub(crate) fn clear() {
    if let Ok(mut guard) = STDIN_LINES.lock() {
        guard.take();
    }
}

/// Next password from stdin, or a no-echo terminal prompt.
pub(crate) fn read_password(prompt: &str) -> Result<Zeroizing<String>> {
    if FROM_STDIN.load(Ordering::Relaxed) {
        return next_stdin_line()?.ok_or_else(|| anyhow!("{}", tr("missing password on stdin")));
    }
    // rpassword returns a plain String; wrap it immediately so it is wiped.
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

/// Like [`read_password`] but `None` when the line is absent or empty.
pub(crate) fn read_optional_password(prompt: &str) -> Result<Option<Zeroizing<String>>> {
    let pw = if FROM_STDIN.load(Ordering::Relaxed) {
        next_stdin_line()?
    } else {
        Some(Zeroizing::new(rpassword::prompt_password(prompt)?))
    };
    Ok(pw.filter(|p| !p.is_empty()))
}

/// Open `path`, asking for its password only when the PDF is encrypted.
pub(crate) fn open_doc(path: &Path) -> Result<(Document, Option<Zeroizing<String>>)> {
    match Document::open(path) {
        Err(Error::PasswordRequired) => {
            let prompt = tr_args(
                "Password for {path}: ",
                &[("path", &path.display().to_string())],
            );
            let pw = read_password(&prompt)?;
            let doc = Document::open_with_password(path, Zeroizing::new(pw.to_string()))?;
            Ok((doc, Some(pw)))
        }
        other => Ok((other?, None)),
    }
}

/// [`open_doc`] without keeping the password.
pub(crate) fn open(path: impl AsRef<Path>) -> Result<Document> {
    Ok(open_doc(path.as_ref())?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_lines_are_consumed_once_and_cleared() {
        let all = Zeroizing::new("first\r\nsecond\n\nthird".to_string());
        let lines = split_lines(&all);
        assert_eq!(
            lines.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            ["first", "second", "", "third"]
        );
        *STDIN_LINES.lock().unwrap() = Some(lines);
        set_from_stdin(true);
        assert_eq!(read_password("").unwrap().as_str(), "first");
        assert_eq!(read_password("").unwrap().as_str(), "second");
        assert!(read_optional_password("").unwrap().is_none());
        // Used lines are gone from the static; only "third" is left.
        assert_eq!(STDIN_LINES.lock().unwrap().as_ref().unwrap().len(), 1);
        clear();
        assert!(STDIN_LINES.lock().unwrap().is_none());
    }
}

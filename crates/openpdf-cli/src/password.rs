//! Password input for encrypted PDFs.
//!
//! Passwords are never taken as command-line values (they would leak through
//! shell history and `ps`). They are read from the terminal without echo or,
//! with `--password-stdin`, one per line from standard input. Every copy we
//! hold is a [`Zeroizing`] string, and none is ever printed.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use openpdf_core::{tr, tr_args, Document, Error, Zeroizing};

static FROM_STDIN: AtomicBool = AtomicBool::new(false);
static STDIN_LINES: Mutex<Option<Vec<Zeroizing<String>>>> = Mutex::new(None);

pub(crate) fn set_from_stdin(enabled: bool) {
    FROM_STDIN.store(enabled, Ordering::Relaxed);
}

/// Line `index` of standard input (read once, kept zeroized).
fn stdin_line(index: usize) -> Result<Option<Zeroizing<String>>> {
    let mut guard = STDIN_LINES.lock().map_err(|_| anyhow!("stdin lock"))?;
    if guard.is_none() {
        let mut all = Zeroizing::new(String::new());
        std::io::stdin().read_to_string(&mut all)?;
        let lines = all
            .lines()
            .map(|l| Zeroizing::new(l.trim_end_matches('\r').to_string()))
            .collect();
        *guard = Some(lines);
    }
    Ok(guard
        .as_ref()
        .and_then(|lines| lines.get(index))
        .map(|l| Zeroizing::new(l.to_string())))
}

/// Password `index` (0-based) from stdin, or a no-echo terminal prompt.
pub(crate) fn read_password(prompt: &str, index: usize) -> Result<Zeroizing<String>> {
    if FROM_STDIN.load(Ordering::Relaxed) {
        return stdin_line(index)?.ok_or_else(|| anyhow!("{}", tr("missing password on stdin")));
    }
    // rpassword returns a plain String; wrap it immediately so it is wiped.
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

/// Like [`read_password`] but `None` when the line is absent or empty.
pub(crate) fn read_optional_password(
    prompt: &str,
    index: usize,
) -> Result<Option<Zeroizing<String>>> {
    let pw = if FROM_STDIN.load(Ordering::Relaxed) {
        stdin_line(index)?
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
            let pw = read_password(&prompt, 0)?;
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

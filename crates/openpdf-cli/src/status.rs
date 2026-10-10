//! CLI process status for failures vs a silent PIN cancel.

use std::io::Write;

use openpdf_core::{tr, tr_args};

/// Neutral cancel. Distinct from [`EXIT_ERROR`]; not a failure.
pub const EXIT_CANCEL: i32 = 0;
/// Generic CLI failure (`openpdf: {error}` on stderr).
pub const EXIT_ERROR: i32 = 1;

/// `openpdf_core::Error::PinCancelled` is a unit variant (no payload).
pub fn is_pin_cancelled(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        matches!(
            c.downcast_ref::<openpdf_core::Error>(),
            Some(openpdf_core::Error::PinCancelled)
        )
    })
}

/// Print a CLI outcome and return the process status.
///
/// `PinCancelled` is not an error: one translated line on stdout, status
/// [`EXIT_CANCEL`]. Everything else is `openpdf: {error}` on stderr and
/// [`EXIT_ERROR`].
pub fn report_cli_error(e: &anyhow::Error) -> i32 {
    report_cli_error_to(e, &mut std::io::stdout(), &mut std::io::stderr())
}

pub fn report_cli_error_to(
    e: &anyhow::Error,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> i32 {
    if is_pin_cancelled(e) {
        let _ = writeln!(stdout, "{}", tr("Signing cancelled"));
        return EXIT_CANCEL;
    }
    let _ = writeln!(
        stderr,
        "{}",
        tr_args("openpdf: {error}", &[("error", &format!("{e:#}"))])
    );
    EXIT_ERROR
}

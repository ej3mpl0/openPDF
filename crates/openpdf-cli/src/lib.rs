//! Library surface for the `openpdf` CLI (status reporting + tests).

mod status;

pub use status::{
    is_pin_cancelled, report_cli_error, report_cli_error_to, EXIT_CANCEL, EXIT_ERROR,
};

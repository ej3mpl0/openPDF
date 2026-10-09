use std::fmt;
use std::io;

/// Crate-wide error type. No UI or signing-backend types leak through here.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Structure(String),
    Pdfium(String),
    PageOutOfRange { page: u32, count: u32 },
    EmptyDocument,
    Invalid(String),
    Sign(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Structure(m) => write!(f, "PDF structure: {m}"),
            Error::Pdfium(m) => write!(f, "PDFium: {m}"),
            Error::PageOutOfRange { page, count } => {
                write!(f, "page {page} is out of range (0..{count})")
            }
            Error::EmptyDocument => write!(f, "document has no pages"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::Sign(m) => write!(f, "sign: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Error::Io(value)
    }
}

impl From<lopdf::Error> for Error {
    fn from(value: lopdf::Error) -> Self {
        Error::Structure(value.to_string())
    }
}

impl Error {
    pub fn structure(msg: impl Into<String>) -> Self {
        Error::Structure(msg.into())
    }

    pub fn pdfium(msg: impl Into<String>) -> Self {
        Error::Pdfium(msg.into())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

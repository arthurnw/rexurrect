//! The error type: a message, with context added as it travels up.

use std::fmt;

#[derive(Debug)]
pub struct Error {
    message: String,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<crate::rex::RexError> for Error {
    fn from(e: crate::rex::RexError) -> Error {
        Error::new(e.to_string())
    }
}

/// Adds "what we were doing" to any displayable error.
pub trait Context<T> {
    fn with_context<D: fmt::Display>(self, what: impl FnOnce() -> D) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn with_context<D: fmt::Display>(self, what: impl FnOnce() -> D) -> Result<T> {
        self.map_err(|e| Error::new(format!("{}: {e}", what())))
    }
}

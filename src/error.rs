// Lightweight error type replacing `anyhow::Error` to reduce flash usage.
//
// All ESP-IDF and I/O errors convert via `From` so `?` works transparently.
// Static messages use `&'static str` (zero allocation); dynamic messages use
// `String` without anyhow's vtable/backtrace overhead.

use core::fmt;

/// Crate-wide error type.  Significantly smaller than `anyhow::Error` because
/// it carries no backtrace, no vtable, and no error-chain machinery.
#[derive(Debug)]
pub enum Error {
    /// Static error message — zero heap allocation.
    Msg(&'static str),
    /// Dynamically formatted error message.
    Owned(String),
    /// ESP-IDF system error.
    #[cfg(feature = "esp")]
    Esp(esp_idf_svc::sys::EspError),
    /// Standard I/O error (DNS socket, etc.).
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Msg(s) => f.write_str(s),
            Error::Owned(s) => f.write_str(s),
            #[cfg(feature = "esp")]
            Error::Esp(e) => write!(f, "{}", e),
            Error::Io(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(feature = "esp")]
impl From<esp_idf_svc::sys::EspError> for Error {
    fn from(e: esp_idf_svc::sys::EspError) -> Self {
        Error::Esp(e)
    }
}

#[cfg(feature = "esp")]
impl From<esp_idf_svc::hal::io::EspIOError> for Error {
    fn from(e: esp_idf_svc::hal::io::EspIOError) -> Self {
        Error::Esp(e.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<&'static str> for Error {
    fn from(s: &'static str) -> Self {
        Error::Msg(s)
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Owned(s)
    }
}

impl From<crate::odf::TreeError> for Error {
    fn from(e: crate::odf::TreeError) -> Self {
        Error::Owned(e.to_string())
    }
}

/// Crate-wide `Result` alias.
pub type Result<T> = core::result::Result<T, Error>;

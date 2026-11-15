use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum TrayError {
    Bus(Box<zbus::Error>),
    Worker(io::Error),
    Protocol(&'static str),
    Limit(&'static str),
    Inactive,
}

impl fmt::Display for TrayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(f, "StatusNotifier D-Bus operation failed: {error}"),
            Self::Worker(error) => write!(f, "could not start StatusNotifier worker: {error}"),
            Self::Protocol(message) => write!(f, "invalid StatusNotifier data: {message}"),
            Self::Limit(message) => write!(f, "StatusNotifier data exceeds {message} limit"),
            Self::Inactive => f.write_str("tray activation is inactive"),
        }
    }
}

impl Error for TrayError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error.as_ref()),
            Self::Worker(error) => Some(error),
            Self::Protocol(_) | Self::Limit(_) | Self::Inactive => None,
        }
    }
}

impl From<zbus::Error> for TrayError {
    fn from(error: zbus::Error) -> Self {
        Self::Bus(Box::new(error))
    }
}

impl From<io::Error> for TrayError {
    fn from(error: io::Error) -> Self {
        Self::Worker(error)
    }
}

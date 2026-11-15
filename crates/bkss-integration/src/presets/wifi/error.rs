use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum WifiError {
    Bus(Box<zbus::Error>),
    Worker(io::Error),
    Protocol {
        operation: &'static str,
        message: String,
    },
    Cancelled,
}

impl fmt::Display for WifiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus(error) => write!(f, "NetworkManager D-Bus operation failed: {error}"),
            Self::Worker(error) => write!(f, "start Wi-Fi worker: {error}"),
            Self::Protocol { operation, message } => write!(f, "{operation}: {message}"),
            Self::Cancelled => f.write_str("Wi-Fi operation was cancelled"),
        }
    }
}

impl Error for WifiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error.as_ref()),
            Self::Worker(error) => Some(error),
            Self::Protocol { .. } | Self::Cancelled => None,
        }
    }
}

impl From<zbus::Error> for WifiError {
    fn from(error: zbus::Error) -> Self {
        Self::Bus(Box::new(error))
    }
}

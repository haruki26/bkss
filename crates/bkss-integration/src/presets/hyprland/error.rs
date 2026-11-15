use std::{error::Error, fmt, future::Future, io, path::PathBuf, pin::Pin};

#[derive(Debug)]
pub enum HyprlandError {
    Io {
        operation: &'static str,
        endpoint: PathBuf,
        source: io::Error,
    },
    Json {
        operation: &'static str,
        endpoint: PathBuf,
        source: serde_json::Error,
    },
    Protocol {
        operation: &'static str,
        endpoint: Option<PathBuf>,
        message: String,
    },
}

impl fmt::Display for HyprlandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                endpoint,
                source,
            } => write!(f, "{operation} at {}: {source}", endpoint.display()),
            Self::Json {
                operation,
                endpoint,
                source,
            } => write!(f, "{operation} at {}: {source}", endpoint.display()),
            Self::Protocol {
                operation,
                endpoint,
                message,
            } => match endpoint {
                Some(endpoint) => {
                    write!(f, "{operation} at {}: {message}", endpoint.display())
                }
                None => write!(f, "{operation}: {message}"),
            },
        }
    }
}

impl Error for HyprlandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Protocol { .. } => None,
        }
    }
}

pub type WorkspaceSwitch =
    Pin<Box<dyn Future<Output = Result<(), HyprlandError>> + Send + 'static>>;

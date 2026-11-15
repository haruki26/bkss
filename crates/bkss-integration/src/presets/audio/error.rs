use std::{error::Error, fmt};

#[derive(Debug)]
pub enum AudioError {
    Worker(std::io::Error),
    Connection(String),
    Pulse {
        operation: &'static str,
        cause: libpulse_binding::error::PAErr,
    },
    Backend {
        operation: &'static str,
        cause: String,
    },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Worker(error) => write!(f, "start PulseAudio worker: {error}"),
            Self::Connection(cause) => write!(f, "connect to PulseAudio: {cause}"),
            Self::Pulse { operation, cause } => write!(f, "{operation}: {cause}"),
            Self::Backend { operation, cause } => write!(f, "{operation}: {cause}"),
        }
    }
}

impl Error for AudioError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Worker(error) => Some(error),
            Self::Pulse { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_failures_retain_the_libpulse_cause_and_operation() {
        let cause = libpulse_binding::error::PAErr(1);
        let error = AudioError::Pulse {
            operation: "subscribe",
            cause,
        };
        assert!(error.to_string().contains("subscribe"));
        assert!(error.source().is_some_and(|source| {
            source.downcast_ref::<libpulse_binding::error::PAErr>() == Some(&cause)
        }));
    }
}

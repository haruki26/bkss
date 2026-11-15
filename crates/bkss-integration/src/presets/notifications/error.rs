use std::{error::Error, fmt, io};

#[derive(Debug)]
pub enum NotificationsError {
    Bus {
        operation: &'static str,
        source: Box<zbus::Error>,
    },
    BusClosed,
    NameConflict {
        source: Box<zbus::Error>,
    },
    Worker(io::Error),
    Inactive,
    UnknownNotification(u32),
    UnknownAction {
        id: u32,
        key: String,
    },
}

impl fmt::Display for NotificationsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bus { operation, source } => write!(f, "{operation}: {source}"),
            Self::BusClosed => f.write_str("notifications bus connection closed"),
            Self::NameConflict { .. } => {
                f.write_str("org.freedesktop.Notifications is already owned")
            }
            Self::Worker(source) => write!(f, "start notifications worker: {source}"),
            Self::Inactive => f.write_str("notifications server is inactive"),
            Self::UnknownNotification(id) => write!(f, "notification {id} is not active"),
            Self::UnknownAction { id, key } => {
                write!(f, "notification {id} has no action {key:?}")
            }
        }
    }
}

impl Error for NotificationsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bus { source, .. } => Some(source.as_ref()),
            Self::Worker(source) => Some(source),
            Self::NameConflict { source } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<io::Error> for NotificationsError {
    fn from(source: io::Error) -> Self {
        Self::Worker(source)
    }
}

pub(super) fn bus(operation: &'static str, source: zbus::Error) -> NotificationsError {
    if matches!(&source, zbus::Error::NameTaken) {
        NotificationsError::NameConflict {
            source: Box::new(source),
        }
    } else {
        NotificationsError::Bus {
            operation,
            source: Box::new(source),
        }
    }
}

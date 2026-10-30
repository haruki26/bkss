use gpui::DisplayId;
use std::{error::Error, fmt, rc::Rc, time::Duration};

/// A fatal failure while waiting for or creating a requested surface.
#[derive(Clone)]
pub enum SurfaceError {
    DisplayReadinessTimeout {
        display_id: DisplayId,
        timeout: Duration,
    },
    Creation {
        display_id: DisplayId,
        cause: Rc<dyn Error + Send + Sync>,
    },
}

impl SurfaceError {
    pub fn display_id(&self) -> DisplayId {
        match self {
            Self::DisplayReadinessTimeout { display_id, .. }
            | Self::Creation { display_id, .. } => *display_id,
        }
    }
}

impl fmt::Debug for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayReadinessTimeout {
                display_id,
                timeout,
            } => f
                .debug_struct("DisplayReadinessTimeout")
                .field("display_id", display_id)
                .field("timeout", timeout)
                .finish(),
            Self::Creation { display_id, cause } => f
                .debug_struct("SurfaceCreation")
                .field("display_id", display_id)
                .field("cause", cause)
                .finish(),
        }
    }
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayReadinessTimeout {
                display_id,
                timeout,
            } => write!(
                f,
                "timed out after {timeout:?} waiting for display {display_id:?}"
            ),
            Self::Creation { display_id, cause } => {
                write!(
                    f,
                    "failed to create surface on display {display_id:?}: {cause}"
                )
            }
        }
    }
}

impl Error for SurfaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Creation { cause, .. } => Some(cause.as_ref()),
            Self::DisplayReadinessTimeout { .. } => None,
        }
    }
}

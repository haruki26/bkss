use super::surface::{SurfaceDescriptorError, SurfaceError};
use crate::{ResourceAcquireError, SourceFailure};
use std::{error::Error, fmt, rc::Rc};

/// Error returned by shell setup or a fatal source failure.
#[derive(Clone)]
pub enum ShellError {
    Source(Rc<SourceFailure>),
    Surface(Rc<SurfaceError>),
    Setup(Rc<dyn Error>),
}

impl ShellError {
    pub fn setup(error: impl Error + 'static) -> Self {
        Self::Setup(Rc::new(error))
    }

    pub(super) fn surface(error: SurfaceError) -> Self {
        Self::Surface(Rc::new(error))
    }
}

impl fmt::Debug for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => f.debug_tuple("Source").field(error).finish(),
            Self::Surface(error) => f.debug_tuple("Surface").field(error).finish(),
            Self::Setup(error) => f.debug_tuple("Setup").field(error).finish(),
        }
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => write!(f, "shell source failure: {error}"),
            Self::Surface(error) => write!(f, "shell surface failure: {error}"),
            Self::Setup(error) => write!(f, "shell setup failed: {error}"),
        }
    }
}

impl Error for ShellError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error.as_ref()),
            Self::Surface(error) => Some(error.as_ref()),
            Self::Setup(error) => Some(error.as_ref()),
        }
    }
}

impl From<ResourceAcquireError> for ShellError {
    fn from(error: ResourceAcquireError) -> Self {
        match error {
            ResourceAcquireError::Failed(failure) => Self::Source(failure),
            error @ (ResourceAcquireError::Closed | ResourceAcquireError::Busy) => {
                Self::setup(error)
            }
        }
    }
}

impl From<SurfaceDescriptorError> for ShellError {
    fn from(error: SurfaceDescriptorError) -> Self {
        Self::setup(error)
    }
}

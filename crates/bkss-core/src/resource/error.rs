use std::{any::TypeId, error::Error, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceFailurePhase {
    Start,
    Subscription,
}

/// A source failure retaining its original typed error as the error source.
pub struct SourceFailure {
    pub(super) source_type: TypeId,
    pub(super) phase: SourceFailurePhase,
    pub(super) cause: Box<dyn Error>,
}

impl SourceFailure {
    pub fn source_type(&self) -> TypeId {
        self.source_type
    }
    pub fn phase(&self) -> SourceFailurePhase {
        self.phase
    }
}

impl std::fmt::Debug for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFailure")
            .field("source_type", &self.source_type)
            .field("phase", &self.phase)
            .field("cause", &self.cause)
            .finish()
    }
}
impl std::fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "source {:?} failed during {:?}: {}",
            self.source_type, self.phase, self.cause
        )
    }
}
impl Error for SourceFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Debug)]
pub enum ResourceAcquireError {
    Closed,
    Busy,
    Failed(Rc<SourceFailure>),
}

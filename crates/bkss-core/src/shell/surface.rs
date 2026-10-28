use gpui::{App, DisplayId, Entity, Pixels, Render, Size, Window};
use std::{
    error::Error,
    fmt,
    rc::Rc,
    time::{Duration, Instant},
};

pub const DEFAULT_DISPLAY_READINESS_TIMEOUT: Duration = Duration::from_secs(5);
pub const DISPLAY_POLL_INTERVAL: Duration = Duration::from_millis(50);
pub(super) const SURFACE_NAMESPACE: &str = "bkss-core-surface";
pub(super) type ContentFactory<V> = Box<dyn FnOnce(&mut Window, &mut App) -> Entity<V>>;

/// An inert declaration for one non-interactive layer-shell surface.
///
/// `size` is expressed in GPUI logical pixels and each dimension must be at
/// least one pixel. The content factory is called at most once, after the
/// requested display appears. Register this during shell setup with
/// [`RuntimeHandle::mount_surface`](super::RuntimeHandle::mount_surface).
pub struct SurfaceDescriptor<V: Render + 'static> {
    pub(super) display_id: DisplayId,
    pub(super) size: Size<Pixels>,
    pub(super) readiness_timeout: Duration,
    pub(super) content: ContentFactory<V>,
}

impl<V: Render + 'static> SurfaceDescriptor<V> {
    /// Creates a descriptor for a single display and a finite size of at least one logical pixel.
    /// The fixed namespace is `bkss-core-surface`.
    pub fn new(
        display_id: DisplayId,
        size: Size<Pixels>,
        content: impl FnOnce(&mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Result<Self, SurfaceDescriptorError> {
        if ![size.width, size.height]
            .into_iter()
            .all(|dimension| f32::from(dimension).is_finite() && f32::from(dimension) >= 1.0)
        {
            return Err(SurfaceDescriptorError::InvalidSize(size));
        }
        Ok(Self {
            display_id,
            size,
            readiness_timeout: DEFAULT_DISPLAY_READINESS_TIMEOUT,
            content: Box::new(content),
        })
    }

    /// Sets the readiness deadline. A zero timeout fails immediately, even if
    /// the display is already present when readiness is first checked.
    pub fn readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SurfaceDescriptorError {
    InvalidSize(Size<Pixels>),
}

impl fmt::Display for SurfaceDescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize(size) => write!(
                f,
                "surface dimensions must be finite and at least one logical pixel, got {:?}",
                size
            ),
        }
    }
}

impl Error for SurfaceDescriptorError {}

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

pub(super) struct DisplayReadiness {
    started: Instant,
    timeout: Duration,
    finished: bool,
}

pub(super) enum Readiness {
    Waiting,
    Ready,
    TimedOut,
    Finished,
}

impl DisplayReadiness {
    pub(super) fn new(timeout: Duration, started: Instant) -> Self {
        Self {
            started,
            timeout,
            finished: false,
        }
    }

    pub(super) fn poll(&mut self, found: bool, now: Instant) -> Readiness {
        if self.finished {
            return Readiness::Finished;
        }
        if now.duration_since(self.started) >= self.timeout {
            self.finished = true;
            return Readiness::TimedOut;
        }
        if found {
            self.finished = true;
            return Readiness::Ready;
        }
        Readiness::Waiting
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_DISPLAY_READINESS_TIMEOUT, DisplayReadiness, Readiness, SurfaceDescriptor,
        SurfaceError,
    };
    use crate::ShellError;
    use gpui::{AppContext, DisplayId, Size, px};
    use std::{
        error::Error,
        rc::Rc,
        time::{Duration, Instant},
    };

    #[test]
    fn readiness_deadline_precedes_presence_and_terminal_states_do_not_repeat() {
        let start = Instant::now();
        let timeout = Duration::from_millis(10);

        let mut before_deadline = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            before_deadline.poll(false, start),
            Readiness::Waiting
        ));
        assert!(matches!(
            before_deadline.poll(true, start + timeout - Duration::from_nanos(1)),
            Readiness::Ready
        ));
        assert!(matches!(
            before_deadline.poll(true, start + timeout),
            Readiness::Finished
        ));

        let mut exact_deadline = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            exact_deadline.poll(true, start + timeout),
            Readiness::TimedOut
        ));
        assert!(matches!(
            exact_deadline.poll(true, start),
            Readiness::Finished
        ));

        let mut delayed_presence = DisplayReadiness::new(timeout, start);
        assert!(matches!(
            delayed_presence.poll(false, start),
            Readiness::Waiting
        ));
        assert!(matches!(
            delayed_presence.poll(true, start + timeout + Duration::from_millis(40)),
            Readiness::TimedOut
        ));
        assert!(matches!(
            delayed_presence.poll(false, start),
            Readiness::Finished
        ));

        let mut zero_timeout = DisplayReadiness::new(Duration::ZERO, start);
        assert!(matches!(
            zero_timeout.poll(true, start),
            Readiness::TimedOut
        ));
        assert!(matches!(
            zero_timeout.poll(true, start),
            Readiness::Finished
        ));
    }

    #[test]
    fn invalid_size_is_rejected_when_descriptor_is_built() {
        let result = SurfaceDescriptor::<gpui::EmptyView>::new(
            DisplayId::new(1),
            Size::new(px(0.), px(10.)),
            |_, cx| cx.new(|_| gpui::EmptyView),
        );
        assert!(result.is_err());
    }

    #[test]
    fn creation_error_keeps_target_and_original_cause_in_shell_error_chain() {
        let target = DisplayId::new(7);
        let cause: Rc<dyn Error + Send + Sync> = Rc::new(std::io::Error::other("open failed"));
        let error = ShellError::surface(SurfaceError::Creation {
            display_id: target,
            cause,
        });
        let ShellError::Surface(surface_error) = error else {
            panic!("expected surface error")
        };
        assert_eq!(surface_error.display_id(), target);
        assert_eq!(
            surface_error
                .source()
                .and_then(|cause| cause.downcast_ref::<std::io::Error>())
                .map(std::io::Error::to_string)
                .as_deref(),
            Some("open failed")
        );
        assert_eq!(
            surface_error
                .as_ref()
                .source()
                .and_then(|cause| cause.downcast_ref::<std::io::Error>())
                .map(std::io::Error::to_string)
                .as_deref(),
            Some("open failed")
        );
    }

    #[test]
    fn readiness_default_is_five_seconds() {
        assert_eq!(DEFAULT_DISPLAY_READINESS_TIMEOUT, Duration::from_secs(5));
    }
}

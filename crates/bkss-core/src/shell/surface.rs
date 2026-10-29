use gpui::{App, Bounds, DisplayId, Entity, Pixels, Render, Size, Window};
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
pub(super) type LayerContentFactory<V> =
    Box<dyn FnMut(DisplayId, &mut Window, &mut App) -> Entity<V>>;

/// Selects the displays targeted by a layer-surface declaration.
#[derive(Clone, Debug, Default)]
pub enum DisplaySelection {
    /// Freeze the connected-display list at registration, or wait for the first
    /// nonempty list when none are connected yet.
    #[default]
    AllConnected,
    /// Target these display IDs, independently of one another.
    Explicit(Vec<DisplayId>),
}

/// Geometry and low-level layer-shell options for one target display.
pub struct LayerSurfaceConfig {
    /// Logical size of the surface.
    pub size: Size<Pixels>,
    /// GPUI layer-shell options passed to window creation.
    pub options: gpui::layer_shell::LayerShellOptions,
}

/// An inert declaration for layer surfaces that can target multiple displays.
pub struct LayerSurfaceDeclaration<V: Render + 'static> {
    pub(super) displays: DisplaySelection,
    pub(super) configure: Box<dyn Fn(DisplayId, Bounds<Pixels>) -> LayerSurfaceConfig>,
    pub(super) content: LayerContentFactory<V>,
    pub(super) readiness_timeout: Duration,
}

impl<V: Render + 'static> LayerSurfaceDeclaration<V> {
    /// Creates an inert declaration. `configure` receives the target display's
    /// full logical bounds once that display is ready.
    pub fn new(
        displays: DisplaySelection,
        configure: impl Fn(DisplayId, Bounds<Pixels>) -> LayerSurfaceConfig + 'static,
        content: impl FnMut(DisplayId, &mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Self {
        Self {
            displays,
            configure: Box::new(configure),
            content: Box::new(content),
            readiness_timeout: DEFAULT_DISPLAY_READINESS_TIMEOUT,
        }
    }

    /// Sets the display readiness deadline, measured from registration.
    pub fn readiness_timeout(mut self, timeout: Duration) -> Self {
        self.readiness_timeout = timeout;
        self
    }
}

/// Errors specific to declaring or configuring a multi-display layer surface.
#[derive(Clone, Copy, Debug)]
pub enum LayerSurfaceError {
    /// No connected displays appeared before the declaration's deadline.
    DisplayEnumerationTimeout { timeout: Duration },
    /// An explicit display selection did not contain any targets.
    EmptyDisplaySelection,
    /// The returned surface dimensions are invalid for the target display.
    InvalidConfiguration { size: Size<Pixels> },
}

impl fmt::Display for LayerSurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayEnumerationTimeout { timeout } => write!(
                f,
                "timed out after {timeout:?} waiting for a connected display"
            ),
            Self::EmptyDisplaySelection => {
                f.write_str("an explicit display selection must contain at least one display")
            }
            Self::InvalidConfiguration { size } => write!(
                f,
                "surface dimensions must be finite and at least one logical pixel, got {size:?}"
            ),
        }
    }
}

impl Error for LayerSurfaceError {}

pub(super) struct StartupTargets {
    targets: Vec<StartupTarget>,
    waiting_for_displays: bool,
    started: Instant,
    timeout: Duration,
    finished: bool,
}

struct StartupTarget {
    display_id: DisplayId,
    initially_ready: Option<Bounds<Pixels>>,
    emitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StartupTargetError {
    EnumerationTimeout {
        timeout: Duration,
    },
    DisplayTimeout {
        display_id: DisplayId,
        timeout: Duration,
    },
}

impl StartupTargets {
    pub(super) fn new(
        selection: DisplaySelection,
        snapshot: Vec<(DisplayId, Bounds<Pixels>)>,
        started: Instant,
        timeout: Duration,
    ) -> Result<Self, LayerSurfaceError> {
        let mut targets = Vec::new();
        let waiting_for_displays;
        match selection {
            DisplaySelection::AllConnected if snapshot.is_empty() => {
                waiting_for_displays = true;
            }
            DisplaySelection::AllConnected => {
                waiting_for_displays = false;
                targets.extend(
                    snapshot
                        .into_iter()
                        .map(|(display_id, bounds)| StartupTarget {
                            display_id,
                            initially_ready: Some(bounds),
                            emitted: false,
                        }),
                );
            }
            DisplaySelection::Explicit(display_ids) if display_ids.is_empty() => {
                return Err(LayerSurfaceError::EmptyDisplaySelection);
            }
            DisplaySelection::Explicit(display_ids) => {
                waiting_for_displays = false;
                for display_id in display_ids {
                    if targets
                        .iter()
                        .any(|target: &StartupTarget| target.display_id == display_id)
                    {
                        continue;
                    }
                    let initially_ready = snapshot
                        .iter()
                        .find(|(present_id, _)| *present_id == display_id)
                        .map(|(_, bounds)| *bounds);
                    targets.push(StartupTarget {
                        display_id,
                        initially_ready,
                        emitted: false,
                    });
                }
            }
        }

        Ok(Self {
            targets,
            waiting_for_displays,
            started,
            timeout,
            finished: false,
        })
    }

    pub(super) fn poll(
        &mut self,
        snapshot: &[(DisplayId, Bounds<Pixels>)],
        now: Instant,
    ) -> Result<Vec<(DisplayId, Bounds<Pixels>)>, StartupTargetError> {
        if self.finished {
            return Ok(Vec::new());
        }

        if self.timeout.is_zero() {
            self.finished = true;
            return if self.waiting_for_displays {
                Err(StartupTargetError::EnumerationTimeout {
                    timeout: self.timeout,
                })
            } else {
                Err(StartupTargetError::DisplayTimeout {
                    display_id: self.targets[0].display_id,
                    timeout: self.timeout,
                })
            };
        }

        if self.waiting_for_displays {
            if now.duration_since(self.started) >= self.timeout {
                self.finished = true;
                return Err(StartupTargetError::EnumerationTimeout {
                    timeout: self.timeout,
                });
            }
            if !snapshot.is_empty() {
                self.waiting_for_displays = false;
                for (display_id, bounds) in snapshot {
                    self.targets.push(StartupTarget {
                        display_id: *display_id,
                        initially_ready: Some(*bounds),
                        emitted: false,
                    });
                }
            }
        }

        let mut ready = Vec::new();
        for target in &mut self.targets {
            if target.emitted {
                continue;
            }
            if let Some(bounds) = target.initially_ready.take() {
                target.emitted = true;
                ready.push((target.display_id, bounds));
            } else if let Some((_, bounds)) = snapshot
                .iter()
                .find(|(display_id, _)| *display_id == target.display_id)
            {
                if now.duration_since(self.started) >= self.timeout {
                    return if ready.is_empty() {
                        self.finished = true;
                        Err(StartupTargetError::DisplayTimeout {
                            display_id: target.display_id,
                            timeout: self.timeout,
                        })
                    } else {
                        Ok(ready)
                    };
                }
                target.emitted = true;
                ready.push((target.display_id, *bounds));
            }
        }

        if !ready.is_empty() {
            self.finished = self.targets.iter().all(|target| target.emitted);
            return Ok(ready);
        }

        if now.duration_since(self.started) >= self.timeout {
            self.finished = true;
            if self.waiting_for_displays {
                return Err(StartupTargetError::EnumerationTimeout {
                    timeout: self.timeout,
                });
            }
            if let Some(target) = self.targets.iter().find(|target| !target.emitted) {
                return Err(StartupTargetError::DisplayTimeout {
                    display_id: target.display_id,
                    timeout: self.timeout,
                });
            }
        }

        self.finished =
            !self.waiting_for_displays && self.targets.iter().all(|target| target.emitted);
        Ok(ready)
    }

    pub(super) fn finished(&self) -> bool {
        self.finished
    }
}

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
        DEFAULT_DISPLAY_READINESS_TIMEOUT, DisplayReadiness, DisplaySelection,
        LayerSurfaceDeclaration, Readiness, StartupTargetError, StartupTargets, SurfaceDescriptor,
        SurfaceError,
    };
    use crate::ShellError;
    use gpui::{AppContext, Bounds, DisplayId, Size, point, px};
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

    #[test]
    fn layer_surface_declaration_is_inert_and_uses_the_default_timeout() {
        let configured = Rc::new(std::cell::Cell::new(false));
        let configured_by_callback = Rc::clone(&configured);
        let declaration = LayerSurfaceDeclaration::<gpui::EmptyView>::new(
            DisplaySelection::default(),
            move |_, _| {
                configured_by_callback.set(true);
                super::LayerSurfaceConfig {
                    size: Size::new(px(48.), px(20.)),
                    options: gpui::layer_shell::LayerShellOptions::default(),
                }
            },
            |_, _, cx| cx.new(|_| gpui::EmptyView),
        );

        assert!(matches!(
            declaration.displays,
            DisplaySelection::AllConnected
        ));
        assert_eq!(
            declaration.readiness_timeout,
            DEFAULT_DISPLAY_READINESS_TIMEOUT
        );
        assert!(!configured.get());
        assert_eq!(
            declaration
                .readiness_timeout(Duration::from_secs(1))
                .readiness_timeout,
            Duration::from_secs(1)
        );
        assert!(!configured.get());
    }

    fn display(id: u64, x: f32) -> (DisplayId, Bounds<gpui::Pixels>) {
        (
            DisplayId::new(id),
            Bounds::new(point(px(x), px(0.)), Size::new(px(100.), px(50.))),
        )
    }

    #[test]
    fn all_connected_freezes_nonempty_registration_snapshot_and_keeps_bounds() {
        let start = Instant::now();
        let a = display(1, 10.);
        let b = display(2, 20.);
        let c = display(3, 30.);
        let mut targets = StartupTargets::new(
            DisplaySelection::AllConnected,
            vec![a, b],
            start,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(targets.poll(&[c], start).unwrap(), vec![a, b]);
        assert!(targets.finished());
        assert!(
            targets
                .poll(&[], start + Duration::from_secs(2))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn initially_ready_targets_survive_late_first_poll() {
        let start = Instant::now();
        let a = display(1, 10.);
        let mut targets = StartupTargets::new(
            DisplaySelection::AllConnected,
            vec![a],
            start,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(
            targets.poll(&[], start + Duration::from_secs(5)).unwrap(),
            vec![a]
        );
        assert!(targets.finished());
    }

    #[test]
    fn empty_all_connected_waits_then_freezes_first_nonempty_snapshot() {
        let start = Instant::now();
        let a = display(1, 10.);
        let b = display(2, 20.);
        let c = display(3, 30.);
        let mut targets = StartupTargets::new(
            DisplaySelection::AllConnected,
            vec![],
            start,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(targets.poll(&[], start).unwrap().is_empty());
        assert_eq!(
            targets
                .poll(&[a, b], start + Duration::from_millis(1))
                .unwrap(),
            vec![a, b]
        );
        assert_eq!(
            targets
                .poll(&[c], start + Duration::from_millis(2))
                .unwrap(),
            vec![]
        );
        assert!(targets.finished());
    }

    #[test]
    fn explicit_targets_are_deduplicated_and_ready_targets_do_not_wait_for_missing_ones() {
        let start = Instant::now();
        let a = display(1, 10.);
        let b = display(2, 20.);
        let mut targets = StartupTargets::new(
            DisplaySelection::Explicit(vec![a.0, b.0, a.0]),
            vec![a],
            start,
            Duration::from_secs(1),
        )
        .unwrap();

        assert_eq!(targets.poll(&[], start).unwrap(), vec![a]);
        assert_eq!(
            targets
                .poll(&[b], start + Duration::from_millis(1))
                .unwrap(),
            vec![b]
        );
        assert!(targets.finished());
    }

    #[test]
    fn exact_and_zero_deadlines_precede_new_presence_and_missing_explicit_target_times_out() {
        let start = Instant::now();
        let a = display(1, 10.);
        let b = display(2, 20.);
        let timeout = Duration::from_secs(1);
        let mut targets = StartupTargets::new(
            DisplaySelection::Explicit(vec![a.0, b.0]),
            vec![a],
            start,
            timeout,
        )
        .unwrap();
        assert_eq!(targets.poll(&[], start).unwrap(), vec![a]);
        assert_eq!(
            targets.poll(&[b], start + timeout),
            Err(StartupTargetError::DisplayTimeout {
                display_id: DisplayId::new(2),
                timeout,
            })
        );
        assert!(targets.finished());

        let mut zero = StartupTargets::new(
            DisplaySelection::Explicit(vec![DisplayId::new(1)]),
            vec![display(1, 10.)],
            start,
            Duration::ZERO,
        )
        .unwrap();
        assert!(matches!(
            zero.poll(&[a], start),
            Err(StartupTargetError::DisplayTimeout { .. })
        ));
    }

    #[test]
    fn empty_all_connected_times_out_and_empty_explicit_selection_is_invalid() {
        let start = Instant::now();
        let timeout = Duration::from_secs(1);
        let mut all =
            StartupTargets::new(DisplaySelection::AllConnected, vec![], start, timeout).unwrap();
        assert_eq!(
            all.poll(&[], start + timeout),
            Err(StartupTargetError::EnumerationTimeout { timeout })
        );
        assert!(matches!(
            StartupTargets::new(DisplaySelection::Explicit(vec![]), vec![], start, timeout),
            Err(super::LayerSurfaceError::EmptyDisplaySelection)
        ));
    }
}

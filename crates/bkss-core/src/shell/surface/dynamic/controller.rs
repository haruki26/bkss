use crate::shell::{ShellError, runtime::RuntimeInner};
use crate::{
    AnimationPlan, AnimationSample, DisplaySelection, LayerSurfaceConfig, Phase,
    surface::{
        DISPLAY_POLL_INTERVAL, DisplayReadiness, LayerSurfaceError, Readiness, StartupTargetError,
        StartupTargets,
    },
};

use super::super::animation::SurfaceAnimation;
use gpui::{
    AnyView, App, AppContext, DisplayId, Entity, ParentElement, Render, Subscription, Task, Window,
    div,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    error::Error,
    fmt,
    rc::{Rc, Weak},
    time::Duration,
};

type Factory =
    Box<dyn FnMut(SurfaceInstanceHandle, &mut Window, &mut App) -> Result<AnyView, ShellError>>;

pub(in crate::shell) struct Controller {
    pub(super) runtime: Weak<RuntimeInner>,
    config: LayerSurfaceConfig,
    timeout: Duration,
    animation_plan: Option<AnimationPlan>,
    pub(super) deferred_animation: bool,
    factory: RefCell<Option<Factory>>,
    next_generation: Cell<u64>,
    pub(super) entries: RefCell<HashMap<DisplayId, Entry>>,
    pub(super) stopped: Cell<bool>,
    startup_requested: Cell<bool>,
    startup_task: RefCell<Option<Task<()>>>,
    window_closed: RefCell<Option<Subscription>>,
}

pub(super) enum Entry {
    Pending {
        generation: u64,
        active: Rc<Cell<bool>>,
        task: Option<Task<()>>,
        animation: Option<SurfaceAnimation>,
    },
    Mounted {
        generation: u64,
        window: gpui::AnyWindowHandle,
        animation: Option<SurfaceAnimation>,
    },
}

#[derive(Clone)]
/// Weak control handle; it does not extend the attached runtime's lifetime.
pub struct DynamicSurfaceHandle(Weak<Controller>);

#[derive(Clone)]
pub struct SurfaceInstanceHandle {
    controller: Weak<Controller>,
    display_id: DisplayId,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceRequestError {
    RuntimeClosed,
    Unavailable,
}

/// Configuration failure reason used to preserve the UI crate's public setup errors.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicSurfaceConfigError {
    InvalidDimension,
    ZeroDimensionWithoutOpposingAnchors,
    InvalidExclusiveZone,
    UnknownAnchorBits,
}

impl fmt::Display for DynamicSurfaceConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidDimension | Self::ZeroDimensionWithoutOpposingAnchors => {
                "invalid dynamic layer surface size"
            }
            Self::InvalidExclusiveZone => "invalid dynamic layer surface exclusive zone",
            Self::UnknownAnchorBits => "invalid dynamic layer surface options",
        })
    }
}

impl Error for DynamicSurfaceConfigError {}

fn validate_dynamic_surface_dimensions(
    config: &LayerSurfaceConfig,
) -> Result<(), DynamicSurfaceConfigError> {
    let anchors = config.options.anchor;
    for (dimension, opposing) in [
        (
            config.size.width,
            anchors.contains(gpui::layer_shell::Anchor::LEFT | gpui::layer_shell::Anchor::RIGHT),
        ),
        (
            config.size.height,
            anchors.contains(gpui::layer_shell::Anchor::TOP | gpui::layer_shell::Anchor::BOTTOM),
        ),
    ] {
        let value = f32::from(dimension);
        if !value.is_finite() || value < 0. || f64::from(value) > f64::from(i32::MAX) {
            return Err(DynamicSurfaceConfigError::InvalidDimension);
        }
        if value == 0. && !opposing {
            return Err(DynamicSurfaceConfigError::ZeroDimensionWithoutOpposingAnchors);
        }
    }
    Ok(())
}

fn validate_dynamic_surface_exclusive_zone(
    config: &LayerSurfaceConfig,
) -> Result<(), DynamicSurfaceConfigError> {
    if config.options.exclusive_zone.is_some_and(|zone| {
        let value = f32::from(zone);
        !value.is_finite() || (value < 0. && value != -1.) || f64::from(value) > f64::from(i32::MAX)
    }) {
        return Err(DynamicSurfaceConfigError::InvalidExclusiveZone);
    }
    Ok(())
}

fn validate_dynamic_surface_anchors(
    config: &LayerSurfaceConfig,
) -> Result<(), DynamicSurfaceConfigError> {
    if config.options.anchor.bits() & !gpui::layer_shell::Anchor::all().bits() != 0 {
        return Err(DynamicSurfaceConfigError::UnknownAnchorBits);
    }
    Ok(())
}

/// Validates native settings for the UI declaration boundary in its historical order.
#[doc(hidden)]
pub fn validate_dynamic_surface_config_for_ui(
    config: &LayerSurfaceConfig,
) -> Result<(), DynamicSurfaceConfigError> {
    validate_dynamic_surface_anchors(config)?;
    validate_dynamic_surface_dimensions(config)?;
    validate_dynamic_surface_exclusive_zone(config)
}

impl fmt::Display for SurfaceRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RuntimeClosed => "shell runtime is closed",
            Self::Unavailable => "dynamic surface controller is unavailable",
        })
    }
}

impl Error for SurfaceRequestError {}

impl DynamicSurfaceHandle {
    /// Opens this surface on a display. After runtime shutdown this is a no-op.
    pub fn open(&self, display: DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        let controller = self.0.upgrade().ok_or(SurfaceRequestError::Unavailable)?;
        controller.open(display, cx)
    }

    pub fn close(&self, display: DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        let Some(controller) = self.0.upgrade() else {
            return Ok(());
        };
        controller.close(display, cx)
    }

    /// Opens this surface on the selected startup displays once per controller.
    /// Repeated calls after the first successful request are no-ops.
    pub fn open_on_startup(
        &self,
        selection: DisplaySelection,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        let controller = self
            .0
            .upgrade()
            .ok_or_else(|| ShellError::setup(SurfaceRequestError::Unavailable))?;
        controller.open_on_startup(selection, cx)
    }
}

impl SurfaceInstanceHandle {
    pub fn display_id(&self) -> DisplayId {
        self.display_id
    }

    pub fn close(&self, cx: &mut App) -> Result<(), SurfaceRequestError> {
        let Some(controller) = self.controller.upgrade() else {
            return Ok(());
        };
        controller.close_generation(self.display_id, self.generation, cx)
    }

    /// Returns the latest runtime-owned animation sample without advancing it.
    #[doc(hidden)]
    pub fn animation_sample(&self) -> AnimationSample {
        self.controller
            .upgrade()
            .and_then(|controller| controller.animation_sample(self.display_id, self.generation))
            .unwrap_or(AnimationSample {
                opacity: None,
                translation_x: None,
                translation_y: None,
                running: false,
                completion: None,
            })
    }

    /// Initializes deferred animation after the first valid content layout.
    #[doc(hidden)]
    pub fn initialize_animation(
        &self,
        plan: AnimationPlan,
        cx: &mut App,
    ) -> Result<AnimationSample, ShellError> {
        let controller = self
            .controller
            .upgrade()
            .ok_or_else(|| ShellError::setup(SurfaceRequestError::Unavailable))?;
        controller.initialize_animation(self.display_id, self.generation, plan, cx)
    }
}

impl Controller {
    fn open(self: &Rc<Self>, display: DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        if self.stopped.get()
            || self
                .runtime
                .upgrade()
                .is_none_or(|runtime| runtime.stopped.get())
        {
            return Err(SurfaceRequestError::RuntimeClosed);
        }
        if self.entries.borrow().contains_key(&display) {
            self.reopen(display, cx);
            return Ok(());
        }
        let generation = self
            .next_generation
            .get()
            .checked_add(1)
            .ok_or(SurfaceRequestError::Unavailable)?;
        self.next_generation.set(generation);
        let active = Rc::new(Cell::new(true));
        self.entries.borrow_mut().insert(
            display,
            Entry::Pending {
                generation,
                active: Rc::clone(&active),
                task: None,
                animation: None,
            },
        );
        let weak = Rc::downgrade(self);
        let started = cx.background_executor().now();
        let timeout = self.timeout;
        let task = cx.spawn(async move |cx| {
            let mut readiness = DisplayReadiness::new(timeout, started);
            loop {
                let Some(controller) = weak.upgrade() else {
                    return;
                };
                let Some(runtime) = controller.runtime.upgrade() else {
                    return;
                };
                if !active.get()
                    || runtime.stopped.get()
                    || !controller.pending(display, generation)
                {
                    return;
                }
                let present =
                    cx.update(|app| app.displays().iter().any(|item| item.id() == display));
                let now = cx.background_executor().now();
                match readiness.poll(present, now) {
                    Readiness::TimedOut => {
                        let was_current = active.get()
                            && controller.pending(display, generation)
                            && controller
                                .runtime
                                .upgrade()
                                .is_some_and(|runtime| !runtime.stopped.get());
                        let current = controller.clear_pending(display, generation, cx);
                        if current
                            && was_current
                            && let Some(runtime) = controller.runtime.upgrade()
                        {
                            cx.update(|app| {
                                crate::shell::runtime::record_surface_failure(
                                    &runtime,
                                    crate::SurfaceError::DisplayReadinessTimeout {
                                        display_id: display,
                                        timeout,
                                    },
                                    app,
                                )
                            });
                        }
                        return;
                    }
                    Readiness::Ready => {
                        controller.create(display, generation, &active, cx).await;
                        return;
                    }
                    Readiness::Finished => return,
                    Readiness::Waiting => {}
                }
                drop(runtime);
                drop(controller);
                cx.background_executor().timer(DISPLAY_POLL_INTERVAL).await;
            }
        });
        if let Some(Entry::Pending { task: slot, .. }) = self.entries.borrow_mut().get_mut(&display)
        {
            *slot = Some(task);
        }
        Ok(())
    }

    fn open_on_startup(
        self: &Rc<Self>,
        selection: DisplaySelection,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        if self.stopped.get()
            || self
                .runtime
                .upgrade()
                .is_none_or(|runtime| runtime.stopped.get())
        {
            return Err(ShellError::setup(SurfaceRequestError::RuntimeClosed));
        }
        if self.startup_requested.get() {
            return Ok(());
        }
        let snapshot = cx
            .displays()
            .iter()
            .map(|display| (display.id(), display.bounds()))
            .collect::<Vec<_>>();
        let started = cx.background_executor().now();
        let targets =
            StartupTargets::new(selection.clone(), snapshot.clone(), started, self.timeout)
                .map_err(ShellError::setup)?;
        self.startup_requested.set(true);

        match selection {
            DisplaySelection::Explicit(displays) => {
                let mut unique = Vec::new();
                for display in displays {
                    if !unique.contains(&display) {
                        unique.push(display);
                    }
                }
                for display in unique {
                    self.open(display, cx).map_err(ShellError::setup)?;
                }
            }
            DisplaySelection::AllConnected if !snapshot.is_empty() => {
                for (display, _) in snapshot {
                    self.open(display, cx).map_err(ShellError::setup)?;
                }
            }
            DisplaySelection::AllConnected => {
                let weak = Rc::downgrade(self);
                let task = cx.spawn(async move |cx| {
                    let mut targets = targets;
                    loop {
                        let Some(controller) = weak.upgrade() else {
                            return;
                        };
                        let Some(runtime) = controller.runtime.upgrade() else {
                            return;
                        };
                        if controller.stopped.get() || runtime.stopped.get() {
                            return;
                        }
                        let snapshot = cx.update(|app| {
                            app.displays()
                                .iter()
                                .map(|display| (display.id(), display.bounds()))
                                .collect::<Vec<_>>()
                        });
                        let ready = match targets.poll(&snapshot, cx.background_executor().now()) {
                            Ok(ready) => ready,
                            Err(StartupTargetError::EnumerationTimeout { timeout }) => {
                                cx.update(|app| {
                                    crate::shell::runtime::record_setup_failure(
                                        &runtime,
                                        ShellError::setup(
                                            LayerSurfaceError::DisplayEnumerationTimeout {
                                                timeout,
                                            },
                                        ),
                                    );
                                    crate::shell::runtime::stop_runtime(&runtime, app);
                                    crate::shell::runtime::request_host_quit(&runtime, app);
                                });
                                return;
                            }
                            Err(StartupTargetError::DisplayTimeout {
                                display_id,
                                timeout,
                            }) => {
                                cx.update(|app| {
                                    crate::shell::runtime::record_surface_failure(
                                        &runtime,
                                        crate::SurfaceError::DisplayReadinessTimeout {
                                            display_id,
                                            timeout,
                                        },
                                        app,
                                    );
                                });
                                return;
                            }
                        };
                        for (display, _) in ready {
                            let result = cx.update(|app| controller.open(display, app));
                            if let Err(error) = result {
                                cx.update(|app| {
                                    crate::shell::runtime::record_setup_failure(
                                        &runtime,
                                        ShellError::setup(error),
                                    );
                                    crate::shell::runtime::stop_runtime(&runtime, app);
                                    crate::shell::runtime::request_host_quit(&runtime, app);
                                });
                                return;
                            }
                        }
                        if targets.finished() {
                            return;
                        }
                        drop(runtime);
                        drop(controller);
                        cx.background_executor().timer(DISPLAY_POLL_INTERVAL).await;
                    }
                });
                *self.startup_task.borrow_mut() = Some(task);
            }
        }
        Ok(())
    }

    fn pending(&self, display: DisplayId, generation: u64) -> bool {
        matches!(self.entries.borrow().get(&display), Some(Entry::Pending { generation: current, .. }) if *current == generation)
    }

    fn clear_pending(&self, display: DisplayId, generation: u64, cx: &gpui::AsyncApp) -> bool {
        let removed = {
            let mut entries = self.entries.borrow_mut();
            if matches!(entries.get(&display), Some(Entry::Pending { generation: current, .. }) if *current == generation)
            {
                entries.remove(&display)
            } else {
                None
            }
        };
        if let Some(Entry::Pending { active, task, .. }) = removed {
            let _ = finish_pending(active, task, None, cx);
            true
        } else {
            false
        }
    }

    async fn create(
        self: &Rc<Self>,
        display: DisplayId,
        generation: u64,
        active: &Rc<Cell<bool>>,
        cx: &mut gpui::AsyncApp,
    ) {
        if !active.get() || !self.pending(display, generation) {
            return;
        }
        let Some(mut factory) = self.factory.borrow_mut().take() else {
            return;
        };
        let mut animation = if let Some(plan) = self.animation_plan {
            match SurfaceAnimation::new(plan, cx.background_executor().now()) {
                Ok(animation) => Some(animation),
                Err(error) => {
                    if let Some(runtime) = self.runtime.upgrade() {
                        cx.update(|app| {
                            crate::shell::runtime::record_setup_failure(
                                &runtime,
                                ShellError::setup(error),
                            );
                            crate::shell::runtime::stop_runtime(&runtime, app);
                            crate::shell::runtime::request_host_quit(&runtime, app);
                        });
                    }
                    self.clear_pending(display, generation, cx);
                    return;
                }
            }
        } else {
            None
        };
        if let Some(Entry::Pending {
            generation: current,
            animation: slot,
            ..
        }) = self.entries.borrow_mut().get_mut(&display)
            && *current == generation
            && let Some(animation) = animation.take()
            && slot.is_none()
        {
            *slot = Some(animation);
        }
        let mut result: Option<Result<AnyView, ShellError>> = None;
        let mut root_entity: Option<Entity<DynamicRoot>> = None;
        let instance = SurfaceInstanceHandle {
            controller: Rc::downgrade(self),
            display_id: display,
            generation,
        };
        let config = &self.config;
        let options = config.options.clone();
        let opened = cx.update(|app| {
            app.open_window(
                crate::shell::surface::layer_window_options(display, config.size, options),
                |window, app| {
                    window.set_input_region(Some(&[]));
                    let root = app.new(|_| DynamicRoot(None));
                    root_entity = Some(root.clone());
                    result = Some(factory(instance, window, app));
                    root
                },
            )
            .map(Into::into)
            .map_err(|cause| cause.into_boxed_dyn_error())
        });
        let mut returned = self.factory.borrow_mut();
        if self.runtime.upgrade().is_some_and(|r| !r.stopped.get()) && !self.stopped.get() {
            *returned = Some(factory);
        }
        drop(returned);
        let current = active.get()
            && self.pending(display, generation)
            && self
                .runtime
                .upgrade()
                .is_some_and(|runtime| !runtime.stopped.get());
        if !current {
            if let Ok(window) = opened {
                crate::shell::surface::remove_window_later(cx, window);
            }
            self.clear_pending(display, generation, cx);
            return;
        }
        if let Some(Err(error)) = result.as_ref() {
            if let Ok(window) = opened {
                crate::shell::surface::remove_window_later(cx, window);
            }
            if let Some(runtime) = self.runtime.upgrade() {
                cx.update(|app| {
                    crate::shell::runtime::record_setup_failure(&runtime, error.clone());
                    crate::shell::runtime::stop_runtime(&runtime, app);
                    crate::shell::runtime::request_host_quit(&runtime, app);
                });
            }
            self.clear_pending(display, generation, cx);
            return;
        }
        match opened {
            Ok(window) if current => {
                if let (Some(Ok(view)), Some(root)) = (result, root_entity) {
                    root.update(cx, |root, cx| {
                        root.0 = Some(view);
                        cx.notify();
                    });
                }
                let old = self.entries.borrow_mut().remove(&display);
                let mut mounted_animation = match old {
                    Some(Entry::Pending {
                        active,
                        task,
                        animation,
                        ..
                    }) => finish_pending(active, task, animation, cx),
                    _ => None,
                };
                self.entries.borrow_mut().insert(
                    display,
                    Entry::Mounted {
                        generation,
                        window,
                        animation: mounted_animation.take(),
                    },
                );
                cx.update(|app| self.queue_frame(display, generation, app));
            }
            Ok(window) => crate::shell::surface::remove_window_later(cx, window),
            Err(cause) => {
                if let Some(runtime) = self.runtime.upgrade() {
                    cx.update(|app| {
                        crate::shell::runtime::record_surface_failure(
                            &runtime,
                            crate::SurfaceError::Creation {
                                display_id: display,
                                cause: Rc::from(cause),
                            },
                            app,
                        )
                    });
                }
                self.clear_pending(display, generation, cx);
            }
        }
    }

    fn close(self: &Rc<Self>, display: DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.close_generation(display, u64::MAX, cx)
    }

    fn close_generation(
        self: &Rc<Self>,
        display: DisplayId,
        generation: u64,
        cx: &mut App,
    ) -> Result<(), SurfaceRequestError> {
        let now = cx.background_executor().now();
        let (old, needs_frame, failure) = {
            let mut entries = self.entries.borrow_mut();
            let matches = entries.get(&display).is_some_and(|entry| match entry {
                Entry::Pending { generation: g, .. } | Entry::Mounted { generation: g, .. } => {
                    generation == u64::MAX || *g == generation
                }
            });
            if !matches {
                (None, false, None)
            } else if let Some(Entry::Mounted {
                generation: current,
                animation: Some(animation),
                ..
            }) = entries.get_mut(&display)
            {
                if generation != u64::MAX && *current != generation {
                    (None, false, None)
                } else {
                    match animation.request(Phase::Exit, now) {
                        Ok(()) if animation.value().running => (None, true, None),
                        Ok(()) => (entries.remove(&display), false, None),
                        Err(error) => (None, false, Some(error)),
                    }
                }
            } else {
                (entries.remove(&display), false, None)
            }
        };
        if let Some(error) = failure {
            self.fail_animation(error, cx);
            return Err(SurfaceRequestError::Unavailable);
        }
        if needs_frame {
            let mounted_generation = match self.entries.borrow().get(&display) {
                Some(Entry::Mounted { generation, .. }) => Some(*generation),
                _ => None,
            };
            if let Some(generation) = mounted_generation {
                self.defer_refresh(display, generation, cx);
                self.queue_frame(display, generation, cx);
            }
        }
        match old {
            Some(Entry::Pending { active, task, .. }) => {
                let _ = finish_pending(active, task, None, &cx.to_async());
            }
            Some(Entry::Mounted { window, .. }) => {
                crate::shell::surface::remove_window_later(&cx.to_async(), window)
            }
            None => {}
        }
        Ok(())
    }
}

struct DynamicRoot(Option<AnyView>);
impl Render for DynamicRoot {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        match self.0.clone() {
            Some(view) => div().child(view),
            None => div(),
        }
    }
}

fn defer_task_drop(cx: &gpui::AsyncApp, task: Task<()>) {
    let app = cx.clone();
    app.spawn(async move |cx| {
        cx.background_executor().timer(Duration::ZERO).await;
        drop(task);
    })
    .detach();
}

fn finish_pending(
    active: Rc<Cell<bool>>,
    task: Option<Task<()>>,
    animation: Option<SurfaceAnimation>,
    cx: &gpui::AsyncApp,
) -> Option<SurfaceAnimation> {
    active.set(false);
    if let Some(task) = task {
        defer_task_drop(cx, task);
    }
    animation
}

pub(in crate::shell) fn register<V: Render + 'static>(
    runtime: &Rc<RuntimeInner>,
    config: LayerSurfaceConfig,
    timeout: Duration,
    animation_plan: Option<AnimationPlan>,
    deferred_animation: bool,
    mut factory: impl FnMut(
        SurfaceInstanceHandle,
        &mut Window,
        &mut App,
    ) -> Result<Entity<V>, ShellError>
    + 'static,
    cx: &mut App,
) -> Result<DynamicSurfaceHandle, ShellError> {
    if let Some(plan) = animation_plan {
        crate::AnimationRunner::new(plan).map_err(ShellError::setup)?;
    }
    if runtime.stopped.get() {
        return Err(ShellError::setup(
            crate::shell::runtime::SurfaceRegistrationError::Closed,
        ));
    }
    validate_dynamic_surface_dimensions(&config)
        .map_err(|reason| ShellError::setup(std::io::Error::other(reason)))?;
    validate_dynamic_surface_exclusive_zone(&config)
        .map_err(|reason| ShellError::setup(std::io::Error::other(reason)))?;
    let anchor_bits = gpui::layer_shell::Anchor::all().bits();
    validate_dynamic_surface_anchors(&config)
        .map_err(|reason| ShellError::setup(std::io::Error::other(reason)))?;
    if config
        .options
        .exclusive_edge
        .is_some_and(|edge| edge.bits() & !anchor_bits != 0)
        || config.options.margin.is_some_and(|margins| {
            [margins.0, margins.1, margins.2, margins.3]
                .into_iter()
                .any(|margin| {
                    let value = f32::from(margin);
                    !value.is_finite() || f64::from(value).abs() > f64::from(i32::MAX)
                })
        })
    {
        return Err(ShellError::setup(std::io::Error::other(
            "invalid dynamic layer surface options",
        )));
    }
    let factory: Factory =
        Box::new(move |instance, window, app| factory(instance, window, app).map(Into::into));
    let controller = Rc::new(Controller {
        runtime: Rc::downgrade(runtime),
        config,
        timeout,
        animation_plan,
        deferred_animation,
        factory: RefCell::new(Some(factory)),
        next_generation: Cell::new(0),
        entries: RefCell::new(HashMap::new()),
        stopped: Cell::new(false),
        startup_requested: Cell::new(false),
        startup_task: RefCell::new(None),
        window_closed: RefCell::new(None),
    });
    let weak = Rc::downgrade(&controller);
    let subscription = cx.on_window_closed(move |_, window_id| {
        if let Some(controller) = weak.upgrade() {
            controller.window_closed(window_id);
        }
    });
    *controller.window_closed.borrow_mut() = Some(subscription);
    runtime
        .dynamic_surfaces
        .borrow_mut()
        .push(Rc::clone(&controller));
    let _ = cx;
    Ok(DynamicSurfaceHandle(Rc::downgrade(&controller)))
}

pub(in crate::shell) fn stop_controllers(runtime: &RuntimeInner, cx: &mut App) {
    let controllers = runtime.dynamic_surfaces.borrow().clone();
    for controller in controllers {
        controller.stop(cx);
    }
}

pub(in crate::shell) fn drop_controllers(
    controllers: &mut Vec<Rc<Controller>>,
    app: &gpui::AsyncApp,
) {
    for controller in std::mem::take(controllers) {
        controller.cleanup(app);
    }
}

impl Controller {
    fn window_closed(&self, window_id: gpui::WindowId) {
        let closed = {
            let mut entries = self.entries.borrow_mut();
            let display = entries.iter().find_map(|(display, entry)| match entry {
                Entry::Mounted { window, .. } if window.window_id() == window_id => Some(*display),
                _ => None,
            });
            display.and_then(|display| entries.remove(&display))
        };
        drop(closed);
    }

    fn stop(&self, cx: &mut App) {
        self.cleanup(&cx.to_async());
    }

    fn cleanup(&self, app: &gpui::AsyncApp) {
        self.stopped.set(true);
        if let Some(task) = self.startup_task.borrow_mut().take() {
            defer_task_drop(app, task);
        }
        let subscription = self.window_closed.borrow_mut().take();
        drop(subscription);
        let entries = std::mem::take(&mut *self.entries.borrow_mut());
        for (_, entry) in entries {
            match entry {
                Entry::Pending { active, task, .. } => {
                    let _ = finish_pending(active, task, None, app);
                }
                Entry::Mounted { window, .. } => {
                    crate::shell::surface::remove_window_later(app, window)
                }
            }
        }
        let factory = self.factory.borrow_mut().take();
        drop(factory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Shell, SourceState, shell::test_support::descriptor};
    use gpui::{AppContext, Size, TestAppContext, layer_shell::LayerShellOptions, px};
    use std::cell::Cell;

    struct LeasedView {
        _binding: crate::MountedBinding<u8>,
        drops: Rc<Cell<usize>>,
    }

    impl Drop for LeasedView {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    impl Render for LeasedView {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
        }
    }

    fn stage_and_create(controller: Rc<Controller>, display: DisplayId, cx: &mut App) -> u64 {
        let generation = controller.next_generation.get() + 1;
        controller.next_generation.set(generation);
        let active = Rc::new(Cell::new(true));
        controller.entries.borrow_mut().insert(
            display,
            Entry::Pending {
                generation,
                active: Rc::clone(&active),
                task: None,
                animation: None,
            },
        );
        let task_controller = Rc::clone(&controller);
        let task_active = Rc::clone(&active);
        let task = cx.spawn(async move |cx| {
            task_controller
                .create(display, generation, &task_active, cx)
                .await;
        });
        if let Some(Entry::Pending { task: slot, .. }) =
            controller.entries.borrow_mut().get_mut(&display)
        {
            *slot = Some(task);
        }
        generation
    }

    fn config(width: f32, height: f32) -> LayerSurfaceConfig {
        LayerSurfaceConfig {
            size: Size::new(px(width), px(height)),
            options: LayerShellOptions::default(),
        }
    }

    #[test]
    fn registration_classifies_ui_shared_config_failures_without_changing_core_io_errors() {
        use gpui::layer_shell::Anchor;

        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());
        let cases = [
            (
                config(-1., 20.),
                DynamicSurfaceConfigError::InvalidDimension,
                "invalid dynamic layer surface size",
            ),
            (
                config(0., 20.),
                DynamicSurfaceConfigError::ZeroDimensionWithoutOpposingAnchors,
                "invalid dynamic layer surface size",
            ),
            (
                config(20., 20.),
                DynamicSurfaceConfigError::InvalidExclusiveZone,
                "invalid dynamic layer surface exclusive zone",
            ),
            (
                {
                    let mut config = config(20., 20.);
                    config.options.anchor = Anchor::from_bits_retain(1 << 4);
                    config
                },
                DynamicSurfaceConfigError::UnknownAnchorBits,
                "invalid dynamic layer surface options",
            ),
        ];
        let factory_calls = Rc::new(Cell::new(0));

        for (mut config, expected_reason, expected_display) in cases {
            if expected_reason == DynamicSurfaceConfigError::InvalidExclusiveZone {
                config.options.exclusive_zone = Some(px(-2.));
            }
            if expected_reason == DynamicSurfaceConfigError::ZeroDimensionWithoutOpposingAnchors {
                config.options.anchor = Anchor::TOP | Anchor::LEFT;
            }
            let calls = factory_calls.clone();
            let result = cx.update(|cx| {
                runtime.register_dynamic_layer_surface(
                    config,
                    Duration::from_secs(5),
                    move |_, _, cx| {
                        calls.set(calls.get() + 1);
                        Ok(cx.new(|_| gpui::EmptyView))
                    },
                    cx,
                )
            });
            let Err(crate::ShellError::Setup(cause)) = result else {
                panic!("expected setup validation error");
            };
            let io_error = cause
                .downcast_ref::<std::io::Error>()
                .expect("core registration preserves io::Error cause");
            assert_eq!(io_error.kind(), std::io::ErrorKind::Other);
            assert_eq!(io_error.to_string(), expected_display);
            assert_eq!(
                io_error
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<DynamicSurfaceConfigError>()),
                Some(&expected_reason)
            );
            assert!(runtime.0.dynamic_surfaces.borrow().is_empty());
            assert_eq!(factory_calls.get(), 0);
        }
        cx.update(|cx| runtime.shutdown(cx));
    }

    fn test_animation_plan() -> AnimationPlan {
        let timing = Some(crate::PlanTiming::Easing {
            duration: Duration::from_millis(100),
            curve: crate::EasingCurve::Linear,
        });
        AnimationPlan {
            opacity: Some(crate::Channel {
                enter: Some(crate::PropertyAnimation {
                    endpoints: [0.0, 1.0],
                    timing,
                }),
                exit: Some(crate::PropertyAnimation {
                    endpoints: [1.0, 0.0],
                    timing,
                }),
            }),
            ..AnimationPlan::default()
        }
    }

    #[test]
    fn staged_display_generations_sample_independent_animation_state() {
        let mut cx = TestAppContext::single();
        let instances = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&instances);
        let (runtime, controller, first, second) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            runtime
                .register_animated_dynamic_layer_surface(
                    config(20., 20.),
                    Duration::from_secs(5),
                    test_animation_plan(),
                    move |instance, _, cx| {
                        captured.borrow_mut().push(instance);
                        Ok(cx.new(|_| gpui::EmptyView))
                    },
                    cx,
                )
                .unwrap();
            let controller = Rc::clone(&runtime.0.dynamic_surfaces.borrow()[0]);
            let first = DisplayId::new(501);
            let second = DisplayId::new(502);
            stage_and_create(Rc::clone(&controller), first, cx);
            stage_and_create(Rc::clone(&controller), second, cx);
            (runtime, controller, first, second)
        });
        cx.run_until_parked();

        assert_eq!(instances.borrow().len(), 2);
        assert_eq!(instances.borrow()[0].display_id(), first);
        assert_eq!(instances.borrow()[1].display_id(), second);
        assert_eq!(instances.borrow()[0].animation_sample().opacity, Some(0.0));
        assert_eq!(instances.borrow()[1].animation_sample().opacity, Some(0.0));
        let first_window = match controller.entries.borrow().get(&first) {
            Some(Entry::Mounted { window, .. }) => *window,
            _ => unreachable!("first staged display should mount"),
        };
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.update_window(first_window, |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 1);
        })
        .unwrap();
        assert!((instances.borrow()[0].animation_sample().opacity.unwrap() - 0.5).abs() < 0.001);
        assert_eq!(instances.borrow()[1].animation_sample().opacity, Some(0.0));
        cx.update(|cx| runtime.shutdown(cx));
        cx.run_until_parked();
    }

    #[test]
    fn staged_controller_ids_have_independent_windows_and_shared_resource_leases() {
        // This isolates keyed controller behavior; the headless test platform cannot model two outputs.
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let drops = Rc::new(Cell::new(0));
        let ids = Rc::new(RefCell::new(Vec::new()));
        let states = Rc::new(RefCell::new(Vec::new()));
        let (runtime, surface, controller, first_id, second_id, first_generation, old_instance) =
            cx.update(|cx| {
                let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
                let (resource, _) = descriptor(starts.clone(), stops.clone(), false);
                let weak_runtime = runtime.downgrade();
                let factory_ids = Rc::clone(&ids);
                let factory_states = Rc::clone(&states);
                let factory_drops = Rc::clone(&drops);
                let surface = runtime
                    .register_dynamic_layer_surface(
                        config(20., 20.),
                        Duration::from_secs(5),
                        move |instance, _, cx| {
                            factory_ids.borrow_mut().push(instance.display_id());
                            let runtime = weak_runtime.upgrade().ok_or_else(|| {
                                ShellError::setup(std::io::Error::other("runtime dropped"))
                            })?;
                            let binding = runtime.mount(&resource, cx).map_err(ShellError::from)?;
                            factory_states.borrow_mut().push(binding.reactive().get(cx));
                            Ok(cx.new(|_| LeasedView {
                                _binding: binding,
                                drops: Rc::clone(&factory_drops),
                            }))
                        },
                        cx,
                    )
                    .unwrap();
                let controller = Rc::clone(&runtime.0.dynamic_surfaces.borrow()[0]);
                let first_id = DisplayId::new(401);
                let second_id = DisplayId::new(402);
                let first_generation = stage_and_create(Rc::clone(&controller), first_id, cx);
                stage_and_create(Rc::clone(&controller), second_id, cx);
                let old_instance = SurfaceInstanceHandle {
                    controller: Rc::downgrade(&controller),
                    display_id: first_id,
                    generation: first_generation,
                };
                (
                    runtime,
                    surface,
                    controller,
                    first_id,
                    second_id,
                    first_generation,
                    old_instance,
                )
            });
        cx.run_until_parked();

        assert_eq!(&*ids.borrow(), &[first_id, second_id]);
        assert_eq!(
            &*states.borrow(),
            &[SourceState::Loading, SourceState::Loading]
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 0);
        let (first_window, second_window) = {
            let entries = controller.entries.borrow();
            let window = |display| match entries.get(&display) {
                Some(Entry::Mounted { window, .. }) => *window,
                _ => unreachable!("staged generation should mount"),
            };
            (window(first_id), window(second_id))
        };
        assert_ne!(first_window.window_id(), second_window.window_id());
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);

        cx.update(|cx| surface.close(first_id, cx).unwrap());
        cx.run_until_parked();
        assert_eq!(drops.get(), 1);
        assert_eq!(stops.get(), 0);
        assert!(matches!(
            controller.entries.borrow().get(&second_id),
            Some(Entry::Mounted { .. })
        ));

        cx.update(|cx| surface.close(second_id, cx).unwrap());
        cx.run_until_parked();
        assert_eq!(drops.get(), 2);
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);

        let next_generation =
            cx.update(|cx| stage_and_create(Rc::clone(&controller), first_id, cx));
        assert!(next_generation > first_generation);
        cx.run_until_parked();
        assert_eq!(starts.get(), 2);
        assert_eq!(stops.get(), 1);
        assert_eq!(states.borrow().last(), Some(&SourceState::Loading));
        cx.update(|cx| old_instance.close(cx).unwrap());
        assert!(matches!(
            controller.entries.borrow().get(&first_id),
            Some(Entry::Mounted { generation, .. }) if *generation == next_generation
        ));
        cx.update(|cx| runtime.shutdown(cx));
        cx.run_until_parked();
        assert_eq!(stops.get(), 2);
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    }
}

use super::{ShellError, runtime::RuntimeInner};
use crate::{
    LayerSurfaceConfig,
    surface::{DISPLAY_POLL_INTERVAL, DisplayReadiness, Readiness},
};
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

pub struct Controller {
    runtime: Weak<RuntimeInner>,
    config: LayerSurfaceConfig,
    timeout: Duration,
    factory: RefCell<Option<Factory>>,
    next_generation: Cell<u64>,
    entries: RefCell<HashMap<DisplayId, Entry>>,
    stopped: Cell<bool>,
    window_closed: RefCell<Option<Subscription>>,
}

enum Entry {
    Pending {
        generation: u64,
        active: Rc<Cell<bool>>,
        task: Option<Task<()>>,
    },
    Mounted {
        generation: u64,
        window: gpui::AnyWindowHandle,
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
                                super::runtime::record_surface_failure(
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
            active.set(false);
            if let Some(task) = task {
                defer_task_drop(cx, task);
            }
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
                super::surface_window::layer_window_options(display, config.size, options),
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
                super::surface_window::remove_window_later(cx, window);
            }
            self.clear_pending(display, generation, cx);
            return;
        }
        if let Some(Err(error)) = result.as_ref() {
            if let Ok(window) = opened {
                super::surface_window::remove_window_later(cx, window);
            }
            if let Some(runtime) = self.runtime.upgrade() {
                cx.update(|app| {
                    super::runtime::record_setup_failure(&runtime, error.clone());
                    super::runtime::stop_runtime(&runtime, app);
                    super::runtime::request_host_quit(&runtime, app);
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
                let old = self
                    .entries
                    .borrow_mut()
                    .insert(display, Entry::Mounted { generation, window });
                if let Some(Entry::Pending { active, task, .. }) = old {
                    active.set(false);
                    if let Some(task) = task {
                        defer_task_drop(cx, task);
                    }
                }
            }
            Ok(window) => super::surface_window::remove_window_later(cx, window),
            Err(cause) => {
                if let Some(runtime) = self.runtime.upgrade() {
                    cx.update(|app| {
                        super::runtime::record_surface_failure(
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

    fn close(&self, display: DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.close_generation(display, u64::MAX, cx)
    }

    fn close_generation(
        &self,
        display: DisplayId,
        generation: u64,
        cx: &mut App,
    ) -> Result<(), SurfaceRequestError> {
        let old = {
            let mut entries = self.entries.borrow_mut();
            let matches = entries.get(&display).is_some_and(|entry| match entry {
                Entry::Pending { generation: g, .. } | Entry::Mounted { generation: g, .. } => {
                    generation == u64::MAX || *g == generation
                }
            });
            if matches {
                entries.remove(&display)
            } else {
                None
            }
        };
        match old {
            Some(Entry::Pending { active, task, .. }) => {
                active.set(false);
                if let Some(task) = task {
                    defer_task_drop(&cx.to_async(), task);
                }
            }
            Some(Entry::Mounted { window, .. }) => {
                super::surface_window::remove_window_later(&cx.to_async(), window)
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

pub(super) fn register<V: Render + 'static>(
    runtime: &Rc<RuntimeInner>,
    config: LayerSurfaceConfig,
    timeout: Duration,
    mut factory: impl FnMut(
        SurfaceInstanceHandle,
        &mut Window,
        &mut App,
    ) -> Result<Entity<V>, ShellError>
    + 'static,
    cx: &mut App,
) -> Result<DynamicSurfaceHandle, ShellError> {
    if runtime.stopped.get() {
        return Err(ShellError::setup(
            super::runtime::SurfaceRegistrationError::Closed,
        ));
    }
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
        if !value.is_finite()
            || value < 0.
            || f64::from(value) > f64::from(i32::MAX)
            || (value == 0. && !opposing)
        {
            return Err(ShellError::setup(std::io::Error::other(
                "invalid dynamic layer surface size",
            )));
        }
    }
    if config.options.exclusive_zone.is_some_and(|v| {
        let value = f32::from(v);
        !value.is_finite() || value < 0. || f64::from(value) > f64::from(i32::MAX)
    }) {
        return Err(ShellError::setup(std::io::Error::other(
            "invalid dynamic layer surface exclusive zone",
        )));
    }
    let anchor_bits = gpui::layer_shell::Anchor::all().bits();
    if config.options.anchor.bits() & !anchor_bits != 0
        || config
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
        factory: RefCell::new(Some(factory)),
        next_generation: Cell::new(0),
        entries: RefCell::new(HashMap::new()),
        stopped: Cell::new(false),
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

pub(super) fn stop_controllers(runtime: &RuntimeInner, cx: &mut App) {
    let controllers = runtime.dynamic_surfaces.borrow().clone();
    for controller in controllers {
        controller.stop(cx);
    }
}

pub(super) fn drop_controllers(controllers: &mut Vec<Rc<Controller>>, app: &gpui::AsyncApp) {
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
        let subscription = self.window_closed.borrow_mut().take();
        drop(subscription);
        let entries = std::mem::take(&mut *self.entries.borrow_mut());
        for (_, entry) in entries {
            match entry {
                Entry::Pending { active, task, .. } => {
                    active.set(false);
                    if let Some(task) = task {
                        defer_task_drop(app, task);
                    }
                }
                Entry::Mounted { window, .. } => {
                    super::surface_window::remove_window_later(app, window)
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

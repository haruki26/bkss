use super::{
    MountedBinding, ShellError,
    surface::{
        DISPLAY_POLL_INTERVAL, DisplayReadiness, Readiness, SURFACE_NAMESPACE, SurfaceDescriptor,
        SurfaceError,
    },
};
use crate::{
    ReactiveSubscription, ResourceAcquireError, ResourceScope, Source, SourceDescriptor,
    SourceFailure,
};
use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Render, Subscription, Task,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    layer_shell::LayerShellOptions, px,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

struct RuntimeInner {
    scope: ResourceScope,
    outcome: RefCell<Option<Result<(), ShellError>>>,
    stopped: Cell<bool>,
    registered_surface: Cell<bool>,
    surfaces: RefCell<Vec<AnyWindowHandle>>,
    surface_task: RefCell<Option<Task<()>>>,
    surface_task_active: Cell<bool>,
    surface_cancel: RefCell<Option<Box<dyn FnOnce()>>>,
    app: AsyncApp,
    quit_host_on_failure: bool,
    host_quit_requested: Cell<bool>,
    _failure_subscription: RefCell<Option<ReactiveSubscription>>,
    _quit_subscription: RefCell<Option<Subscription>>,
}

/// Cloneable handle to a single attached shell runtime.
///
/// Keep at least one handle alive for the runtime's lifetime. Dropping the last
/// handle stops its resources and best-effort schedules owned-window cleanup on
/// GPUI's foreground executor; cleanup completion is not guaranteed. It does not
/// request that the host app quit or change its quit mode. GPUI may still apply
/// its existing last-window quit policy when removing the app's only window.
#[derive(Clone)]
pub struct RuntimeHandle(Rc<RuntimeInner>);

impl RuntimeHandle {
    pub(super) fn new(cx: &mut App, quit_host_on_failure: bool) -> Self {
        let scope = ResourceScope::new(cx);
        let inner = Rc::new(RuntimeInner {
            scope,
            outcome: RefCell::new(None),
            stopped: Cell::new(false),
            registered_surface: Cell::new(false),
            surfaces: RefCell::new(Vec::new()),
            surface_task: RefCell::new(None),
            surface_task_active: Cell::new(false),
            surface_cancel: RefCell::new(None),
            app: cx.to_async(),
            quit_host_on_failure,
            host_quit_requested: Cell::new(false),
            _failure_subscription: RefCell::new(None),
            _quit_subscription: RefCell::new(None),
        });

        let weak = Rc::downgrade(&inner);
        let subscription = inner
            .scope
            .failure_reactive()
            .observe_app(cx, move |failure, cx| {
                if let Some(failure) = failure
                    && let Some(inner) = weak.upgrade()
                {
                    record_failure(&inner, Rc::clone(failure));
                    stop_runtime(&inner, cx);
                    request_host_quit(&inner, cx);
                }
                true
            });
        *inner._failure_subscription.borrow_mut() = Some(subscription);

        let weak = Rc::downgrade(&inner);
        let subscription = cx.on_app_quit(move |cx| {
            if let Some(inner) = weak.upgrade() {
                stop_runtime(&inner, cx);
                finalize_success(&inner);
            }
            std::future::ready(())
        });
        *inner._quit_subscription.borrow_mut() = Some(subscription);
        Self(inner)
    }

    pub fn mount<S: Source>(
        &self,
        descriptor: &SourceDescriptor<S>,
        cx: &mut App,
    ) -> Result<MountedBinding<S::Value>, ResourceAcquireError> {
        self.0
            .scope
            .acquire(descriptor, cx)
            .map(MountedBinding::new)
    }

    /// Registers the runtime's single low-level surface. Registration is allowed
    /// once, before shutdown; its factory runs once only after the explicitly
    /// selected display is present. Shutdown before readiness prevents creation.
    pub fn mount_surface<V: Render + 'static>(
        &self,
        descriptor: SurfaceDescriptor<V>,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        if self.0.stopped.get() {
            return Err(ShellError::setup(SurfaceRegistrationError::Closed));
        }
        if self.0.registered_surface.replace(true) {
            return Err(ShellError::setup(
                SurfaceRegistrationError::AlreadyRegistered,
            ));
        }

        let weak = Rc::downgrade(&self.0);
        let display_id = descriptor.display_id;
        let size = descriptor.size;
        let timeout = descriptor.readiness_timeout;
        let content = Rc::new(RefCell::new(Some(descriptor.content)));
        let cancel_content = Rc::clone(&content);
        *self.0.surface_cancel.borrow_mut() = Some(Box::new(move || {
            cancel_content.borrow_mut().take();
        }));
        let task_content = Rc::clone(&content);
        let task = cx.spawn(async move |cx| {
            let mut readiness = DisplayReadiness::new(timeout, Instant::now());
            loop {
                let Some(inner) = weak.upgrade() else { break };
                inner.surface_task_active.set(true);
                if inner.stopped.get() {
                    break;
                }
                let found = cx.update(|cx| {
                    cx.displays()
                        .iter()
                        .any(|display| display.id() == display_id)
                });
                match readiness.poll(found, Instant::now()) {
                    Readiness::Waiting => {
                        inner.surface_task_active.set(false);
                        drop(inner);
                        cx.background_executor().timer(DISPLAY_POLL_INTERVAL).await;
                    }
                    Readiness::TimedOut => {
                        inner.surface_task_active.set(false);
                        inner.surface_cancel.borrow_mut().take();
                        cx.update(|app| {
                            record_surface_failure(
                                &inner,
                                SurfaceError::DisplayReadinessTimeout {
                                    display_id,
                                    timeout,
                                },
                                app,
                            )
                        });
                        break;
                    }
                    Readiness::Ready => {
                        if inner.stopped.get() {
                            break;
                        }
                        let Some(content) = task_content.borrow_mut().take() else {
                            break;
                        };
                        let opened = cx
                            .open_window(
                                WindowOptions {
                                    display_id: Some(display_id),
                                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                                        origin: gpui::point(px(0.), px(0.)),
                                        size,
                                    })),
                                    focus: false,
                                    window_background: WindowBackgroundAppearance::Transparent,
                                    kind: WindowKind::LayerShell(LayerShellOptions {
                                        namespace: SURFACE_NAMESPACE.to_owned(),
                                        exclusive_zone: Some(px(0.)),
                                        keyboard_interactivity:
                                            gpui::layer_shell::KeyboardInteractivity::None,
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                },
                                |window, cx| {
                                    window.set_input_region(Some(&[]));
                                    content(window, cx)
                                },
                            )
                            .map(Into::into)
                            .map_err(|cause| cause.into_boxed_dyn_error());
                        inner.surface_cancel.borrow_mut().take();
                        cx.update(|app| finish_surface_open(&inner, display_id, opened, app));
                        inner.surface_task_active.set(false);
                        break;
                    }
                    Readiness::Finished => break,
                }
            }
        });
        *self.0.surface_task.borrow_mut() = Some(task);
        Ok(())
    }

    /// Stops this runtime's resources and owned surfaces. An attached runtime
    /// leaves the host app running; [`Shell::run`](super::Shell::run) owns and
    /// requests app quit for runtimes it starts. GPUI's existing last-window
    /// policy may still apply if an attached surface is the app's only window.
    pub fn shutdown(&self, cx: &mut App) {
        stop_runtime(&self.0, cx);
        request_host_quit(&self.0, cx);
    }

    /// Returns a non-consuming result snapshot.
    ///
    /// Returns `None` while running or after attach-local shutdown while the host
    /// continues, `Some(Err(_))` after fatal failure, and `Some(Ok(()))` after
    /// normal finalization. This is not a cleanup-completion barrier for surfaces
    /// or asynchronous cancellation.
    pub fn result(&self) -> Option<Result<(), ShellError>> {
        self.0.outcome.borrow().clone()
    }

    pub(super) fn setup_failed(&self, error: ShellError, cx: &mut App) -> ShellError {
        record_setup_failure(&self.0, error.clone());
        stop_runtime(&self.0, cx);
        request_host_quit(&self.0, cx);
        self.result().and_then(Result::err).unwrap_or(error)
    }

    pub(super) fn finish_after_run(&self) -> Option<Result<(), ShellError>> {
        self.0.scope.shutdown();
        finalize_success(&self.0);
        self.result()
    }
}

#[derive(Debug)]
enum SurfaceRegistrationError {
    Closed,
    AlreadyRegistered,
}

impl std::fmt::Display for SurfaceRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("cannot register a surface after runtime shutdown"),
            Self::AlreadyRegistered => f.write_str("runtime already has a registered surface"),
        }
    }
}

impl std::error::Error for SurfaceRegistrationError {}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.scope.shutdown();
        self.surface_task.get_mut().take();
        let handles = std::mem::take(self.surfaces.get_mut());
        if !handles.is_empty() {
            let app = self.app.clone();
            app.spawn(async move |cx| {
                for handle in handles {
                    let _ = cx.update_window(handle, |_, window, _| window.remove_window());
                }
            })
            .detach();
        }
    }
}

fn remove_surface(cx: &mut impl AppContext, handle: AnyWindowHandle) {
    let _ = cx.update_window(handle, |_, window, _| window.remove_window());
}

fn finish_surface_open(
    inner: &RuntimeInner,
    display_id: gpui::DisplayId,
    opened: Result<AnyWindowHandle, Box<dyn std::error::Error + Send + Sync>>,
    cx: &mut App,
) {
    match opened {
        Ok(handle) if !inner.stopped.get() => inner.surfaces.borrow_mut().push(handle),
        Ok(handle) => remove_surface(cx, handle),
        Err(cause) => record_surface_failure(
            inner,
            SurfaceError::Creation {
                display_id,
                cause: Rc::from(cause),
            },
            cx,
        ),
    }
}

fn stop_runtime(inner: &RuntimeInner, cx: &mut App) {
    if inner.stopped.replace(true) {
        return;
    }
    inner.scope.shutdown();
    if !inner.surface_task_active.get() {
        inner.surface_task.borrow_mut().take();
        if let Some(cancel) = inner.surface_cancel.borrow_mut().take() {
            cancel();
        }
    }
    let handles = std::mem::take(&mut *inner.surfaces.borrow_mut());
    for handle in handles {
        remove_surface(cx, handle);
    }
}

fn record_surface_failure(inner: &RuntimeInner, error: SurfaceError, cx: &mut App) {
    let mut outcome = inner.outcome.borrow_mut();
    if outcome.is_none() {
        *outcome = Some(Err(ShellError::surface(error)));
    }
    drop(outcome);
    stop_runtime(inner, cx);
    request_host_quit(inner, cx);
}

fn record_failure(inner: &RuntimeInner, failure: Rc<SourceFailure>) {
    let mut outcome = inner.outcome.borrow_mut();
    if outcome.is_none() {
        *outcome = Some(Err(ShellError::Source(failure)));
    }
}

fn record_setup_failure(inner: &RuntimeInner, error: ShellError) {
    let failure = inner.scope.failure();
    let mut outcome = inner.outcome.borrow_mut();
    if outcome.is_none() {
        *outcome = Some(Err(failure.map_or(error, ShellError::Source)));
    }
}

fn finalize_success(inner: &RuntimeInner) {
    let mut outcome = inner.outcome.borrow_mut();
    if outcome.is_none() {
        *outcome = Some(Ok(()));
    }
}

fn request_host_quit(inner: &RuntimeInner, cx: &mut App) {
    if inner.quit_host_on_failure && !inner.host_quit_requested.replace(true) {
        cx.quit();
    }
}

#[cfg(test)]
mod tests {
    use super::super::Shell;
    use super::super::test_support::*;
    use super::*;
    use crate::ResourceAcquireError;
    use gpui::{AppContext, DisplayId, Size, TestAppContext, WindowOptions, px};
    use std::{cell::Cell, error::Error, rc::Rc};

    #[test]
    fn ordinary_gpui_quit_finalizes_without_prior_runtime_shutdown() {
        let cx = TestAppContext::single();
        let (runtime, stops, _binding) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            let stops = Rc::new(Cell::new(0));
            let (descriptor, _) = descriptor(Rc::new(Cell::new(0)), stops.clone(), false);
            let binding = runtime.mount(&descriptor, cx).unwrap();
            (runtime, stops, binding)
        });

        assert_eq!(stops.get(), 0);
        cx.quit();
        assert!(matches!(runtime.result(), Some(Ok(()))));
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn shutdown_is_runtime_only_for_attach_and_host_quit_finalizes_result() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());

        runtime.0.scope.shutdown();
        assert!(runtime.result().is_none());
        cx.update(|cx| {
            runtime.shutdown(cx);
            assert!(runtime.result().is_none());
            assert!(!runtime.0.host_quit_requested.get());
            runtime.shutdown(cx);
        });
        assert!(runtime.result().is_none());
        cx.quit();
        assert!(matches!(runtime.result(), Some(Ok(()))));
    }

    #[test]
    fn run_owned_runtime_shutdown_requests_gpui_quit() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| RuntimeHandle::new(cx, true));

        cx.update(|cx| runtime.shutdown(cx));

        assert!(runtime.0.host_quit_requested.get());
        cx.quit();
    }

    #[test]
    fn setup_error_stops_retained_resources_without_quitting_host_app() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let retained_runtime = Rc::new(RefCell::new(None));
        let retained_binding = Rc::new(RefCell::new(None));
        let runtime_slot = Rc::clone(&retained_runtime);
        let closed_in_drop = Rc::new(Cell::new(false));
        let observed_closed = Rc::clone(&closed_in_drop);
        let weak_runtime = Rc::new(RefCell::new(None::<std::rc::Weak<RuntimeInner>>));
        let weak_slot = Rc::clone(&weak_runtime);
        let (descriptor, _) = descriptor_with_options(
            starts,
            stops.clone(),
            false,
            None,
            Some(Rc::new(move || {
                observed_closed.set(
                    weak_slot
                        .borrow()
                        .as_ref()
                        .and_then(std::rc::Weak::upgrade)
                        .is_some(),
                );
            })),
            1,
        );
        let runtime_for_setup = Rc::clone(&runtime_slot);
        let binding_for_setup = Rc::clone(&retained_binding);
        let weak_for_setup = Rc::clone(&weak_runtime);
        let result = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *weak_for_setup.borrow_mut() = Some(Rc::downgrade(&runtime.0));
                *runtime_for_setup.borrow_mut() = Some(runtime.clone());
                *binding_for_setup.borrow_mut() = Some(runtime.mount(&descriptor, cx)?);
                Err(ShellError::setup(TestError("setup")))
            })
        });

        assert!(matches!(result, Err(ShellError::Setup(_))));
        assert_eq!(stops.get(), 1);
        assert!(closed_in_drop.get());
        let runtime = retained_runtime.borrow().as_ref().unwrap().clone();
        assert!(matches!(
            cx.update(|cx| runtime.mount(&descriptor, cx)),
            Err(ResourceAcquireError::Closed)
        ));
        assert!(matches!(runtime.result(), Some(Err(ShellError::Setup(_)))));
        drop(retained_binding);
    }

    #[test]
    fn attach_setup_failure_preserves_the_host_window() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });

        let result = cx.update(|cx| {
            Shell::attach(cx, |_, _| {
                Err(ShellError::setup(std::io::Error::other("setup")))
            })
        });

        assert!(matches!(result, Err(ShellError::Setup(_))));
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
    }

    #[test]
    fn surface_registration_is_single_use_and_rejected_after_shutdown() {
        let cx = TestAppContext::single();
        let called = Rc::new(Cell::new(0));
        let called_first = Rc::clone(&called);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(99),
                    Size::new(px(20.), px(20.)),
                    move |_, cx| {
                        called_first.set(called_first.get() + 1);
                        cx.new(|_| gpui::EmptyView)
                    },
                )?;
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });
        let duplicate_called = Rc::clone(&called);
        let duplicate = SurfaceDescriptor::new(
            DisplayId::new(1),
            Size::new(px(20.), px(20.)),
            move |_, cx| {
                duplicate_called.set(duplicate_called.get() + 1);
                cx.new(|_| gpui::EmptyView)
            },
        )
        .unwrap();
        assert!(matches!(
            cx.update(|cx| runtime.mount_surface(duplicate, cx)),
            Err(ShellError::Setup(_))
        ));

        cx.update(|cx| runtime.shutdown(cx));
        let after_shutdown =
            SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(20.), px(20.)), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap();
        assert!(matches!(
            cx.update(|cx| runtime.mount_surface(after_shutdown, cx)),
            Err(ShellError::Setup(_))
        ));
        cx.run_until_parked();
        assert_eq!(called.get(), 0);
    }

    #[test]
    fn dropping_last_runtime_handle_stops_resources_without_quitting_app() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (descriptor, _) = descriptor(starts, stops.clone(), false);
        let (runtime, binding) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            let binding = runtime.mount(&descriptor, cx).unwrap();
            (runtime, binding)
        });
        let cloned = runtime.clone();

        drop(runtime);
        assert_eq!(stops.get(), 0);
        drop(cloned);
        assert_eq!(stops.get(), 1);
        drop(binding);
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn attached_runtime_closes_only_its_surface_and_leaves_host_window_alive() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let created = Rc::new(Cell::new(0));
        let created_by_factory = Rc::clone(&created);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(1),
                    Size::new(px(48.), px(20.)),
                    move |_, cx| {
                        created_by_factory.set(created_by_factory.get() + 1);
                        cx.new(|_| gpui::EmptyView)
                    },
                )?;
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });

        cx.run_until_parked();
        assert_eq!(created.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);
        cx.update(|cx| runtime.shutdown(cx));
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
    }

    #[test]
    fn shutdown_during_factory_removes_the_surface_created_after_reentrant_shutdown() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let runtime_slot = Rc::new(RefCell::new(None::<RuntimeHandle>));
        let factory_runtime = Rc::clone(&runtime_slot);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *runtime_slot.borrow_mut() = Some(runtime.clone());
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(1),
                    Size::new(px(48.), px(20.)),
                    move |_, cx| {
                        factory_runtime.borrow().as_ref().unwrap().shutdown(cx);
                        cx.new(|_| gpui::EmptyView)
                    },
                )?;
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });

        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        assert!(runtime.result().is_none());
    }

    #[test]
    fn shutdown_cancels_waiting_surface_before_its_factory_runs() {
        let cx = TestAppContext::single();
        let called = Rc::new(Cell::new(false));
        let called_by_factory = Rc::clone(&called);
        let factory_lifetime = Rc::new(());
        let factory_weak = Rc::downgrade(&factory_lifetime);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(99),
                    Size::new(px(48.), px(20.)),
                    move |_, cx| {
                        let _ = &factory_lifetime;
                        called_by_factory.set(true);
                        cx.new(|_| gpui::EmptyView)
                    },
                )?
                .readiness_timeout(std::time::Duration::from_secs(30));
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(!runtime.0.surface_task_active.get());
        cx.update(|cx| runtime.shutdown(cx));
        assert!(factory_weak.upgrade().is_none());
        cx.run_until_parked();
        assert!(!called.get());
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    }

    #[test]
    fn readiness_timeout_is_a_fatal_typed_error_for_the_selected_display() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(99),
                    Size::new(px(48.), px(20.)),
                    |_, cx| cx.new(|_| gpui::EmptyView),
                )?
                .readiness_timeout(std::time::Duration::ZERO);
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });

        cx.run_until_parked();
        let Some(Err(ShellError::Surface(error))) = runtime.result() else {
            panic!("expected typed display readiness failure")
        };
        assert_eq!(error.display_id(), DisplayId::new(99));
        assert!(matches!(
            error.as_ref(),
            SurfaceError::DisplayReadinessTimeout { timeout, .. }
                if *timeout == std::time::Duration::ZERO
        ));
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    }

    #[test]
    fn creation_result_error_is_fatal_once_and_preserves_cause_target_and_host_window() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (resource, _) = descriptor(starts.clone(), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let target = DisplayId::new(99);
        let factory_lifetime = Rc::new(());
        let factory_weak = Rc::downgrade(&factory_lifetime);
        let called = Rc::new(Cell::new(false));
        let called_by_factory = Rc::clone(&called);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
                let surface =
                    SurfaceDescriptor::new(target, Size::new(px(48.), px(20.)), move |_, cx| {
                        let _ = &factory_lifetime;
                        called_by_factory.set(true);
                        cx.new(|_| gpui::EmptyView)
                    })?;
                runtime.mount_surface(surface, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(!runtime.0.surface_task_active.get());
        let cause: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::other("layer creation failed"));
        cx.update(|cx| finish_surface_open(&runtime.0, target, Err(cause), cx));

        let Some(Err(ShellError::Surface(error))) = runtime.result() else {
            panic!("expected surface creation failure")
        };
        assert_eq!(error.display_id(), target);
        assert_eq!(
            error
                .source()
                .and_then(|cause| cause.downcast_ref::<std::io::Error>())
                .map(std::io::Error::to_string)
                .as_deref(),
            Some("layer creation failed")
        );
        assert_eq!((starts.get(), stops.get()), (1, 1));
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        assert!(factory_weak.upgrade().is_none());
        assert!(!called.get());

        let retry =
            SurfaceDescriptor::new(DisplayId::new(1), Size::new(px(48.), px(20.)), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap();
        assert!(matches!(
            cx.update(|cx| runtime.mount_surface(retry, cx)),
            Err(ShellError::Setup(_))
        ));
        cx.run_until_parked();
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        drop(binding);
    }

    #[test]
    fn an_existing_source_failure_remains_primary_over_a_late_creation_error() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let stops = Rc::new(Cell::new(0));
        let (resource, publisher) = descriptor(Rc::new(Cell::new(0)), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let target = DisplayId::new(99);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
                let surface =
                    SurfaceDescriptor::new(target, Size::new(px(48.), px(20.)), |_, cx| {
                        cx.new(|_| gpui::EmptyView)
                    })?;
                runtime.mount_surface(surface, cx)
            })
            .unwrap()
        });
        let publisher = publisher.borrow().as_ref().unwrap().clone();

        cx.update(|cx| publisher.fail(TestError("source failed first"), cx));
        let cause: Box<dyn std::error::Error + Send + Sync> =
            Box::new(std::io::Error::other("late create error"));
        cx.update(|cx| finish_surface_open(&runtime.0, target, Err(cause), cx));

        let Some(Err(ShellError::Source(error))) = runtime.result() else {
            panic!("expected the first source failure to remain primary")
        };
        assert_eq!(
            error
                .source()
                .and_then(|cause| cause.downcast_ref::<TestError>())
                .map(|error| error.0),
            Some("source failed first")
        );
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        drop(binding);
    }

    #[test]
    fn fatal_source_failure_after_surface_creation_closes_only_owned_surface_once() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let stops = Rc::new(Cell::new(0));
        let (resource, publisher) = descriptor(Rc::new(Cell::new(0)), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let binding_slot = Rc::clone(&binding);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                *binding_slot.borrow_mut() = Some(runtime.mount(&resource, cx)?);
                let surface = SurfaceDescriptor::new(
                    DisplayId::new(1),
                    Size::new(px(48.), px(20.)),
                    |_, cx| cx.new(|_| gpui::EmptyView),
                )?;
                runtime.mount_surface(surface, cx)
            })
            .unwrap()
        });
        let publisher = publisher.borrow().as_ref().unwrap().clone();

        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);
        cx.update(|cx| publisher.fail(TestError("source failed"), cx));
        cx.update(|cx| {
            runtime.shutdown(cx);
            runtime.shutdown(cx);
        });

        assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
        assert_eq!(stops.get(), 1);
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
        drop(binding);
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn dropping_last_handle_schedules_cleanup_of_owned_surface_only() {
        let cx = TestAppContext::single();
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(1),
                    Size::new(px(48.), px(20.)),
                    |_, cx| cx.new(|_| gpui::EmptyView),
                )?;
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);

        drop(runtime);
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);
        assert!(host.read_with(&cx, |_, _| ()).is_ok());
    }

    #[test]
    fn host_quit_cleans_runtime_surface_and_finalizes_success() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let descriptor = SurfaceDescriptor::new(
                    DisplayId::new(1),
                    Size::new(px(48.), px(20.)),
                    |_, cx| cx.new(|_| gpui::EmptyView),
                )?;
                runtime.mount_surface(descriptor, cx)
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows().len()), 1);

        cx.quit();

        assert!(matches!(runtime.result(), Some(Ok(()))));
        assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    }

    #[test]
    fn failure_during_start_disposes_late_guard_once() {
        let cx = TestAppContext::single();
        let stops = Rc::new(Cell::new(0));
        let on_start: TestStart =
            Rc::new(|publisher, cx| publisher.fail(TestError("during start"), cx));
        let (descriptor, _) = descriptor_with_options(
            Rc::new(Cell::new(0)),
            stops.clone(),
            false,
            Some(on_start),
            None,
            1,
        );
        let (runtime, mount_result) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            let result = runtime.mount(&descriptor, cx);
            (runtime, result)
        });

        assert!(matches!(mount_result, Err(ResourceAcquireError::Failed(_))));
        assert_eq!(stops.get(), 1);
        assert!(matches!(runtime.result(), Some(Err(ShellError::Source(_)))));
        cx.quit();
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn mount_is_safe_inside_entity_update_and_busy_closed_are_nonfatal() {
        let cx = TestAppContext::single();
        let (runtime_slot, descriptor_slot) = (
            Rc::new(RefCell::new(None::<RuntimeHandle>)),
            Rc::new(RefCell::new(None::<TestDescriptor>)),
        );
        let busy_seen = Rc::new(Cell::new(false));
        let runtime_for_start = Rc::clone(&runtime_slot);
        let runtime_for_setup = Rc::clone(&runtime_slot);
        let descriptor_for_start = Rc::clone(&descriptor_slot);
        let busy_for_start = Rc::clone(&busy_seen);
        let on_start = Rc::new(move |_, cx: &mut App| {
            let runtime = runtime_for_start.borrow().as_ref().unwrap().clone();
            let descriptor = descriptor_for_start.borrow().as_ref().unwrap().clone();
            busy_for_start.set(matches!(
                runtime.mount(&descriptor, cx),
                Err(ResourceAcquireError::Busy)
            ));
            runtime.0.scope.shutdown();
            assert!(matches!(
                runtime.mount(&descriptor, cx),
                Err(ResourceAcquireError::Closed)
            ));
        });
        let (descriptor, _) = descriptor_with_options(
            Rc::new(Cell::new(0)),
            Rc::new(Cell::new(0)),
            false,
            Some(on_start),
            None,
            1,
        );
        *descriptor_slot.borrow_mut() = Some(descriptor.clone());
        let (runtime, mount_result) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |runtime, _| {
                *runtime_for_setup.borrow_mut() = Some(runtime);
                Ok(())
            })
            .unwrap();
            let owner = cx.new(|_| ());
            let result = owner.update(cx, |_, cx| runtime.mount(&descriptor, cx));
            (runtime, result)
        });

        assert!(busy_seen.get());
        assert!(matches!(mount_result, Err(ResourceAcquireError::Closed)));
        assert!(runtime.result().is_none());
        assert!(matches!(
            cx.update(|cx| runtime.mount(&descriptor, cx)),
            Err(ResourceAcquireError::Closed)
        ));
        assert!(runtime.result().is_none());
    }
}

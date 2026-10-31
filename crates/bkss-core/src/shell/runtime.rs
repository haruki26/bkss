use super::{MountedBinding, ShellError};
use crate::{
    ReactiveSubscription, ResourceAcquireError, ResourceScope, Source, SourceDescriptor,
    SourceFailure,
    surface::{LayerSurfaceDeclaration, SurfaceDescriptor, SurfaceError},
};
use gpui::{AnyWindowHandle, App, AsyncApp, Render, Subscription};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub(super) struct RuntimeInner {
    scope: ResourceScope,
    pub(super) outcome: RefCell<Option<Result<(), ShellError>>>,
    pub(super) stopped: Cell<bool>,
    pub(super) surfaces: RefCell<Vec<AnyWindowHandle>>,
    pub(super) surface_works: RefCell<Vec<Rc<super::surface_runtime::SurfaceWork>>>,
    pub(super) dynamic_surfaces: RefCell<Vec<Rc<super::dynamic_surface_runtime::Controller>>>,
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
pub struct RuntimeHandle(pub(super) Rc<RuntimeInner>);

impl RuntimeHandle {
    pub(super) fn new(cx: &mut App, quit_host_on_failure: bool) -> Self {
        let scope = ResourceScope::new(cx);
        let inner = Rc::new(RuntimeInner {
            scope,
            outcome: RefCell::new(None),
            stopped: Cell::new(false),
            surfaces: RefCell::new(Vec::new()),
            surface_works: RefCell::new(Vec::new()),
            dynamic_surfaces: RefCell::new(Vec::new()),
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

    /// Registers a low-level surface. Its factory runs once after the explicitly
    /// selected display is present. Shutdown before readiness prevents creation.
    pub fn mount_surface<V: Render + 'static>(
        &self,
        descriptor: SurfaceDescriptor<V>,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        super::surface_runtime::mount_surface(&self.0, descriptor, cx)
    }

    /// Registers a non-interactive layer surface declaration for startup displays.
    pub fn mount_layer_surface<V: Render + 'static>(
        &self,
        declaration: LayerSurfaceDeclaration<V>,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        super::surface_runtime::mount_layer_surface(&self.0, declaration, cx)
    }

    pub fn register_dynamic_layer_surface<V: Render + 'static>(
        &self,
        config: crate::LayerSurfaceConfig,
        readiness_timeout: std::time::Duration,
        factory: impl FnMut(
            crate::SurfaceInstanceHandle,
            &mut gpui::Window,
            &mut App,
        ) -> Result<gpui::Entity<V>, ShellError>
        + 'static,
        cx: &mut App,
    ) -> Result<crate::DynamicSurfaceHandle, ShellError> {
        super::dynamic_surface_runtime::register(&self.0, config, readiness_timeout, factory, cx)
    }

    pub fn downgrade(&self) -> WeakRuntimeHandle {
        WeakRuntimeHandle(Rc::downgrade(&self.0))
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

#[derive(Clone)]
pub struct WeakRuntimeHandle(std::rc::Weak<RuntimeInner>);

impl WeakRuntimeHandle {
    pub fn upgrade(&self) -> Option<RuntimeHandle> {
        self.0.upgrade().map(RuntimeHandle)
    }
}

#[derive(Debug)]
pub(super) enum SurfaceRegistrationError {
    Closed,
}

impl std::fmt::Display for SurfaceRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("cannot register a surface after runtime shutdown"),
        }
    }
}

impl std::error::Error for SurfaceRegistrationError {}

impl Drop for RuntimeInner {
    fn drop(&mut self) {
        self.scope.shutdown();
        super::surface_runtime::drop_surface_works(self.surface_works.get_mut());
        super::dynamic_surface_runtime::drop_controllers(
            self.dynamic_surfaces.get_mut(),
            &self.app,
        );
        super::surface_window::remove_windows_later(
            &self.app,
            std::mem::take(self.surfaces.get_mut()),
        );
    }
}
pub(super) fn stop_runtime(inner: &RuntimeInner, cx: &mut App) {
    if inner.stopped.replace(true) {
        return;
    }
    inner.scope.shutdown();
    super::surface_runtime::stop_surface_works(inner);
    super::dynamic_surface_runtime::stop_controllers(inner, cx);
    let handles = std::mem::take(&mut *inner.surfaces.borrow_mut());
    for handle in handles {
        super::surface_window::remove_window(cx, handle);
    }
}

pub(super) fn record_surface_failure(inner: &RuntimeInner, error: SurfaceError, cx: &mut App) {
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

pub(super) fn record_setup_failure(inner: &RuntimeInner, error: ShellError) {
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

pub(super) fn request_host_quit(inner: &RuntimeInner, cx: &mut App) {
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
    use gpui::{AppContext, TestAppContext, WindowOptions};
    use std::{cell::Cell, rc::Rc};

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

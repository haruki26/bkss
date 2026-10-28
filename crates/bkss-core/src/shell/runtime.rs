use super::{MountedBinding, ShellError};
use crate::{
    ReactiveSubscription, ResourceAcquireError, ResourceScope, Source, SourceDescriptor,
    SourceFailure,
};
use gpui::{App, Subscription};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct RuntimeInner {
    scope: ResourceScope,
    outcome: RefCell<Option<Result<(), ShellError>>>,
    quit_requested: Cell<bool>,
    _failure_subscription: RefCell<Option<ReactiveSubscription>>,
    _quit_subscription: RefCell<Option<Subscription>>,
}

/// Cloneable handle to a single attached shell runtime.
///
/// Keep at least one handle alive for the runtime's lifetime. Dropping the last
/// handle stops its resources but does not request that the host app quit.
#[derive(Clone)]
pub struct RuntimeHandle(Rc<RuntimeInner>);

impl RuntimeHandle {
    pub(super) fn new(cx: &mut App) -> Self {
        let scope = ResourceScope::new(cx);
        let inner = Rc::new(RuntimeInner {
            scope,
            outcome: RefCell::new(None),
            quit_requested: Cell::new(false),
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
                    request_quit(&inner, cx);
                }
                true
            });
        *inner._failure_subscription.borrow_mut() = Some(subscription);

        let weak = Rc::downgrade(&inner);
        let subscription = cx.on_app_quit(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.scope.shutdown();
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

    /// Requests normal resource shutdown and asks GPUI to quit.
    pub fn shutdown(&self, cx: &mut App) {
        self.0.scope.shutdown();
        request_quit(&self.0, cx);
    }

    /// Returns a non-consuming result snapshot.
    ///
    /// Returns `None` while running, `Some(Err(_))` after fatal failure, and
    /// `Some(Ok(()))` after normal finalization. This is not a cleanup-completion
    /// barrier for surfaces or asynchronous cancellation.
    pub fn result(&self) -> Option<Result<(), ShellError>> {
        self.0.outcome.borrow().clone()
    }

    pub(super) fn setup_failed(&self, error: ShellError, cx: &mut App) -> ShellError {
        record_setup_failure(&self.0, error.clone());
        self.0.scope.shutdown();
        request_quit(&self.0, cx);
        self.result().and_then(Result::err).unwrap_or(error)
    }

    pub(super) fn finish_after_run(&self) -> Option<Result<(), ShellError>> {
        self.0.scope.shutdown();
        finalize_success(&self.0);
        self.result()
    }
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

fn request_quit(inner: &RuntimeInner, cx: &mut App) {
    if !inner.quit_requested.replace(true) {
        cx.quit();
    }
}

#[cfg(test)]
mod tests {
    use super::super::Shell;
    use super::super::test_support::*;
    use super::*;
    use crate::ResourceAcquireError;
    use gpui::{AppContext, TestAppContext};
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
    fn scope_shutdown_is_resource_only_and_runtime_quit_request_is_deduplicated() {
        let cx = TestAppContext::single();
        let runtime = cx.update(|cx| Shell::attach(cx, |_, _| Ok(())).unwrap());

        runtime.0.scope.shutdown();
        assert!(runtime.result().is_none());
        assert!(!runtime.0.quit_requested.get());
        cx.update(|cx| {
            runtime.shutdown(cx);
            assert!(runtime.result().is_none());
            assert!(runtime.0.quit_requested.get());
            runtime.shutdown(cx);
        });
        assert!(runtime.result().is_none());
        cx.quit();
        assert!(matches!(runtime.result(), Some(Ok(()))));
    }

    #[test]
    fn setup_error_stops_retained_resources_before_requesting_quit() {
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
        assert!(!runtime.0.quit_requested.get());
    }
}

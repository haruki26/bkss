use super::{
    ResourceAcquireError, ResourceLease, Source, SourceDescriptor, SourceFailure,
    SourceFailurePhase, SourcePublisher,
};
use crate::reactive::{Reactive, SourceState, State};
use gpui::{App, AppContext};
use std::{
    any::{Any, TypeId},
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

#[derive(Clone)]
struct RegistryStopHooks {
    invalidate: InvalidateHook,
    clear: Rc<dyn Fn()>,
}
type ErasedGuard = Box<dyn Any>;
type InvalidateHook = Rc<dyn Fn(&mut Vec<ErasedGuard>)>;

pub(super) struct ScopeInner {
    pub(super) closed: Cell<bool>,
    pub(super) failure: RefCell<Option<Rc<SourceFailure>>>,
    pub(super) failure_state: State<Option<Rc<SourceFailure>>>,
    pub(super) registries: RefCell<HashMap<TypeId, Box<dyn Any>>>,
    stop_hooks: RefCell<Vec<RegistryStopHooks>>,
}

/// Owns source activations. Leases and publishers do not keep this scope alive.
pub struct ResourceScope {
    pub(super) inner: Rc<ScopeInner>,
}

pub(super) struct Activation<S: Source> {
    pub(super) state: State<SourceState<S::Value>>,
    pub(super) alive: Rc<Cell<bool>>,
    pub(super) consumers: usize,
    pub(super) phase: ActivationPhase,
    pub(super) guard: Option<S::Guard>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivationPhase {
    Starting,
    Running,
    Stopping,
}

pub(super) type ActivationMap<S> = HashMap<<S as Source>::Key, Rc<RefCell<Activation<S>>>>;

pub(super) fn lease_running<S: Source>(
    inner: &Rc<ScopeInner>,
    map: &Rc<RefCell<ActivationMap<S>>>,
    key: S::Key,
    activation: Rc<RefCell<Activation<S>>>,
) -> Result<ResourceLease<S::Value>, ResourceAcquireError> {
    if activation.borrow().phase != ActivationPhase::Running {
        return Err(ResourceAcquireError::Busy);
    }
    activation.borrow_mut().consumers += 1;
    Ok(ResourceLease::new(inner, map, key, activation))
}

impl ResourceScope {
    pub fn new<C: AppContext>(cx: &mut C) -> Self {
        let failure_state = State::new(None, cx);
        Self {
            inner: Rc::new(ScopeInner {
                closed: Cell::new(false),
                failure: RefCell::new(None),
                failure_state,
                registries: RefCell::new(HashMap::new()),
                stop_hooks: RefCell::new(Vec::new()),
            }),
        }
    }

    pub fn failure(&self) -> Option<Rc<SourceFailure>> {
        self.inner.failure.borrow().clone()
    }
    pub fn failure_reactive(&self) -> Reactive<Option<Rc<SourceFailure>>> {
        self.inner.failure_state.reactive()
    }

    /// Acquires one consumer lease. A same-key activation that is starting or
    /// being cleaned up returns `Busy`; descriptor and reactive clones do not
    /// count as consumers.
    pub fn acquire<S: Source>(
        &self,
        descriptor: &SourceDescriptor<S>,
        cx: &mut App,
    ) -> Result<ResourceLease<S::Value>, ResourceAcquireError> {
        check_scope(&self.inner)?;
        let map = {
            let mut registries = self.inner.registries.borrow_mut();
            registries.entry(TypeId::of::<S>()).or_insert_with(|| {
                let map = Rc::new(RefCell::new(ActivationMap::<S>::new()));
                let weak = Rc::downgrade(&map);
                let invalidate_map = weak.clone();
                self.inner.stop_hooks.borrow_mut().push(RegistryStopHooks {
                    invalidate: Rc::new(move |guards| {
                        if let Some(map) = invalidate_map.upgrade() {
                            invalidate_map_activations(&map, guards);
                        }
                    }),
                    clear: Rc::new(move || {
                        if let Some(map) = weak.upgrade() {
                            map.borrow_mut().clear();
                        }
                    }),
                });
                Box::new(map)
            });
            registries
                .get(&TypeId::of::<S>())
                .and_then(|map| map.downcast_ref::<Rc<RefCell<ActivationMap<S>>>>())
                .cloned()
        };
        let Some(map) = map else {
            return Err(ResourceAcquireError::Closed);
        };
        let activation = if let Some(found) = map.borrow().get(&descriptor.key).cloned() {
            return lease_running(&self.inner, &map, descriptor.key.clone(), found);
        } else {
            let state = State::new(SourceState::Loading, cx);
            let activation = Rc::new(RefCell::new(Activation {
                state,
                alive: Rc::new(Cell::new(true)),
                consumers: 1,
                phase: ActivationPhase::Starting,
                guard: None,
            }));
            map.borrow_mut()
                .insert(descriptor.key.clone(), Rc::clone(&activation));
            activation
        };
        let publisher =
            SourcePublisher::new(Rc::downgrade(&self.inner), Rc::downgrade(&activation));
        let started = descriptor.source.start(publisher, cx);
        match started {
            Ok(guard) => {
                let mut a = activation.borrow_mut();
                if a.phase == ActivationPhase::Starting
                    && a.alive.get()
                    && !self.inner.closed.get()
                    && self.failure().is_none()
                {
                    a.guard = Some(guard);
                    a.phase = ActivationPhase::Running;
                    drop(a);
                    Ok(ResourceLease::new(
                        &self.inner,
                        &map,
                        descriptor.key.clone(),
                        activation,
                    ))
                } else {
                    drop(a);
                    drop(guard);
                    Err(self.acquire_status())
                }
            }
            Err(error) => {
                let failure = Rc::new(SourceFailure {
                    source_type: TypeId::of::<S>(),
                    phase: SourceFailurePhase::Start,
                    cause: Box::new(error),
                });
                self.fail_scope(failure, cx);
                Err(self.acquire_status())
            }
        }
    }

    fn acquire_status(&self) -> ResourceAcquireError {
        self.failure()
            .map(ResourceAcquireError::Failed)
            .unwrap_or(ResourceAcquireError::Closed)
    }

    fn fail_scope(&self, failure: Rc<SourceFailure>, cx: &mut App) {
        fail_scope(Rc::clone(&self.inner), failure, cx);
    }

    /// Stops all activations. Existing leases and escaped reactive views remain
    /// readable but cannot publish or keep this scope alive.
    pub fn shutdown(&self) {
        if self.inner.closed.replace(true) {
            return;
        }
        stop_all(&self.inner);
    }
}

pub(super) fn check_scope(inner: &ScopeInner) -> Result<(), ResourceAcquireError> {
    if let Some(failure) = inner.failure.borrow().clone() {
        Err(ResourceAcquireError::Failed(failure))
    } else if inner.closed.get() {
        Err(ResourceAcquireError::Closed)
    } else {
        Ok(())
    }
}

fn stop_all(inner: &ScopeInner) {
    let callbacks = inner.stop_hooks.borrow().clone();
    let mut guards = Vec::<ErasedGuard>::new();
    for hooks in &callbacks {
        (hooks.invalidate)(&mut guards);
    }
    drop(guards);
    for hooks in callbacks {
        (hooks.clear)();
    }
}

fn invalidate_map_activations<S: Source>(
    map: &Rc<RefCell<ActivationMap<S>>>,
    guards: &mut Vec<ErasedGuard>,
) {
    let activations: Vec<_> = map.borrow().values().cloned().collect();
    for activation in &activations {
        let mut a = activation.borrow_mut();
        a.alive.set(false);
        a.phase = ActivationPhase::Stopping;
        if let Some(guard) = a.guard.take() {
            guards.push(Box::new(guard));
        }
    }
}

pub(super) fn fail_scope(inner: Rc<ScopeInner>, failure: Rc<SourceFailure>, cx: &mut App) {
    if inner.closed.replace(true) {
        return;
    }
    *inner.failure.borrow_mut() = Some(Rc::clone(&failure));
    stop_all(&inner);
    inner.failure_state.set(Some(failure), cx);
}

impl Drop for ResourceScope {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::source::test_support::*;
    use gpui::TestAppContext;
    use std::{
        cell::Cell,
        error::Error,
        rc::{Rc, Weak},
    };

    #[test]
    fn failed_start_retains_original_error_and_rejects_later_acquire() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let publisher = Rc::new(RefCell::new(None));
        let desc = SourceDescriptor::new(FakeSource {
            key: 1,
            starts: Rc::clone(&starts),
            stops: Rc::clone(&stops),
            publisher,
            initial: None,
            fail_start: true,
            on_start: None,
        });
        let scope = cx.update(ResourceScope::new);
        let error = cx.update(|cx| match scope.acquire(&desc, cx) {
            Err(error) => error,
            Ok(_) => panic!("expected source failure"),
        });
        let ResourceAcquireError::Failed(failure) = error else {
            panic!("expected source failure")
        };
        assert_eq!(failure.phase(), SourceFailurePhase::Start);
        assert_eq!(failure.source_type(), TypeId::of::<FakeSource>());
        assert_eq!(
            failure
                .source()
                .and_then(|error| error.downcast_ref::<FakeError>())
                .map(|error| error.0),
            Some("start failed")
        );
        assert!(
            matches!(cx.update(|cx| scope.acquire(&desc, cx)), Err(ResourceAcquireError::Failed(found)) if Rc::ptr_eq(&failure, &found))
        );
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 0);
    }

    #[test]
    fn publisher_failure_is_primary_once_and_notifies_after_all_guard_drops() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let other_starts = Rc::new(Cell::new(0));
        let other_stops = Rc::new(Cell::new(0));
        let scope_weak: Rc<RefCell<Option<Weak<ScopeInner>>>> = Rc::new(RefCell::new(None));
        let blocked_during_cleanup = Rc::new(Cell::new(false));
        let weak_cb = scope_weak.clone();
        let blocked_cb = blocked_during_cleanup.clone();
        let publisher = Rc::new(RefCell::new(None));
        let desc = source(1, starts, stops.clone(), publisher.clone());
        let other = SourceDescriptor::new(OtherSource {
            key: 1,
            starts: other_starts,
            stops: other_stops.clone(),
            on_drop: Some(Rc::new(move || {
                let inner = weak_cb.borrow().as_ref().unwrap().upgrade().unwrap();
                let registries = inner.registries.borrow();
                let all_invalid = registries
                    .get(&TypeId::of::<FakeSource>())
                    .and_then(|map| map.downcast_ref::<Rc<RefCell<ActivationMap<FakeSource>>>>())
                    .is_some_and(|map| {
                        map.borrow()
                            .values()
                            .all(|activation| !activation.borrow().alive.get())
                    })
                    && registries
                        .get(&TypeId::of::<OtherSource>())
                        .and_then(|map| {
                            map.downcast_ref::<Rc<RefCell<ActivationMap<OtherSource>>>>()
                        })
                        .is_some_and(|map| {
                            map.borrow()
                                .values()
                                .all(|activation| !activation.borrow().alive.get())
                        });
                blocked_cb.set(
                    all_invalid
                        && matches!(check_scope(&inner), Err(ResourceAcquireError::Failed(_))),
                );
            })),
        });
        let scope = cx.update(ResourceScope::new);
        *scope_weak.borrow_mut() = Some(Rc::downgrade(&scope.inner));
        let (lease, other_lease) = cx.update(|cx| {
            (
                scope.acquire(&desc, cx).unwrap(),
                scope.acquire(&other, cx).unwrap(),
            )
        });
        let notified_stops = Rc::new(Cell::new(0));
        let notification_calls = Rc::new(Cell::new(0));
        let notified = notified_stops.clone();
        let calls = notification_calls.clone();
        let a = stops.clone();
        let b = other_stops.clone();
        let failure_reactive = scope.failure_reactive();
        let owner = cx.update(|cx| cx.new(|_| ()));
        let sub = cx.update(|cx| {
            owner.update(cx, |_, cx| {
                failure_reactive.observe(cx, move |_, failure, _| {
                    assert!(failure.is_some());
                    calls.set(calls.get() + 1);
                    notified.set(a.get() + b.get());
                })
            })
        });
        let first_publisher = publisher.borrow().as_ref().unwrap().clone();
        cx.update(|cx| first_publisher.fail(FakeError("subscription failed"), cx));
        assert_eq!(stops.get(), 1);
        assert_eq!(other_stops.get(), 1);
        assert!(blocked_during_cleanup.get());
        assert_eq!(notified_stops.get(), 2);
        assert_eq!(notification_calls.get(), 1);
        let primary = scope.failure().unwrap();
        assert_eq!(primary.phase(), SourceFailurePhase::Subscription);
        assert_eq!(primary.source_type(), TypeId::of::<FakeSource>());
        assert_eq!(
            primary
                .source()
                .and_then(|e| e.downcast_ref::<FakeError>())
                .map(|e| e.0),
            Some("subscription failed")
        );
        cx.update(|cx| {
            first_publisher.fail(FakeError("later failure"), cx);
            scope.shutdown();
        });
        assert!(Rc::ptr_eq(&primary, &scope.failure().unwrap()));
        assert_eq!(notified_stops.get(), 2);
        assert_eq!(notification_calls.get(), 1);
        assert_eq!((stops.get(), other_stops.get()), (1, 1));
        drop((lease, other_lease, sub, owner));
    }

    #[test]
    fn failure_inside_start_disposes_late_guard_and_does_not_revive_activation() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let publisher_slot = Rc::new(RefCell::new(None));
        let publisher_for_start = publisher_slot.clone();
        let desc = SourceDescriptor::new(FakeSource {
            key: 1,
            starts: starts.clone(),
            stops: stops.clone(),
            publisher: publisher_slot,
            initial: None,
            fail_start: false,
            on_start: Some(Rc::new(move |publisher, cx| {
                *publisher_for_start.borrow_mut() = Some(publisher.clone());
                publisher.fail(FakeError("failed while starting"), cx);
            })),
        });
        let scope = cx.update(ResourceScope::new);
        let result = cx.update(|cx| scope.acquire(&desc, cx));
        assert!(matches!(result, Err(ResourceAcquireError::Failed(_))));
        assert_eq!(starts.get(), 1);
        assert_eq!(stops.get(), 1);
        let failure = scope.failure().unwrap();
        assert_eq!(failure.phase(), SourceFailurePhase::Subscription);
        assert!(
            matches!(cx.update(|cx| scope.acquire(&desc, cx)), Err(ResourceAcquireError::Failed(found)) if Rc::ptr_eq(&failure, &found))
        );
        scope.shutdown();
        scope.shutdown();
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn start_failure_cleans_other_live_source_types_once() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let live_starts = Rc::new(Cell::new(0));
        let live_stops = Rc::new(Cell::new(0));
        let live = SourceDescriptor::new(OtherSource {
            key: 4,
            starts: live_starts,
            stops: live_stops.clone(),
            on_drop: None,
        });
        let failing = SourceDescriptor::new(FakeSource {
            key: 2,
            starts,
            stops,
            publisher: Rc::new(RefCell::new(None)),
            initial: None,
            fail_start: true,
            on_start: None,
        });
        let scope = cx.update(ResourceScope::new);
        let lease = cx.update(|cx| scope.acquire(&live, cx).unwrap());
        let _ = cx.update(|cx| scope.acquire(&failing, cx));
        assert_eq!(live_stops.get(), 1);
        scope.shutdown();
        assert_eq!(live_stops.get(), 1);
        drop(lease);
    }
}

use super::{
    Source,
    scope::{Activation, ActivationMap, ActivationPhase, ScopeInner},
};
use crate::reactive::{Reactive, SourceState};
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

pub struct ResourceLease<T: Clone + 'static> {
    state: Reactive<SourceState<T>>,
    release: Option<Box<dyn FnOnce()>>,
}
impl<T: Clone + 'static> ResourceLease<T> {
    pub(super) fn new<S: Source<Value = T>>(
        inner: &Rc<ScopeInner>,
        map: &Rc<RefCell<ActivationMap<S>>>,
        key: S::Key,
        activation: Rc<RefCell<Activation<S>>>,
    ) -> Self {
        let state = activation.borrow().state.reactive();
        let weak_inner = Rc::downgrade(inner);
        let weak_map = Rc::downgrade(map);
        Self {
            state,
            release: Some(Box::new(move || {
                release::<S>(weak_inner, weak_map, key, activation)
            })),
        }
    }
    /// Returns a read-only view; retaining it does not retain a consumer lease.
    pub fn reactive(&self) -> Reactive<SourceState<T>> {
        self.state.clone()
    }
}
impl<T: Clone + 'static> Drop for ResourceLease<T> {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

fn release<S: Source>(
    inner: Weak<ScopeInner>,
    map: Weak<RefCell<ActivationMap<S>>>,
    key: S::Key,
    activation: Rc<RefCell<Activation<S>>>,
) {
    let Some(inner) = inner.upgrade() else { return };
    let (guard, stop) = {
        let mut a = activation.borrow_mut();
        if a.consumers == 0 {
            return;
        }
        a.consumers -= 1;
        if a.consumers != 0 {
            return;
        }
        a.alive.set(false);
        a.phase = ActivationPhase::Stopping;
        (a.guard.take(), true)
    };
    drop(guard);
    if stop
        && let Some(map) = map.upgrade()
        && map
            .borrow()
            .get(&key)
            .is_some_and(|current| Rc::ptr_eq(current, &activation))
    {
        map.borrow_mut().remove(&key);
    }
    let _ = inner;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::{SourceState, State};
    use crate::resource::{
        ResourceAcquireError, ResourceScope, SourceDescriptor, SourcePublisher,
        scope::lease_running, source::test_support::*,
    };
    use gpui::{AppContext, TestAppContext};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[test]
    fn normalized_descriptors_share_and_last_lease_stops_before_fresh_loading_reacquire() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let publisher = Rc::new(RefCell::new(None));
        let first = source(
            11,
            Rc::clone(&starts),
            Rc::clone(&stops),
            Rc::clone(&publisher),
        );
        let alias = source(
            19,
            Rc::clone(&starts),
            Rc::clone(&stops),
            Rc::clone(&publisher),
        );
        let scope = cx.update(ResourceScope::new);
        let (a, b) = cx.update(|cx| {
            (
                scope.acquire(&first, cx).unwrap(),
                scope.acquire(&alias, cx).unwrap(),
            )
        });
        assert_eq!(starts.get(), 1);
        let reactive = a.reactive();
        let stale_publisher = publisher.borrow().as_ref().unwrap().clone();
        cx.update(|cx| publisher.borrow().as_ref().unwrap().publish(None, cx));
        assert_eq!(reactive.get(&cx), SourceState::Ready(None));
        drop(a);
        assert_eq!(stops.get(), 0);
        drop(b);
        assert_eq!(stops.get(), 1);
        cx.update(|cx| stale_publisher.publish(Some(9), cx));
        assert_eq!(reactive.get(&cx), SourceState::Ready(None));
        let next = cx.update(|cx| scope.acquire(&alias, cx).unwrap());
        assert_eq!(starts.get(), 2);
        assert_eq!(next.reactive().get(&cx), SourceState::Loading);
        cx.update(|cx| stale_publisher.fail(FakeError("stale"), cx));
        assert!(scope.failure().is_none());
        drop(next);
        scope.shutdown();
        assert_eq!(stops.get(), 2);
    }

    #[test]
    fn scope_drop_invalidates_publishers_without_leases_owning_scope() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let publisher = Rc::new(RefCell::new(None));
        let desc = source(1, starts, Rc::clone(&stops), Rc::clone(&publisher));
        let scope = cx.update(ResourceScope::new);
        let lease = cx.update(|cx| scope.acquire(&desc, cx).unwrap());
        let old = lease.reactive();
        let late = publisher.borrow().as_ref().unwrap().clone();
        drop(scope);
        assert_eq!(stops.get(), 1);
        cx.update(|cx| late.publish(Some(4), cx));
        assert_eq!(old.get(&cx), SourceState::Loading);
        drop(lease);
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn identity_separates_key_type_and_scope() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let pub1 = Rc::new(RefCell::new(None));
        let pub2 = Rc::new(RefCell::new(None));
        let a = source(11, starts.clone(), stops.clone(), pub1);
        let b = source(21, starts.clone(), stops.clone(), pub2);
        let other_starts = Rc::new(Cell::new(0));
        let other_stops = Rc::new(Cell::new(0));
        let other = SourceDescriptor::new(OtherSource {
            key: 1,
            starts: other_starts.clone(),
            stops: other_stops.clone(),
            on_drop: None,
        });
        let one = cx.update(ResourceScope::new);
        let two = cx.update(ResourceScope::new);
        let leases = cx.update(|cx| {
            (
                one.acquire(&a, cx).unwrap(),
                one.acquire(&b, cx).unwrap(),
                one.acquire(&other, cx).unwrap(),
                two.acquire(&a, cx).unwrap(),
            )
        });
        assert_eq!(starts.get(), 3);
        assert_eq!(other_starts.get(), 1);
        drop(leases);
    }

    #[test]
    fn equal_publications_notify_and_ready_derivations_propagate() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let publisher = Rc::new(RefCell::new(None));
        let desc = source(1, starts.clone(), stops.clone(), publisher.clone());
        let scope = cx.update(ResourceScope::new);
        let lease = cx.update(|cx| scope.acquire(&desc, cx).unwrap());
        let (mapped, combined, count, owner, _sub) = cx.update(|cx| {
            let reactive = lease.reactive();
            let mapped = reactive.map_ready(cx, |value| value.map(|n| n + 1));
            let other = State::new(SourceState::Ready(4), cx).reactive();
            let combined = reactive.combine_ready(&other, cx, |left, right| (left, right));
            let count = Rc::new(Cell::new(0));
            let observed = mapped.clone();
            let count_cb = count.clone();
            let owner = cx.new(|_| ());
            let sub = owner.update(cx, |_, cx| {
                observed.observe(cx, move |_, _, _| count_cb.set(count_cb.get() + 1))
            });
            (mapped, combined, count, owner, sub)
        });
        cx.update(|cx| {
            publisher.borrow().as_ref().unwrap().publish(Some(2), cx);
            publisher.borrow().as_ref().unwrap().publish(Some(2), cx);
        });
        assert_eq!(mapped.get(&cx), SourceState::Ready(Some(3)));
        assert_eq!(combined.get(&cx), SourceState::Ready((Some(2), 4)));
        assert_eq!(count.get(), 2);
        drop(lease);
        assert_eq!(stops.get(), 1);
        let renewed = cx.update(|cx| scope.acquire(&desc, cx).unwrap());
        assert_eq!(starts.get(), 2);
        assert_eq!(renewed.reactive().get(&cx), SourceState::Loading);
        drop((renewed, owner));
    }

    #[test]
    fn same_key_acquire_during_start_is_busy() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let scope_holder: Rc<RefCell<Option<Rc<ResourceScope>>>> = Rc::new(RefCell::new(None));
        let alias_holder: Rc<RefCell<Option<SourceDescriptor<FakeSource>>>> =
            Rc::new(RefCell::new(None));
        let result = Rc::new(RefCell::new(None));
        let scope_cb = scope_holder.clone();
        let alias_cb = alias_holder.clone();
        let result_cb = result.clone();
        let publisher = Rc::new(RefCell::new(None));
        let desc = SourceDescriptor::new(FakeSource {
            key: 11,
            starts: starts.clone(),
            stops: stops.clone(),
            publisher,
            initial: None,
            fail_start: false,
            on_start: Some(Rc::new(move |_, cx| {
                let outcome = scope_cb
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .acquire(alias_cb.borrow().as_ref().unwrap(), cx);
                *result_cb.borrow_mut() = Some(matches!(outcome, Err(ResourceAcquireError::Busy)));
            })),
        });
        let alias = source(19, starts.clone(), stops, Rc::new(RefCell::new(None)));
        let scope = Rc::new(cx.update(ResourceScope::new));
        *scope_holder.borrow_mut() = Some(scope.clone());
        *alias_holder.borrow_mut() = Some(alias);
        let lease = cx.update(|cx| scope.acquire(&desc, cx).unwrap());
        assert_eq!(*result.borrow(), Some(true));
        assert_eq!(starts.get(), 1);
        drop(lease);
    }

    #[test]
    fn same_key_acquire_while_activation_is_stopping_is_busy() {
        let cx = TestAppContext::single();
        let scope = cx.update(ResourceScope::new);
        let map = Rc::new(RefCell::new(ActivationMap::<FakeSource>::new()));
        let stops = Rc::new(Cell::new(0));
        let activation = Rc::new(RefCell::new(Activation {
            state: cx.update(|cx| State::new(SourceState::Loading, cx)),
            alive: Rc::new(Cell::new(false)),
            consumers: 0,
            phase: ActivationPhase::Stopping,
            guard: Some(FakeGuard(stops.clone())),
        }));
        map.borrow_mut().insert(7, activation.clone());
        assert!(matches!(
            lease_running(&scope.inner, &map, 7, activation),
            Err(ResourceAcquireError::Busy)
        ));
        drop(map);
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn invalidation_skips_queued_reentrant_value_then_reacquires_fresh_activation() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let scope = Rc::new(cx.update(ResourceScope::new));
        let lease_slot: Rc<RefCell<Option<ResourceLease<Option<u8>>>>> =
            Rc::new(RefCell::new(None));
        let alias_slot: Rc<RefCell<Option<SourceDescriptor<FakeSource>>>> =
            Rc::new(RefCell::new(None));
        let old_publisher: Rc<RefCell<Option<SourcePublisher<FakeSource>>>> =
            Rc::new(RefCell::new(None));
        let published: Rc<RefCell<Vec<SourceState<Option<u8>>>>> =
            Rc::new(RefCell::new(Vec::new()));
        let publisher_cb = old_publisher.clone();
        let first = SourceDescriptor::new(FakeSource {
            key: 11,
            starts: starts.clone(),
            stops: stops.clone(),
            publisher: old_publisher.clone(),
            initial: None,
            fail_start: false,
            on_start: Some(Rc::new(move |publisher, _| {
                *publisher_cb.borrow_mut() = Some(publisher)
            })),
        });
        let alias = source(
            19,
            starts.clone(),
            stops.clone(),
            Rc::new(RefCell::new(None)),
        );
        *alias_slot.borrow_mut() = Some(alias);
        let lease = cx.update(|cx| scope.acquire(&first, cx).unwrap());
        let reactive = lease.reactive();
        *lease_slot.borrow_mut() = Some(lease);
        let owner = cx.update(|cx| cx.new(|_| ()));
        let observed = reactive.clone();
        let values = published.clone();
        let publisher_for_cb = old_publisher.clone();
        let scope_for_cb = scope.clone();
        let alias_for_cb = alias_slot.clone();
        let leases_for_cb = lease_slot.clone();
        let subscription = cx.update(|cx| {
            owner.update(cx, |_, cx| {
                observed.observe(cx, move |_, value, cx| {
                    values.borrow_mut().push(value.clone());
                    if *value == SourceState::Ready(Some(1)) {
                        let old = publisher_for_cb.borrow().as_ref().unwrap().clone();
                        old.publish(Some(2), cx);
                        drop(leases_for_cb.borrow_mut().take());
                        let fresh = scope_for_cb
                            .acquire(alias_for_cb.borrow().as_ref().unwrap(), cx)
                            .unwrap();
                        assert_eq!(fresh.reactive().get(cx), SourceState::Loading);
                        *leases_for_cb.borrow_mut() = Some(fresh);
                    }
                })
            })
        });
        cx.update(|cx| {
            old_publisher
                .borrow()
                .as_ref()
                .unwrap()
                .publish(Some(1), cx)
        });
        assert_eq!(*published.borrow(), [SourceState::Ready(Some(1))]);
        assert_eq!(reactive.get(&cx), SourceState::Ready(Some(1)));
        assert_eq!(starts.get(), 2);
        drop(subscription);
        drop(owner);
        drop(lease_slot.borrow_mut().take());
    }
}

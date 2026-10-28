mod cell;
mod source_state;

pub use cell::ReactiveSubscription;
use cell::{Observer, ValueCell, invoke_observer, listen, publish, publish_guarded, publish_weak};
use gpui::{App, AppContext, Context, Entity};
pub use source_state::SourceState;
use std::{
    any::Any,
    cell::{Cell as Counter, RefCell},
    collections::VecDeque,
    rc::Rc,
};

struct ReactiveLifetime {
    _subscriptions: Vec<ReactiveSubscription>,
    _dependencies: Vec<Box<dyn Any>>,
}

/// A writable, GPUI-managed value. Clones share the same entity.
pub struct State<T> {
    entity: Entity<ValueCell<T>>,
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self {
            entity: self.entity.clone(),
        }
    }
}

impl<T: 'static> State<T> {
    /// Creates a ready state in the supplied GPUI context.
    pub fn new<C: AppContext>(value: T, cx: &mut C) -> Self {
        Self {
            entity: cx.new(|_| ValueCell::new(value)),
        }
    }

    /// Publishes a value and notifies dependents, including for equal values.
    pub fn set(&self, value: T, cx: &mut App) {
        publish(&self.entity, value, cx);
    }

    pub(crate) fn set_guarded(
        &self,
        value: T,
        alive: std::rc::Weak<std::cell::Cell<bool>>,
        cx: &mut App,
    ) {
        publish_guarded(&self.entity, value, Some(alive), cx);
    }

    /// Returns a read-only view of this state.
    pub fn reactive(&self) -> Reactive<T> {
        Reactive {
            entity: self.entity.clone(),
            lifetime: None,
        }
    }
}

/// A read-only view of a GPUI-managed value.
pub struct Reactive<T> {
    entity: Entity<ValueCell<T>>,
    lifetime: Option<Rc<ReactiveLifetime>>,
}

impl<T> Clone for Reactive<T> {
    fn clone(&self) -> Self {
        Self {
            entity: self.entity.clone(),
            lifetime: self.lifetime.clone(),
        }
    }
}

impl<T: Clone + 'static> Reactive<T> {
    /// Reads a cloned snapshot of the current value.
    pub fn get<C: AppContext>(&self, cx: &C) -> T
    where
        T: Clone,
    {
        self.entity
            .read_with(cx, |cell, _| cell.value.as_ref().clone())
    }

    /// Observes every synchronous publication from a GPUI entity context.
    ///
    /// Keep the returned subscription alive for as long as notifications are needed.
    pub fn observe<Owner: 'static>(
        &self,
        cx: &mut Context<Owner>,
        callback: impl FnMut(&mut Owner, &T, &mut Context<Owner>) + 'static,
    ) -> ReactiveSubscription {
        let owner = cx.weak_entity();
        let observer = Rc::new(Observer {
            callback: RefCell::new(Some(Box::new(callback))),
            pending: RefCell::new(VecDeque::new()),
        });
        listen(&self.entity, cx, move |publication, app| {
            if owner.upgrade().is_none() {
                return false;
            }
            invoke_observer(&observer, &owner, Rc::clone(&publication.value), app);
            true
        })
    }

    /// Creates a derived value by synchronously mapping each publication.
    pub fn map<U, C, F>(&self, cx: &mut C, map: F) -> Reactive<U>
    where
        T: Clone,
        U: 'static,
        C: AppContext,
        F: Fn(T) -> U + 'static,
    {
        let initial = map(self.get(cx));
        let entity = cx.new(|_| ValueCell::new(initial));
        let source = self.entity.clone();
        let target = entity.downgrade();
        let subscription = listen(&source, cx, move |publication, cx| {
            publish_weak(&target, map(publication.value.as_ref().clone()), cx)
        });
        Reactive {
            entity,
            lifetime: Some(Rc::new(ReactiveLifetime {
                _subscriptions: vec![subscription],
                _dependencies: vec![Box::new(self.clone())],
            })),
        }
    }

    /// Combines the latest values from two reactive inputs on every publication.
    pub fn combine<B, U, C, F>(&self, other: &Reactive<B>, cx: &mut C, combine: F) -> Reactive<U>
    where
        T: Clone,
        B: Clone + 'static,
        U: 'static,
        C: AppContext,
        F: Fn(T, B) -> U + 'static,
    {
        let initial = combine(self.get(cx), other.get(cx));
        let entity = cx.new(|_| ValueCell::new(initial));
        let left = self.entity.clone();
        let right = other.entity.clone();
        let left_weak = left.downgrade();
        let right_weak = right.downgrade();
        let target = entity.downgrade();
        let combine = Rc::new(combine);
        let same_source = left.entity_id() == right.entity_id();
        let same_source_sequence = Rc::new(Counter::new(None));

        let left_subscription = listen(&left, cx, {
            let target = target.clone();
            let same_source_sequence = Rc::clone(&same_source_sequence);
            let combine = Rc::clone(&combine);
            move |publication, cx| {
                if same_source
                    && same_source_sequence.replace(Some(publication.sequence))
                        == Some(publication.sequence)
                {
                    return true;
                }
                let Some(right) = right_weak.upgrade() else {
                    return false;
                };
                let right_value = right.read_with(cx, |cell, _| Rc::clone(&cell.value));
                publish_weak(
                    &target,
                    combine(
                        publication.value.as_ref().clone(),
                        right_value.as_ref().clone(),
                    ),
                    cx,
                )
            }
        });
        let right_subscription = listen(&right, cx, {
            let combine = Rc::clone(&combine);
            move |publication, cx| {
                if same_source
                    && same_source_sequence.replace(Some(publication.sequence))
                        == Some(publication.sequence)
                {
                    return true;
                }
                let Some(left) = left_weak.upgrade() else {
                    return false;
                };
                let left_value = left.read_with(cx, |cell, _| Rc::clone(&cell.value));
                publish_weak(
                    &target,
                    combine(
                        left_value.as_ref().clone(),
                        publication.value.as_ref().clone(),
                    ),
                    cx,
                )
            }
        });
        Reactive {
            entity,
            lifetime: Some(Rc::new(ReactiveLifetime {
                _subscriptions: vec![left_subscription, right_subscription],
                _dependencies: vec![Box::new(self.clone()), Box::new(other.clone())],
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    use gpui::{AppContext, TestAppContext};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn state_starts_ready_clones_share_and_publication_notifies_even_when_equal() {
        let mut cx = TestAppContext::single();
        let (state, reactive) = cx.update(|cx| {
            let state = State::new(7, cx);
            let reactive = state.reactive();
            (state, reactive)
        });
        let clone = state.clone();
        let notifications = Rc::new(Cell::new(0));
        let observer = cx.update(|cx| cx.new(|_| Rc::clone(&notifications)));
        let observed = reactive.clone();
        let _subscription = observer.update(&mut cx, |_, owner_cx| {
            observed.observe(owner_cx, |notifications, _, _| {
                notifications.set(notifications.get() + 1);
            })
        });

        assert_eq!(reactive.get(&cx), 7);
        cx.update(|cx| {
            clone.set(7, cx);
            clone.set(9, cx);
        });
        assert_eq!(reactive.get(&cx), 9);
        assert_eq!(notifications.get(), 2);
    }

    #[test]
    fn guarded_publication_rejects_an_inactive_token() {
        let cx = TestAppContext::single();
        let (state, reactive) = cx.update(|cx| {
            let state = State::new(0, cx);
            (state.clone(), state.reactive())
        });
        let alive = Rc::new(Cell::new(false));

        cx.update(|cx| state.set_guarded(1, Rc::downgrade(&alive), cx));

        assert_eq!(reactive.get(&cx), 0);
    }

    #[test]
    fn invalidated_queued_guarded_publication_is_skipped_and_queue_drains() {
        let cx = TestAppContext::single();
        let (state, values, first_owner, later_owner, _first, _later) = cx.update(|cx| {
            let state = State::new(0, cx);
            let reactive = state.reactive();
            let alive = Rc::new(Cell::new(true));
            let first_owner = cx.new(|_| ());
            let queued_state = state.clone();
            let queued_alive = Rc::downgrade(&alive);
            let first = first_owner.update(cx, |_, owner_cx| {
                reactive.observe(owner_cx, move |_, value, owner_cx| {
                    if *value == 1 {
                        queued_state.set_guarded(2, queued_alive.clone(), owner_cx);
                        alive.set(false);
                        queued_state.set(3, owner_cx);
                    }
                })
            });
            let later_owner = cx.new(|_| ());
            let values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observed_values = Rc::clone(&values);
            let later = later_owner.update(cx, |_, owner_cx| {
                reactive.observe(owner_cx, move |_, value, _| {
                    observed_values.borrow_mut().push(*value);
                })
            });
            (state, values, first_owner, later_owner, first, later)
        });

        cx.update(|cx| state.set(1, cx));

        assert_eq!(state.reactive().get(&cx), 3);
        assert_eq!(*values.borrow(), [1, 3]);
        drop((first_owner, later_owner));
    }

    #[test]
    fn map_and_combine_propagate_latest_values_synchronously() {
        let cx = TestAppContext::single();
        let (left, right, combined, notifications, _subscription) = cx.update(|cx| {
            let left = State::new(2, cx);
            let right = State::new(5, cx);
            let mapped = left.reactive().map(cx, |value| value * 10);
            let combined = mapped.combine(&right.reactive(), cx, |left, right| left + right);
            let notifications = Rc::new(Cell::new(0));
            let observed = Rc::clone(&notifications);
            let subscription = cx.observe(&combined.entity, move |_, _| {
                observed.set(observed.get() + 1);
            });
            (left, right, combined, notifications, subscription)
        });

        assert_eq!(combined.get(&cx), 25);
        cx.update(|cx| left.set(3, cx));
        assert_eq!(combined.get(&cx), 35);
        cx.update(|cx| right.set(8, cx));
        assert_eq!(combined.get(&cx), 38);
        assert_eq!(notifications.get(), 2);
    }

    #[test]
    fn each_publication_in_one_app_update_reaches_map_and_combined_observer() {
        let cx = TestAppContext::single();
        let (
            state,
            combined,
            map_inputs,
            observed_values,
            observed_snapshots,
            _observer,
            _subscription,
        ) = cx.update(|cx| {
            let state = State::new(0, cx);
            let map_inputs = Rc::new(std::cell::RefCell::new(Vec::new()));
            let inputs = Rc::clone(&map_inputs);
            let mapped = state.reactive().map(cx, move |value| {
                inputs.borrow_mut().push(value);
                value * 10
            });
            let fixed = State::new(5, cx);
            let combined = mapped.combine(&fixed.reactive(), cx, |value, fixed| value + fixed);
            let observed_values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observed_snapshots = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer = cx.new(|_| ());
            let values = Rc::clone(&observed_values);
            let snapshots = Rc::clone(&observed_snapshots);
            let observed = combined.clone();
            let callback_observed = observed.clone();
            let subscription = observer.update(cx, |_, owner_cx| {
                observed.observe(owner_cx, move |_, value, owner_cx| {
                    values.borrow_mut().push(*value);
                    snapshots.borrow_mut().push(callback_observed.get(owner_cx));
                })
            });
            (
                state,
                combined,
                map_inputs,
                observed_values,
                observed_snapshots,
                observer,
                subscription,
            )
        });

        cx.update(|cx| {
            state.set(1, cx);
            state.set(1, cx);
            state.set(2, cx);
        });

        assert_eq!(combined.get(&cx), 25);
        assert_eq!(*map_inputs.borrow(), [0, 1, 1, 2]);
        assert_eq!(*observed_values.borrow(), [15, 15, 25]);
        assert_eq!(*observed_snapshots.borrow(), [15, 15, 25]);
    }

    #[test]
    fn reentrant_publication_preserves_order_for_later_listeners_and_derived_values() {
        let cx = TestAppContext::single();
        let (
            source,
            mapped,
            map_inputs,
            reentrant_snapshots,
            observer_values,
            observer_snapshots,
            _first_owner,
            _later_owner,
            _first_subscription,
            _later_subscription,
        ) = cx.update(|cx| {
            let source = State::new(0, cx);
            let source_reactive = source.reactive();
            let first_owner = cx.new(|_| ());
            let reentrant_source = source.clone();
            let reentrant_snapshots = Rc::new(std::cell::RefCell::new(Vec::new()));
            let snapshots = Rc::clone(&reentrant_snapshots);
            let observed_source = source_reactive.clone();
            let first_subscription = first_owner.update(cx, |_, owner_cx| {
                source_reactive.observe(owner_cx, move |_, value, owner_cx| {
                    if *value == 1 {
                        reentrant_source.set(2, owner_cx);
                        snapshots.borrow_mut().push(observed_source.get(owner_cx));
                    }
                })
            });

            let map_inputs = Rc::new(std::cell::RefCell::new(Vec::new()));
            let inputs = Rc::clone(&map_inputs);
            let mapped = source_reactive.map(cx, move |value| {
                inputs.borrow_mut().push(value);
                value * 10
            });

            let later_owner = cx.new(|_| ());
            let observer_values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer_snapshots = Rc::new(std::cell::RefCell::new(Vec::new()));
            let values = Rc::clone(&observer_values);
            let snapshots = Rc::clone(&observer_snapshots);
            let observed_mapped = mapped.clone();
            let later_subscription = later_owner.update(cx, |_, owner_cx| {
                source_reactive.observe(owner_cx, move |_, value, owner_cx| {
                    values.borrow_mut().push(*value);
                    snapshots.borrow_mut().push(observed_mapped.get(owner_cx));
                })
            });

            (
                source,
                mapped,
                map_inputs,
                reentrant_snapshots,
                observer_values,
                observer_snapshots,
                first_owner,
                later_owner,
                first_subscription,
                later_subscription,
            )
        });

        cx.update(|cx| source.set(1, cx));

        let final_source = source.reactive().get(&cx);
        let final_mapped = mapped.get(&cx);
        assert_eq!(final_source, 2);
        assert_eq!(final_mapped, 20);
        assert_eq!(*map_inputs.borrow(), [0, 1, 2]);
        assert_eq!(*reentrant_snapshots.borrow(), [1]);
        assert_eq!(*observer_values.borrow(), [1, 2]);
        assert_eq!(*observer_snapshots.borrow(), [10, 20]);
    }

    #[test]
    fn reentrant_publications_are_fifo_and_preserve_equal_values() {
        let cx = TestAppContext::single();
        let (
            source,
            mapped,
            map_inputs,
            reentrant_snapshots,
            observer_values,
            observer_snapshots,
            _first_owner,
            _later_owner,
            _first_subscription,
            _later_subscription,
        ) = cx.update(|cx| {
            let source = State::new(0, cx);
            let source_reactive = source.reactive();
            let first_owner = cx.new(|_| ());
            let reentrant_source = source.clone();
            let queued_equal = Rc::new(Cell::new(false));
            let reentrant_snapshots = Rc::new(std::cell::RefCell::new(Vec::new()));
            let snapshots = Rc::clone(&reentrant_snapshots);
            let queued = Rc::clone(&queued_equal);
            let observed_source = source_reactive.clone();
            let first_subscription = first_owner.update(cx, |_, owner_cx| {
                source_reactive.observe(owner_cx, move |_, value, owner_cx| match *value {
                    1 => {
                        reentrant_source.set(2, owner_cx);
                        snapshots.borrow_mut().push(observed_source.get(owner_cx));
                        reentrant_source.set(3, owner_cx);
                        snapshots.borrow_mut().push(observed_source.get(owner_cx));
                    }
                    2 if !queued.replace(true) => {
                        reentrant_source.set(2, owner_cx);
                        snapshots.borrow_mut().push(observed_source.get(owner_cx));
                    }
                    _ => {}
                })
            });

            let map_inputs = Rc::new(std::cell::RefCell::new(Vec::new()));
            let inputs = Rc::clone(&map_inputs);
            let mapped = source_reactive.map(cx, move |value| {
                inputs.borrow_mut().push(value);
                value * 10
            });
            let later_owner = cx.new(|_| ());
            let observer_values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer_snapshots = Rc::new(std::cell::RefCell::new(Vec::new()));
            let values = Rc::clone(&observer_values);
            let snapshots = Rc::clone(&observer_snapshots);
            let observed_mapped = mapped.clone();
            let later_subscription = later_owner.update(cx, |_, owner_cx| {
                source_reactive.observe(owner_cx, move |_, value, owner_cx| {
                    values.borrow_mut().push(*value);
                    snapshots.borrow_mut().push(observed_mapped.get(owner_cx));
                })
            });

            (
                source,
                mapped,
                map_inputs,
                reentrant_snapshots,
                observer_values,
                observer_snapshots,
                first_owner,
                later_owner,
                first_subscription,
                later_subscription,
            )
        });

        cx.update(|cx| source.set(1, cx));

        assert_eq!(source.reactive().get(&cx), 2);
        assert_eq!(mapped.get(&cx), 20);
        assert_eq!(*map_inputs.borrow(), [0, 1, 2, 3, 2]);
        assert_eq!(*reentrant_snapshots.borrow(), [1, 1, 2]);
        assert_eq!(*observer_values.borrow(), [1, 2, 3, 2]);
        assert_eq!(*observer_snapshots.borrow(), [10, 20, 30, 20]);
    }

    #[test]
    fn combine_publishes_each_input_in_order_and_same_input_only_once() {
        let cx = TestAppContext::single();
        let (left, right, combined, values, _observer, _subscription) = cx.update(|cx| {
            let left = State::new(0, cx);
            let right = State::new(10, cx);
            let combined = left
                .reactive()
                .combine(&right.reactive(), cx, |left, right| (left, right));
            let values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer = cx.new(|_| ());
            let observed = combined.clone();
            let output = Rc::clone(&values);
            let subscription = observer.update(cx, |_, owner_cx| {
                observed.observe(owner_cx, move |_, value, _| {
                    output.borrow_mut().push(*value)
                })
            });
            (left, right, combined, values, observer, subscription)
        });

        assert_eq!(combined.get(&cx), (0, 10));
        cx.update(|cx| {
            left.set(1, cx);
            right.set(20, cx);
            left.set(2, cx);
        });
        assert_eq!(*values.borrow(), [(1, 10), (1, 20), (2, 20)]);
        assert_eq!(combined.get(&cx), (2, 20));

        let (source, same, same_values, _observer, _subscription) = cx.update(|cx| {
            let source = State::new(2, cx);
            let reactive = source.reactive();
            let same = reactive.combine(&reactive, cx, |left, right| left + right);
            let same_values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer = cx.new(|_| ());
            let output = Rc::clone(&same_values);
            let observed = same.clone();
            let subscription = observer.update(cx, |_, owner_cx| {
                observed.observe(owner_cx, move |_, value, _| {
                    output.borrow_mut().push(*value)
                })
            });
            (source, same, same_values, observer, subscription)
        });
        cx.update(|cx| source.set(3, cx));
        assert_eq!(*same_values.borrow(), [6]);
        assert_eq!(same.get(&cx), 6);
    }

    #[test]
    fn mixed_depth_diamond_delivers_each_root_publication_and_finishes_current() {
        let cx = TestAppContext::single();
        let (source, combined, values, _observer, _subscription) = cx.update(|cx| {
            let source = State::new(0, cx);
            let shallow = source.reactive().map(cx, |value| value + 1);
            let deep = source
                .reactive()
                .map(cx, |value| value * 10)
                .map(cx, |value| value + 100);
            let combined = shallow.combine(&deep, cx, |shallow, deep| (shallow, deep));
            let values = Rc::new(std::cell::RefCell::new(Vec::new()));
            let observer = cx.new(|_| ());
            let output = Rc::clone(&values);
            let observed = combined.clone();
            let subscription = observer.update(cx, |_, owner_cx| {
                observed.observe(owner_cx, move |_, value, _| {
                    output.borrow_mut().push(*value)
                })
            });
            (source, combined, values, observer, subscription)
        });

        cx.update(|cx| {
            source.set(1, cx);
            source.set(2, cx);
        });

        assert_eq!(*values.borrow(), [(2, 100), (2, 110), (3, 110), (3, 120)]);
        assert_eq!(combined.get(&cx), (3, 120));
    }

    #[test]
    fn observer_subscription_and_owner_drop_stop_callbacks() {
        let cx = TestAppContext::single();
        let (source, calls, live_owner, dropped_owner, cancelled, owner_bound) = cx.update(|cx| {
            let source = State::new(0, cx);
            let calls = Rc::new(Cell::new(0));
            let reactive = source.reactive();
            let live_owner = cx.new(|_| ());
            let dropped_owner = cx.new(|_| ());
            let count = Rc::clone(&calls);
            let cancelled = live_owner.update(cx, |_, owner_cx| {
                reactive.observe(owner_cx, move |_, _, _| count.set(count.get() + 1))
            });
            let count = Rc::clone(&calls);
            let owner_bound = dropped_owner.update(cx, |_, owner_cx| {
                reactive.observe(owner_cx, move |_, _, _| count.set(count.get() + 1))
            });
            (
                source,
                calls,
                live_owner,
                dropped_owner,
                cancelled,
                owner_bound,
            )
        });

        drop(cancelled);
        drop(dropped_owner);
        cx.update(|cx| source.set(1, cx));
        cx.update(|cx| source.set(2, cx));
        assert_eq!(calls.get(), 0);
        drop(owner_bound);
        drop(live_owner);
    }

    #[test]
    fn derived_subscription_is_released_with_its_entity() {
        let cx = TestAppContext::single();
        let mapped_calls = Rc::new(Cell::new(0));
        let (source, mapped) = cx.update(|cx| {
            let source = State::new(1, cx);
            let calls = Rc::clone(&mapped_calls);
            let mapped = source.reactive().map(cx, move |value| {
                calls.set(calls.get() + 1);
                value + 1
            });
            (source, mapped)
        });

        assert_eq!(mapped.get(&cx), 2);
        assert_eq!(mapped_calls.get(), 1);
        drop(mapped);
        cx.update(|cx| source.set(2, cx));
        assert_eq!(mapped_calls.get(), 1);
    }
}

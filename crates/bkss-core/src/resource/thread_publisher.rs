use super::{Source, SourcePublisher};
use async_channel::{Sender, TrySendError};
use gpui::{App, Task};
use std::{cell::Cell, rc::Rc};

enum Message<P, E> {
    Value(P),
    Failure(E),
}

/// Returned when the UI-side bridge has been closed, retaining the payload.
#[derive(Debug, PartialEq, Eq)]
pub enum ThreadPublishError<T> {
    Closed(T),
}

/// A cloneable, non-blocking sender for publishing from other threads.
///
/// Clones do not retain the source activation. Successful sends only enqueue;
/// they do not guarantee UI delivery.
pub struct ThreadPublisher<P, E> {
    sender: Sender<Message<P, E>>,
}

impl<P, E> Clone for ThreadPublisher<P, E> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<P, E> ThreadPublisher<P, E> {
    /// Enqueues a value payload without blocking.
    pub fn publish(&self, payload: P) -> Result<(), ThreadPublishError<P>> {
        self.sender
            .try_send(Message::Value(payload))
            .map_err(|error| match error {
                TrySendError::Closed(Message::Value(payload)) => {
                    ThreadPublishError::Closed(payload)
                }
                TrySendError::Closed(Message::Failure(_)) => unreachable!(),
                TrySendError::Full(_) => unreachable!("unbounded channel cannot be full"),
            })
    }

    /// Enqueues an error payload without blocking.
    pub fn fail(&self, payload: E) -> Result<(), ThreadPublishError<E>> {
        self.sender
            .try_send(Message::Failure(payload))
            .map_err(|error| match error {
                TrySendError::Closed(Message::Failure(payload)) => {
                    ThreadPublishError::Closed(payload)
                }
                TrySendError::Closed(Message::Value(_)) => unreachable!(),
                TrySendError::Full(_) => unreachable!("unbounded channel cannot be full"),
            })
    }
}

/// Owns and synchronously stops a thread publisher's UI-side delivery task.
pub struct ThreadPublisherGuard {
    closed: Rc<Cell<bool>>,
    close: Option<Box<dyn FnOnce()>>,
    task: Option<Task<()>>,
}

impl Drop for ThreadPublisherGuard {
    fn drop(&mut self) {
        self.closed.set(true);
        if let Some(close) = self.close.take() {
            close();
        }
        drop(self.task.take());
    }
}

impl<S: Source> SourcePublisher<S> {
    /// Bridges `Send` payloads to this UI-local source publisher.
    ///
    /// Mapping callbacks are pure, synchronous UI-thread transformations and
    /// need not be `Send`. The unbounded queue has no memory-use ceiling if
    /// producers outpace UI delivery. FIFO order is preserved within this
    /// bridge, but not across activations or concurrent senders. The returned
    /// guard MUST be retained with the source activation's guard; dropping it
    /// closes the transport and cancels queued UI delivery synchronously.
    pub fn thread_bridge<P, E>(
        &self,
        map_value: impl Fn(P) -> S::Value + 'static,
        map_error: impl Fn(E) -> S::Error + 'static,
        cx: &mut App,
    ) -> (ThreadPublisher<P, E>, ThreadPublisherGuard)
    where
        P: Send + 'static,
        E: Send + 'static,
    {
        let (sender, receiver) = async_channel::unbounded();
        let closed = Rc::new(Cell::new(false));
        let task_closed = Rc::clone(&closed);
        let task_receiver = receiver.clone();
        let publisher = self.clone();
        let task = cx.spawn(async move |cx| {
            while let Ok(message) = task_receiver.recv().await {
                if task_closed.get() {
                    break;
                }
                match message {
                    Message::Value(payload) => {
                        let value = map_value(payload);
                        cx.update(|cx| publisher.publish(value, cx));
                    }
                    Message::Failure(payload) => {
                        let error = map_error(payload);
                        cx.update(|cx| publisher.fail(error, cx));
                    }
                }
            }
        });
        (
            ThreadPublisher { sender },
            ThreadPublisherGuard {
                closed,
                close: Some(Box::new(move || {
                    receiver.close();
                })),
                task: Some(task),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ResourceScope, SourceDescriptor, SourceState};
    use gpui::{AppContext, TestAppContext};
    use std::{
        cell::{Cell, RefCell},
        error::Error,
        fmt,
        rc::Rc,
        thread,
    };

    struct LocalError(Rc<String>);
    impl fmt::Display for LocalError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl fmt::Debug for LocalError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            fmt::Display::fmt(self, f)
        }
    }
    impl Error for LocalError {}

    struct HarnessGuard {
        _bridge: Option<ThreadPublisherGuard>,
        drops: Rc<Cell<usize>>,
    }

    impl Drop for HarnessGuard {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    struct HarnessSource {
        senders: Rc<RefCell<Vec<ThreadPublisher<i32, String>>>>,
        escaped_guards: Option<Rc<RefCell<Vec<ThreadPublisherGuard>>>>,
        guard_drops: Rc<Cell<usize>>,
    }

    impl Source for HarnessSource {
        type Key = ();
        type Value = Rc<Vec<i32>>;
        type Error = LocalError;
        type Guard = HarnessGuard;

        fn key(&self) {}

        fn start(
            &self,
            publisher: SourcePublisher<Self>,
            cx: &mut App,
        ) -> Result<Self::Guard, Self::Error> {
            let (sender, guard) = publisher.thread_bridge(
                |value| Rc::new(vec![value]),
                |message| LocalError(Rc::new(message)),
                cx,
            );
            self.senders.borrow_mut().push(sender);
            let bridge = if let Some(slot) = &self.escaped_guards {
                slot.borrow_mut().push(guard);
                None
            } else {
                Some(guard)
            };
            Ok(HarnessGuard {
                _bridge: bridge,
                drops: Rc::clone(&self.guard_drops),
            })
        }
    }

    fn harness_source(
        senders: Rc<RefCell<Vec<ThreadPublisher<i32, String>>>>,
        escaped_guards: Option<Rc<RefCell<Vec<ThreadPublisherGuard>>>>,
        guard_drops: Rc<Cell<usize>>,
    ) -> SourceDescriptor<HarnessSource> {
        SourceDescriptor::new(HarnessSource {
            senders,
            escaped_guards,
            guard_drops,
        })
    }

    struct LocalSource {
        bridge: Rc<RefCell<Option<ThreadPublisher<i32, String>>>>,
        mapping_thread: Rc<RefCell<Option<thread::ThreadId>>>,
    }
    impl Source for LocalSource {
        type Key = ();
        type Value = Rc<Vec<i32>>;
        type Error = LocalError;
        type Guard = ThreadPublisherGuard;

        fn key(&self) {}

        fn start(
            &self,
            publisher: SourcePublisher<Self>,
            cx: &mut App,
        ) -> Result<Self::Guard, Self::Error> {
            let mapping_thread = Rc::clone(&self.mapping_thread);
            let (sender, guard) = publisher.thread_bridge(
                move |value| {
                    *mapping_thread.borrow_mut() = Some(thread::current().id());
                    Rc::new(vec![value])
                },
                |message| LocalError(Rc::new(message)),
                cx,
            );
            *self.bridge.borrow_mut() = Some(sender);
            Ok(guard)
        }
    }

    #[test]
    fn sends_from_thread_map_on_ui_and_guard_closes_synchronously() {
        let cx = TestAppContext::single();
        let bridge = Rc::new(RefCell::new(None));
        let mapping_thread = Rc::new(RefCell::new(None));
        let scope = cx.update(ResourceScope::new);
        let desc = SourceDescriptor::new(LocalSource {
            bridge: Rc::clone(&bridge),
            mapping_thread: Rc::clone(&mapping_thread),
        });
        let lease = cx.update(|cx| scope.acquire(&desc, cx).unwrap());
        let ui_thread = thread::current().id();
        let sender = bridge.borrow().as_ref().unwrap().clone();
        let worker_sender = sender.clone();
        thread::spawn(move || {
            worker_sender.publish(7).unwrap();
            worker_sender.publish(7).unwrap();
            worker_sender
                .fail("subscription failed".to_owned())
                .unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
        cx.run_until_parked();
        assert_eq!(
            lease.reactive().get(&cx),
            SourceState::Ready(Rc::new(vec![7]))
        );
        assert_eq!(*mapping_thread.borrow(), Some(ui_thread));
        let failure = scope.failure().expect("thread failure should reach source");
        assert!(
            failure
                .source()
                .and_then(|error| error.downcast_ref::<LocalError>())
                .is_some_and(|error| error.0.as_str() == "subscription failed")
        );
        drop(lease);
        assert!(matches!(
            sender.publish(8),
            Err(ThreadPublishError::Closed(8))
        ));
    }

    #[test]
    fn preserves_fifo_and_notifies_for_equal_values_before_failure() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = cx.update(ResourceScope::new);
        let descriptor = harness_source(senders.clone(), None, drops);
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let observed = Rc::new(RefCell::new(Vec::new()));
        let owner = cx.update(|cx| cx.new(|_| ()));
        let reactive = lease.reactive();
        let observed_values = Rc::clone(&observed);
        let subscription = cx.update(|cx| {
            owner.update(cx, |_, cx| {
                reactive.observe(cx, move |_, state, _| {
                    observed_values.borrow_mut().push(state.clone());
                })
            })
        });
        let sender = senders.borrow()[0].clone();
        let worker_sender = sender.clone();
        thread::spawn(move || {
            for value in [1, 2, 2, 3] {
                worker_sender.publish(value).unwrap();
            }
            worker_sender.fail("first".to_owned()).unwrap();
        })
        .join()
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            *observed.borrow(),
            [1, 2, 2, 3].map(|value| SourceState::Ready(Rc::new(vec![value])))
        );
        let failure = scope
            .failure()
            .expect("failure should be applied after values");
        assert!(
            failure
                .source()
                .and_then(|error| error.downcast_ref::<LocalError>())
                .is_some_and(|error| error.0.as_str() == "first")
        );
        drop((subscription, owner, lease));
    }

    #[test]
    fn last_lease_closes_clones_and_discards_queued_messages_before_reacquire() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = cx.update(ResourceScope::new);
        let descriptor = harness_source(senders.clone(), None, drops.clone());
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let old = senders.borrow()[0].clone();
        let old_clone = old.clone();
        old.publish(1).unwrap();
        old.fail("stale".to_owned()).unwrap();
        drop(lease);
        assert_eq!(drops.get(), 1);
        assert!(matches!(
            old_clone.publish(2),
            Err(ThreadPublishError::Closed(2))
        ));
        assert!(
            matches!(old.fail("closed".to_owned()), Err(ThreadPublishError::Closed(value)) if value == "closed")
        );

        let renewed = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let reactive = renewed.reactive();
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        cx.run_until_parked();
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        assert!(scope.failure().is_none());
        drop(renewed);
    }

    #[test]
    fn explicit_shutdown_closes_escaped_senders_without_fatal_failure() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = cx.update(ResourceScope::new);
        let descriptor = harness_source(senders.clone(), None, drops.clone());
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let reactive = lease.reactive();
        let sender = senders.borrow()[0].clone();
        let sender_clone = sender.clone();
        scope.shutdown();
        assert_eq!(drops.get(), 1);
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        assert!(scope.failure().is_none());
        assert!(matches!(
            sender.publish(1),
            Err(ThreadPublishError::Closed(1))
        ));
        assert!(
            matches!(sender_clone.fail("late".to_owned()), Err(ThreadPublishError::Closed(value)) if value == "late")
        );
        drop(lease);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn scope_drop_does_not_retain_scope_through_escaped_lease_or_sender() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = Rc::new(cx.update(ResourceScope::new));
        let weak_scope = Rc::downgrade(&scope);
        let descriptor = harness_source(senders.clone(), None, drops.clone());
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let sender = senders.borrow()[0].clone();
        drop(scope);
        assert!(weak_scope.upgrade().is_none());
        assert_eq!(drops.get(), 1);
        assert!(matches!(
            sender.publish(5),
            Err(ThreadPublishError::Closed(5))
        ));
        assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
        drop(lease);
    }

    #[test]
    fn retained_old_bridge_uses_weak_activation_checks_after_reacquire() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let escaped_guards = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = cx.update(ResourceScope::new);
        let descriptor =
            harness_source(senders.clone(), Some(escaped_guards.clone()), drops.clone());
        let old_lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let old_sender = senders.borrow()[0].clone();
        old_sender.publish(1).unwrap();
        old_sender.fail("old failure".to_owned()).unwrap();
        drop(old_lease);

        let renewed = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let reactive = renewed.reactive();
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        cx.run_until_parked();
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        assert!(scope.failure().is_none());
        assert!(old_sender.publish(2).is_ok());
        cx.run_until_parked();
        assert_eq!(reactive.get(&cx), SourceState::Loading);
        assert!(scope.failure().is_none());
        drop(renewed);
        escaped_guards.borrow_mut().clear();
        assert!(matches!(
            old_sender.publish(3),
            Err(ThreadPublishError::Closed(3))
        ));
    }

    #[test]
    fn reentrant_shutdown_during_first_failure_does_not_repeat_cleanup() {
        let cx = TestAppContext::single();
        let senders = Rc::new(RefCell::new(Vec::new()));
        let drops = Rc::new(Cell::new(0));
        let scope = Rc::new(cx.update(ResourceScope::new));
        let descriptor = harness_source(senders.clone(), None, drops.clone());
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let sender = senders.borrow()[0].clone();
        let failure_state = scope.failure_reactive();
        let owner = cx.update(|cx| cx.new(|_| ()));
        let callback_scope = Rc::clone(&scope);
        let subscription = cx.update(|cx| {
            owner.update(cx, |_, cx| {
                failure_state.observe(cx, move |_, failure, _| {
                    assert!(failure.is_some());
                    callback_scope.shutdown();
                })
            })
        });
        let worker_sender = sender.clone();
        thread::spawn(move || {
            worker_sender.fail("first".to_owned()).unwrap();
            worker_sender.fail("second".to_owned()).unwrap();
        })
        .join()
        .unwrap();
        cx.run_until_parked();
        assert_eq!(drops.get(), 1);
        assert!(matches!(
            sender.publish(9),
            Err(ThreadPublishError::Closed(9))
        ));
        let failure = scope.failure().expect("first failure should remain");
        assert!(
            failure
                .source()
                .and_then(|error| error.downcast_ref::<LocalError>())
                .is_some_and(|error| error.0.as_str() == "first")
        );
        drop((subscription, owner, lease));
    }
}

use super::{
    SourceFailure, SourceFailurePhase,
    scope::{Activation, ScopeInner, fail_scope},
};
use crate::reactive::SourceState;
use gpui::App;
use std::{
    any::TypeId,
    cell::RefCell,
    error::Error,
    hash::Hash,
    rc::{Rc, Weak},
};

/// A runtime-owned input that can be started and cancelled.
///
/// `key` must return the normalized identity for this source; descriptors with
/// the same key and concrete source type must have compatible start behavior.
/// If `start` returns an error after partial setup, it must clean that setup up
/// itself. A returned guard's context-free, nonblocking, panic-free `Drop`
/// requests cancellation; asynchronous cancellation completion is not awaited.
pub trait Source: Sized + 'static {
    type Key: Eq + Hash + Clone + 'static;
    type Value: Clone + 'static;
    type Error: Error + 'static;
    type Guard: 'static;

    fn key(&self) -> Self::Key;
    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error>;
}

/// An inert source declaration. Cloning it does not clone or start its source.
/// The normalized key and concrete source type determine identity in a scope.
pub struct SourceDescriptor<S: Source> {
    pub(super) source: Rc<S>,
    pub(super) key: S::Key,
}

impl<S: Source> Clone for SourceDescriptor<S> {
    fn clone(&self) -> Self {
        Self {
            source: Rc::clone(&self.source),
            key: self.key.clone(),
        }
    }
}

impl<S: Source> SourceDescriptor<S> {
    pub fn new(source: S) -> Self {
        let key = source.key();
        Self {
            source: Rc::new(source),
            key,
        }
    }
}

type PublishFn<S> = Rc<dyn Fn(<S as Source>::Value, &mut App)>;
type FailFn<S> = Rc<dyn Fn(<S as Source>::Error, &mut App)>;

/// A cloneable activation publisher. Cloning it does not acquire a consumer.
pub struct SourcePublisher<S: Source> {
    publish: PublishFn<S>,
    fail: FailFn<S>,
}
impl<S: Source> Clone for SourcePublisher<S> {
    fn clone(&self) -> Self {
        Self {
            publish: Rc::clone(&self.publish),
            fail: Rc::clone(&self.fail),
        }
    }
}
impl<S: Source> SourcePublisher<S> {
    pub(super) fn new(inner: Weak<ScopeInner>, activation: Weak<RefCell<Activation<S>>>) -> Self {
        let publish_inner = inner.clone();
        let publish_activation = activation.clone();
        Self {
            publish: Rc::new(move |value, cx| {
                let (Some(inner), Some(activation)) =
                    (publish_inner.upgrade(), publish_activation.upgrade())
                else {
                    return;
                };
                let target = {
                    let a = activation.borrow();
                    (a.alive.get() && !inner.closed.get() && inner.failure.borrow().is_none())
                        .then(|| (a.state.clone(), Rc::clone(&a.alive)))
                };
                if let Some((state, alive)) = target {
                    state.set_guarded(SourceState::Ready(value), Rc::downgrade(&alive), cx);
                }
            }),
            fail: Rc::new(move |error, cx| {
                let (Some(inner), Some(activation)) = (inner.upgrade(), activation.upgrade())
                else {
                    return;
                };
                let alive = activation.borrow().alive.get();
                if alive {
                    fail_scope(
                        inner,
                        Rc::new(SourceFailure {
                            source_type: TypeId::of::<S>(),
                            phase: SourceFailurePhase::Subscription,
                            cause: Box::new(error),
                        }),
                        cx,
                    );
                }
            }),
        }
    }
    pub fn publish(&self, value: S::Value, cx: &mut App) {
        (self.publish)(value, cx);
    }
    /// Reports a fatal subscription failure. Only the first live failure wins.
    pub fn fail(&self, error: S::Error, cx: &mut App) {
        (self.fail)(error, cx);
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use crate::resource::{Source, SourceDescriptor, SourcePublisher};
    use gpui::App;
    use std::{
        cell::{Cell, RefCell},
        error::Error,
        rc::Rc,
    };

    #[derive(Debug)]
    pub(in crate::resource) struct FakeError(pub &'static str);
    impl std::fmt::Display for FakeError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }
    impl Error for FakeError {}

    pub(in crate::resource) struct FakeSource {
        pub(in crate::resource) key: u8,
        pub(in crate::resource) starts: Rc<Cell<usize>>,
        pub(in crate::resource) stops: Rc<Cell<usize>>,
        pub(in crate::resource) publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
        pub(in crate::resource) initial: Option<u8>,
        pub(in crate::resource) fail_start: bool,
        pub(in crate::resource) on_start: Option<FakeStart>,
    }
    pub(in crate::resource) type FakeStart = Rc<dyn Fn(SourcePublisher<FakeSource>, &mut App)>;
    pub(in crate::resource) struct FakeGuard(pub Rc<Cell<usize>>);
    impl Drop for FakeGuard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    impl Source for FakeSource {
        type Key = u8;
        type Value = Option<u8>;
        type Error = FakeError;
        type Guard = FakeGuard;
        fn key(&self) -> u8 {
            self.key / 10
        }
        fn start(
            &self,
            publisher: SourcePublisher<Self>,
            cx: &mut App,
        ) -> Result<FakeGuard, FakeError> {
            self.starts.set(self.starts.get() + 1);
            *self.publisher.borrow_mut() = Some(publisher.clone());
            if let Some(value) = self.initial {
                publisher.publish(Some(value), cx);
            }
            if let Some(on_start) = &self.on_start {
                on_start(publisher.clone(), cx);
            }
            if self.fail_start {
                return Err(FakeError("start failed"));
            }
            Ok(FakeGuard(Rc::clone(&self.stops)))
        }
    }
    pub(in crate::resource) fn source(
        key: u8,
        starts: Rc<Cell<usize>>,
        stops: Rc<Cell<usize>>,
        publisher: Rc<RefCell<Option<SourcePublisher<FakeSource>>>>,
    ) -> SourceDescriptor<FakeSource> {
        SourceDescriptor::new(FakeSource {
            key,
            starts,
            stops,
            publisher,
            initial: None,
            fail_start: false,
            on_start: None,
        })
    }

    pub(in crate::resource) struct OtherSource {
        pub(in crate::resource) key: u8,
        pub(in crate::resource) starts: Rc<Cell<usize>>,
        pub(in crate::resource) stops: Rc<Cell<usize>>,
        pub(in crate::resource) on_drop: Option<Rc<dyn Fn()>>,
    }
    pub(in crate::resource) struct OtherGuard(Rc<Cell<usize>>, Option<Rc<dyn Fn()>>);
    impl Drop for OtherGuard {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
            if let Some(on_drop) = &self.1 {
                on_drop();
            }
        }
    }
    impl Source for OtherSource {
        type Key = u8;
        type Value = u8;
        type Error = FakeError;
        type Guard = OtherGuard;
        fn key(&self) -> u8 {
            self.key
        }
        fn start(&self, _: SourcePublisher<Self>, _: &mut App) -> Result<Self::Guard, FakeError> {
            self.starts.set(self.starts.get() + 1);
            Ok(OtherGuard(Rc::clone(&self.stops), self.on_drop.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::reactive::SourceState;
    use crate::resource::ResourceScope;
    use gpui::TestAppContext;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn descriptor_clones_are_inert_and_first_descriptor_controls_each_activation() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let first_pub = Rc::new(RefCell::new(None));
        let alias_pub = Rc::new(RefCell::new(None));
        let first = SourceDescriptor::new(FakeSource {
            key: 11,
            starts: starts.clone(),
            stops: stops.clone(),
            publisher: first_pub,
            initial: Some(3),
            fail_start: false,
            on_start: None,
        });
        let clone = first.clone();
        let alias = SourceDescriptor::new(FakeSource {
            key: 19,
            starts: starts.clone(),
            stops: stops.clone(),
            publisher: alias_pub,
            initial: Some(8),
            fail_start: false,
            on_start: None,
        });
        let scope = cx.update(ResourceScope::new);
        assert_eq!(starts.get(), 0);
        let lease = cx.update(|cx| scope.acquire(&clone, cx).unwrap());
        assert_eq!(lease.reactive().get(&cx), SourceState::Ready(Some(3)));
        let shared = cx.update(|cx| scope.acquire(&alias, cx).unwrap());
        assert_eq!(starts.get(), 1);
        drop((lease, shared));
        let next = cx.update(|cx| scope.acquire(&alias, cx).unwrap());
        assert_eq!(next.reactive().get(&cx), SourceState::Ready(Some(8)));
        assert_eq!(starts.get(), 2);
    }
}

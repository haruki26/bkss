use crate::{ResourceLease, SourceState, reactive::Reactive};

/// A non-clone mounted consumer. Dropping it releases its resource lease.
#[must_use = "keep the mounted binding alive for as long as its resource is mounted"]
pub struct MountedBinding<T: Clone + 'static> {
    lease: ResourceLease<T>,
}

impl<T: Clone + 'static> MountedBinding<T> {
    pub(super) fn new(lease: ResourceLease<T>) -> Self {
        Self { lease }
    }

    /// Returns a reactive view that does not itself keep the resource mounted.
    pub fn reactive(&self) -> Reactive<SourceState<T>> {
        self.lease.reactive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Shell;
    use super::super::test_support::*;
    use crate::SourceState;
    use gpui::TestAppContext;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[test]
    fn attach_mounts_share_consumer_leases_and_quit_cleans_runtime() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (descriptor, _) = descriptor(starts.clone(), stops.clone(), false);
        let binding = Rc::new(RefCell::new(None));
        let held_binding = Rc::clone(&binding);
        let runtime = cx.update(|cx| {
            Shell::attach(cx, |runtime, cx| {
                let first = runtime.mount(&descriptor, cx)?;
                let second = runtime.mount(&descriptor, cx)?;
                assert_eq!(starts.get(), 1);
                drop(first);
                assert_eq!(stops.get(), 0);
                let reactive = second.reactive();
                assert_eq!(reactive.get(cx), SourceState::Loading);
                *held_binding.borrow_mut() = Some(second);
                Ok(())
            })
            .unwrap()
        });

        assert!(runtime.result().is_none());
        cx.update(|cx| runtime.shutdown(cx));
        assert_eq!(stops.get(), 1);
        assert!(runtime.result().is_none());
        cx.quit();
        assert!(matches!(runtime.result(), Some(Ok(()))));
        assert_eq!(stops.get(), 1);
        drop(binding);
        assert_eq!(stops.get(), 1);
    }

    #[test]
    fn retained_reactive_does_not_mount_and_old_publisher_cannot_affect_reacquire() {
        let cx = TestAppContext::single();
        let starts = Rc::new(Cell::new(0));
        let stops = Rc::new(Cell::new(0));
        let (descriptor, publishers) = descriptor(starts.clone(), stops.clone(), false);
        let (runtime, old_reactive, old_publisher, _renewed) = cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            let binding = runtime.mount(&descriptor, cx).unwrap();
            let reactive = binding.reactive().map(cx, |state| state);
            let old_publisher = publishers.borrow().as_ref().unwrap().clone();
            drop(binding);
            assert_eq!(stops.get(), 1);
            let renewed = runtime.mount(&descriptor, cx).unwrap();
            assert_eq!(renewed.reactive().get(cx), SourceState::Loading);
            (runtime, reactive, old_publisher, renewed)
        });

        cx.update(|cx| {
            old_publisher.publish(8, cx);
            old_publisher.fail(TestError("stale"), cx);
        });
        assert_eq!(starts.get(), 2);
        assert_eq!(stops.get(), 1);
        assert_eq!(old_reactive.get(&cx), SourceState::Loading);
        assert!(runtime.result().is_none());
        cx.quit();
        assert!(matches!(runtime.result(), Some(Ok(()))));
        assert_eq!(stops.get(), 2);
    }
}

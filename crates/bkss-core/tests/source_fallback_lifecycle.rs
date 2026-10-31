use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
};

use bkss_core::{
    MountedBinding, RuntimeHandle, Shell, Source, SourceDescriptor, SourcePublisher, SourceState,
    State,
};
use gpui::{App, AppContext, TestAppContext};

struct DelayedSource {
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
    publishers: Rc<RefCell<Vec<SourcePublisher<Self>>>>,
}

struct StopCount(Rc<Cell<usize>>);

impl Drop for StopCount {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Source for DelayedSource {
    type Key = ();
    type Value = Option<Vec<i32>>;
    type Error = io::Error;
    type Guard = StopCount;

    fn key(&self) {}

    fn start(&self, publisher: SourcePublisher<Self>, _: &mut App) -> io::Result<Self::Guard> {
        self.starts.set(self.starts.get() + 1);
        self.publishers.borrow_mut().push(publisher);
        Ok(StopCount(Rc::clone(&self.stops)))
    }
}

fn mount(
    cx: &mut App,
    runtime: &RuntimeHandle,
    descriptor: &SourceDescriptor<DelayedSource>,
) -> MountedBinding<Option<Vec<i32>>> {
    runtime.mount(descriptor, cx).unwrap()
}

#[test]
fn delayed_source_fallback_shares_activation_and_reacquires_without_stale_updates() {
    let cx = TestAppContext::single();
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let publishers = Rc::new(RefCell::new(Vec::new()));
    let descriptor = SourceDescriptor::new(DelayedSource {
        starts: Rc::clone(&starts),
        stops: Rc::clone(&stops),
        publishers: Rc::clone(&publishers),
    });

    let (runtime, first, second, reactive, mapped, combined, fixed, owner, changes, subscription) =
        cx.update(|cx| {
            let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
            let first = mount(cx, &runtime, &descriptor);
            let second = mount(cx, &runtime, &descriptor);
            let reactive = first.reactive();
            let mapped = reactive.map_ready(cx, |value| value.map(|items| items.len()));
            let fixed = State::new(SourceState::Ready(10), cx);
            let combined =
                mapped.combine_ready(&fixed.reactive(), cx, |count, value| (count, value));
            let changes = Rc::new(RefCell::new(Vec::new()));
            let owner = cx.new(|_| ());
            let output = Rc::clone(&changes);
            let observed = combined.clone();
            let subscription = owner.update(cx, |_, owner_cx| {
                observed.observe(owner_cx, move |_, state, _| {
                    output.borrow_mut().push(state.clone())
                })
            });
            (
                runtime,
                first,
                second,
                reactive,
                mapped,
                combined,
                fixed,
                owner,
                changes,
                subscription,
            )
        });

    assert_eq!(reactive.get(&cx), SourceState::Loading);
    assert_eq!(mapped.get(&cx), SourceState::Loading);
    assert_eq!(combined.get(&cx), SourceState::Loading);
    assert_eq!(starts.get(), 1);
    assert_eq!(stops.get(), 0);

    let old_publisher = publishers.borrow()[0].clone();
    cx.update(|cx| old_publisher.publish(None, cx));
    assert_eq!(reactive.get(&cx), SourceState::Ready(None));
    assert_eq!(mapped.get(&cx), SourceState::Ready(None));
    assert_eq!(combined.get(&cx), SourceState::Ready((None, 10)));
    assert_eq!(*changes.borrow(), [SourceState::Ready((None, 10))]);

    cx.update(|cx| old_publisher.publish(Some(Vec::new()), cx));
    assert_eq!(mapped.get(&cx), SourceState::Ready(Some(0)));
    assert_eq!(combined.get(&cx), SourceState::Ready((Some(0), 10)));
    assert_eq!(changes.borrow().len(), 2);

    drop(first);
    assert_eq!(stops.get(), 0);
    drop(second);
    assert_eq!(stops.get(), 1);

    let next = cx.update(|cx| mount(cx, &runtime, &descriptor));
    assert_eq!(starts.get(), 2);
    assert_eq!(next.reactive().get(&cx), SourceState::Loading);
    cx.update(|cx| old_publisher.publish(Some(vec![1, 2]), cx));
    assert_eq!(next.reactive().get(&cx), SourceState::Loading);

    let new_publisher = publishers.borrow()[1].clone();
    cx.update(|cx| new_publisher.publish(Some(Vec::new()), cx));
    assert_eq!(
        next.reactive().get(&cx),
        SourceState::Ready(Some(Vec::new()))
    );
    drop(next);
    assert_eq!(stops.get(), 2);

    // Read-only and derived handles, observers, publishers, and descriptors do not lease resources.
    assert_eq!(reactive.get(&cx), SourceState::Ready(Some(Vec::new())));
    assert_eq!(starts.get(), 2);
    drop((
        mapped,
        combined,
        fixed,
        owner,
        subscription,
        old_publisher,
        new_publisher,
        descriptor,
    ));
}

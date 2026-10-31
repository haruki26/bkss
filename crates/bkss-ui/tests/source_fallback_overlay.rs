use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
};

use bkss_core::{
    MountedBinding, Shell, ShellError, Source, SourceDescriptor, SourcePublisher, SourceState,
};
use bkss_ui::{KeyboardFocus, OverlayDefinition, OverlayRuntimeHandleExt};
use gpui::{App, AppContext, Context, Render, TestAppContext, Window, div, prelude::*, px};

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

struct LeaseContent {
    _binding: MountedBinding<Option<Vec<i32>>>,
}

impl Render for LeaseContent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(100.)).h(px(40.)).child("delayed resource")
    }
}

#[test]
fn overlay_consumers_share_and_release_delayed_source_activations() {
    let cx = TestAppContext::single();
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let publishers = Rc::new(RefCell::new(Vec::new()));
    let descriptor = SourceDescriptor::new(DelayedSource {
        starts: Rc::clone(&starts),
        stops: Rc::clone(&stops),
        publishers: Rc::clone(&publishers),
    });
    let mounted_values = Rc::new(RefCell::new(Vec::new()));

    let (runtime, handles, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let handles = (0..2)
            .map(|_| {
                let weak = runtime.downgrade();
                let descriptor = descriptor.clone();
                let values = Rc::clone(&mounted_values);
                runtime
                    .mount_overlay(
                        OverlayDefinition::new(KeyboardFocus::None),
                        move |_, _, cx| {
                            let runtime = weak.upgrade().ok_or_else(|| {
                                ShellError::setup(io::Error::other("runtime gone"))
                            })?;
                            let binding = runtime.mount(&descriptor, cx)?;
                            values.borrow_mut().push(binding.reactive());
                            Ok(cx.new(|_| LeaseContent { _binding: binding }))
                        },
                        cx,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        (runtime, handles, cx.displays()[0].id())
    });

    assert_eq!(starts.get(), 0);
    assert!(mounted_values.borrow().is_empty());
    cx.update(|cx| {
        handles[0].open(display, cx).unwrap();
        handles[1].open(display, cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(mounted_values.borrow().len(), 2);
    assert!(
        mounted_values
            .borrow()
            .iter()
            .all(|reactive| reactive.get(&cx) == SourceState::Loading)
    );
    assert_eq!(starts.get(), 1);

    let old_publisher = publishers.borrow()[0].clone();
    cx.update(|cx| old_publisher.publish(None, cx));
    assert!(
        mounted_values
            .borrow()
            .iter()
            .all(|reactive| reactive.get(&cx) == SourceState::Ready(None))
    );

    cx.update(|cx| handles[0].close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(stops.get(), 0);
    cx.update(|cx| handles[1].close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(stops.get(), 1);

    cx.update(|cx| handles[0].open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(starts.get(), 2);
    assert_eq!(mounted_values.borrow().len(), 3);
    let remounted = mounted_values.borrow()[2].clone();
    assert_eq!(remounted.get(&cx), SourceState::Loading);
    cx.update(|cx| old_publisher.publish(Some(vec![1]), cx));
    assert_eq!(remounted.get(&cx), SourceState::Loading);

    let current_publisher = publishers.borrow()[1].clone();
    cx.update(|cx| current_publisher.publish(Some(Vec::new()), cx));
    assert_eq!(remounted.get(&cx), SourceState::Ready(Some(Vec::new())));

    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(stops.get(), 2);
}

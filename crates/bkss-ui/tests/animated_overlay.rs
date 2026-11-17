use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
    time::Duration,
};

use bkss_core::{
    MountedBinding, RuntimeHandle, Shell, ShellError, Source, SourceDescriptor, SourcePublisher,
};
use bkss_ui::{Overlay, SurfaceHandle, Transition};
use gpui::{
    AnyWindowHandle, App, AppContext, Context, DisplayId, Render, TestAppContext, Window,
    WindowOptions, div, prelude::*, px,
};

#[derive(Default)]
struct Counts {
    starts: Cell<usize>,
    stops: Cell<usize>,
    mounts: Cell<usize>,
    drops: Cell<usize>,
}

struct Sample {
    counts: Rc<Counts>,
    publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
}
struct Guard(Rc<Counts>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.stops.set(self.0.stops.get() + 1);
    }
}
impl Source for Sample {
    type Key = ();
    type Value = ();
    type Error = io::Error;
    type Guard = Guard;
    fn key(&self) {}
    fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<Guard> {
        self.counts.starts.set(self.counts.starts.get() + 1);
        publisher.publish((), cx);
        *self.publisher.borrow_mut() = Some(publisher);
        Ok(Guard(self.counts.clone()))
    }
}
struct Content {
    _binding: MountedBinding<()>,
    counts: Rc<Counts>,
}
impl Drop for Content {
    fn drop(&mut self) {
        self.counts.drops.set(self.counts.drops.get() + 1);
    }
}
impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(160.))
            .h(px(80.))
            .bg(gpui::rgb(0xffffff))
            .child("Opacity content")
    }
}
struct Mounted {
    runtime: RuntimeHandle,
    handle: SurfaceHandle,
    display: DisplayId,
    window: AnyWindowHandle,
    counts: Rc<Counts>,
    publisher: Rc<RefCell<Option<SourcePublisher<Sample>>>>,
}
fn mount(cx: &TestAppContext, animation: Option<Transition>, enter_only: bool) -> Mounted {
    let counts = Rc::new(Counts::default());
    let publisher = Rc::new(RefCell::new(None));
    let source = SourceDescriptor::new(Sample {
        counts: counts.clone(),
        publisher: publisher.clone(),
    });
    let (runtime, handle, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let definition = if let Some(animation) = animation {
            if enter_only {
                Overlay::new().enter_transition(animation)
            } else {
                Overlay::new().transition(animation)
            }
        } else {
            Overlay::new()
        };
        let captured = counts.clone();
        let handle = definition
            .mount(
                &runtime,
                move |_, weak, _, cx| {
                    captured.mounts.set(captured.mounts.get() + 1);
                    let runtime = weak
                        .upgrade()
                        .ok_or_else(|| ShellError::setup(io::Error::other("runtime stopped")))?;
                    let binding = runtime.mount(&source, cx)?;
                    Ok(cx.new(|_| Content {
                        _binding: binding,
                        counts: captured.clone(),
                    }))
                },
                cx,
            )
            .unwrap();
        assert!(cx.windows().is_empty());
        assert_eq!(counts.starts.get(), 0);
        let display = cx.displays()[0].id();
        handle.show(display, cx).unwrap();
        (runtime, handle, display)
    });
    cx.run_until_parked();
    let window = cx.update(|cx| cx.windows()[0]);
    cx.simulate_window_resize(window, gpui::size(px(400.), px(300.)));
    cx.run_until_parked();
    Mounted {
        runtime,
        handle,
        display,
        window,
        counts,
        publisher,
    }
}
fn tick(cx: &TestAppContext, window: AnyWindowHandle, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    if cx.update(|cx| cx.windows().contains(&window)) {
        cx.update(|cx| {
            window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap()
        });
    }
    cx.run_until_parked();
}

#[test]
fn exit_retains_content_and_lease_and_reopen_retargets_the_same_mount() {
    let cx = TestAppContext::single();
    let mounted = mount(
        &cx,
        Some(Transition::fade().duration(Duration::from_secs(1))),
        false,
    );
    tick(&cx, mounted.window, 1000);
    cx.update(|cx| {
        mounted.handle.hide(mounted.display, cx).unwrap();
        mounted.handle.hide(mounted.display, cx).unwrap();
    });
    tick(&cx, mounted.window, 400);
    assert_eq!(mounted.counts.drops.get(), 0);
    assert_eq!(mounted.counts.stops.get(), 0);
    cx.update(|cx| {
        mounted.handle.show(mounted.display, cx).unwrap();
        mounted.handle.show(mounted.display, cx).unwrap();
    });
    tick(&cx, mounted.window, 700);
    assert_eq!(
        cx.update(|cx| cx.windows()),
        vec![mounted.window],
        "old exit must not remove the reopened window"
    );
    assert_eq!(mounted.counts.mounts.get(), 1);
    assert_eq!(mounted.counts.starts.get(), 1);
    tick(&cx, mounted.window, 300);
    cx.update(|cx| mounted.handle.hide(mounted.display, cx).unwrap());
    tick(&cx, mounted.window, 1000);
    assert!(cx.update(|cx| cx.windows().is_empty()));
    assert_eq!(mounted.counts.drops.get(), 1);
    assert_eq!(mounted.counts.stops.get(), 1);
    cx.update(|cx| mounted.handle.show(mounted.display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(mounted.counts.mounts.get(), 2);
    assert_eq!(mounted.counts.starts.get(), 2);
    cx.update(|cx| mounted.runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(mounted.counts.drops.get(), 2);
    assert_eq!(mounted.counts.stops.get(), 2);
}

#[test]
fn shutdown_and_fatal_cancel_exit_without_waiting_and_preserve_the_host() {
    for fatal in [false, true] {
        let cx = TestAppContext::single();
        let mounted = mount(
            &cx,
            Some(Transition::fade().duration(Duration::from_secs(1))),
            false,
        );
        tick(&cx, mounted.window, 1000);
        let host = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| gpui::EmptyView)
            })
            .unwrap()
        });
        cx.update(|cx| {
            mounted.handle.hide(mounted.display, cx).unwrap();
            if fatal {
                mounted
                    .publisher
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .fail(io::Error::other("animation test failure"), cx);
            } else {
                mounted.runtime.shutdown(cx);
            }
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.windows()), vec![host.into()]);
        assert_eq!(mounted.counts.drops.get(), 1);
        assert_eq!(mounted.counts.stops.get(), 1);
        if fatal {
            let error = mounted.runtime.result().unwrap().unwrap_err();
            assert!(error.to_string().contains("animation test failure"));
        }
        assert!(
            cx.update(|cx| mounted.handle.show(mounted.display, cx))
                .is_err()
        );
    }
}

#[test]
fn native_window_removal_cancels_exit_and_the_next_open_mounts_fresh_content() {
    let cx = TestAppContext::single();
    let mounted = mount(
        &cx,
        Some(Transition::fade().duration(Duration::from_secs(1))),
        false,
    );
    tick(&cx, mounted.window, 1000);
    cx.update(|cx| {
        mounted.handle.hide(mounted.display, cx).unwrap();
        mounted
            .window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert_eq!(mounted.counts.drops.get(), 1);
    assert_eq!(mounted.counts.stops.get(), 1);
    cx.update(|cx| mounted.handle.show(mounted.display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(mounted.counts.mounts.get(), 2);
    cx.update(|cx| mounted.runtime.shutdown(cx));
}

#[test]
fn missing_exit_is_immediate_and_pending_close_never_mounts_content() {
    for (animation, enter_only) in [
        (None, false),
        (
            Some(Transition::fade().duration(Duration::from_secs(1))),
            true,
        ),
    ] {
        let cx = TestAppContext::single();
        let mounted = mount(&cx, animation, enter_only);
        cx.update(|cx| {
            let missing = DisplayId::new(999);
            mounted.handle.show(missing, cx).unwrap();
            mounted.handle.show(missing, cx).unwrap();
            mounted.handle.hide(missing, cx).unwrap();
            mounted.handle.hide(mounted.display, cx).unwrap();
        });
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.windows().is_empty()));
        assert_eq!(mounted.counts.mounts.get(), 1);
        assert_eq!(mounted.counts.drops.get(), 1);
        assert_eq!(mounted.counts.stops.get(), 1);
        cx.update(|cx| mounted.runtime.shutdown(cx));
    }
}

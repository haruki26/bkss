use std::{cell::Cell, error::Error, io, rc::Rc};

use bkss_core::{MountedBinding, Shell, ShellError, Source, SourceDescriptor, SourcePublisher};
use bkss_ui::{
    KeyboardFocus, OverlayAnchors, OverlayDefinition, OverlayGeometry, OverlayGeometryError,
    OverlayPlacement, OverlayRuntimeHandleExt, PointerDismiss,
};
use gpui::{App, AppContext, Context, Render, TestAppContext, Window, div, prelude::*, px};

struct CounterSource {
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
}
struct CounterGuard(Rc<Cell<usize>>);
impl Drop for CounterGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
impl Source for CounterSource {
    type Key = ();
    type Value = u64;
    type Error = io::Error;
    type Guard = CounterGuard;
    fn key(&self) {}
    fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<CounterGuard> {
        self.starts.set(self.starts.get() + 1);
        publisher.publish(7, cx);
        Ok(CounterGuard(self.stops.clone()))
    }
}

struct LeaseContent {
    binding: MountedBinding<u64>,
}
impl Render for LeaseContent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(100.))
            .h(px(40.))
            .child(format!("{:?}", self.binding.reactive().get(cx)))
    }
}

#[test]
fn public_overlay_api_is_inert_weak_and_mounts_independent_content_with_shared_leases() {
    let definition = OverlayDefinition::new(KeyboardFocus::Interactive)
        .with_pointer_dismiss(PointerDismiss::Backdrop)
        .with_geometry(OverlayGeometry {
            placement: OverlayPlacement::Anchor {
                edges: OverlayAnchors {
                    top: true,
                    right: true,
                    ..Default::default()
                },
                offset: [-12., 16.],
            },
            max_logical_size: Some([500., 300.]),
        })
        .unwrap();
    let cx = TestAppContext::single();
    let starts = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let calls = Rc::new(Cell::new(0));
    let source = SourceDescriptor::new(CounterSource {
        starts: starts.clone(),
        stops: stops.clone(),
    });
    let (runtime, handles, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let weak = runtime.downgrade();
            let source = source.clone();
            let calls = calls.clone();
            handles.push(
                runtime
                    .mount_overlay(
                        definition.clone(),
                        move |_, _, cx| {
                            calls.set(calls.get() + 1);
                            let runtime = weak.upgrade().ok_or_else(|| {
                                ShellError::setup(io::Error::other("runtime gone"))
                            })?;
                            let binding = runtime.mount(&source, cx)?;
                            Ok(cx.new(|_| LeaseContent { binding }))
                        },
                        cx,
                    )
                    .unwrap(),
            );
        }
        assert!(cx.windows().is_empty());
        (runtime, handles, cx.displays()[0].id())
    });
    assert_eq!(calls.get(), 0);
    assert_eq!(starts.get(), 0);
    cx.update(|cx| {
        handles[0].open(display, cx).unwrap();
        handles[0].open(display, cx).unwrap();
        handles[1].open(display, cx).unwrap();
    });
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    assert_eq!(starts.get(), 1);
    // A second target may wait independently without remounting the ready target.
    let missing = gpui::DisplayId::new(999);
    cx.update(|cx| handles[0].open(missing, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    cx.update(|cx| handles[0].close(missing, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(starts.get(), 1);
    assert_eq!(stops.get(), 0);
    cx.update(|cx| handles[0].close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(stops.get(), 0);
    cx.update(|cx| handles[1].close(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(stops.get(), 1);
    cx.update(|cx| handles[0].open(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 3);
    assert_eq!(starts.get(), 2);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(stops.get(), 2);
    assert!(cx.update(|cx| handles[0].open(display, cx)).is_err());
    drop(runtime);
    assert!(cx.update(|cx| handles[0].open(display, cx)).is_err());
    assert_eq!(
        OverlayGeometry {
            max_logical_size: Some([0., 1.]),
            ..Default::default()
        }
        .validate(),
        Err(OverlayGeometryError::InvalidMaximumSize)
    );
}

#[test]
fn content_factory_failure_keeps_the_exact_shell_error_and_cleans_its_surface() {
    let cx = TestAppContext::single();
    let cause: Rc<dyn Error> = Rc::new(io::Error::other("content setup failed"));
    let failure = ShellError::Setup(cause.clone());
    let (runtime, handle, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let handle = runtime
            .mount_overlay(
                OverlayDefinition::new(KeyboardFocus::None),
                move |_, _, _| Err::<gpui::Entity<gpui::EmptyView>, _>(failure.clone()),
                cx,
            )
            .unwrap();
        (runtime, handle, cx.displays()[0].id())
    });
    cx.update(|cx| handle.open(display, cx).unwrap());
    cx.run_until_parked();
    let Some(Err(ShellError::Setup(returned))) = runtime.result() else {
        panic!("factory failure was lost");
    };
    assert!(Rc::ptr_eq(&cause, &returned));
    assert!(cx.update(|cx| cx.windows().is_empty()));
}

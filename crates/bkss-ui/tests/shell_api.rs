use bkss_core::{Shell, ShellError, SurfaceInstanceHandle};
use bkss_ui::{
    Anchor, Bar, DisplaySelection, Glass, KeyboardInteractivity, Layer, Overlay, Placement,
    ShellSurface, Thickness, Transition,
};
use gpui::{
    AppContext, Context, EmptyView, Render, TestAppContext, VisualTestContext, Window,
    WindowOptions, div, point, prelude::*, px, size,
};
use std::{
    cell::{Cell, RefCell},
    error::Error,
    io,
    rc::Rc,
};

#[test]
fn declarations_are_reusable_inert_and_factories_compose_fresh_gpui_content() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let output = cx.displays()[0].id();
        let surface = Overlay::new()
            .output(output)
            .layer(Layer::Overlay)
            .keyboard_interactivity(KeyboardInteractivity::None)
            .placement(Placement::Anchor {
                edges: Anchor::TOP | Anchor::RIGHT,
                offset: point(px(-12.), px(16.)),
            })
            .content_limit(size(px(300.), px(200.)))
            .p_4()
            .w(px(100.));
        assert!(cx.windows().is_empty());
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        for _ in 0..2 {
            let calls = calls.clone();
            surface
                .clone()
                .mount(
                    &runtime,
                    move |_, _, _, cx| {
                        calls.set(calls.get() + 1);
                        Ok(cx.new(|_| EmptyView))
                    },
                    cx,
                )
                .unwrap();
        }
        assert_eq!(calls.get(), 0);
        assert!(cx.windows().is_empty());
        runtime.shutdown(cx);
    });
    // ParentElement belongs to fresh GPUI content, not to reusable declarations.
    drop(div().child(div().child("Content")));
}

#[test]
fn invalid_values_are_rejected_before_a_window_or_factory() {
    for value in [f32::NAN, f32::INFINITY, -1., 0., 0.5, f32::MAX] {
        assert!(Thickness::new(px(value)).is_err());
    }
    let cx = TestAppContext::single();
    cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let factory = |_,
                       _,
                       _: &mut gpui::Window,
                       _: &mut gpui::App|
         -> Result<gpui::Entity<EmptyView>, ShellError> {
            panic!("invalid declarations must not invoke a factory")
        };
        assert!(
            Bar::new(Thickness::new(px(40.)).unwrap())
                .edge(Anchor::TOP | Anchor::LEFT)
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            Bar::new(Thickness::new(px(40.)).unwrap())
                .exclusive_zone(px(f32::NAN))
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            Overlay::new()
                .exclusive_zone(px(-2.))
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            Overlay::new()
                .transition(Transition::slide_up().distance(f32::NAN))
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            Overlay::new()
                .outputs(DisplaySelection::Explicit(vec![]))
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            ShellSurface::new()
                .anchor(Anchor::TOP)
                .mount(&runtime, factory, cx)
                .is_err()
        );
        assert!(
            Overlay::new()
                .placement(Placement::Anchor {
                    edges: Anchor::TOP | Anchor::BOTTOM,
                    offset: point(px(0.), px(0.)),
                })
                .mount(&runtime, factory, cx)
                .is_err()
        );
        for (width, height) in [
            (0., 100.),
            (-1., 100.),
            (f32::NAN, 100.),
            (f32::INFINITY, 100.),
            (100., 0.),
            (100., -1.),
            (100., f32::NAN),
            (100., f32::INFINITY),
        ] {
            assert!(
                Overlay::new()
                    .content_limit(size(px(width), px(height)))
                    .mount(&runtime, factory, cx)
                    .is_err(),
                "content limit {width}x{height} must be rejected"
            );
        }
        for maximum in [0., -1., f32::NAN, f32::INFINITY, f32::MAX] {
            assert!(
                Bar::new(Thickness::new(px(40.)).unwrap())
                    .max_length(px(maximum))
                    .mount(&runtime, factory, cx)
                    .is_err(),
                "bar maximum length {maximum} must be rejected"
            );
        }
        for margin in [-1., f32::NAN, f32::INFINITY, f32::MAX] {
            for margins in [
                gpui::Edges {
                    top: px(margin),
                    ..gpui::Edges::all(px(0.))
                },
                gpui::Edges {
                    right: px(margin),
                    ..gpui::Edges::all(px(0.))
                },
                gpui::Edges {
                    bottom: px(margin),
                    ..gpui::Edges::all(px(0.))
                },
                gpui::Edges {
                    left: px(margin),
                    ..gpui::Edges::all(px(0.))
                },
            ] {
                assert!(
                    Bar::new(Thickness::new(px(40.)).unwrap())
                        .margins(margins)
                        .mount(&runtime, factory, cx)
                        .is_err(),
                    "bar margin {margin} must be rejected"
                );
            }
        }
        for margins in [
            gpui::Edges {
                left: px(1_500_000_000.),
                right: px(1_500_000_000.),
                ..gpui::Edges::all(px(0.))
            },
            gpui::Edges {
                top: px(1_500_000_000.),
                bottom: px(1_500_000_000.),
                ..gpui::Edges::all(px(0.))
            },
            gpui::Edges {
                bottom: px(1_000_000_000.),
                ..gpui::Edges::all(px(0.))
            },
        ] {
            let thickness = if margins.bottom == px(1_000_000_000.) {
                Thickness::new(px(1_500_000_000.)).unwrap()
            } else {
                Thickness::new(px(40.)).unwrap()
            };
            assert!(
                Bar::new(thickness)
                    .margins(margins)
                    .mount(&runtime, factory, cx)
                    .is_err(),
                "overflowing bar margins/allocation must be rejected"
            );
        }
        assert!(cx.windows().is_empty());
        runtime.shutdown(cx);
    });
}

#[test]
fn registration_after_shutdown_rejects_without_invoking_factory() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        runtime.shutdown(cx);
        let factory_calls = calls.clone();
        let result = Overlay::new().mount(
            &runtime,
            move |_, _, _, cx| {
                factory_calls.set(factory_calls.get() + 1);
                Ok(cx.new(|_| EmptyView))
            },
            cx,
        );
        assert!(result.is_err());
        assert_eq!(calls.get(), 0);
        assert!(cx.windows().is_empty());
    });
}

struct TrackedSurface(Rc<Cell<usize>>);

impl Drop for TrackedSurface {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

impl Render for TrackedSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(100.)).h(px(60.))
    }
}

#[test]
fn factory_failure_preserves_exact_error_and_cleans_surfaces_without_closing_host() {
    let cx = TestAppContext::single();
    let host = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyView))
            .unwrap()
    });
    let drops = Rc::new(Cell::new(0));
    let instance = Rc::new(RefCell::new(None::<SurfaceInstanceHandle>));
    let cause: Rc<dyn Error> = Rc::new(io::Error::other("specific content setup cause"));
    let factory_error = ShellError::Setup(cause.clone());
    let (runtime, created, failing, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let tracked_drops = drops.clone();
        let saved_instance = instance.clone();
        let created = Overlay::new()
            .mount(
                &runtime,
                move |surface, _, _, cx| {
                    *saved_instance.borrow_mut() = Some(surface);
                    Ok(cx.new(|_| TrackedSurface(tracked_drops.clone())))
                },
                cx,
            )
            .unwrap();
        let failure = factory_error.clone();
        let failing = Overlay::new()
            .mount(
                &runtime,
                move |_, _, _, _| Err::<gpui::Entity<EmptyView>, _>(failure.clone()),
                cx,
            )
            .unwrap();
        (runtime, created, failing, cx.displays()[0].id())
    });

    cx.update(|cx| created.show(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    assert!(instance.borrow().is_some());

    cx.update(|cx| failing.show(display, cx).unwrap());
    cx.run_until_parked();

    let Some(Err(ShellError::Setup(returned))) = runtime.result() else {
        panic!("factory failure was not retained as the runtime result");
    };
    assert!(Rc::ptr_eq(&cause, &returned));
    assert_eq!(
        returned
            .downcast_ref::<io::Error>()
            .map(io::Error::to_string),
        Some("specific content setup cause".to_owned())
    );
    assert_eq!(
        ShellError::Setup(returned).to_string(),
        "shell setup failed: specific content setup cause"
    );
    assert_eq!(
        drops.get(),
        1,
        "the already-created surface content is dropped"
    );
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert!(host.read_with(&cx, |_, _| ()).is_ok());
    assert!(cx.update(|cx| created.show(display, cx)).is_err());
    cx.update(|cx| {
        instance.borrow().as_ref().unwrap().close(cx).unwrap();
    });
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

#[test]
fn overlay_supports_fixed_native_size_anchors_and_protocol_minus_one_zone() {
    let cx = TestAppContext::single();
    cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let handle = Overlay::new()
            .surface_size(size(px(320.), px(240.)))
            .anchor(Anchor::TOP | Anchor::RIGHT)
            .exclusive_edge(Anchor::TOP)
            .exclusive_zone(px(-1.))
            .mount(&runtime, |_, _, _, cx| Ok(cx.new(|_| EmptyView)), cx)
            .unwrap();
        assert!(cx.windows().is_empty());
        handle.show(cx.displays()[0].id(), cx).unwrap();
        runtime.shutdown(cx);
    });
    cx.run_until_parked();
    assert!(cx.update(|cx| cx.windows().is_empty()));
}

#[test]
fn effects_mount_the_core_reactive_owner_and_compose_fresh_elements() {
    struct Owner {
        glass: bkss_ui::effects::MountedGlass,
    }
    let cx = TestAppContext::single();
    cx.update(|cx| {
        let owner = cx.new(|cx| Owner {
            glass: bkss_ui::effects::MountedGlass::new(Default::default(), cx),
        });
        let glass = &owner.read(cx).glass;
        drop(glass.element(Default::default(), cx).p_4().child("First"));
        drop(glass.element(Default::default(), cx).p_4().child("Second"));
    });
    drop(Glass::new().rounded_xl().p_4().child("Static content"));
    drop(Glass::default().child("Another fresh element"));
}

#[test]
fn fresh_static_glass_reuses_core_decoration_without_fading_its_children() {
    struct Card(Rc<Cell<usize>>);
    impl Render for Card {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            Glass::new()
                .rounded_xl()
                .w(px(100.))
                .h(px(80.))
                .child(div().w(px(20.)).h(px(10.)).bg(gpui::rgb(0xffffff)))
        }
    }
    let cx = TestAppContext::single();
    let renders = Rc::new(Cell::new(0));
    let window = cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|_| Card(renders.clone()))
        })
        .unwrap()
    });
    let mut visual = VisualTestContext::from_window(window.into(), &cx);
    cx.run_until_parked();
    for _ in 0..2 {
        visual.update(|window, _| {
            let quads = window.painted_quads();
            assert_eq!(quads.len(), 2);
            assert!((quads[0].background.as_solid().unwrap().a - 0.15).abs() < 0.001);
            assert_eq!(quads[1].background.as_solid().unwrap().a, 1.);
            window.refresh();
        });
        cx.run_until_parked();
    }
    assert!(renders.get() > 1);
}

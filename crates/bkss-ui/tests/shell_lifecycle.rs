use bkss_core::{Shell, SurfaceInstanceHandle};
use bkss_ui::{Bar, DisplaySelection, Overlay, Thickness, Transition};
use gpui::{
    AppContext, Context, Render, TestAppContext, VisualTestContext, Window, div, prelude::*, px,
};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Content {
    released: Rc<Cell<usize>>,
    renders: Rc<Cell<usize>>,
}

impl Drop for Content {
    fn drop(&mut self) {
        self.released.set(self.released.get() + 1);
    }
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().w(px(100.)).h(px(60.)).bg(gpui::rgb(0xffffff))
    }
}

#[test]
fn exit_retains_content_reversal_reuses_it_and_completed_exit_remounts() {
    let cx = TestAppContext::single();
    let calls = Rc::new(Cell::new(0));
    let released = Rc::new(Cell::new(0));
    let renders = Rc::new(Cell::new(0));
    let instances = Rc::new(std::cell::RefCell::new(Vec::<SurfaceInstanceHandle>::new()));
    let (runtime, handle, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let factory_calls = calls.clone();
        let dropped = released.clone();
        let rendered = renders.clone();
        let captured = instances.clone();
        let handle = Overlay::new()
            .transition(
                Transition::fade()
                    .and(Transition::slide_up())
                    .duration(Duration::from_secs(1)),
            )
            .mount(
                &runtime,
                move |instance, weak, _, cx| {
                    assert!(weak.upgrade().is_some());
                    factory_calls.set(factory_calls.get() + 1);
                    captured.borrow_mut().push(instance);
                    Ok(cx.new(|_| Content {
                        released: dropped.clone(),
                        renders: rendered.clone(),
                    }))
                },
                cx,
            )
            .unwrap();
        (runtime, handle, cx.displays()[0].id())
    });
    cx.update(|cx| handle.show(display, cx).unwrap());
    cx.run_until_parked();
    let window = cx.update(|cx| cx.windows()[0]);
    cx.simulate_window_resize(window, gpui::size(px(400.), px(300.)));
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window, &cx);
    let advance = |visual: &mut VisualTestContext, millis| {
        cx.executor().advance_clock(Duration::from_millis(millis));
        visual.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    };
    advance(&mut visual, 1000);
    cx.update(|cx| handle.hide(display, cx).unwrap());
    advance(&mut visual, 200);
    assert_eq!(released.get(), 0);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update(|cx| handle.show(display, cx).unwrap());
    advance(&mut visual, 1000);
    assert_eq!(calls.get(), 1);
    assert!(renders.get() > 1);
    cx.update(|cx| handle.hide(display, cx).unwrap());
    advance(&mut visual, 1000);
    assert_eq!(released.get(), 1);
    assert!(cx.update(|cx| cx.windows().is_empty()));
    cx.update(|cx| handle.show(display, cx).unwrap());
    cx.run_until_parked();
    assert_eq!(calls.get(), 2);
    cx.update(|cx| instances.borrow()[0].close(cx).unwrap());
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(released.get(), 2);
}

#[test]
fn bars_open_selected_startup_outputs_and_shutdown_releases_their_content() {
    let cx = TestAppContext::single();
    let released = Rc::new(Cell::new(0));
    let (runtime, handle, display) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let display = cx.displays()[0].id();
        let dropped = released.clone();
        let handle = Bar::new(Thickness::new(px(40.)).unwrap())
            .outputs(DisplaySelection::Explicit(vec![display, display]))
            .h(px(2.))
            .w(px(80.))
            .exit_transition(Transition::fade_out().duration(Duration::from_millis(100)))
            .mount(
                &runtime,
                move |_, _, _, cx| {
                    Ok(cx.new(|_| Content {
                        released: dropped.clone(),
                        renders: Rc::new(Cell::new(0)),
                    }))
                },
                cx,
            )
            .unwrap();
        (runtime, handle, display)
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    cx.simulate_window_resize(
        cx.update(|cx| cx.windows()[0]),
        gpui::size(px(400.), px(52.)),
    );
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(cx.update(|cx| cx.windows()[0]), &cx);
    cx.update(|cx| handle.hide(display, cx).unwrap());
    assert_eq!(released.get(), 0);
    cx.executor().advance_clock(Duration::from_millis(100));
    visual.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    assert_eq!(released.get(), 1);
    cx.update(|cx| runtime.shutdown(cx));
}

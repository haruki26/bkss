use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
};

use bkss_core::{
    MountedBinding, ReactiveSubscription, Shell, Source, SourceDescriptor, SourcePublisher,
    SourceState,
};
use bkss_integration::{ClockState, HyprlandState, IntegrationView, Usage};
use gpui::{
    AnyElement, App, AppContext, Context, Render, TestAppContext, Window, WindowOptions, div,
    prelude::*, rgb,
};

type ReadyRenderer<T> = Box<dyn FnMut(&T, &mut Window, &mut Context<SourceView<T>>) -> AnyElement>;
type LoadingRenderer<T> = Box<dyn FnMut(&mut Window, &mut Context<SourceView<T>>) -> AnyElement>;

struct SourceView<T: Clone + 'static> {
    binding: MountedBinding<T>,
    _subscription: ReactiveSubscription,
    ready: ReadyRenderer<T>,
    loading: LoadingRenderer<T>,
}

impl<T: Clone + 'static> SourceView<T> {
    fn new_with_render<E: IntoElement>(
        binding: MountedBinding<T>,
        cx: &mut Context<Self>,
        mut render: impl FnMut(&T, &mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        let subscription = binding.reactive().observe(cx, |_, _, cx| cx.notify());
        Self {
            binding,
            _subscription: subscription,
            ready: Box::new(move |value, window, cx| render(value, window, cx).into_any_element()),
            loading: Box::new(|_, _| "Loading…".into_any_element()),
        }
    }

    fn new(binding: MountedBinding<T>, cx: &mut Context<Self>) -> Self
    where
        T: IntegrationView,
    {
        Self::new_with_render(binding, cx, |value, window, cx| value.render(window, cx))
    }

    fn with_render<E: IntoElement>(
        mut self,
        mut render: impl FnMut(&T, &mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        self.ready =
            Box::new(move |value, window, cx| render(value, window, cx).into_any_element());
        self
    }

    fn with_loading<E: IntoElement>(
        mut self,
        mut render: impl FnMut(&mut Window, &mut Context<Self>) -> E + 'static,
    ) -> Self {
        self.loading = Box::new(move |window, cx| render(window, cx).into_any_element());
        self
    }
}

impl<T: Clone + 'static> Render for SourceView<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.binding.reactive().get(cx) {
            SourceState::Loading => (self.loading)(window, cx),
            SourceState::Ready(value) => (self.ready)(&value, window, cx),
        }
    }
}

#[derive(Clone)]
struct CustomState {
    item: Option<String>,
    default_rendered: Rc<RefCell<Vec<String>>>,
}

impl IntegrationView for CustomState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let text = self.item.as_deref().unwrap_or("No item").to_owned();
        self.default_rendered.borrow_mut().push(text.clone());
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().size_2().rounded_full().bg(rgb(0xeeeeee)))
            .child(text)
            .into_any_element()
    }
}

#[derive(Default)]
struct Probe {
    starts: Cell<usize>,
    stops: Cell<usize>,
    publishers: RefCell<Vec<SourcePublisher<CustomSource>>>,
}

struct CustomSource(Rc<Probe>);
struct Guard(Rc<Probe>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.stops.set(self.0.stops.get() + 1);
    }
}

impl Source for CustomSource {
    type Key = ();
    type Value = CustomState;
    type Error = io::Error;
    type Guard = Guard;

    fn key(&self) {}

    fn start(&self, publisher: SourcePublisher<Self>, _: &mut App) -> io::Result<Guard> {
        self.0.starts.set(self.0.starts.get() + 1);
        self.0.publishers.borrow_mut().push(publisher);
        Ok(Guard(self.0.clone()))
    }
}

#[test]
fn integration_elements_and_custom_content_render_updates_from_a_downstream_source() {
    let cx = TestAppContext::single();
    let probe = Rc::new(Probe::default());
    let descriptor = SourceDescriptor::new(CustomSource(probe.clone()));
    let default_rendered = Rc::new(RefCell::new(Vec::new()));
    let rendered = Rc::new(RefCell::new(Vec::new()));
    let notifications = Rc::new(Cell::new(0));

    let (runtime, _subscription) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let binding = runtime.mount(&descriptor, cx).unwrap();
        assert!(matches!(binding.reactive().get(cx), SourceState::Loading));
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|cx| SourceView::new(binding, cx))
        })
        .unwrap();
        let binding = runtime.mount(&descriptor, cx).unwrap();
        let ready = rendered.clone();
        let loading = rendered.clone();
        let custom_window = cx
            .open_window(WindowOptions::default(), |_, cx| {
                cx.new(|cx| {
                    SourceView::new(binding, cx)
                        .with_render(move |state, _, _| {
                            let text = format!("Item: {}", state.item.as_deref().unwrap_or("none"));
                            ready.borrow_mut().push(text.clone());
                            div().px_2().text_color(rgb(0xeeeeee)).child(text)
                        })
                        .with_loading(move |_, _| {
                            loading.borrow_mut().push("Waiting".to_owned());
                            div().child("Waiting")
                        })
                })
            })
            .unwrap();
        let view = custom_window.update(cx, |_, _, cx| cx.entity()).unwrap();
        let count = notifications.clone();
        let subscription = cx.observe(&view, move |_, _| count.set(count.get() + 1));
        (runtime, subscription)
    });
    cx.run_until_parked();
    assert_eq!(probe.starts.get(), 1);
    assert!(
        default_rendered.borrow().is_empty(),
        "Loading must not render a ready integration"
    );
    assert_eq!(
        rendered.borrow().last().map(String::as_str),
        Some("Waiting")
    );

    let publisher = probe.publishers.borrow()[0].clone();
    for (item, expected_default, expected_custom) in [
        (Some("Headphones"), "Headphones", "Item: Headphones"),
        (None, "No item", "Item: none"),
        (Some(""), "", "Item: "),
    ] {
        let before = notifications.get();
        cx.update(|cx| {
            publisher.publish(
                CustomState {
                    item: item.map(str::to_owned),
                    default_rendered: default_rendered.clone(),
                },
                cx,
            );
        });
        cx.run_until_parked();
        assert!(
            notifications.get() > before,
            "publication must notify the view"
        );
        assert_eq!(
            default_rendered.borrow().last().map(String::as_str),
            Some(expected_default)
        );
        assert_eq!(
            rendered.borrow().last().map(String::as_str),
            Some(expected_custom)
        );
    }

    cx.update(|cx| runtime.shutdown(cx));
    cx.run_until_parked();
    assert_eq!(probe.stops.get(), 1);
}

#[test]
fn preset_states_provide_composable_gpui_elements() {
    struct Presets;
    impl gpui::Render for Presets {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let clock = ClockState {
                date: "2026-11-15".into(),
                time: "12:34".into(),
            };
            let usage = Usage::from_parts(1, 4);
            let hyprland = HyprlandState {
                workspaces: Vec::new(),
                active_workspace: None,
                active_window: None,
            };
            div()
                .flex()
                .gap_2()
                .child(clock.render(window, cx))
                .child(usage.render(window, cx))
                .child(hyprland.render(window, cx))
        }
    }
    let cx = TestAppContext::single();
    cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Presets))
            .unwrap();
    });
    cx.run_until_parked();
}

#[test]
fn custom_renderer_accepts_values_without_an_integration_view() {
    struct NumberSource;
    impl Source for NumberSource {
        type Key = ();
        type Value = u32;
        type Error = io::Error;
        type Guard = ();

        fn key(&self) {}

        fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<()> {
            publisher.publish(7, cx);
            Ok(())
        }
    }

    let cx = TestAppContext::single();
    let rendered = Rc::new(Cell::new(None));
    let value = rendered.clone();
    let runtime = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let binding = runtime
            .mount(&SourceDescriptor::new(NumberSource), cx)
            .unwrap();
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|cx| {
                SourceView::new_with_render(binding, cx, move |number, _, _| {
                    value.set(Some(*number));
                    div().child(format!("Count: {number}"))
                })
            })
        })
        .unwrap();
        runtime
    });
    cx.run_until_parked();
    assert_eq!(rendered.get(), Some(7));
    cx.update(|cx| runtime.shutdown(cx));
}

#[test]
fn view_lifetime_owns_the_binding_but_does_not_extend_the_runtime() {
    let cx = TestAppContext::single();
    let probe = Rc::new(Probe::default());
    let descriptor = SourceDescriptor::new(CustomSource(probe.clone()));
    let (runtime, first, second, retained_reactive) = cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        let first = runtime.mount(&descriptor, cx).unwrap();
        let retained = first.reactive();
        let first = cx.new(|cx| SourceView::new(first, cx));
        let second = runtime.mount(&descriptor, cx).unwrap();
        let second = cx.new(|cx| SourceView::new(second, cx));
        (runtime, first, second, retained)
    });
    assert_eq!(probe.starts.get(), 1);
    cx.update(|_| drop(first));
    cx.run_until_parked();
    assert_eq!(probe.stops.get(), 0);
    cx.update(|_| drop(second));
    cx.run_until_parked();
    assert_eq!(probe.stops.get(), 1);
    assert!(matches!(retained_reactive.get(&cx), SourceState::Loading));

    let (third, current) = cx.update(|cx| {
        let binding = runtime.mount(&descriptor, cx).unwrap();
        let current = binding.reactive();
        (cx.new(|cx| SourceView::new(binding, cx)), current)
    });
    assert_eq!(probe.starts.get(), 2);
    assert!(matches!(current.get(&cx), SourceState::Loading));
    drop(runtime);
    cx.run_until_parked();
    assert_eq!(probe.stops.get(), 2, "the view must not retain the runtime");
    cx.update(|_| drop(third));
    cx.run_until_parked();
    assert_eq!(probe.stops.get(), 2);
}

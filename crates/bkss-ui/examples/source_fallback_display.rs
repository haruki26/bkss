//! Run `mise x -- cargo run -p bkss-ui --example source_fallback_display -- --seconds 300`.
use std::{
    cell::{Cell, RefCell},
    io,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use bkss_core::{
    GlassColor, GlassStyle, MountedBinding, ReactiveSubscription, Shell, ShellError, Source,
    SourceDescriptor, SourcePublisher, SourceState, WeakRuntimeHandle,
};
use bkss_ui::{
    KeyboardFocus, OverlayAnchors, OverlayDefinition, OverlayGeometry, OverlayHandle,
    OverlayPlacement, OverlayRuntimeHandleExt,
};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, DisplayId, Render, Subscription, Task,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
};

const USAGE: &str = "source_fallback_display [--display ID] [--seconds 1..300]
Defaults: all currently connected displays, 120-second watchdog. No display hotplug support.
The ordinary window is a control panel, not a source consumer. A/B are layer overlays.
Open A to see Loading. After the 800ms GPUI delay, explicitly release a Ready value.
Ready populated, Ready empty list, and Ready None are separate acquired states.
Open B: both share one activation. Close either: no stop. Close the last: one stop.
Reopen: a new activation starts in Loading, with another delay and explicit release.
Every overlay has peer open/close, release, and Stop controls if it covers the panel.
No animation, external integration, keyboard arbitration, or native focus guarantee.";
const DELAY: Duration = Duration::from_millis(800);
type Items = Option<Vec<String>>;

struct Args {
    display: Option<DisplayId>,
    seconds: u64,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> io::Result<Args> {
    let mut parsed = Args {
        display: None,
        seconds: 120,
    };
    let mut seen_seconds = false;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| io::Error::other(format!("{flag} needs a value")))?;
        match flag.as_str() {
            "--display" if parsed.display.is_none() => {
                parsed.display = Some(DisplayId::new(value.parse().map_err(io::Error::other)?));
            }
            "--seconds" if !seen_seconds => {
                seen_seconds = true;
                parsed.seconds = value.parse().map_err(io::Error::other)?;
                if !(1..=300).contains(&parsed.seconds) {
                    return Err(io::Error::other("seconds must be 1..=300"));
                }
            }
            _ => {
                return Err(io::Error::other(format!(
                    "unknown or repeated option: {flag}"
                )));
            }
        }
    }
    Ok(parsed)
}

#[derive(Default)]
struct Probe {
    starts: Cell<u64>,
    stops: Cell<u64>,
    publisher: RefCell<Option<SourcePublisher<DelayedSource>>>,
}

struct DelayedSource(Rc<Probe>);
struct DelayedGuard {
    probe: Rc<Probe>,
    _task: Task<()>,
}

impl Drop for DelayedGuard {
    fn drop(&mut self) {
        self.probe.publisher.borrow_mut().take();
        self.probe.stops.set(self.probe.stops.get() + 1);
        println!(
            "SOURCE_STOP activation={} starts={} stops={}",
            self.probe.starts.get(),
            self.probe.starts.get(),
            self.probe.stops.get()
        );
    }
}

impl Source for DelayedSource {
    type Key = ();
    type Value = Items;
    type Error = io::Error;
    type Guard = DelayedGuard;

    fn key(&self) {}

    fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<DelayedGuard> {
        self.0.starts.set(self.0.starts.get() + 1);
        println!("SOURCE_START activation={}; Loading", self.0.starts.get());
        let probe = self.0.clone();
        // Keep Loading until a user release, even if native surface setup exceeds the delay.
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(DELAY).await;
            cx.update(|cx| {
                *probe.publisher.borrow_mut() = Some(publisher);
                println!(
                    "DELAY_DONE activation={}; waiting for release",
                    probe.starts.get()
                );
                cx.refresh_windows();
            });
        });
        cx.refresh_windows();
        Ok(DelayedGuard {
            probe: self.0.clone(),
            _task: task,
        })
    }
}

struct Session {
    runtime: WeakRuntimeHandle,
    display: Option<DisplayId>,
    overlays: RefCell<Vec<OverlayHandle>>,
    probe: Rc<Probe>,
    request_error: RefCell<Option<String>>,
    control: Cell<Option<AnyWindowHandle>>,
    subscriptions: RefCell<Vec<Subscription>>,
    watchdog: RefCell<Option<Task<()>>>,
}

impl Session {
    fn request(&self, index: usize, display: DisplayId, open: bool, cx: &mut App) {
        let handle = self.overlays.borrow().get(index).cloned();
        if let Some(handle) = handle {
            let result = if open {
                handle.open(display, cx)
            } else {
                handle.close(display, cx)
            };
            *self.request_error.borrow_mut() = result.err().map(|error| {
                let message = format!(
                    "{} {} on {display:?}: {error}",
                    if open { "Open" } else { "Close" },
                    name(index)
                );
                eprintln!("REQUEST_ERROR {message}");
                message
            });
        }
        cx.refresh_windows();
    }

    fn release(&self, value: Items, cx: &mut App) {
        let publisher = self.probe.publisher.borrow().clone();
        if let Some(publisher) = publisher {
            println!(
                "SOURCE_RELEASE activation={} value={value:?}",
                self.probe.starts.get()
            );
            publisher.publish(value, cx);
        }
    }

    fn stop(&self, cx: &mut App) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.shutdown(cx);
        }
    }
}

fn name(index: usize) -> &'static str {
    if index == 0 { "A" } else { "B" }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x284a57))
        .child(label)
        .when(enabled, |button| {
            button
                .focusable()
                .tab_stop(true)
                .cursor_pointer()
                .hover(|style| style.bg(rgb(0x356576)))
                .focus_visible(|style| style.border_2().border_color(rgb(0xeec589)))
                .on_click(move |_, _, cx| action(cx))
        })
        .when(!enabled, |button| button.opacity(0.45))
}

fn stop_button(session: Rc<Session>) -> impl IntoElement {
    button("stop", "Stop example", true, move |cx| {
        let session = session.clone();
        cx.defer(move |cx| session.stop(cx));
    })
}

fn overlay_controls(session: &Rc<Session>, display: DisplayId) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap_2();
    for index in 0..2 {
        for open in [true, false] {
            let session = session.clone();
            row = row.child(button(
                (if open { "open" } else { "close" }, index),
                format!("{} {}", if open { "Open" } else { "Close" }, name(index)),
                true,
                move |cx| {
                    let session = session.clone();
                    cx.defer(move |cx| session.request(index, display, open, cx));
                },
            ));
        }
    }
    row
}

fn source_controls(session: &Rc<Session>) -> impl IntoElement {
    let enabled = session.probe.publisher.borrow().is_some();
    let status = if enabled {
        "Delay complete. Choose a Ready value; updates reach every open consumer."
    } else if session.probe.starts.get() > session.probe.stops.get() {
        "800ms acquisition delay. Loading remains until you release a value."
    } else {
        "Source stopped. Open an overlay to start acquisition."
    };
    let mut buttons = div().flex().flex_wrap().gap_2();
    for (index, label, value) in [
        (
            0_usize,
            "Release populated",
            Some(vec!["Desk lamp".into(), "Headphones".into()]),
        ),
        (1, "Release empty list", Some(Vec::new())),
        (2, "Release None", None),
    ] {
        let session = session.clone();
        buttons = buttons.child(button(("release", index), label, enabled, move |cx| {
            session.release(value.clone(), cx);
        }));
    }
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().text_color(rgb(0xeec589)).child(format!(
            "Starts {}   /   Stops {}   /   Activation {}",
            session.probe.starts.get(),
            session.probe.stops.get(),
            session.probe.starts.get()
        )))
        .child(status)
        .child(buttons)
        .when_some(session.request_error.borrow().clone(), |view, error| {
            view.child(div().text_color(rgb(0xffb3a8)).child(error))
        })
}

struct Controls(Rc<Session>);
impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let displays = self.0.display.map_or_else(
            || {
                cx.displays()
                    .into_iter()
                    .map(|display| display.id())
                    .collect()
            },
            |display| vec![display],
        );
        let mut rows = div().flex().flex_col().gap_3();
        for (index, display) in displays.iter().copied().enumerate() {
            rows = rows.child(
                div()
                    .id(("display", index))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(format!("Display {display:?}"))
                    .child(overlay_controls(&self.0, display)),
            );
        }
        div().id("control-panel").size_full().overflow_y_scroll().p_6()
            .bg(rgb(0x14232b)).text_color(rgb(0xeaf0ed)).font_family("sans-serif")
            .flex().flex_col().gap_4()
            .child(div().text_color(rgb(0xeec589)).child("BKSS / CONTROL WINDOW"))
            .child(div().text_size(px(28.)).child("Source fallback"))
            .child("This window does not acquire the source. Only open layer overlays hold consumers.")
            .child("1. Open A: Loading.  2. Wait for release controls.  3. Release a Ready value.")
            .child(rows)
            .when(displays.is_empty(), |view| view.child("Displays not ready. Refresh to retry."))
            .child(button("refresh", "Refresh displays / counters", true, |cx| cx.refresh_windows()))
            .child(source_controls(&self.0))
            .child("Open B to share. Close A: B keeps the source alive. Close B: source stops. Reopen A: Loading, not the old Ready value.")
            .child(stop_button(self.0.clone()))
    }
}

fn presentation(state: &SourceState<Items>) -> (&'static str, String, u32) {
    match state {
        SourceState::Loading => (
            "Loading / fallback",
            "Waiting for the first acquired value. Release a result after the delay.".into(),
            0xeec589,
        ),
        SourceState::Ready(None) => (
            "Ready / None",
            "Acquired: no connected device. This is not Loading.".into(),
            0x99d4bc,
        ),
        SourceState::Ready(Some(items)) if items.is_empty() => (
            "Ready / empty list",
            "Acquired: 0 items. This is not Loading.".into(),
            0x99d4bc,
        ),
        SourceState::Ready(Some(items)) => ("Ready / populated", items.join("\n"), 0x99d4bc),
    }
}

struct Content {
    session: Rc<Session>,
    display: DisplayId,
    index: usize,
    binding: MountedBinding<Items>,
    _subscription: ReactiveSubscription,
    _release: Subscription,
}

impl Render for Content {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, detail, color) = presentation(&self.binding.reactive().get(cx));
        div().id("source-content").w(px(410.)).max_h(px(580.)).overflow_y_scroll().p_5()
            .text_color(rgb(0xeaf0ed)).font_family("sans-serif").flex().flex_col().gap_4()
            .child(div().text_color(rgb(0xeec589)).child(format!("LAYER OVERLAY {} / {:?}", name(self.index), self.display)))
            .child(div().p_4().rounded_md().bg(rgb(0x20343d)).flex().flex_col().gap_2()
                .child(div().text_size(px(25.)).text_color(rgb(color)).child(title))
                .child(detail))
            .child(overlay_controls(&self.session, self.display))
            .child(source_controls(&self.session))
            .child("A/B share one source. Closing the last consumer cancels its delay and stops acquisition.")
            .child(stop_button(self.session.clone()))
    }
}

fn run(args: Args) -> Result<(), ShellError> {
    Shell::run(move |runtime, cx| {
        let probe = Rc::new(Probe::default());
        let descriptor = SourceDescriptor::new(DelayedSource(probe.clone()));
        let session = Rc::new(Session {
            runtime: runtime.downgrade(),
            display: args.display,
            overlays: RefCell::new(Vec::new()),
            probe,
            request_error: RefCell::new(None),
            control: Cell::new(None),
            subscriptions: RefCell::new(Vec::new()),
            watchdog: RefCell::new(None),
        });
        let style = GlassStyle::default()
            .with_tint(GlassColor::rgba(0.07, 0.13, 0.16, 0.99).map_err(ShellError::setup)?);
        for index in 0..2 {
            let weak_runtime = runtime.downgrade();
            let descriptor = descriptor.clone();
            let session = session.clone();
            let definition = OverlayDefinition::new(KeyboardFocus::Interactive)
                .with_style(style.clone())
                .with_geometry(OverlayGeometry {
                    placement: OverlayPlacement::Anchor {
                        edges: OverlayAnchors {
                            left: index == 0,
                            right: index == 1,
                            ..Default::default()
                        },
                        offset: [if index == 0 { 24. } else { -24. }, 0.],
                    },
                    max_logical_size: Some([410., 580.]),
                })
                .map_err(ShellError::setup)?;
            let content_session = session.clone();
            let handle = runtime.mount_overlay(
                definition,
                move |display, _, cx| {
                    let runtime = weak_runtime.upgrade().ok_or_else(|| {
                        ShellError::setup(io::Error::other("runtime already stopped"))
                    })?;
                    let binding = runtime.mount(&descriptor, cx)?;
                    println!(
                        "CONTENT_MOUNT overlay={} display={display:?} state={:?}",
                        name(index),
                        binding.reactive().get(cx)
                    );
                    let session = content_session.clone();
                    Ok(cx.new(|cx| {
                        let subscription = binding.reactive().observe(cx, move |_, state, cx| {
                            println!(
                                "CONTENT_UPDATE overlay={} display={display:?} state={state:?}",
                                name(index)
                            );
                            cx.notify();
                        });
                        let release = cx.on_release(move |_, cx| {
                            println!(
                                "CONTENT_UNMOUNT overlay={} display={display:?}",
                                name(index)
                            );
                            cx.defer(|cx| cx.refresh_windows());
                        });
                        Content {
                            session,
                            display,
                            index,
                            binding,
                            _subscription: subscription,
                            _release: release,
                        }
                    }))
                },
                cx,
            )?;
            session.overlays.borrow_mut().push(handle);
        }
        let control_session = session.clone();
        let control = cx
            .open_window(
                WindowOptions {
                    app_id: Some("bkss-source-fallback".into()),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(760.), px(650.)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Controls(control_session)),
            )
            .map_err(|error| ShellError::setup(io::Error::other(error)))?;
        session.control.set(Some(control.into()));
        let weak = Rc::downgrade(&session);
        let closed = cx.on_window_closed(move |cx, id| {
            if let Some(session) = weak.upgrade()
                && session
                    .control
                    .get()
                    .is_some_and(|handle| handle.window_id() == id)
            {
                session.control.take();
                session.stop(cx);
            }
        });
        let weak = Rc::downgrade(&session);
        let quit = cx.on_app_quit(move |cx| {
            if let Some(session) = weak.upgrade()
                && let Some(control) = session.control.take()
            {
                let _ = control.update(cx, |_, window, _| window.remove_window());
            }
            std::future::ready(())
        });
        session.subscriptions.borrow_mut().extend([closed, quit]);
        let weak_runtime = runtime.downgrade();
        *session.watchdog.borrow_mut() = Some(cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(args.seconds))
                .await;
            cx.update(|cx| {
                cx.defer(move |cx| {
                    if let Some(runtime) = weak_runtime.upgrade() {
                        runtime.shutdown(cx);
                    }
                })
            });
        }));
        Ok(())
    })
}

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = parse_args(std::env::args().skip(1))
        .map_err(Box::<dyn std::error::Error>::from)
        .and_then(|args| run(args).map_err(Into::into));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("source_fallback_display: {error}");
            let mut cause = error.source();
            while let Some(error) = cause {
                eprintln!("  caused by: {error}");
                cause = error.source();
            }
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_source_waits_for_release_and_cancels_on_drop() {
        let mut cx = gpui::TestAppContext::single();
        let probe = Rc::new(Probe::default());
        let descriptor = SourceDescriptor::new(DelayedSource(probe.clone()));
        let scope = bkss_core::ResourceScope::new(&mut cx);
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
        assert!(probe.publisher.borrow().is_none());
        cx.run_until_parked();
        cx.background_executor.advance_clock(DELAY);
        cx.run_until_parked();
        assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
        let publisher = probe.publisher.borrow().clone().unwrap();
        cx.update(|cx| publisher.publish(None, cx));
        assert_eq!(lease.reactive().get(&cx), SourceState::Ready(None));
        drop(lease);
        assert!(probe.publisher.borrow().is_none());
        assert_eq!(probe.stops.get(), 1);

        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
        cx.run_until_parked();
        drop(lease);
        cx.background_executor.advance_clock(DELAY);
        cx.run_until_parked();
        assert!(probe.publisher.borrow().is_none());
        assert_eq!((probe.starts.get(), probe.stops.get()), (2, 2));
    }

    #[test]
    fn arguments_are_bounded_and_options_cannot_repeat() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|arg| (*arg).to_owned()));
        assert_eq!(parse(&[]).unwrap().seconds, 120);
        assert_eq!(
            parse(&["--display", "1", "--seconds", "300"])
                .unwrap()
                .display,
            Some(DisplayId::new(1))
        );
        for args in [
            vec!["--seconds", "0"],
            vec!["--seconds", "301"],
            vec!["--seconds", "1", "--seconds", "2"],
            vec!["--display", "1", "--display", "2"],
            vec!["--display", "invalid"],
            vec!["--seconds"],
            vec!["--other", "1"],
        ] {
            assert!(parse(&args).is_err());
        }
    }

    #[test]
    fn only_loading_uses_fallback() {
        let states = [
            SourceState::Loading,
            SourceState::Ready(Some(vec!["Item".into()])),
            SourceState::Ready(Some(Vec::new())),
            SourceState::Ready(None),
        ];
        let titles: Vec<_> = states.iter().map(|state| presentation(state).0).collect();
        assert_eq!(
            titles,
            [
                "Loading / fallback",
                "Ready / populated",
                "Ready / empty list",
                "Ready / None"
            ]
        );
    }
}

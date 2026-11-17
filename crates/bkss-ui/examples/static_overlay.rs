//! Run `mise x -- cargo run -p bkss-ui --example static_overlay -- --seconds 120`.
use std::{
    cell::{Cell, RefCell},
    io,
    process::ExitCode,
    rc::Rc,
    time::{Duration, Instant},
};

use bkss_core::{
    GlassBorder, GlassColor, GlassStyle, MountedBinding, ReactiveSubscription, Shell, ShellError,
    Source, SourceDescriptor, SourcePublisher, WeakRuntimeHandle,
};
use bkss_ui::effects::{GlassState, MountedGlass};
use bkss_ui::{
    Bar as ShellBar, DisplaySelection, KeyboardInteractivity, Overlay, SurfaceHandle, Thickness,
};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, DisplayId, Render, Subscription, Task,
    Window, WindowBounds, WindowOptions, canvas, div, prelude::*, px, rgb, size,
};

const USAGE: &str = "static_overlay [--display ID ...] [--seconds 1..300]
Defaults: displays connected when reference bars are requested, 120-second watchdog.
First click Mount reserving bars; wait for their layout and a 500ms warm-up, then Open A/B.
The content cards have peer open/close and Stop buttons if the ordinary panel is covered.
A is Interactive / outside-close; B is keyboard None / no backdrop.
No native activation, focus restoration, keyboard arbitration, or same-layer ordering promise.
Stable positive reservations only. Live reservation tracking and non-reserving bar ordering
are unsupported; the actual configured local viewport is used. No compositor changes.";

struct Args {
    displays: Vec<DisplayId>,
    seconds: u64,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> io::Result<Args> {
    let mut parsed = Args {
        displays: Vec::new(),
        seconds: 120,
    };
    let mut seen_seconds = false;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| io::Error::other(format!("{flag} needs a value")))?;
        match flag.as_str() {
            "--display" => {
                let id = DisplayId::new(value.parse().map_err(io::Error::other)?);
                if parsed.displays.contains(&id) {
                    return Err(io::Error::other("repeated display"));
                }
                parsed.displays.push(id);
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

struct SampleSource {
    publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
}
struct SampleGuard;
impl Drop for SampleGuard {
    fn drop(&mut self) {
        println!("RESOURCE_STOP shared-sample");
    }
}
impl Source for SampleSource {
    type Key = ();
    type Value = u64;
    type Error = io::Error;
    type Guard = SampleGuard;
    fn key(&self) {}
    fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<SampleGuard> {
        println!("RESOURCE_START shared-sample");
        publisher.publish(0, cx);
        *self.publisher.borrow_mut() = Some(publisher);
        Ok(SampleGuard)
    }
}

struct Session {
    runtime: WeakRuntimeHandle,
    requested: Vec<DisplayId>,
    targets: RefCell<Vec<DisplayId>>,
    bars_requested: Cell<bool>,
    bar_ready: RefCell<Vec<(DisplayId, Instant)>>,
    overlays: RefCell<Vec<SurfaceHandle>>,
    publisher: Rc<RefCell<Option<SourcePublisher<SampleSource>>>>,
    value: Cell<u64>,
    control: Cell<Option<AnyWindowHandle>>,
    subscriptions: RefCell<Vec<Subscription>>,
    watchdog: RefCell<Option<Task<()>>>,
    failure: Rc<RefCell<Option<ShellError>>>,
}

impl Session {
    fn mount_bars(self: &Rc<Self>, cx: &mut App) {
        if self.bars_requested.get() {
            return;
        }
        let targets = if self.requested.is_empty() {
            cx.displays().into_iter().map(|d| d.id()).collect()
        } else {
            self.requested.clone()
        };
        if targets.is_empty() {
            println!("WAIT displays not ready; click Mount reserving bars again");
            return;
        }
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        *self.targets.borrow_mut() = targets.clone();
        let weak = Rc::downgrade(self);
        let result = match Thickness::new(px(40.)) {
            Ok(thickness) => ShellBar::new(thickness)
                .margins(gpui::Edges::all(px(12.)))
                .outputs(DisplaySelection::Explicit(targets))
                .mount(
                    &runtime,
                    move |instance, _, _, cx| {
                        Ok(cx.new(|cx| Bar {
                            glass: MountedGlass::new(GlassStyle::default(), cx),
                            display: instance.display_id(),
                            session: weak.clone(),
                        }))
                    },
                    cx,
                ),
            Err(error) => Err(ShellError::setup(error)),
        };
        match result {
            Ok(_) => self.bars_requested.set(true),
            Err(error) => {
                *self.failure.borrow_mut() = Some(error);
                let runtime = runtime.downgrade();
                cx.defer(move |cx| {
                    if let Some(runtime) = runtime.upgrade() {
                        runtime.shutdown(cx);
                    }
                });
            }
        }
        cx.refresh_windows();
    }

    fn open(&self, index: usize, display: DisplayId, cx: &mut App) {
        let ready = self
            .bar_ready
            .borrow()
            .iter()
            .any(|(id, time)| *id == display && time.elapsed() >= Duration::from_millis(500));
        if !ready {
            println!("WAIT display={display:?} reference bar must lay out, then warm up for 500ms");
            return;
        }
        let handle = self.overlays.borrow().get(index).cloned();
        if let Some(handle) = handle
            && let Err(error) = handle.show(display, cx)
        {
            eprintln!("OPEN_ERROR {error}");
        }
    }

    fn close(&self, index: usize, display: DisplayId, cx: &mut App) {
        let handle = self.overlays.borrow().get(index).cloned();
        if let Some(handle) = handle
            && let Err(error) = handle.hide(display, cx)
        {
            eprintln!("CLOSE_ERROR {error}");
        }
    }

    fn stop(&self, cx: &mut App) {
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.shutdown(cx);
        }
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .cursor_pointer()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x315966))
        .hover(|s| s.bg(rgb(0x43788a)))
        .child(label)
        .on_click(move |_, _, cx| action(cx))
}

fn stop_button(session: Rc<Session>) -> impl IntoElement {
    button("stop", "Stop everything", move |cx| {
        let session = session.clone();
        cx.defer(move |cx| session.stop(cx));
    })
}

struct Controls(Rc<Session>);
impl Render for Controls {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let session = self.0.clone();
        let mount = button("mount-bars", "Mount reserving bars", move |cx| {
            session.mount_bars(cx)
        });
        let mut rows = div().flex().flex_col().gap_3();
        for (row_index, display) in self.0.targets.borrow().iter().copied().enumerate() {
            let mut row = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(format!("Display {display:?}"));
            for index in 0..2 {
                let session = self.0.clone();
                row = row.child(button(
                    ("open", index),
                    format!("Open {}", if index == 0 { "A" } else { "B" }),
                    move |cx| session.open(index, display, cx),
                ));
            }
            rows = rows.child(div().id(("display", row_index)).child(row));
        }
        div().id("controls").size_full().overflow_y_scroll().p_6().bg(rgb(0x14242d)).text_color(rgb(0xeaf0ed))
            .font_family("sans-serif").flex().flex_col().gap_4()
            .child(div().text_color(rgb(0xe9b577)).child("BKSS / STATIC OVERLAY"))
            .child(div().font_family("serif").text_size(px(30.)).child("A quiet place for content"))
            .child("1. Mount reference bars.  2. Wait at least half a second.  3. Open an overlay on each target display.")
            .child(mount).child(rows)
            .child("Stable reservations only: the overlay fits its configured local viewport, not a cached full-display rectangle. No live reservation tracking, native activation, or focus restoration.")
            .child(stop_button(self.0.clone()))
    }
}

struct Bar {
    display: DisplayId,
    session: std::rc::Weak<Session>,
    glass: MountedGlass,
}
impl Render for Bar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let display = self.display;
        let session = self.session.clone();
        self.glass.element(GlassState::default(), cx)
            .relative().text_color(rgb(0xeaf0ed)).px_3().child(format!("Independent reserving Top BottomBar · {display:?}"))
            .child(canvas(move |_, window, cx| {
                if window.viewport_size().height > px(0.) && let Some(session) = session.upgrade() {
                    let mut ready = session.bar_ready.borrow_mut();
                    if !ready.iter().any(|(id, _)| *id == display) {
                        println!("BAR_READY display={display:?} viewport={:?}; wait 500ms before opening", window.viewport_size());
                        ready.push((display, Instant::now()));
                        drop(ready);
                        cx.refresh_windows();
                    }
                }
            }, |_, (), _, _| {}).absolute().inset_0().size_full())
    }
}

struct Content {
    session: Rc<Session>,
    display: DisplayId,
    index: usize,
    binding: MountedBinding<u64>,
    _subscription: ReactiveSubscription,
    clicks: u64,
    focus: gpui::FocusHandle,
    glass: MountedGlass,
}
impl Render for Content {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = if self.index == 0 { "A" } else { "B" };
        let peer = 1 - self.index;
        let display = self.display;
        let session = self.session.clone();
        let open = button(
            "peer-open",
            format!("Open {} from {name}", if peer == 0 { "A" } else { "B" }),
            move |cx| {
                let session = session.clone();
                cx.defer(move |cx| session.open(peer, display, cx));
            },
        );
        let session = self.session.clone();
        let close_peer = button("peer-close", "Close other", move |cx| {
            let session = session.clone();
            cx.defer(move |cx| session.close(peer, display, cx));
        });
        let session = self.session.clone();
        let index = self.index;
        let close = button("close", "Close this overlay", move |cx| {
            let session = session.clone();
            cx.defer(move |cx| session.close(index, display, cx));
        });
        let session = self.session.clone();
        let publish = button("publish", "Update shared sample", move |cx| {
            let value = session.value.get() + 1;
            session.value.set(value);
            let publisher = session.publisher.borrow().clone();
            if let Some(publisher) = publisher {
                publisher.publish(value, cx);
            }
        });
        let content = self.glass.element(GlassState::default(), cx)
            .w(px(500.)).p_5().flex().flex_col().gap_3().text_color(rgb(0xeaf0ed)).font_family("sans-serif")
            .child(format!("OVERLAY {name} / {display:?}"))
            .child(div().flex().flex_wrap().gap_2().child(open).child(close_peer).child(close).child(stop_button(self.session.clone())))
            .child(div().font_family("serif").text_size(px(28.)).child("Independent view, shared resource"))
            .child(format!("Sample {:?} · local clicks {}", self.binding.reactive().get(cx), self.clicks))
            .child(publish)
            .child(div().id("explicit-scroll").h(px(120.)).overflow_y_scroll().children((1..=20).map(|row| div().h(px(28.)).flex_shrink_0().child(format!("Explicit scroll row {row:02}")))))
            .child("A: Interactive, outside-close. B: None, pointer controls still work. Escape only closes an interactive recipient when delivered and not consumed by a child.");
        div()
            .track_focus(&self.focus)
            .on_key_down(move |event, window, _| {
                let key = if event.keystroke.key == "escape" { "Escape" } else { "other-redacted" };
                println!("KEY content={name} display={display:?} key={key} active_observation={}", window.is_window_active());
            })
            .on_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.clicks += 1;
                println!("CONTENT_POINTER index={} display={:?} count={} local={:?} active_observation={}", this.index, this.display, this.clicks, event.position, window.is_window_active());
                cx.notify();
            }))
            .child(content)
    }
}

fn run(args: Args) -> Result<(), ShellError> {
    let failure = Rc::new(RefCell::new(None));
    let callback_failure = failure.clone();
    let result = Shell::run(move |runtime, cx| {
        let publisher = Rc::new(RefCell::new(None));
        let descriptor = SourceDescriptor::new(SampleSource {
            publisher: publisher.clone(),
        });
        let session = Rc::new(Session {
            runtime: runtime.downgrade(),
            requested: args.displays,
            targets: RefCell::new(Vec::new()),
            bars_requested: Cell::new(false),
            bar_ready: RefCell::new(Vec::new()),
            overlays: RefCell::new(Vec::new()),
            publisher,
            value: Cell::new(0),
            control: Cell::new(None),
            subscriptions: RefCell::new(Vec::new()),
            watchdog: RefCell::new(None),
            failure: callback_failure,
        });
        let style = GlassStyle::default()
            .with_tint(GlassColor::rgba(0.07, 0.14, 0.18, 0.97).map_err(ShellError::setup)?)
            .with_border(Some(
                GlassBorder::new(
                    1.,
                    GlassColor::rgba(0.6, 0.8, 0.85, 0.4).map_err(ShellError::setup)?,
                )
                .map_err(ShellError::setup)?,
            ));
        for index in 0..2 {
            let descriptor = descriptor.clone();
            let session_for_content = session.clone();
            let content_style = style.clone();
            let overlay = Overlay::new()
                .keyboard_interactivity(if index == 0 {
                    KeyboardInteractivity::OnDemand
                } else {
                    KeyboardInteractivity::None
                })
                .content_limit(size(px(560.), px(600.)));
            let overlay = if index == 0 {
                overlay.dismiss_on_backdrop()
            } else {
                overlay
            };
            let overlay = overlay.outputs(DisplaySelection::AllConnected);
            let handle = overlay.mount(
                &runtime,
                move |instance, weak, window, cx| {
                    let runtime = weak.upgrade().ok_or_else(|| {
                        ShellError::setup(io::Error::other("runtime already stopped"))
                    })?;
                    let binding = runtime.mount(&descriptor, cx)?;
                    let session = session_for_content.clone();
                    let display = instance.display_id();
                    println!(
                        "OVERLAY_CREATED index={index} display={display:?}; new content lease"
                    );
                    Ok(cx.new(|cx| {
                        let subscription = binding.reactive().observe(cx, |_, _, cx| cx.notify());
                        let focus = cx.focus_handle();
                        if index == 0 {
                            focus.focus(window, cx);
                        }
                        Content {
                            session,
                            display,
                            index,
                            binding,
                            _subscription: subscription,
                            clicks: 0,
                            focus,
                            glass: MountedGlass::new(content_style.clone(), cx),
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
                    app_id: Some("bkss-static-overlay".into()),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(780.), px(550.)),
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
                && session.control.get().is_some_and(|h| h.window_id() == id)
            {
                session.control.take();
                session.stop(cx);
            }
        });
        let weak = Rc::downgrade(&session);
        let quit = cx.on_app_quit(move |cx| {
            if let Some(session) = weak.upgrade() {
                let control = session.control.take();
                if let Some(control) = control {
                    let _ = control.update(cx, |_, window, _| window.remove_window());
                }
            }
            std::future::ready(())
        });
        session.subscriptions.borrow_mut().extend([closed, quit]);
        let weak_runtime = runtime.downgrade();
        let watchdog = cx.spawn(async move |cx| {
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
        });
        *session.watchdog.borrow_mut() = Some(watchdog);
        Ok(())
    });
    if let Some(error) = failure.borrow_mut().take() {
        if let Err(secondary) = result {
            eprintln!("CLEANUP_SECONDARY {secondary}");
        }
        return Err(error);
    }
    result
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
            eprintln!("static_overlay: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_are_bounded_and_repeated_displays_rejected() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|s| (*s).to_owned()));
        assert_eq!(parse(&[]).unwrap().seconds, 120);
        assert_eq!(
            parse(&["--display", "1", "--display", "2"])
                .unwrap()
                .displays
                .len(),
            2
        );
        for args in [
            vec!["--seconds", "0"],
            vec!["--seconds", "301"],
            vec!["--seconds", "1", "--seconds", "2"],
            vec!["--display", "1", "--display", "1"],
            vec!["--display"],
            vec!["--other", "1"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
}

//! `mise x -- cargo run -p bkss-ui --example animated_overlay -- --seconds 180`
//! `mise x -- cargo run -p bkss-ui --example animated_overlay -- --help` prints help without launching.
use std::{
    cell::{Cell, RefCell},
    io,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use bkss_core::{
    GlassBorder, GlassColor, GlassInnerEffect, GlassStyle, MountedBinding, Shell, ShellError,
    Source, SourceDescriptor, SourcePublisher, WeakRuntimeHandle,
};
use bkss_ui::effects::{GlassState, MountedGlass};
use bkss_ui::{KeyboardInteractivity, Overlay, SpringPreset, SurfaceHandle, Transition};
use gpui::{
    App, AppContext, Bounds, Context, DisplayId, Render, Task, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, size,
};

const USAGE: &str = "animated_overlay [--seconds 1..300]
Default: 180-second watchdog. No overlay opens until you click Open.
Choose a connected display and open Default, Slow, or Spring.
Default uses the unchanged 160ms ease-out fade. Slow combines fade + upward slide
over 1200ms with ease-out; Spring combines fade + upward slide with a Snappy spring.
Slow and Spring move content within a fixed viewport; click or scroll at its moving
position, not its old position. Default and Slow have a fixed dim backdrop;
Spring has no backdrop. Close + reopen sends Open after 600ms for Slow, 80ms otherwise,
so you can watch an unfinished exit reverse without remounting the card.
Stop and Fail source end the shell without waiting for exit.
Escape dismisses the receiving overlay only when actually delivered.
Backdrop is not animated. No scale, activation, focus restoration, or ordering guarantee.
Run alone on the compositor; this program does not change compositor settings.";

fn parse_seconds(args: impl IntoIterator<Item = String>) -> io::Result<u64> {
    let mut args = args.into_iter();
    let Some(flag) = args.next() else {
        return Ok(180);
    };
    if flag != "--seconds" {
        return Err(io::Error::other("expected --seconds"));
    }
    let seconds = args
        .next()
        .ok_or_else(|| io::Error::other("--seconds needs a value"))?
        .parse()
        .map_err(io::Error::other)?;
    if args.next().is_some() || !(1..=300).contains(&seconds) {
        return Err(io::Error::other("use --seconds once, between 1 and 300"));
    }
    Ok(seconds)
}

#[derive(Default)]
struct Counts {
    mounts: Cell<u64>,
    drops: Cell<u64>,
    starts: Cell<u64>,
    stops: Cell<u64>,
}
struct Sample {
    counts: Rc<Counts>,
    publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
}
struct Guard(Rc<Counts>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.stops.set(self.0.stops.get() + 1);
        println!("RESOURCE_STOP count={}", self.0.stops.get());
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
        println!("RESOURCE_START count={}", self.counts.starts.get());
        Ok(Guard(self.counts.clone()))
    }
}

const NAMES: [&str; 3] = ["Default", "Slow", "Spring"];
const MOTIONS: [&str; 3] = [
    "fade, 160ms ease-out",
    "fade + slide up, 1200ms ease-out",
    "fade + slide up, Snappy spring",
];
fn transition(index: usize) -> Transition {
    match index {
        0 => Transition::fade().duration(Duration::from_millis(160)),
        1 => Transition::fade()
            .and(Transition::slide_up())
            .duration(Duration::from_millis(1200)),
        _ => Transition::fade()
            .and(Transition::slide_up())
            .spring(SpringPreset::Snappy),
    }
}

struct Session {
    runtime: WeakRuntimeHandle,
    handles: RefCell<Vec<SurfaceHandle>>,
    counts: Rc<Counts>,
    publisher: Rc<RefCell<Option<SourcePublisher<Sample>>>>,
    script: RefCell<Option<Task<()>>>,
    watchdog: RefCell<Option<Task<()>>>,
}
impl Session {
    fn request(&self, index: usize, display: DisplayId, open: bool, cx: &mut App) {
        let handle = self.handles.borrow().get(index).cloned();
        if let Some(handle) = handle {
            println!(
                "REQUEST profile={} motion={} display={display:?} open={open}",
                NAMES[index], MOTIONS[index]
            );
            let result = if open {
                handle.show(display, cx)
            } else {
                handle.hide(display, cx)
            };
            if let Err(error) = result {
                eprintln!("REQUEST_ERROR {error}");
            }
        }
        cx.refresh_windows();
    }
    fn reopen(self: &Rc<Self>, index: usize, display: DisplayId, cx: &mut App) {
        self.request(index, display, false, cx);
        let delay = if index == 1 { 600 } else { 80 };
        println!(
            "REOPEN profile={} display={display:?} delay_ms={delay} keeps_content_if_exit_unfinished=true",
            NAMES[index]
        );
        let weak = Rc::downgrade(self);
        // This timer scripts a user request, not animation sampling or frames.
        let task = cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            cx.update(|cx| {
                if let Some(session) = weak.upgrade() {
                    session.request(index, display, true, cx);
                }
            });
        });
        *self.script.borrow_mut() = Some(task);
    }
    fn stop(&self, cx: &mut App) {
        println!("STOP_REQUEST");
        self.script.borrow_mut().take();
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.shutdown(cx);
        }
    }
    fn fail(&self, cx: &mut App) {
        let publisher = self.publisher.borrow().clone();
        if let Some(publisher) = publisher {
            println!("FAIL_REQUEST");
            publisher.fail(
                io::Error::other("requested native animation test failure"),
                cx,
            );
        } else {
            println!("WAIT open an overlay before failing its source");
        }
    }
    fn counts(&self) -> String {
        format!(
            "Content: {} created / {} dropped   ·   Resource: {} starts / {} stops",
            self.counts.mounts.get(),
            self.counts.drops.get(),
            self.counts.starts.get(),
            self.counts.stops.get()
        )
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    let label = label.into();
    let action = Rc::new(action);
    let keyboard_action = action.clone();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x62526e))
        .text_color(rgb(0xf3eef7))
        .hover(|style| style.bg(rgb(0x7a6388)))
        .focus(|style| style.bg(rgb(0x4b6473)))
        .child(label)
        .on_click(move |_, _, cx| action(cx))
        .on_key_down(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                keyboard_action(cx);
            }
        })
}
fn controls(session: &Rc<Session>, index: usize, display: DisplayId) -> impl IntoElement {
    let open = session.clone();
    let close = session.clone();
    let reopen = session.clone();
    let stop = session.clone();
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .child(button("open", "Open", move |cx| {
            let session = open.clone();
            cx.defer(move |cx| session.request(index, display, true, cx));
        }))
        .child(button("close", "Close", move |cx| {
            let session = close.clone();
            cx.defer(move |cx| session.request(index, display, false, cx));
        }))
        .child(button("reopen", "Close + reopen", move |cx| {
            let session = reopen.clone();
            cx.defer(move |cx| session.reopen(index, display, cx));
        }))
        .child(button("stop", "Stop shell", move |cx| {
            let session = stop.clone();
            cx.defer(move |cx| session.stop(cx));
        }))
}

struct Controls(Rc<Session>);
impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut displays = div().flex().flex_col().gap_4();
        for (position, display) in cx.displays().into_iter().enumerate() {
            let mut profiles = div().flex().flex_col().gap_2();
            for (index, name) in NAMES.iter().enumerate() {
                profiles = profiles.child(
                    div()
                        .id(("profile", index))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(format!("{name} — {}", MOTIONS[index]))
                        .child(controls(&self.0, index, display.id())),
                );
            }
            displays = displays.child(
                div()
                    .id(("display", position))
                    .p_4()
                    .rounded_lg()
                    .bg(rgb(0xd5cddd))
                    .child(format!("Display {:?}", display.id()))
                    .child(profiles),
            );
        }
        div().id("animation-controls").size_full().overflow_y_scroll().p_6().bg(rgb(0xc7bfd3))
            .text_color(rgb(0x292638)).font_family("DejaVu Sans").flex().flex_col().gap_4()
            .child(div().font_family("DejaVu Serif").text_size(px(30.)).child("Watch the whole card fade and slide"))
            .child("Open an overlay to begin. Default fades only. Slow and Spring also slide: click and scroll at the card's moving position. The dim backdrop stays fixed; Spring has none.")
            .child(displays)
            .child(div().font_family("DejaVu Sans Mono").text_size(px(12.)).child(self.0.counts()))
            .child(button("refresh", "Refresh displays / counts", |cx| cx.refresh_windows()))
            .child("Use one compositor test at a time. Escape depends on actual key delivery. Scroll belongs to the card, not the wrapper.")
    }
}

struct Card {
    session: Rc<Session>,
    display: DisplayId,
    index: usize,
    serial: u64,
    _binding: MountedBinding<()>,
    glass: MountedGlass,
}
impl Drop for Card {
    fn drop(&mut self) {
        self.session
            .counts
            .drops
            .set(self.session.counts.drops.get() + 1);
        println!(
            "CONTENT_DROP profile={} display={:?} serial={} count={}",
            NAMES[self.index],
            self.display,
            self.serial,
            self.session.counts.drops.get()
        );
    }
}
impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fail = self.session.clone();
        let mut swatches = div().flex().gap_2();
        for (index, color) in [0xc0d8cf, 0xe5bd79, 0xb5a3d4, 0x8cafd0]
            .into_iter()
            .enumerate()
        {
            swatches = swatches.child(
                div()
                    .id(("swatch", index))
                    .w(px(72.))
                    .h(px(38.))
                    .rounded_md()
                    .bg(rgb(color)),
            );
        }
        self.glass.element(GlassState::default(), cx)
            .w(px(510.)).max_w_full().p_5().font_family("DejaVu Sans").text_color(rgb(0xf3eef7))
            .flex().flex_col().gap_3()
            .child(div().flex().justify_between()
                .child(format!("{} / {:?}", NAMES[self.index], self.display))
                .child(div().text_color(rgb(0xe5bd79)).child("◒  ☁  ✨")))
            .child(div().font_family("DejaVu Serif").text_size(px(31.)).child(
                if self.index == 0 { "One card. One fade." } else { "One card. Fade and slide." }
            ))
            .child(swatches)
            .child(if self.index == 0 {
                "Text, colored shapes, Glass edges, and inset shadows should fade together. Size and pointer regions stay unchanged."
            } else {
                "Text, colored shapes, Glass edges, and inset shadows fade and slide together. Click and scroll at the moving card, not its old position."
            })
            .child(controls(&self.session, self.index, self.display))
            .child(div().font_family("DejaVu Sans Mono").text_size(px(12.)).child(format!("Content #{} · stays mounted during exit and reopen", self.serial)))
            .child(div().id("card-scroll").h(px(108.)).overflow_y_scroll().bg(rgb(0x363044)).rounded_md().px_3()
                .children((1..=12).map(|row| div().h(px(27.)).flex_shrink_0().child(format!("Scroll sample {row:02}  —  text stays readable")))))
            .child(button("fail", "Fail source and stop", move |cx| { let session = fail.clone(); cx.defer(move |cx| session.fail(cx)); }))
            .child("Reopen before exit completes to keep the same content number. After a completed close, Open creates a new number. Console counters record resource lifetime.")
    }
}

fn run(seconds: u64) -> Result<(), ShellError> {
    Shell::run(move |runtime, cx| {
        let counts = Rc::new(Counts::default());
        let publisher = Rc::new(RefCell::new(None));
        let source = SourceDescriptor::new(Sample {
            counts: counts.clone(),
            publisher: publisher.clone(),
        });
        let session = Rc::new(Session {
            runtime: runtime.downgrade(),
            handles: RefCell::new(Vec::new()),
            counts,
            publisher,
            script: RefCell::new(None),
            watchdog: RefCell::new(None),
        });
        let style = GlassStyle::default()
            .with_tint(GlassColor::rgba(0.16, 0.14, 0.22, 0.98).map_err(ShellError::setup)?)
            .with_border(Some(
                GlassBorder::new(
                    1.,
                    GlassColor::rgba(0.78, 0.72, 0.87, 0.8).map_err(ShellError::setup)?,
                )
                .map_err(ShellError::setup)?,
            ))
            .with_inner_shadow(Some(
                GlassInnerEffect::new(
                    GlassColor::rgba(0., 0., 0., 0.5).map_err(ShellError::setup)?,
                    [0., 3.],
                    10.,
                    0.,
                )
                .map_err(ShellError::setup)?,
            ));
        for (index, name) in NAMES.into_iter().enumerate() {
            let mut overlay = Overlay::new()
                .keyboard_interactivity(KeyboardInteractivity::OnDemand)
                .content_limit(size(px(550.), px(650.)))
                .transition(transition(index));
            if index != 2 {
                overlay = overlay.dismiss_on_backdrop();
            }
            let captured = session.clone();
            let source = source.clone();
            let content_style = style.clone();
            let handle = overlay.mount(
                &runtime,
                move |instance, weak, _, cx| {
                    let runtime = weak
                        .upgrade()
                        .ok_or_else(|| ShellError::setup(io::Error::other("runtime stopped")))?;
                    let binding = runtime.mount(&source, cx)?;
                    let display = instance.display_id();
                    let serial = captured.counts.mounts.get() + 1;
                    captured.counts.mounts.set(serial);
                    println!(
                        "CONTENT_MOUNT profile={} motion={} display={display:?} serial={serial}",
                        name, MOTIONS[index]
                    );
                    Ok(cx.new(|cx| Card {
                        session: captured.clone(),
                        display,
                        index,
                        serial,
                        _binding: binding,
                        glass: MountedGlass::new(content_style.clone(), cx),
                    }))
                },
                cx,
            )?;
            session.handles.borrow_mut().push(handle);
        }
        let captured = session.clone();
        cx.open_window(
            WindowOptions {
                app_id: Some("bkss-animated-overlay".into()),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(760.), px(680.)),
                    cx,
                ))),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Controls(captured)),
        )
        .map_err(|error| ShellError::setup(io::Error::other(error)))?;
        let weak = Rc::downgrade(&session);
        let watchdog = cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(seconds))
                .await;
            cx.update(|cx| {
                if let Some(session) = weak.upgrade() {
                    cx.defer(move |cx| session.stop(cx));
                }
            });
        });
        *session.watchdog.borrow_mut() = Some(watchdog);
        Ok(())
    })
}

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match parse_seconds(std::env::args().skip(1))
        .map_err(Box::<dyn std::error::Error>::from)
        .and_then(|seconds| run(seconds).map_err(Into::into))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("animated_overlay: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seconds_are_bounded_and_arguments_rejected_before_launch() {
        let parse = |args: &[&str]| parse_seconds(args.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap(), 180);
        assert_eq!(parse(&["--seconds", "300"]).unwrap(), 300);
        for args in [
            vec!["--seconds"],
            vec!["--seconds", "0"],
            vec!["--seconds", "301"],
            vec!["--seconds", "1", "--seconds", "2"],
            vec!["--other", "1"],
        ] {
            assert!(parse(&args).is_err());
        }
    }

    #[test]
    fn default_stays_fade_only_while_slow_and_spring_move_the_measured_card() {
        struct SizedContent;
        impl Render for SizedContent {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(px(100.)).h(px(80.))
            }
        }
        for index in 0..NAMES.len() {
            let cx = gpui::TestAppContext::single();
            let instance = Rc::new(RefCell::new(None));
            let captured = instance.clone();
            let runtime = cx.update(|cx| {
                let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
                let handle = Overlay::new()
                    .transition(transition(index))
                    .mount(
                        &runtime,
                        move |instance, _, _, cx| {
                            *captured.borrow_mut() = Some(instance);
                            Ok(cx.new(|_| SizedContent))
                        },
                        cx,
                    )
                    .unwrap();
                handle.show(cx.displays()[0].id(), cx).unwrap();
                runtime
            });
            cx.run_until_parked();
            let window = cx.update(|cx| cx.windows()[0]);
            cx.simulate_window_resize(window, size(px(400.), px(300.)));
            cx.run_until_parked();
            let initial = instance.borrow().as_ref().unwrap().animation_sample();
            assert_eq!(initial.opacity, Some(0.));
            assert_eq!(initial.translation_x, None);
            assert_eq!(
                initial.translation_y,
                if index == 0 { None } else { Some(80.) }
            );
            let elapsed = match index {
                0 => 160,
                1 => 600,
                _ => 80,
            };
            cx.executor().advance_clock(Duration::from_millis(elapsed));
            let mut visual = gpui::VisualTestContext::from_window(window, &cx);
            visual.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            let sample = instance.borrow().as_ref().unwrap().animation_sample();
            if index == 0 {
                assert_eq!(sample.opacity, Some(1.));
                assert!(!sample.running);
            } else {
                assert!(sample.opacity.unwrap() > 0. && sample.opacity.unwrap() < 1.);
                assert!(sample.translation_y.unwrap() > 0. && sample.translation_y.unwrap() < 80.);
                assert!(sample.running);
            }
            cx.update(|cx| runtime.shutdown(cx));
        }
    }
}

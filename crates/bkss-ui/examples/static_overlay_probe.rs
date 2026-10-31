//! Experimental native observations, not the production Overlay implementation.
//! Run with `mise x -- cargo run -p bkss-ui --example static_overlay_probe -- --help`.
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt, io,
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use bkss_core::{RuntimeHandle, Shell};
use bkss_ui::{BottomBar, BottomBarGeometry, BottomBarWidth, DisplaySelection, RuntimeHandleExt};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, DisplayId, FocusHandle, Pixels, Render,
    Size, Subscription, Task, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, canvas, div,
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
    point,
    prelude::*,
    px, rgb, rgba, size,
};

const USAGE: &str = "static_overlay_probe --case keyboard|geometry|stacking
    [--display ID] [--seconds 1..300] [--layer top|overlay]
    [--keyboard none|on-demand|exclusive] [--backdrop off|full|bar-hole|shared-scene]
Defaults: primary (or first ready) display, 120 seconds, overlay, keyboard none, backdrop off.
--keyboard applies only to keyboard; --backdrop applies only to stacking.
Geometry offers display-sized / zero-sized stretch requests and zone 0 / -1 in the panel.
Only clicking the ordinary control panel creates trials. Exclusive can take keyboard focus.
The watchdog starts at process setup; closing the control panel shuts down all owned windows.
--help prints this text without starting GPUI. No compositor settings are changed.
Hyprland / single-seat observations only; no BKSS focus arbitration or OS guarantee.
UNSUPPORTED: runtime keyboard-mode setter and seat identification at the pinned GPUI API.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Keyboard,
    Geometry,
    Stacking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backdrop {
    Off,
    Full,
    BarHole,
    SharedScene,
}

#[derive(Clone, Debug)]
struct Args {
    case: Case,
    display: Option<DisplayId>,
    seconds: u64,
    keyboard: KeyboardInteractivity,
    layer: Layer,
    backdrop: Backdrop,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> io::Result<Args> {
    let mut parsed = Args {
        case: Case::Keyboard,
        display: None,
        seconds: 120,
        keyboard: KeyboardInteractivity::None,
        layer: Layer::Overlay,
        backdrop: Backdrop::Off,
    };
    let mut seen = HashSet::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err(io::Error::other(format!("repeated option: {flag}")));
        }
        if !matches!(
            flag.as_str(),
            "--case" | "--display" | "--seconds" | "--keyboard" | "--layer" | "--backdrop"
        ) {
            return Err(io::Error::other(format!("unknown option: {flag}")));
        }
        let value = args
            .next()
            .ok_or_else(|| io::Error::other(format!("{flag} needs a value")))?;
        match (flag.as_str(), value.as_str()) {
            ("--case", "keyboard") => parsed.case = Case::Keyboard,
            ("--case", "geometry") => parsed.case = Case::Geometry,
            ("--case", "stacking") => parsed.case = Case::Stacking,
            ("--display", _) => {
                parsed.display = Some(DisplayId::new(value.parse().map_err(io::Error::other)?));
            }
            ("--seconds", _) => {
                parsed.seconds = value.parse().map_err(io::Error::other)?;
                if !(1..=300).contains(&parsed.seconds) {
                    return Err(io::Error::other("seconds must be 1..=300"));
                }
            }
            ("--keyboard", "none") => parsed.keyboard = KeyboardInteractivity::None,
            ("--keyboard", "on-demand") => parsed.keyboard = KeyboardInteractivity::OnDemand,
            ("--keyboard", "exclusive") => parsed.keyboard = KeyboardInteractivity::Exclusive,
            ("--layer", "top") => parsed.layer = Layer::Top,
            ("--layer", "overlay") => parsed.layer = Layer::Overlay,
            ("--backdrop", "off") => parsed.backdrop = Backdrop::Off,
            ("--backdrop", "full") => parsed.backdrop = Backdrop::Full,
            ("--backdrop", "bar-hole") => parsed.backdrop = Backdrop::BarHole,
            ("--backdrop", "shared-scene") => parsed.backdrop = Backdrop::SharedScene,
            _ => return Err(io::Error::other(format!("invalid value for {flag}"))),
        }
    }
    if !seen.contains("--case") {
        return Err(io::Error::other("--case is required"));
    }
    if parsed.case != Case::Keyboard && seen.contains("--keyboard") {
        return Err(io::Error::other("--keyboard applies only to keyboard"));
    }
    if parsed.case != Case::Stacking && seen.contains("--backdrop") {
        return Err(io::Error::other("--backdrop applies only to stacking"));
    }
    Ok(parsed)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn intersection(a: Bounds<Pixels>, b: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let left = a.left().max(b.left());
    let top = a.top().max(b.top());
    let right = a.right().min(b.right());
    let bottom = a.bottom().min(b.bottom());
    (right > left && bottom > top)
        .then(|| Bounds::new(point(left, top), size(right - left, bottom - top)))
}

fn subtract(area: Bounds<Pixels>, hole: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    let Some(hole) = intersection(area, hole) else {
        return vec![area];
    };
    [
        Bounds::new(area.origin, size(area.size.width, hole.top() - area.top())),
        Bounds::new(
            point(area.left(), hole.bottom()),
            size(area.size.width, area.bottom() - hole.bottom()),
        ),
        Bounds::new(
            point(area.left(), hole.top()),
            size(hole.left() - area.left(), hole.size.height),
        ),
        Bounds::new(
            point(hole.right(), hole.top()),
            size(area.right() - hole.right(), hole.size.height),
        ),
    ]
    .into_iter()
    .filter(|r| r.size.width > px(0.) && r.size.height > px(0.))
    .collect()
}

// Wayland uses integer logical coordinates. Inward rounding must not make an
// invisible margin interactive, even at fractional scale or a clipped viewport.
fn integer_region(area: Bounds<Pixels>, viewport: Size<Pixels>) -> Option<Bounds<Pixels>> {
    let values = [
        f32::from(area.left()),
        f32::from(area.top()),
        f32::from(area.right()),
        f32::from(area.bottom()),
        f32::from(viewport.width),
        f32::from(viewport.height),
    ];
    if values.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // The largest exactly representable f32 below i32::MAX avoids a saturating
    // native cast rounding the upper boundary outward.
    let maximum = 2_147_483_520.;
    let left = values[0].max(0.).ceil().min(maximum);
    let top = values[1].max(0.).ceil().min(maximum);
    let right = values[2].min(values[4]).min(maximum).floor();
    let bottom = values[3].min(values[5]).min(maximum).floor();
    (right > left && bottom > top).then(|| rect(left, top, right - left, bottom - top))
}

fn backdrop_pieces(
    mode: Backdrop,
    viewport: Size<Pixels>,
    content: Bounds<Pixels>,
    bar: Option<Bounds<Pixels>>,
) -> Vec<Bounds<Pixels>> {
    if mode == Backdrop::Off {
        return Vec::new();
    }
    let mut pieces = subtract(Bounds::new(point(px(0.), px(0.)), viewport), content);
    if matches!(mode, Backdrop::BarHole | Backdrop::SharedScene)
        && let Some(bar) = bar
    {
        pieces = pieces.into_iter().flat_map(|r| subtract(r, bar)).collect();
    }
    pieces
}

fn native_regions(
    mode: Backdrop,
    viewport: Size<Pixels>,
    content: Bounds<Pixels>,
    bar: Option<Bounds<Pixels>>,
) -> Vec<Bounds<Pixels>> {
    let mut regions: Vec<_> = integer_region(content, viewport).into_iter().collect();
    regions.extend(
        backdrop_pieces(mode, viewport, content, bar)
            .into_iter()
            .filter_map(|r| integer_region(r, viewport)),
    );
    if mode == Backdrop::SharedScene
        && let Some(bar) = bar
    {
        regions.extend(
            subtract(bar, content)
                .into_iter()
                .filter_map(|r| integer_region(r, viewport)),
        );
    }
    regions
}

fn reference_geometry() -> BottomBarGeometry {
    BottomBarGeometry {
        width: BottomBarWidth::FullDisplayWidth,
        horizontal_padding: 0.,
        ..Default::default()
    }
}

fn bar_box(viewport: Size<Pixels>) -> Bounds<Pixels> {
    let g = reference_geometry();
    Bounds::new(
        point(
            px(g.margins.left),
            viewport.height - px(g.margins.bottom + g.logical_height),
        ),
        size(
            (viewport.width - px(g.margins.left + g.margins.right)).max(px(0.)),
            px(g.logical_height),
        ),
    )
}

fn content_box(viewport: Size<Pixels>, case: Case, name: &str) -> Bounds<Pixels> {
    let width = (f32::from(viewport.width) - 48.)
        .clamp(0., if case == Case::Geometry { 760. } else { 540. });
    let height = (f32::from(viewport.height) - 160.)
        .clamp(0., if case == Case::Geometry { 440. } else { 300. });
    let x = if name == "B" {
        (f32::from(viewport.width) - width - 24.).max(0.)
    } else {
        24.
    };
    rect(x, 72., width, height)
}

fn keyboard_open_target<'a>(
    recipient: &str,
    occupied: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    let peer = match recipient {
        "A" => "B",
        "B" => "A",
        _ => return None,
    };
    (!occupied.into_iter().any(|name| name == peer)).then_some(peer)
}

#[derive(Debug)]
struct ProbeError {
    operation: &'static str,
    cause: Box<dyn Error>,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation, self.cause)
    }
}

impl Error for ProbeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

struct Owner {
    args: Args,
    runtime: RuntimeHandle,
    stopped: Cell<bool>,
    display: Cell<Option<DisplayId>>,
    root: Cell<Option<AnyWindowHandle>>,
    witness: Cell<Option<AnyWindowHandle>>,
    trials: RefCell<BTreeMap<&'static str, AnyWindowHandle>>,
    pending: RefCell<BTreeMap<&'static str, u64>>,
    serial: Cell<u64>,
    reference_started: Cell<bool>,
    measured_bar: Rc<Cell<Option<Bounds<Pixels>>>>,
    error: Rc<RefCell<Option<ProbeError>>>,
    monitor: RefCell<Option<Task<()>>>,
    monitor_active: Cell<bool>,
    subscriptions: RefCell<Vec<Subscription>>,
}

impl Owner {
    fn fail(self: &Rc<Self>, operation: &'static str, cause: impl Error + 'static, cx: &mut App) {
        if self.error.borrow().is_none() {
            *self.error.borrow_mut() = Some(ProbeError {
                operation,
                cause: Box::new(cause),
            });
        }
        // A failed launch can originate in the control window's own handler.
        // Remove that window only after its current update has released it.
        let owner = self.clone();
        cx.defer(move |cx| owner.stop("failure", cx));
    }

    fn stop(&self, reason: &str, cx: &mut App) {
        if self.stopped.replace(true) {
            return;
        }
        println!("SHUTDOWN reason={reason}");
        self.pending.borrow_mut().clear();
        let trials = std::mem::take(&mut *self.trials.borrow_mut());
        let witness = self.witness.take();
        let root = self.root.take();
        if !self.monitor_active.get() {
            let task = self.monitor.borrow_mut().take();
            drop(task);
        }
        for handle in trials.into_values().chain(witness).chain(root) {
            if let Err(error) = handle.update(cx, |_, window, _| window.remove_window()) {
                eprintln!(
                    "CLEANUP_SECONDARY window={:?} cause={error:#}",
                    handle.window_id()
                );
            }
        }
        self.runtime.shutdown(cx);
    }

    fn close(&self, name: &'static str, cx: &mut App) {
        self.pending.borrow_mut().remove(name);
        let handle = self.trials.borrow_mut().remove(name);
        if let Some(handle) = handle {
            println!("TRIAL_CLOSE name={name}");
            if let Err(error) = handle.update(cx, |_, window, _| window.remove_window()) {
                eprintln!("CLEANUP_SECONDARY name={name} cause={error:#}");
            }
        }
        cx.refresh_windows();
    }

    fn close_recipient(&self, name: &'static str, window: &mut Window, cx: &mut App) {
        self.pending.borrow_mut().remove(name);
        self.trials.borrow_mut().remove(name);
        println!("TRIAL_CLOSE name={name} recipient=true");
        window.remove_window();
        cx.refresh_windows();
    }

    fn options(&self, title: &str, cx: &App) -> WindowOptions {
        WindowOptions {
            app_id: Some("bkss-static-overlay-probe".into()),
            display_id: self.display.get(),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(title.to_owned().into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                self.display.get(),
                size(px(840.), px(620.)),
                cx,
            ))),
            ..Default::default()
        }
    }

    fn open_controls(self: &Rc<Self>, cx: &mut App) {
        let owner = self.clone();
        let opened = cx.open_window(
            self.options("BKSS · Native observation desk", cx),
            |_, cx| cx.new(|_| Controls { owner }),
        );
        match opened {
            Ok(handle) if !self.stopped.get() => self.root.set(Some(handle.into())),
            Ok(handle) => {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            Err(error) => self.fail("open control panel", io::Error::other(error), cx),
        }
    }

    fn open_witness(self: &Rc<Self>, cx: &mut App) {
        if self.stopped.get() || self.witness.get().is_some() {
            return;
        }
        let mut options = self.options("BKSS · Ordinary witness (place behind trial)", cx);
        options.focus = false;
        options.window_bounds = Some(WindowBounds::Maximized(Bounds::centered(
            self.display.get(),
            size(px(1100.), px(760.)),
            cx,
        )));
        match cx.open_window(options, |_, cx| cx.new(|_| Witness { clicks: 0 })) {
            Ok(handle) if !self.stopped.get() => self.witness.set(Some(handle.into())),
            Ok(handle) => {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            Err(error) => self.fail("open ordinary witness", io::Error::other(error), cx),
        }
    }

    fn reference(self: &Rc<Self>, cx: &mut App) {
        if self.stopped.get() || self.reference_started.replace(true) {
            return;
        }
        let Some(display) = self.display.get() else {
            return;
        };
        let bar = BottomBar::new(reference_geometry())
            .map(|bar| bar.with_displays(DisplaySelection::Explicit(vec![display])));
        let bar = match bar {
            Ok(bar) => bar,
            Err(error) => {
                self.fail("declare reference bar", io::Error::other(error), cx);
                return;
            }
        };
        let measured = self.measured_bar.clone();
        let result = self.runtime.mount_bottom_bar(
            bar,
            move |display, window, cx| {
                println!("BAR_CREATED display={display:?} layer=Top reservation=52 source=unchanged-BottomBar");
                window.set_input_region(Some(&[]));
                let measured = measured.clone();
                cx.new(|_| Reference {
                    display,
                    measured,
                    clicks: 0,
                })
            },
            cx,
        );
        if let Err(error) = result {
            self.fail("mount reference Top BottomBar", error, cx);
        }
    }

    fn bar(&self, mode: Backdrop, viewport: Size<Pixels>) -> Option<Bounds<Pixels>> {
        match mode {
            Backdrop::BarHole => Some(self.measured_bar.get().unwrap_or_else(|| bar_box(viewport))),
            Backdrop::SharedScene => Some(bar_box(viewport)),
            Backdrop::Off | Backdrop::Full => None,
        }
    }

    fn open_keyboard_peer(self: &Rc<Self>, recipient: &'static str, cx: &mut App) {
        if self.args.case != Case::Keyboard || self.stopped.get() {
            return;
        }
        let target = {
            let trials = self.trials.borrow();
            let pending = self.pending.borrow();
            keyboard_open_target(recipient, trials.keys().chain(pending.keys()).copied())
        };
        if let Some(target) = target {
            self.open_trial(target, -1, false, cx);
        } else {
            println!(
                "KEYBOARD_OPEN_SKIPPED from={recipient} reason=peer-open-or-pending activation_requested=false recreation=false"
            );
        }
    }

    fn open_trial(self: &Rc<Self>, name: &'static str, zone: i32, stretch: bool, cx: &mut App) {
        if self.stopped.get() || self.error.borrow().is_some() {
            return;
        }
        if self.trials.borrow().contains_key(name) {
            self.close(name, cx);
        }
        if self.pending.borrow().contains_key(name) {
            return;
        }
        if self.args.case != Case::Keyboard {
            self.open_witness(cx);
            if self.args.backdrop != Backdrop::SharedScene {
                self.reference(cx);
            }
        }
        if self.stopped.get() || self.error.borrow().is_some() {
            return;
        }
        let Some(display) = self
            .display
            .get()
            .and_then(|id| cx.displays().into_iter().find(|d| d.id() == id))
        else {
            self.fail(
                "resolve trial output",
                io::Error::other("selected output disconnected"),
                cx,
            );
            return;
        };
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        self.pending.borrow_mut().insert(name, serial);
        let requested = if stretch {
            size(px(0.), px(0.))
        } else {
            display.bounds().size
        };
        println!(
            "TRIAL_REQUEST name={name} serial={serial} display={:?} full={:?} visible={:?} requested={requested:?} zone={zone} layer={:?} keyboard={:?} backdrop={:?}",
            display.id(),
            display.bounds(),
            display.visible_bounds(),
            self.args.layer,
            self.args.keyboard,
            self.args.backdrop
        );
        let owner = self.clone();
        let opened = cx.open_window(
            WindowOptions {
                display_id: Some(display.id()),
                app_id: Some("bkss-static-overlay-probe".into()),
                titlebar: None,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    requested,
                ))),
                focus: false,
                window_background: WindowBackgroundAppearance::Transparent,
                kind: WindowKind::LayerShell(LayerShellOptions {
                    namespace: format!("bkss-static-probe-{name}"),
                    layer: self.args.layer,
                    anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                    exclusive_zone: Some(px(zone as f32)),
                    exclusive_edge: None,
                    margin: None,
                    keyboard_interactivity: self.args.keyboard,
                }),
                ..Default::default()
            },
            |window, cx| {
                window.set_input_region(Some(&[]));
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    // This selects GPUI's internal key recipient, not native activation.
                    focus.focus(window, cx);
                    Trial {
                        owner,
                        name,
                        serial,
                        focus,
                        content_clicks: 0,
                        keys: 0,
                        last_key: "—",
                        bar_clicks: 0,
                        last_layout: Rc::new(Cell::new(None)),
                        zone,
                        stretch,
                    }
                })
            },
        );
        let current = self.pending.borrow().get(name).copied() == Some(serial);
        if current {
            self.pending.borrow_mut().remove(name);
        }
        match opened {
            Ok(handle) if current && !self.stopped.get() => {
                self.trials.borrow_mut().insert(name, handle.into());
                println!("TRIAL_OPEN name={name} serial={serial}");
            }
            Ok(handle) => {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            Err(error) => self.fail("create trial layer surface", io::Error::other(error), cx),
        }
        cx.refresh_windows();
    }
}

fn button(
    id: &'static str,
    label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label = label.into();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .cursor_pointer()
        .px_4()
        .py_2()
        .rounded_md()
        .bg(rgb(0x284956))
        .hover(|s| s.bg(rgb(0x376576)))
        .child(label)
        .on_click(move |_, window, cx| action(window, cx))
}

fn panel() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_5()
        .rounded_lg()
        .bg(rgb(0x192b35))
}

struct Controls {
    owner: Rc<Owner>,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let o = &self.owner;
        let mut actions = div().flex().flex_wrap().gap_3();
        match o.args.case {
            Case::Keyboard => {
                for name in ["A", "B"] {
                    let owner = o.clone();
                    let open = o.trials.borrow().contains_key(name);
                    actions = actions.child(button(
                        name,
                        format!("{} {name}", if open { "Close" } else { "Open" }),
                        move |_, cx| {
                            if open {
                                owner.close(name, cx);
                            } else {
                                owner.open_trial(name, -1, false, cx);
                            }
                        },
                    ));
                }
            }
            Case::Geometry => {
                for (id, label, zone, stretch) in [
                    ("display-0", "Display size · zone 0", 0, false),
                    ("stretch-0", "Stretch · zone 0", 0, true),
                    ("display-minus1", "Display size · zone -1", -1, false),
                    ("stretch-minus1", "Stretch · zone -1", -1, true),
                ] {
                    let owner = o.clone();
                    actions = actions.child(button(id, label, move |_, cx| {
                        owner.open_trial("geometry", zone, stretch, cx)
                    }));
                }
                let owner = o.clone();
                actions = actions.child(button("close-geometry", "Close trial", move |_, cx| {
                    owner.close("geometry", cx)
                }));
            }
            Case::Stacking => {
                let owner = o.clone();
                actions = actions.child(button(
                    "open-stacking",
                    "Open / reopen trial",
                    move |_, cx| owner.open_trial("stacking", -1, false, cx),
                ));
                let owner = o.clone();
                actions = actions.child(button("close-stacking", "Close trial", move |_, cx| {
                    owner.close("stacking", cx)
                }));
            }
        }
        let owner = o.clone();
        let witness = button("witness", "Open ordinary witness", move |_, cx| {
            owner.open_witness(cx)
        });
        let owner = o.clone();
        let quit = button("quit", "Stop everything", move |_, cx| {
            let owner = owner.clone();
            cx.defer(move |cx| owner.stop("stop-button", cx));
        });
        let instructions = match o.args.case {
            Case::Keyboard => {
                "1  Open A, then use Open B from A inside its card.\n2  Press A / B without clicking B. Read the actual KEY recipient.\n3  Escape closes its recipient; press again without clicking A.\n4  Repeat in reverse order. None receives no keys: use the in-card close/stop buttons."
            }
            Case::Geometry => {
                "Launch one variant at a time. Compare edge markers and VIEWPORT logs.\nA reserving, unchanged Top BottomBar is mounted on first launch.\nStretch requests zero width and height with all four anchors.\nCompare the clipped and scrollable oversized samples inside the card."
            }
            Case::Stacking => {
                "Place the ordinary witness behind the trial using your window controls.\nClick content, then outside, then the bar. Reopen and compare counters.\nOFF: full transparent allocation; only content accepts pointer input.\nFULL: outside pointer-down is consumed and closes this trial."
            }
        };
        div().id("desk").size_full().overflow_y_scroll().bg(rgb(0x101d26))
            .text_color(rgb(0xe8eff0)).font_family("sans-serif").text_size(px(14.))
            .p_6().flex().flex_col().gap_4()
            .child(div().text_color(rgb(0xf2bc68)).text_size(px(12.)).child("EXPERIMENT / HYPRLAND · SINGLE SEAT"))
            .child(div().font_family("serif").text_size(px(32.)).child("Native observation desk"))
            .child(format!("{:?}   /   display {:?}   /   {:?}   /   keyboard {:?}   /   backdrop {:?}", o.args.case, o.display.get(), o.args.layer, o.args.keyboard, o.args.backdrop))
            .child(panel().child("Start deliberately").child(format!("No native trial opens until you click below. Watchdog: {} seconds from setup.", o.args.seconds)).child(actions))
            .child(panel().child(instructions))
            .child(div().text_color(rgb(0xa8bdc5)).child("UNSUPPORTED: runtime keyboard-mode setter; seat identification. Creation modes only. Active-window flags are observations, not evidence of key delivery. No BKSS arbitration or compositor guarantee."))
            .child(if o.args.backdrop == Backdrop::SharedScene { "Shared scene uses a mock, non-reserving embedded bar in the same root. No independent BottomBar is mounted; this does not preserve production bar independence." } else { "Same-layer ordering is unspecified. A painted/input hole does not establish native surface ordering. BAR-HOLE maps measured reference content to the display bottom; native global surface origin is unavailable, so alignment is approximate." })
            .child(div().flex().gap_3().child(witness).child(quit))
    }
}

struct Witness {
    clicks: u64,
}

impl Render for Witness {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().id("ordinary-witness").size_full().bg(rgb(0x315650)).text_color(rgb(0xe8eff0))
            .p_6().flex().flex_col().gap_4().font_family("sans-serif")
            .on_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.clicks += 1;
                println!("WITNESS_POINTER count={} local={:?} viewport={:?}", this.clicks, event.position, window.viewport_size());
                cx.notify();
            }))
            .child(div().font_family("serif").text_size(px(36.)).child("Ordinary window witness"))
            .child("This is a separate normal application window owned by the probe. Place it behind the trial; compositor placement is not controlled here.")
            .child(format!("WITNESS_POINTER   {}", self.clicks))
            .child("Only a pointer event delivered here increases this count. Green showing through a hole is not proof of native ordering.")
    }
}

struct Reference {
    display: DisplayId,
    measured: Rc<Cell<Option<Bounds<Pixels>>>>,
    clicks: u64,
}

impl Render for Reference {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.measured.clone();
        let display = self.display;
        let width = (window.viewport_size().width - px(24.)).max(px(0.));
        div().id("reference-bar").relative().w(width).h(px(40.)).overflow_hidden()
            .bg(rgb(0xa56b32)).text_color(rgb(0xffffff)).font_family("sans-serif").text_size(px(14.))
            .flex().items_center().justify_center().cursor_pointer()
            .on_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.clicks += 1;
                println!("BAR_POINTER source=independent-Top count={} local={:?} viewport={:?}", this.clicks, event.position, window.viewport_size());
                cx.stop_propagation();
                cx.notify();
            }))
            .child(format!("Independent Top BottomBar · reserves 52 logical px · BAR_POINTER {}", self.clicks))
            .child(canvas(move |bounds, window, cx| {
                if let Some(display) = cx.displays().into_iter().find(|d| d.id() == display) {
                    let mut mapped = bounds;
                    mapped.origin.y += display.bounds().size.height - window.viewport_size().height;
                    if measured.replace(Some(mapped)) != Some(mapped) {
                        println!("BAR_VISUAL local={bounds:?} viewport={:?} output_local_estimate={mapped:?} basis=bottom-anchor-native-origin-unavailable", window.viewport_size());
                        cx.refresh_windows();
                    }
                }
            }, |_, (), _, _| {}).absolute().inset_0().size_full())
    }
}

type LayoutSnapshot = (Size<Pixels>, f32, Option<Bounds<Pixels>>);

struct Trial {
    owner: Rc<Owner>,
    name: &'static str,
    serial: u64,
    focus: FocusHandle,
    content_clicks: u64,
    keys: u64,
    last_key: &'static str,
    bar_clicks: u64,
    last_layout: Rc<Cell<Option<LayoutSnapshot>>>,
    zone: i32,
    stretch: bool,
}

fn place(bounds: Bounds<Pixels>) -> gpui::Div {
    div()
        .absolute()
        .left(bounds.origin.x)
        .top(bounds.origin.y)
        .w(bounds.size.width)
        .h(bounds.size.height)
}

fn oversized(scroll: bool) -> impl IntoElement {
    let sample = div()
        .flex()
        .flex_col()
        .gap_2()
        .w(px(1200.))
        .h(px(280.))
        .children((0..8).map(|row| {
            div().flex().gap_2().children((0..12).map(move |column| {
                div()
                    .flex_shrink_0()
                    .w(px(90.))
                    .h(px(28.))
                    .bg(rgb(if (row + column) % 2 == 0 {
                        0x38616b
                    } else {
                        0x28434e
                    }))
                    .child(format!("{column}:{row}"))
            }))
        }));
    let box_ = div()
        .id(if scroll {
            "scroll-sample"
        } else {
            "clip-sample"
        })
        .w_full()
        .h(px(105.))
        .flex_shrink_0();
    if scroll {
        box_.overflow_scroll().child(sample)
    } else {
        box_.overflow_hidden().child(sample)
    }
}

impl Render for Trial {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let case = self.owner.args.case;
        let mode = if case == Case::Stacking {
            self.owner.args.backdrop
        } else {
            Backdrop::Off
        };
        let content = content_box(viewport, case, self.name);
        let bar = self.owner.bar(mode, viewport);
        let mut root = div()
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .overflow_hidden()
            .text_color(rgb(0xe8eff0))
            .font_family("sans-serif")
            .text_size(px(14.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = match event.keystroke.key.as_str() {
                    "a" | "A" => "A",
                    "b" | "B" => "B",
                    "escape" => "Escape",
                    _ => "other-redacted",
                };
                this.keys += 1;
                this.last_key = key;
                println!(
                    "KEY recipient={} serial={} count={} key={key} active_observation={}",
                    this.name,
                    this.serial,
                    this.keys,
                    window.is_window_active()
                );
                cx.stop_propagation();
                cx.notify();
                if key == "Escape" {
                    this.owner.close_recipient(this.name, window, cx);
                }
            }));
        for (index, piece) in backdrop_pieces(mode, viewport, content, bar)
            .into_iter()
            .enumerate()
        {
            let owner = self.owner.clone();
            let name = self.name;
            root = root.child(place(piece).id(("backdrop", index)).bg(rgba(0x07111bb8))
                .on_any_mouse_down(move |event, window, cx| {
                    println!("BACKDROP_POINTER recipient={name} count=1 local={:?} consumed=true close=true", event.position);
                    cx.stop_propagation();
                    owner.close_recipient(name, window, cx);
                }));
        }
        if case == Case::Geometry {
            for x in (0..f32::from(viewport.width).clamp(0., 32768.) as usize).step_by(100) {
                root = root.child(
                    div()
                        .absolute()
                        .left(px(x as f32))
                        .top_0()
                        .h_full()
                        .w(px(1.))
                        .bg(rgba(0x9bd9de66)),
                );
            }
            for y in (0..f32::from(viewport.height).clamp(0., 32768.) as usize).step_by(100) {
                root = root.child(
                    div()
                        .absolute()
                        .top(px(y as f32))
                        .left_0()
                        .w_full()
                        .h(px(1.))
                        .bg(rgba(0x9bd9de66)),
                );
            }
            root = root
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bg(rgb(0x3b6470))
                        .child("LOCAL (0,0) · 100 logical px grid"),
                )
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .bg(rgb(0x3b6470))
                        .child(format!("LOCAL bottom-right {viewport:?}")),
                );
        }
        let mut card = place(content).id("trial-content").p_5().rounded_lg().bg(rgb(0x1e3542)).border_1().border_color(rgb(0x70a9b5))
            .flex().flex_col().gap_3().overflow_y_scroll()
            .on_any_mouse_down(cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.content_clicks += 1;
                this.focus.focus(window, cx);
                println!("CONTENT_POINTER recipient={} serial={} count={} local={:?} active_observation={}", this.name, this.serial, this.content_clicks, event.position, window.is_window_active());
                cx.stop_propagation();
                cx.notify();
            }))
            .child(div().text_color(rgb(0xf2bc68)).child(format!("NATIVE TRIAL / {} / #{}", self.name, self.serial)));
        if case == Case::Keyboard
            && let Some(peer) = keyboard_open_target(self.name, [])
        {
            let owner = self.owner.clone();
            let name = self.name;
            let open_peer = button(
                "open-peer",
                format!("Open {peer} from {name}"),
                move |_, cx| {
                    let owner = owner.clone();
                    // Do not create/remove native windows inside the recipient's update.
                    cx.defer(move |cx| owner.open_keyboard_peer(name, cx));
                },
            );
            let owner = self.owner.clone();
            let close_peer = button("close-peer", format!("Close {peer}"), move |_, cx| {
                let owner = owner.clone();
                cx.defer(move |cx| owner.close(peer, cx));
            });
            let owner = self.owner.clone();
            let close_self = button(
                "close-recipient",
                format!("Close {name}"),
                move |window, cx| {
                    owner.close_recipient(name, window, cx);
                },
            );
            let owner = self.owner.clone();
            let stop = button("stop-from-trial", "Stop everything", move |_, cx| {
                let owner = owner.clone();
                cx.defer(move |cx| owner.stop("trial-stop-button", cx));
            });
            card = card
                .child(div().flex().flex_col().gap_2().flex_shrink_0()
                    .child(div().flex().flex_wrap().gap_2().child(open_peer).child(close_peer))
                    .child(div().flex().flex_wrap().gap_2().child(close_self).child(stop)))
                .child(div().text_size(px(12.)).flex_shrink_0().child(format!(
                    "Open {peer}, then press A / B without clicking it. Escape closes the actual recipient; press again without clicking the previous card. Already open? Open is a no-op; use Close {peer} first."
                )));
        }
        card = card
            .child(
                div()
                    .font_family("serif")
                    .text_size(px(28.))
                    .child(match case {
                        Case::Keyboard => "Who received the key?",
                        Case::Geometry => "Allocation is not usable bounds",
                        Case::Stacking => "Separate paint from input",
                    }),
            )
            .child(format!(
                "CONTENT_POINTER {} · active observation {}",
                self.content_clicks,
                window.is_window_active()
            ))
            .child(format!(
                "KEY received {} · last {}",
                self.keys, self.last_key
            ))
            .child(format!(
                "Viewport {viewport:?} · scale {} · zone {} · {}",
                window.scale_factor(),
                self.zone,
                if self.stretch {
                    "requested 0 × 0"
                } else {
                    "requested display size"
                }
            ));
        card = match case {
            Case::Keyboard => card.child(format!("Creation mode {:?}. None receives no keys: use Close or Stop everything. No runtime setter or seat identification; single-seat observations only.", self.owner.args.keyboard)).child("Native allocation remains display-sized; use the controls in this card even if the original panel is covered. Only KEY events establish delivery; no focus activation is requested by Open."),
            Case::Geometry => card.child("CLIPPED · oversized 1200 × 280 logical px").child(oversized(false)).child("SCROLLABLE · same content; wheel / horizontal scroll").child(oversized(true)),
            Case::Stacking => card.child(format!("Backdrop {mode:?}. Content clicks never dismiss. Outside pointer-down dismisses only when a backdrop is enabled.")).child("BAR-HOLE uses approximate bottom-anchored mapping of the measured reference visual. If content overlaps that band, content keeps its input area. No physical ordering guarantee."),
        };
        let name = self.name;
        if case != Case::Keyboard {
            let owner = self.owner.clone();
            card = card.child(button(
                "close-recipient",
                "Close this trial",
                move |window, cx| owner.close_recipient(name, window, cx),
            ));
        }
        root = root.child(card);
        if mode == Backdrop::SharedScene {
            let mock = bar_box(viewport);
            // Content retains its full hit area even on a very short output.
            for (index, piece) in subtract(mock, content).into_iter().enumerate() {
                root = root.child(
                    place(piece)
                        .id(("embedded-bar", index))
                        .bg(rgb(0xa56b32))
                        .overflow_hidden()
                        .on_any_mouse_down(cx.listener(
                            |this, event: &gpui::MouseDownEvent, _, cx| {
                                this.bar_clicks += 1;
                                println!(
                                    "BAR_POINTER source=embedded-mock count={} local={:?}",
                                    this.bar_clicks, event.position
                                );
                                cx.stop_propagation();
                                cx.notify();
                            },
                        ))
                        .child(format!(
                            "MOCK · same root · no reservation · BAR_POINTER {}",
                            self.bar_clicks
                        )),
                );
            }
        }
        let last_layout = self.last_layout.clone();
        let owner = self.owner.clone();
        let serial = self.serial;
        root.child(canvas(move |_, window, cx| {
            let viewport = window.viewport_size();
            let content = content_box(viewport, case, name);
            let bar = owner.bar(mode, viewport);
            let regions = native_regions(mode, viewport, content, bar);
            window.set_input_region(Some(&regions));
            let observation = (viewport, window.scale_factor(), bar);
            if last_layout.replace(Some(observation)) != Some(observation) {
                let display = owner.display.get().and_then(|id| cx.displays().into_iter().find(|d| d.id() == id));
                println!("VIEWPORT name={name} serial={serial} local_logical={viewport:?} scale={} physical_estimate={:?} full={:?} visible={:?} active_observation={} input_logical={regions:?} bar_estimate={bar:?} configure_is_not_usable_bounds=true", window.scale_factor(), size(f32::from(viewport.width) * window.scale_factor(), f32::from(viewport.height) * window.scale_factor()), display.as_ref().map(|d| d.bounds()), display.as_ref().map(|d| d.visible_bounds()), window.is_window_active());
            }
        }, |_, (), _, _| {}).absolute().inset_0().size_full())
    }
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let error = Rc::new(RefCell::new(None));
    let app_error = error.clone();
    let retained = Rc::new(RefCell::new(None));
    let app_retained = retained.clone();
    let shell_result = Shell::run(move |runtime, cx| {
        let owner = Rc::new(Owner {
            args,
            runtime,
            stopped: Cell::new(false),
            display: Cell::new(None),
            root: Cell::new(None),
            witness: Cell::new(None),
            trials: RefCell::new(BTreeMap::new()),
            pending: RefCell::new(BTreeMap::new()),
            serial: Cell::new(0),
            reference_started: Cell::new(false),
            measured_bar: Rc::new(Cell::new(None)),
            error: app_error,
            monitor: RefCell::new(None),
            monitor_active: Cell::new(false),
            subscriptions: RefCell::new(Vec::new()),
        });
        *app_retained.borrow_mut() = Some(owner.clone());
        let weak = Rc::downgrade(&owner);
        let closed = cx.on_window_closed(move |cx, id| {
            let Some(owner) = weak.upgrade() else {
                return;
            };
            if owner.root.get().is_some_and(|h| h.window_id() == id) {
                owner.root.take();
                owner.stop("control-window-closed", cx);
            } else {
                if owner.witness.get().is_some_and(|h| h.window_id() == id) {
                    owner.witness.take();
                }
                owner.trials.borrow_mut().retain(|_, h| h.window_id() != id);
                cx.refresh_windows();
            }
        });
        let weak = Rc::downgrade(&owner);
        let quit = cx.on_app_quit(move |cx| {
            if let Some(owner) = weak.upgrade() {
                owner.stop("app-quit", cx);
            }
            std::future::ready(())
        });
        owner.subscriptions.borrow_mut().extend([closed, quit]);
        let weak = Rc::downgrade(&owner);
        let started = cx.background_executor().now();
        let task = cx.spawn(async move |cx| {
            loop {
                // Resolve only after the event loop can receive display announcements.
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let Some(owner) = weak.upgrade() else {
                    break;
                };
                if owner.stopped.get() {
                    break;
                }
                owner.monitor_active.set(true);
                let elapsed = cx.background_executor().now() - started;
                cx.update(|cx| {
                    if elapsed >= Duration::from_secs(owner.args.seconds) {
                        if owner.root.get().is_none() {
                            owner.fail(
                                "display/control readiness",
                                io::Error::other("watchdog expired before control panel was ready"),
                                cx,
                            );
                        } else {
                            owner.stop("watchdog", cx);
                        }
                    } else if owner.display.get().is_none() {
                        let displays = cx.displays();
                        let display = if let Some(id) = owner.args.display {
                            displays.iter().find(|d| d.id() == id).cloned()
                        } else {
                            cx.primary_display()
                                .filter(|d| displays.iter().any(|ready| ready.id() == d.id()))
                                .or_else(|| displays.first().cloned())
                        };
                        if let Some(display) = display {
                            println!(
                                "DISPLAY_READY selected={:?} full={:?} visible={:?} connected={:?}",
                                display.id(),
                                display.bounds(),
                                display.visible_bounds(),
                                displays.iter().map(|d| d.id()).collect::<Vec<_>>()
                            );
                            owner.display.set(Some(display.id()));
                            owner.open_controls(cx);
                        } else if elapsed >= Duration::from_secs(5) {
                            owner.fail(
                                "display readiness",
                                io::Error::other(format!(
                                    "no matching display after 5 seconds (requested {:?})",
                                    owner.args.display
                                )),
                                cx,
                            );
                        }
                    }
                });
                owner.monitor_active.set(false);
                if owner.stopped.get() {
                    break;
                }
            }
        });
        *owner.monitor.borrow_mut() = Some(task);
        Ok(())
    });
    // The first probe failure remains primary even if shell teardown also fails.
    if let Some(error) = error.borrow_mut().take() {
        if let Err(secondary) = shell_result {
            eprintln!("CLEANUP_SECONDARY shell={secondary}");
        }
        return Err(Box::new(error));
    }
    shell_result?;
    Ok(())
}

fn main() -> ExitCode {
    if std::env::args().skip(1).any(|arg| arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = parse_args(std::env::args().skip(1)).map_err(Box::<dyn Error>::from).and_then(|args| {
        println!("PROBE_START case={:?} seconds={} native_trials=click-to-start observations_only=true", args.case, args.seconds);
        println!("UNSUPPORTED runtime_keyboard_mode_setter; seat_identification; no_BKSS_arbitration");
        run(args)
    });
    match result {
        Ok(()) => {
            println!("PROBE_STOPPED");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("static_overlay_probe: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> io::Result<Args> {
        parse_args(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn arguments_are_explicit_bounded_and_case_specific() {
        let args = parse(&["--case", "keyboard"]).unwrap();
        assert_eq!(args.keyboard, KeyboardInteractivity::None);
        assert_eq!(args.seconds, 120);
        assert_eq!(args.display, None);
        assert_eq!(
            parse(&[
                "--case",
                "stacking",
                "--backdrop",
                "shared-scene",
                "--layer",
                "top",
                "--display",
                "7"
            ])
            .unwrap()
            .backdrop,
            Backdrop::SharedScene
        );
        for args in [
            vec![],
            vec!["--case"],
            vec!["--case", "other"],
            vec!["--case", "keyboard", "--case", "geometry"],
            vec!["--case", "geometry", "--keyboard", "none"],
            vec!["--case", "keyboard", "--backdrop", "off"],
            vec!["--case", "keyboard", "--seconds", "0"],
            vec!["--case", "keyboard", "--seconds", "301"],
            vec!["--case", "keyboard", "--seconds", "1", "--seconds", "2"],
            vec!["--case", "keyboard", "--display", "-1"],
            vec!["--case", "keyboard", "--display", "1", "--display", "2"],
            vec!["--case", "keyboard", "--layer", "bottom"],
            vec!["--case", "keyboard", "--keyboard", "dynamic"],
            vec!["--case", "stacking", "--backdrop", "unknown"],
            vec!["--case", "keyboard", "--unknown", "x"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    fn area(r: Bounds<Pixels>) -> f32 {
        f32::from(r.size.width) * f32::from(r.size.height)
    }

    #[test]
    fn keyboard_peer_actions_open_only_the_other_absent_trial() {
        for (recipient, peer) in [("A", "B"), ("B", "A")] {
            assert_eq!(keyboard_open_target(recipient, []), Some(peer));
            assert_eq!(keyboard_open_target(recipient, [recipient]), Some(peer));
            assert_eq!(keyboard_open_target(recipient, [peer]), None);
            assert_eq!(keyboard_open_target(recipient, [recipient, peer]), None);
            // The live/pending names use one occupancy check; either blocks recreation.
            assert_eq!(
                keyboard_open_target(recipient, [recipient].into_iter().chain([peer])),
                None
            );
            assert_ne!(recipient, peer);
        }
        assert_eq!(keyboard_open_target("geometry", []), None);
        assert_eq!(keyboard_open_target("stacking", []), None);
    }

    #[test]
    fn subtraction_is_nonoverlapping_and_never_inverts_or_expands() {
        let allocation = rect(0., 0., 100., 80.);
        for hole in [
            rect(20., 10., 40., 30.),
            rect(-20., -20., 40., 40.),
            rect(0., 0., 100., 80.),
            rect(200., 0., 10., 10.),
            rect(50., 40., 0., 0.),
        ] {
            let pieces = subtract(allocation, hole);
            for (i, piece) in pieces.iter().enumerate() {
                assert!(piece.size.width > px(0.) && piece.size.height > px(0.));
                assert_eq!(intersection(*piece, allocation), Some(*piece));
                assert!(intersection(*piece, hole).is_none());
                for other in &pieces[i + 1..] {
                    assert!(intersection(*piece, *other).is_none());
                }
            }
            assert_eq!(
                pieces.into_iter().map(area).sum::<f32>(),
                area(allocation) - intersection(allocation, hole).map(area).unwrap_or(0.)
            );
        }
    }

    #[test]
    fn regions_round_inward_clamp_and_keep_off_transparency_noninteractive() {
        let viewport = size(px(100.), px(80.));
        assert_eq!(
            integer_region(rect(-5.2, 1.2, 40.9, 90.), viewport),
            Some(rect(0., 2., 35., 78.))
        );
        assert_eq!(integer_region(rect(99.2, 1., 1., 10.), viewport), None);
        assert_eq!(integer_region(rect(f32::NAN, 0., 1., 1.), viewport), None);
        assert_eq!(
            integer_region(rect(0., 0., f32::INFINITY, 1.), viewport),
            None
        );
        let content = rect(10.2, 12.2, 30.6, 20.6);
        assert_eq!(
            native_regions(Backdrop::Off, viewport, content, None),
            vec![rect(11., 13., 29., 19.)]
        );
        assert!(backdrop_pieces(Backdrop::Off, viewport, content, None).is_empty());
        for mode in [Backdrop::Full, Backdrop::BarHole, Backdrop::SharedScene] {
            let hole = rect(4.5, 24.5, 90., 30.);
            let regions = native_regions(mode, viewport, content, Some(hole));
            assert!(regions.contains(&integer_region(content, viewport).unwrap()));
            for (i, region) in regions.iter().enumerate() {
                assert_eq!(
                    intersection(*region, rect(0., 0., 100., 80.)),
                    Some(*region)
                );
                for other in &regions[i + 1..] {
                    assert!(intersection(*region, *other).is_none());
                }
                if mode == Backdrop::BarHole {
                    for outside_content in subtract(*region, content) {
                        assert!(intersection(outside_content, hole).is_none());
                    }
                }
            }
        }
    }
}

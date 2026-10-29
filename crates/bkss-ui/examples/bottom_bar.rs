//! Run with `mise x -- cargo run -p bkss-ui --example bottom_bar -- --seconds 30`.
//! Use `--probe-window --no-reserve` for ordinary-window pointer pass-through checks.
use std::{io, process::ExitCode, time::Duration};

use bkss_core::{GlassBorder, GlassColor, GlassStyle, Shell, ShellError};
use bkss_ui::{
    BottomBar, BottomBarAlignment, BottomBarGeometry, BottomBarWidth, DisplaySelection,
    RuntimeHandleExt,
};
use gpui::{
    AppContext, Bounds, Context, DisplayId, MouseButton, Render, Window, WindowBounds,
    WindowOptions, canvas, div, prelude::*, px, rgb, size,
};

const USAGE: &str = "bottom_bar [--display ID ... | --all] [--full] [--no-reserve]
    [--align left|center|right] [--max-width N] [--left N] [--right N] [--bottom N]
    [--height N] [--padding N] [--seconds 1..300] [--probe-window] [--missing-display ID]
Defaults: all connected displays, floating centered bar, height 40, padding/margins 12,
reservation on, 30-second clean shutdown. Values are logical pixels.
--probe-window opens a visible normal app window behind the bar (opt-in, no input layer).
Use --no-reserve so it can extend below Glass; the compositor controls its placement.
--missing-display adds a second bar on that ID; use an unconnected ID to check fatal
readiness timeout and cleanup after 5 seconds. It does not change compositor settings.";

struct Args {
    geometry: BottomBarGeometry,
    displays: DisplaySelection,
    seconds: u64,
    probe: bool,
    missing: Option<DisplayId>,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, io::Error> {
    let mut geometry = BottomBarGeometry::default();
    let mut displays = Vec::new();
    let mut all = false;
    let mut seconds = 30;
    let mut probe = false;
    let mut missing = None;
    let mut alignment = BottomBarAlignment::Center;
    let mut maximum = None;
    let mut full = false;
    let mut seen = std::collections::HashSet::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        if flag != "--display" && !seen.insert(flag.clone()) {
            return Err(io::Error::other(format!("repeated flag: {flag}")));
        }
        match flag.as_str() {
            "--all" => all = true,
            "--full" => full = true,
            "--no-reserve" => geometry.reserve = false,
            "--probe-window" => probe = true,
            "--display" | "--seconds" | "--missing-display" | "--align" | "--max-width"
            | "--left" | "--right" | "--bottom" | "--height" | "--padding" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::other(format!("{flag} needs a value")))?;
                match flag.as_str() {
                    "--display" => displays.push(DisplayId::new(
                        value.parse::<u64>().map_err(io::Error::other)?,
                    )),
                    "--missing-display" => {
                        missing = Some(DisplayId::new(
                            value.parse::<u64>().map_err(io::Error::other)?,
                        ))
                    }
                    "--seconds" => {
                        seconds = value.parse().map_err(io::Error::other)?;
                        if !(1..=300).contains(&seconds) {
                            return Err(io::Error::other("seconds must be 1..=300"));
                        }
                    }
                    "--align" => {
                        alignment = match value.as_str() {
                            "left" => BottomBarAlignment::Left,
                            "center" => BottomBarAlignment::Center,
                            "right" => BottomBarAlignment::Right,
                            _ => {
                                return Err(io::Error::other(
                                    "alignment must be left, center, or right",
                                ));
                            }
                        }
                    }
                    _ => {
                        let number = value.parse().map_err(io::Error::other)?;
                        match flag.as_str() {
                            "--max-width" => maximum = Some(number),
                            "--left" => geometry.margins.left = number,
                            "--right" => geometry.margins.right = number,
                            "--bottom" => geometry.margins.bottom = number,
                            "--height" => geometry.logical_height = number,
                            _ => geometry.horizontal_padding = number,
                        }
                    }
                }
            }
            _ => return Err(io::Error::other(format!("unknown option: {flag}"))),
        }
    }
    if all && !displays.is_empty() {
        return Err(io::Error::other("choose --all or --display, not both"));
    }
    if full && (maximum.is_some() || seen.contains("--align")) {
        return Err(io::Error::other(
            "--align and --max-width apply only to a floating bar",
        ));
    }
    geometry.width = if full {
        BottomBarWidth::FullDisplayWidth
    } else {
        BottomBarWidth::FloatingContentWidth {
            max_logical_width: maximum,
            horizontal_alignment: alignment,
        }
    };
    geometry.validate().map_err(io::Error::other)?;
    Ok(Args {
        geometry,
        displays: if displays.is_empty() {
            DisplaySelection::AllConnected
        } else {
            DisplaySelection::Explicit(displays)
        },
        seconds,
        probe,
        missing,
    })
}

struct Controls {
    display: DisplayId,
    clicks: u64,
    logged_layout: bool,
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let log_layout = !self.logged_layout;
        self.logged_layout = true;
        let display = self.display;
        div().relative().flex().items_center().gap_4().text_color(rgb(0xf1f5f9))
            .font_family("sans-serif").text_size(px(14.)).whitespace_nowrap()
            .child(format!("Display {display:?}"))
            .child(div().id("pointer-check").role(gpui::Role::Button).aria_label("Count pointer clicks")
                .cursor_pointer().px_3().py_1().rounded_md().bg(rgb(0x335d6b))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.clicks += 1;
                    println!("BAR_CLICK display={:?} count={} active={} local={:?}", this.display, this.clicks, window.is_window_active(), window.mouse_position());
                    cx.notify();
                }))
                .child(format!("Pointer clicks: {}", self.clicks)))
            .child(canvas(move |bounds, window, _| {
                if log_layout { println!("BAR_CONTENT display={display:?} local={bounds:?} viewport={:?} scale={} active={}", window.viewport_size(), window.scale_factor(), window.is_window_active()); }
            }, |_, (), _, _| {}).absolute().inset_0().size_full())
    }
}

struct Probe {
    clicks: u64,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().id("behind-probe").size_full().bg(rgb(0x253642)).text_color(rgb(0xf1f5f9))
            .font_family("sans-serif").p_6().flex().flex_col().gap_3()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                this.clicks += 1;
                println!("PROBE_CLICK count={} local={:?} viewport={:?}", this.clicks, event.position, window.viewport_size());
                cx.notify();
            }))
            .child(div().text_size(px(24.)).child("Window behind the bar"))
            .child("Click the bar button, then the transparent sides and bottom margin.")
            .child("Only clicks delivered to this normal window increase its count.")
            .child("Use reservation off. If needed, place this window behind the bar using normal window controls.")
            .child(format!("Window clicks: {}", self.clicks))
    }
}

fn run(args: Args, bar: BottomBar) -> Result<(), ShellError> {
    Shell::run(move |runtime, cx| {
        if args.probe {
            let display = match &args.displays {
                DisplaySelection::Explicit(ids) => ids.first().copied(),
                DisplaySelection::AllConnected => None,
            };
            cx.open_window(
                WindowOptions {
                    app_id: Some("bkss-bottom-bar-probe".into()),
                    display_id: display,
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("BKSS bar pointer check".into()),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Maximized(Bounds::centered(
                        None,
                        size(px(1100.), px(700.)),
                        cx,
                    ))),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Probe { clicks: 0 }),
            )
            .map_err(|error| ShellError::setup(io::Error::other(error)))?;
        }
        runtime.mount_bottom_bar(
            bar,
            |display, _, cx| {
                println!("BAR_CREATED display={display:?}");
                cx.new(|_| Controls {
                    display,
                    clicks: 0,
                    logged_layout: false,
                })
            },
            cx,
        )?;
        if let Some(display) = args.missing {
            println!(
                "WAITING_FOR_DISPLAY {display:?}; an absent ID must fail the whole runtime after 5 seconds"
            );
            runtime.mount_bottom_bar(
                BottomBar::default().with_displays(DisplaySelection::Explicit(vec![display])),
                |_, _, cx| cx.new(|_| gpui::EmptyView),
                cx,
            )?;
        }
        let shutdown = runtime.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(args.seconds))
                .await;
            cx.update(|cx| {
                println!("TIMED_SHUTDOWN");
                shutdown.shutdown(cx);
            });
        })
        .detach();
        Ok(())
    })
}

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = (|| {
        let args = parse_args(std::env::args().skip(1))?;
        let style = GlassStyle::default()
            .with_tint(GlassColor::rgba(0.06, 0.12, 0.16, 0.94)?)
            .with_border(Some(GlassBorder::new(
                1.,
                GlassColor::rgba(0.6, 0.8, 0.9, 0.35)?,
            )?));
        let bar = BottomBar::new(args.geometry)?
            .with_style(style)
            .with_displays(args.displays.clone());
        println!(
            "BOTTOM_BAR geometry={:?} displays={:?} seconds={}; no compositor configuration changes",
            args.geometry, args.displays, args.seconds
        );
        run(args, bar)?;
        println!("BOTTOM_BAR_STOPPED");
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bottom_bar: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_cover_display_selection_geometry_and_reject_bad_input() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|arg| (*arg).to_owned()));
        assert!(matches!(
            parse(&[]).unwrap().displays,
            DisplaySelection::AllConnected
        ));
        let args = parse(&[
            "--display",
            "1",
            "--display",
            "2",
            "--no-reserve",
            "--align",
            "right",
            "--bottom",
            "20",
            "--max-width",
            "600",
        ])
        .unwrap();
        assert!(matches!(args.displays, DisplaySelection::Explicit(ids) if ids.len() == 2));
        assert!(!args.geometry.reserve);
        assert_eq!(args.geometry.margins.bottom, 20.);
        assert!(matches!(
            args.geometry.width,
            BottomBarWidth::FloatingContentWidth {
                max_logical_width: Some(600.),
                horizontal_alignment: BottomBarAlignment::Right
            }
        ));
        assert_eq!(
            parse(&["--full"]).unwrap().geometry.width,
            BottomBarWidth::FullDisplayWidth
        );
        for args in [
            vec!["--all", "--display", "1"],
            vec!["--height", "0"],
            vec!["--bottom", "NaN"],
            vec!["--left", "-1"],
            vec!["--seconds", "0"],
            vec!["--seconds", "301"],
            vec!["--full", "--align", "left"],
            vec!["--align", "other"],
            vec!["--padding"],
            vec!["--full", "--full"],
            vec!["--unknown"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}

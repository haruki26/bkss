use std::{io, process::ExitCode, time::Duration};

use bkss_core::{
    Glass, GlassBorder, GlassColor, GlassInnerEffect, GlassOpacity, GlassPatch, GlassRadius,
    GlassRim, GlassState, GlassStyle, GlassStyleError, Shell, ShellError, State, SurfaceDescriptor,
};
use gpui::{AppContext, Context, DisplayId, Render, Size, Task, Window, div, prelude::*, px, rgb};

const USAGE: &str = "usage: glass_surface --display ID [--seconds 30]\nUse the numeric DisplayId from list_displays. Duration must be 1..=300 seconds. This surface is passive and does not receive input.";
const PROPERTIES: [&str; 7] = [
    "radius",
    "opacity",
    "tint",
    "border",
    "rim",
    "inner highlight",
    "inner shadow",
];
const VALUES: [[&str; 2]; 7] = [
    ["radius 12 px", "radius 32 px"],
    ["opacity 1.00", "opacity 0.35"],
    ["blue tint · alpha 0.75", "purple tint · alpha 0.75"],
    ["border off", "border on · 2 px"],
    ["rim off", "rim on · inset 6 px, width 1.5 px"],
    ["inner highlight off", "inner highlight on"],
    ["inner shadow off", "inner shadow on"],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Args {
    display: DisplayId,
    seconds: u64,
}

fn parse_args(args: &[String]) -> Result<Args, &'static str> {
    let mut display = None;
    let mut seconds = None;
    let mut flags = args.iter();
    while let Some(flag) = flags.next() {
        let value = flags.next().ok_or("flag requires a value")?;
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("flag values must be unsigned decimal integers");
        }
        match flag.as_str() {
            "--display" if display.is_none() => {
                display = Some(DisplayId::new(
                    value.parse().map_err(|_| "invalid display ID")?,
                ))
            }
            "--seconds" if seconds.is_none() => {
                let value = value.parse().map_err(|_| "invalid seconds")?;
                if !(1..=300).contains(&value) {
                    return Err("seconds must be 1..=300");
                }
                seconds = Some(value);
            }
            _ => return Err("unknown or repeated flag"),
        }
    }
    Ok(Args {
        display: display.ok_or("--display ID is required")?,
        seconds: seconds.unwrap_or(30),
    })
}

struct Palette {
    base: GlassStyle,
    border: GlassBorder,
    rim: GlassRim,
    highlight: GlassInnerEffect,
    shadow: GlassInnerEffect,
    radii: [GlassRadius; 2],
    opacities: [GlassOpacity; 2],
    tints: [GlassColor; 2],
}

impl Palette {
    fn new() -> Result<Self, GlassStyleError> {
        let white = GlassColor::rgba(0.86, 0.96, 1., 0.85)?;
        let tints = [
            GlassColor::rgba(0.08, 0.22, 0.33, 0.75)?,
            GlassColor::rgba(0.28, 0.18, 0.42, 0.75)?,
        ];
        Ok(Self {
            base: GlassStyle::default().with_tint(tints[0]),
            border: GlassBorder::new(2., white)?,
            rim: GlassRim::new(6., 1.5, GlassColor::rgba(0.45, 0.85, 1., 0.9)?)?,
            highlight: GlassInnerEffect::new(white, [0., 3.], 7., 1.)?,
            shadow: GlassInnerEffect::new(
                GlassColor::rgba(0.01, 0.03, 0.09, 0.9)?,
                [0., -5.],
                12.,
                3.,
            )?,
            radii: [GlassRadius::new(12.)?, GlassRadius::new(32.)?],
            opacities: [GlassOpacity::new(1.)?, GlassOpacity::new(0.35)?],
            tints,
        })
    }

    fn full(&self) -> GlassStyle {
        self.base
            .clone()
            .with_border(Some(self.border))
            .with_rim(Some(self.rim))
            .with_inner_highlight(Some(self.highlight))
            .with_inner_shadow(Some(self.shadow))
    }
}

struct Samples {
    comparisons: Vec<(&'static str, Glass)>,
    opacity: [Glass; 2],
    previews: Glass,
    reactive: Glass,
    radius: State<GlassRadius>,
    alpha: State<GlassOpacity>,
    tint: State<GlassColor>,
    border: State<Option<GlassBorder>>,
    rim: State<Option<GlassRim>>,
    highlight: State<Option<GlassInnerEffect>>,
    shadow: State<Option<GlassInnerEffect>>,
    palette: Palette,
    tick: usize,
    _timer: Task<()>,
}

impl Samples {
    fn new(palette: Palette, cx: &mut Context<Self>) -> Self {
        let comparisons = [
            ("No effects", palette.base.clone()),
            (
                "Border · outer edge",
                palette.base.clone().with_border(Some(palette.border)),
            ),
            (
                "Rim · 6 px inset",
                palette.base.clone().with_rim(Some(palette.rim)),
            ),
            (
                "Inner highlight",
                palette
                    .base
                    .clone()
                    .with_inner_highlight(Some(palette.highlight)),
            ),
            (
                "Inner shadow",
                palette.base.clone().with_inner_shadow(Some(palette.shadow)),
            ),
            ("All effects", palette.full()),
        ]
        .into_iter()
        .map(|(label, style)| (label, Glass::new(style, cx)))
        .collect();
        let opacity = [
            Glass::new(palette.full().with_opacity(palette.opacities[0]), cx),
            Glass::new(palette.full().with_opacity(palette.opacities[1]), cx),
        ];
        let previews = Glass::new(
            palette
                .base
                .clone()
                .with_hover(GlassPatch::default().with_tint(palette.tints[1]))
                .with_pressed(GlassPatch::default().with_inner_shadow(Some(palette.shadow)))
                .with_focus(GlassPatch::default().with_border(Some(palette.border))),
            cx,
        );
        let radius = State::new(palette.radii[0], cx);
        let alpha = State::new(palette.opacities[0], cx);
        let tint = State::new(palette.tints[0], cx);
        let border = State::new(None, cx);
        let rim = State::new(None, cx);
        let highlight = State::new(None, cx);
        let shadow = State::new(None, cx);
        let reactive = Glass::new(
            palette
                .base
                .clone()
                .with_radius(radius.reactive())
                .with_opacity(alpha.reactive())
                .with_tint(tint.reactive())
                .with_border(border.reactive())
                .with_rim(rim.reactive())
                .with_inner_highlight(highlight.reactive())
                .with_inner_shadow(shadow.reactive()),
            cx,
        );
        let timer = cx.spawn(async move |owner, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if owner.update(cx, |owner, cx| owner.advance(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            comparisons,
            opacity,
            previews,
            reactive,
            radius,
            alpha,
            tint,
            border,
            rim,
            highlight,
            shadow,
            palette,
            tick: 0,
            _timer: timer,
        }
    }

    fn advance(&mut self, cx: &mut Context<Self>) {
        let property = self.tick % 7;
        let enabled = (self.tick / 7).is_multiple_of(2);
        let index = usize::from(enabled);
        match property {
            0 => self.radius.set(self.palette.radii[index], cx),
            1 => self.alpha.set(self.palette.opacities[index], cx),
            2 => self.tint.set(self.palette.tints[index], cx),
            3 => self.border.set(enabled.then_some(self.palette.border), cx),
            4 => self.rim.set(enabled.then_some(self.palette.rim), cx),
            5 => self
                .highlight
                .set(enabled.then_some(self.palette.highlight), cx),
            _ => self.shadow.set(enabled.then_some(self.palette.shadow), cx),
        }
        self.tick += 1;
        cx.notify();
    }
}

fn sample(
    glass: &Glass,
    label: &'static str,
    state: GlassState,
    cx: &impl AppContext,
) -> impl IntoElement {
    glass
        .element(state, cx)
        .flex_1()
        .min_w_0()
        .h(px(76.))
        .p_4()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .child(div().text_size(px(14.)).child(label))
        .child(div().h(px(3.)).w(px(56.)).bg(rgb(0x73dbf2)))
}

impl Render for Samples {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut comparisons = div().flex().flex_col().gap_3();
        for row in self.comparisons.chunks(3) {
            comparisons = comparisons.child(
                div().flex().gap_3().children(
                    row.iter()
                        .map(|(label, glass)| sample(glass, label, GlassState::default(), cx)),
                ),
            );
        }
        let status = if self.tick == 0 {
            "Initial values; radius changes first".to_owned()
        } else {
            format!(
                "Last change: {} · next: {}",
                VALUES[(self.tick - 1) % 7][usize::from(((self.tick - 1) / 7).is_multiple_of(2))],
                PROPERTIES[self.tick % 7]
            )
        };
        let previews = [
            ("Base", GlassState::default()),
            (
                "Hover",
                GlassState {
                    hovered: true,
                    ..Default::default()
                },
            ),
            (
                "Hover + pressed",
                GlassState {
                    hovered: true,
                    pressed: true,
                    focused: false,
                },
            ),
            (
                "All + focus",
                GlassState {
                    hovered: true,
                    pressed: true,
                    focused: true,
                },
            ),
        ];
        div().size_full().p_6().bg(rgb(0x34495b)).text_color(rgb(0xf2f6fb))
            .font_family("sans-serif").flex().flex_col().gap_3()
            .child(div().text_size(px(24.)).child("Glass decoration checks"))
            .child(div().text_size(px(13.)).child("Passive single-display surface · no background blur · no pointer or keyboard input"))
            .child(comparisons)
            .child(div().text_size(px(14.)).child("Decoration opacity — both text and cyan marks should stay equally bright"))
            .child(div().flex().gap_3()
                .child(sample(&self.opacity[0], "Opacity 1.00 · same child content", GlassState::default(), cx))
                .child(sample(&self.opacity[1], "Opacity 0.35 · same child content", GlassState::default(), cx)))
            .child(div().text_size(px(14.)).child("Reactive properties — one update every 2 seconds; no animation"))
            .child(sample(&self.reactive, "Same mounted Glass; independent reactive properties", GlassState::default(), cx))
            .child(div().text_size(px(12.)).child(status))
            .child(div().text_size(px(12.)).child("Order: radius → opacity → tint → border → rim → highlight → shadow; repeat to restore each value."))
            .child(div().text_size(px(14.)).child("Manual state-style previews — labels are simulated flags, not real input"))
            .child(div().flex().gap_3().children(previews.into_iter().map(|(label, state)| sample(&self.previews, label, state, cx))))
    }
}

fn run(args: Args, palette: Palette) -> Result<(), ShellError> {
    Shell::run(move |runtime, cx| {
        let shutdown = runtime.clone();
        let descriptor = SurfaceDescriptor::new(
            args.display,
            Size::new(px(980.), px(840.)),
            move |_window, cx| {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs(args.seconds))
                        .await;
                    cx.update(|cx| shutdown.shutdown(cx));
                })
                .detach();
                cx.new(|cx| Samples::new(palette, cx))
            },
        )?;
        runtime.mount_surface(descriptor, cx)
    })
}

fn main() -> ExitCode {
    let result = (|| {
        let args =
            parse_args(&std::env::args().skip(1).collect::<Vec<_>>()).map_err(io::Error::other)?;
        let palette = Palette::new()?;
        println!(
            "Glass on {:?} for {} seconds. Compare rounded edges and inset effects; use glass_interactive for real input checks.",
            args.display, args.seconds
        );
        run(args, palette)?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("glass_surface: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_sequence_changes_one_validated_property_at_a_time_then_restores_it() {
        let mut cx = gpui::TestAppContext::single();
        let owner = cx.update(|cx| cx.new(|cx| Samples::new(Palette::new().unwrap(), cx)));
        owner.update(&mut cx, |samples, cx| {
            let snapshot = |samples: &Samples, cx: &Context<Samples>| {
                (
                    samples.radius.reactive().get(cx),
                    samples.alpha.reactive().get(cx),
                    samples.tint.reactive().get(cx),
                    samples.border.reactive().get(cx),
                    samples.rim.reactive().get(cx),
                    samples.highlight.reactive().get(cx),
                    samples.shadow.reactive().get(cx),
                )
            };
            let initial = snapshot(samples, cx);
            for step in 0..14 {
                let before = snapshot(samples, cx);
                samples.advance(cx);
                let after = snapshot(samples, cx);
                let changes = [
                    before.0 != after.0,
                    before.1 != after.1,
                    before.2 != after.2,
                    before.3 != after.3,
                    before.4 != after.4,
                    before.5 != after.5,
                    before.6 != after.6,
                ];
                assert_eq!(changes.iter().filter(|changed| **changed).count(), 1);
                assert!(changes[step % 7]);
            }
            assert_eq!(snapshot(samples, cx), initial);
        });
    }

    #[test]
    fn requires_one_display_and_a_bounded_duration() {
        let parse = |args: &[&str]| {
            parse_args(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
        };
        assert_eq!(
            parse(&["--display", "4"]),
            Ok(Args {
                display: DisplayId::new(4),
                seconds: 30
            })
        );
        assert!(parse(&["--display", "0", "--seconds", "300"]).is_ok());
        for args in [
            vec![],
            vec!["--display"],
            vec!["--display", "-1"],
            vec!["--display", "4", "--display", "4"],
            vec!["--display", "4", "--seconds", "0"],
            vec!["--display", "4", "--seconds", "301"],
            vec!["--other", "1"],
        ] {
            assert!(parse(&args).is_err());
        }
        assert!(Palette::new().is_ok());
    }
}

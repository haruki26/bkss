use bkss_integration::Notification;
use gpui::{
    App, Context, IntoElement, SharedString, Window, div, prelude::*, px, rems, rgb, rgba, size,
};
use gpui::{
    AvailableSpace, Bounds, DefiniteLength, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Length, Pixels, Rems, ShapedLine, Size, TextStyle,
    WindowTextSystem,
};

use super::NotificationView;
use crate::ui::{
    feedback,
    icons::{self, Icon},
    shared::{label, muted, row},
    theme::{self, MUTED},
};
use bkss_ui::effects::GlassState;

const CARD_PADDING: Rems = rems(1.);

pub(super) fn app_name(notification: &Notification) -> String {
    if notification.app_name.trim().is_empty() {
        "アプリケーション".into()
    } else {
        notification.app_name.clone()
    }
}

fn notification_text_column(summary: gpui::Div, description: Option<gpui::Div>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            summary
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .line_clamp(2),
        )
        .children(description)
}

pub(super) fn notification_body_width(
    panel: &mut impl Styled,
    center_card_width: Option<Pixels>,
    window: &Window,
) -> Option<Pixels> {
    // A clickable card's title can make its flex column wider than the panel.
    // Use the center's fixed surface width rather than its latest viewport.
    let rem = window.rem_size();
    let width = match (center_card_width, panel.style().size.width) {
        (Some(width), _) => width,
        (None, Some(Length::Definite(DefiniteLength::Absolute(width)))) => width.to_pixels(rem),
        _ => return None,
    };
    let padding = window.pixel_snap(CARD_PADDING.to_pixels(rem));
    Some((window.pixel_snap(width) - padding * 2.).max(px(0.)))
}

fn notification_body(text: String, max_width: Option<Pixels>) -> gpui::Div {
    div()
        .text_size(px(13.))
        .text_color(rgb(theme::TEXT))
        .overflow_hidden()
        .when_some(max_width, |body, width| body.max_w(width))
        .child(NotificationBody(text.into()))
}

struct NotificationBody(SharedString);

impl IntoElement for NotificationBody {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for NotificationBody {
    type RequestLayoutState = ();
    type PrepaintState = [Option<ShapedLine>; 2];

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let text = self.0.clone();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = notification_body_line_height(&style, font_size, window);
        let id =
            window.request_measured_layout(Default::default(), move |known, space, window, _| {
                let width = known.width.unwrap_or(match space.width {
                    AvailableSpace::Definite(width) => width,
                    _ => Pixels::MAX,
                });
                let lines =
                    notification_body_lines(window.text_system(), &style, font_size, &text, width);
                lines
                    .iter()
                    .flatten()
                    .fold(Size::default(), |measured, line| {
                        size(
                            measured.width.max(line.width()).ceil(),
                            measured.height + line_height,
                        )
                    })
            });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        notification_body_lines(
            window.text_system(),
            &style,
            font_size,
            &self.0,
            bounds.size.width,
        )
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        lines: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = notification_body_line_height(&style, font_size, window);
        let mut origin = bounds.origin;
        for line in lines.iter().flatten() {
            let _ = line.paint(
                origin,
                line_height,
                style.text_align,
                Some(bounds.size.width),
                window,
                cx,
            );
            origin.y += line_height;
        }
    }
}

fn notification_body_line_height(style: &TextStyle, font_size: Pixels, window: &Window) -> Pixels {
    window.pixel_snap(
        style
            .line_height
            .to_pixels(font_size.into(), window.rem_size()),
    )
}

fn is_breakable_horizontal_space(c: char) -> bool {
    matches!(
        c,
        '\t' | ' ' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{205f}' | '\u{3000}'
    )
}

fn notification_body_lines(
    text_system: &WindowTextSystem,
    style: &TextStyle,
    font_size: Pixels,
    text: &str,
    width: Pixels,
) -> [Option<ShapedLine>; 2] {
    let shape = |text: &str| {
        text_system.shape_line(text.into(), font_size, &[style.to_run(text.len())], None)
    };
    let mut logical = text
        .splitn(3, '\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    let first_text = logical.next().unwrap_or_default();
    let mut first = shape(first_text);
    let mut second = logical.next().map(|text| (text, logical.next().is_some()));
    if first.width() > width {
        // GPUI's clamp doesn't share a row budget across logical lines. We only
        // need its first wrap boundary, and check the reshaped prefix because
        // contextual glyph advances can change at the break.
        let native = text_system
            .shape_text(
                first.text.clone(),
                font_size,
                &[style.to_run(first_text.len())],
                Some(width),
                Some(2),
            )
            .ok()
            .and_then(|lines| {
                let line = lines.first()?;
                let boundary = line.wrap_boundaries.first()?;
                // A visual-order break in bidirectional text may not be a source prefix.
                if !line
                    .runs()
                    .iter()
                    .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index))
                    .is_sorted()
                {
                    return None;
                }
                let end = line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                (end > 0).then(|| shape(&first_text[..end]))
            });
        first = native
            .filter(|line| line.width() <= width)
            .unwrap_or_else(|| fit_notification_line(&first, width, "", shape));
        let rest = &first_text[first.len()..];
        // Only breakable horizontal spaces carried over by a soft wrap
        // are separators; keep indentation on explicit rows.
        let rest = if first
            .text
            .trim_start_matches(is_breakable_horizontal_space)
            .is_empty()
        {
            rest
        } else {
            rest.trim_start_matches(is_breakable_horizontal_space)
        };
        if !rest.is_empty() {
            second = Some((rest, second.is_some()));
        }
    }
    let second = second.map(|(text, has_more)| {
        let line = shape(text);
        if has_more || line.width() > width {
            fit_notification_line(&line, width, "…", shape)
        } else {
            line
        }
    });
    [Some(first), second]
}

fn fit_notification_line(
    line: &ShapedLine,
    width: Pixels,
    suffix: &str,
    shape: impl Fn(&str) -> ShapedLine,
) -> ShapedLine {
    let with_suffix = |end| {
        let prefix = &line.text[..end];
        if suffix.is_empty() {
            shape(prefix)
        } else {
            shape(&format!("{}{suffix}", prefix.trim_end()))
        }
    };
    let full = with_suffix(line.text.len());
    if full.width() <= width {
        return full;
    }
    // Search shaped cluster boundaries in source order, so a cut doesn't
    // split a cluster. Always measure the prefix with its suffix in place.
    let mut cuts: Vec<_> = line
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let fitting = cuts.partition_point(|&end| with_suffix(end).width() <= width);
    with_suffix(cuts[..fitting].last().copied().unwrap_or(0))
}

impl NotificationView {
    pub(super) fn card(
        &self,
        notification: &Notification,
        show_actions: bool,
        can_activate: impl Fn() -> bool + Clone + 'static,
        body_width: Option<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = notification.id;
        let close = cx.entity();
        let open = self.feed.clone();
        let feedback = self.desktop.feedback.clone();
        let has_default = notification.actions.iter().any(|(key, _)| key == "default");
        let is_banner = self.banners;
        let interactive = can_activate();
        let received = self
            .feed
            .read(cx)
            .received_at(id)
            .map(|time| time.format("%H:%M").to_string())
            .unwrap_or_default();
        let mut actions = row().gap_1();
        if show_actions {
            for (index, (key, text)) in notification
                .actions
                .iter()
                .filter(|(key, _)| key != "default")
                .enumerate()
            {
                let key = key.clone();
                let feed = self.feed.clone();
                let feedback = self.desktop.feedback.clone();
                let can_activate = can_activate.clone();
                actions = actions.child(
                    card_button(
                        ("notification-action", (u64::from(id) << 8) | index as u64),
                        text.clone(),
                        can_activate,
                        move |_, cx| {
                            if let Some(state) = feed.read(cx).action_state(id, &key, cx) {
                                feedback::dispatch(
                                    state.invoke_action(id, key.clone()),
                                    "通知の操作を実行できません",
                                    &feedback,
                                    cx,
                                );
                            }
                        },
                    )
                    .flex_1()
                    .bg(rgba(0xffffff0e))
                    .child(text.clone()),
                );
            }
        }
        let body = notification_text_column(
            label(notification.summary.clone()),
            (!notification.body.is_empty())
                .then(|| notification_body(notification.body.clone(), body_width)),
        );
        let body = if has_default {
            let can_activate = can_activate.clone();
            card_button(
                ("notification-body", id),
                "通知を開く",
                can_activate,
                move |_, cx| {
                    if is_banner && !open.read(cx).is_live_banner(id) {
                        return;
                    }
                    if let Some(state) = open.read(cx).action_state(id, "default", cx) {
                        feedback::dispatch(
                            state.invoke_action(id, "default"),
                            "通知を開けません",
                            &feedback,
                            cx,
                        );
                    }
                    if is_banner {
                        open.update(cx, |feed, cx| feed.hide_banner(id, cx));
                    }
                },
            )
            .w_full()
            .justify_start()
            .px_0()
            .py_0()
            .child(body)
            .into_any_element()
        } else {
            body.into_any_element()
        };
        let banner_feed = self.feed.clone();
        div()
            .id(("notification", id))
            .on_hover(move |hovered, _, cx| {
                if !is_banner {
                    return;
                }
                banner_feed.update(cx, |feed, cx| {
                    if *hovered && interactive {
                        feed.pause(id);
                    } else if !*hovered {
                        feed.resume(id, cx);
                    }
                });
            })
            .child(
                self.glass
                    .element(GlassState::default(), cx)
                    .w_full()
                    .p(CARD_PADDING)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        row()
                            .gap_2()
                            .child(self.desktop.app_icon(&notification.app_icon, 26.))
                            .child(
                                label(app_name(notification))
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(MUTED))
                                    .text_size(px(11.)),
                            )
                            .child(
                                muted(received)
                                    .font_family(theme::DATA_FONT)
                                    .text_size(px(10.)),
                            )
                            .child(
                                card_button(
                                    ("dismiss-notification", id),
                                    "通知を削除",
                                    can_activate,
                                    move |_, cx| {
                                        close.update(cx, |view, cx| {
                                            if !view.banners
                                                || view.feed.read(cx).is_live_banner(id)
                                            {
                                                view.dismiss(&[id], cx);
                                            }
                                        });
                                    },
                                )
                                .min_h(px(22.))
                                .px_1()
                                .py_0()
                                .child(icons::icon(Icon::Close).size(px(13.))),
                            ),
                    )
                    .child(body)
                    .when(!notification.actions.is_empty() && show_actions, |card| {
                        card.child(actions)
                    }),
            )
    }
}

pub(super) fn card_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    can_activate: impl Fn() -> bool + 'static,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let tab_stop = can_activate();
    crate::ui::shared::button(id, label, move |window, cx| {
        if can_activate() {
            action(window, cx);
        }
    })
    .when(!tab_stop, |view| view.tab_stop(false).cursor_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TextStyle, WindowTextSystem, px};
    #[test]
    fn notification_body_wraps_and_marks_only_omitted_text() {
        let cx = gpui::TestAppContext::single();
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let style = TextStyle::default();
        let font_size = px(10.);
        let width = |text: &str| {
            text_system
                .shape_line(text.into(), font_size, &[style.to_run(text.len())], None)
                .width()
        };
        let narrow = width("abcdef");
        let wide = width("abcdefgh");
        for (text, available, expected) in [
            ("short", narrow, ["short"].as_slice()),
            ("four five", narrow, &["four ", "five"]),
            ("abcdefghijkl", narrow, &["abcdef", "ghijkl"]),
            ("abcdefghijklm", narrow, &["abcdef", "ghijk…"]),
            ("abcdefghijklm", wide, &["abcdefgh", "ijklm"]),
            ("first\nsecond", narrow, &["first", "second"]),
            ("first\r\nsecond", narrow, &["first", "second"]),
            ("first\nsecond\nthird", narrow, &["first", "secon…"]),
            ("first\nlongunbroken", narrow, &["first", "longu…"]),
            ("first\n\nthird", narrow, &["first", "…"]),
            ("first\n", narrow, &["first", ""]),
            ("abcdefghij\nlater", narrow, &["abcdef", "ghij…"]),
            // The logical first line fits exactly in two rows, but "later" was omitted.
            ("abcdefghijkl\nlater", narrow, &["abcdef", "ghijk…"]),
            ("日本語日本語日本語", narrow, &["日本語日本語", "日本語"]),
            (
                "日本語日本語日本語日本語日",
                narrow,
                &["日本語日本語", "日本語日本…"],
            ),
            ("😀😀😀😀😀😀😀", narrow, &["😀😀😀", "😀😀…"]),
        ] {
            let lines = notification_body_lines(&text_system, &style, font_size, text, available);
            let lines: Vec<_> = lines.iter().flatten().collect();
            assert_eq!(
                lines
                    .iter()
                    .map(|line| line.text.as_ref())
                    .collect::<Vec<_>>(),
                expected,
                "{text:?} at {available:?}",
            );
            assert!(lines.iter().all(|line| line.width() <= available));
        }
    }

    #[test]
    fn notification_body_consumes_spaces_only_at_a_soft_wrap() {
        let cx = gpui::TestAppContext::single();
        let text_system = WindowTextSystem::new(cx.text_system().clone());
        let style = TextStyle::default();
        let first = "a".repeat(44);
        let second = "b".repeat(44);
        let cases = [
            (format!("{first} {second}"), Some(second.as_str())),
            (format!("{first}  {second}"), Some(second.as_str())),
            (format!("{first}\u{3000}{second}"), Some(second.as_str())),
            (format!("{first}\u{2003}{second}"), Some(second.as_str())),
            (format!("{first}\n  indented"), Some("  indented")),
            (format!("{first} \n  indented"), Some("  indented")),
            (
                format!("{first}\u{3000}\n\u{2003}\u{3000}indented"),
                Some("\u{2003}\u{3000}indented"),
            ),
            (format!("{first} "), None),
        ];
        let preserved = ['\u{a0}', '\u{2007}', '\u{202f}', '\u{2028}', '\u{2029}']
            .map(|space| format!("{space}{}", "b".repeat(43)));
        let preserved = preserved
            .iter()
            .map(|rest| (format!("{first}{rest}"), Some(rest.as_str())));
        for (text, expected_second) in cases.into_iter().chain(preserved) {
            let [first_line, second_line] =
                notification_body_lines(&text_system, &style, px(13.), &text, px(348.));
            assert_eq!(first_line.unwrap().text.as_ref(), first);
            assert_eq!(
                second_line.as_ref().map(|line| line.text.as_ref()),
                expected_second,
                "{text:?}",
            );
        }
    }
}

#[cfg(test)]
mod probes;

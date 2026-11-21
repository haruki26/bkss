use std::{cell::Cell, rc::Rc};

use crate::ui::{
    desktop::Panel,
    feedback,
    icons::{self, Icon},
    shared::{button, label, muted, row},
};
use bkss_integration::Notification;
use gpui::{
    AnimationExt, Context, Div, ElementId, IntoElement, Pixels, SpringAnimation, SpringConfig,
    Stateful, Window, canvas, div, prelude::*, px, rgba,
};

use super::{NotificationView, card::notification_body_width};
use crate::ui::theme;

const GROUP_SPRING: SpringConfig = SpringConfig::new(300., 35., 1.);
const GROUP_GAP: gpui::Rems = gpui::rems(0.5);

fn grouped(notifications: &[Notification]) -> Vec<(String, Vec<&Notification>)> {
    let mut groups: Vec<(String, Vec<&Notification>)> = Vec::new();
    for notification in notifications {
        let name = super::card::app_name(notification);
        if let Some((_, items)) = groups.iter_mut().find(|(existing, _)| *existing == name) {
            items.push(notification);
        } else {
            groups.push((name, vec![notification]));
        }
    }
    groups
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum GroupPhase {
    #[default]
    Collapsed,
    Opening,
    Expanded,
    Closing,
}

impl GroupPhase {
    fn expanded(self) -> bool {
        matches!(self, Self::Opening | Self::Expanded)
    }
}

#[derive(Default)]
pub(super) struct GroupExpansion {
    phase: Cell<GroupPhase>,
    natural_height: Cell<Pixels>,
    origin: Cell<Pixels>,
}

impl GroupExpansion {
    fn toggle(&self) {
        let phase = self.phase.get();
        if phase == GroupPhase::Collapsed {
            self.origin.set(px(0.));
        } else if phase == GroupPhase::Expanded {
            self.origin.set(self.natural_height.get());
        }
        self.phase.set(if phase.expanded() {
            GroupPhase::Closing
        } else {
            GroupPhase::Opening
        });
    }

    fn without_tail(&self) {
        self.phase.set(if self.phase.get().expanded() {
            GroupPhase::Expanded
        } else {
            GroupPhase::Collapsed
        });
        self.natural_height.set(px(0.));
    }

    fn tail(self: &Rc<Self>, extra: Option<gpui::Div>, rem: Pixels) -> gpui::AnyElement {
        let Some(extra) = extra else {
            return group_hint().into_any_element();
        };
        let phase = self.phase.get();
        let known_height = self.natural_height.get();
        let sample = Rc::new(Cell::new(px(0.)));
        let measured_sample = sample.clone();
        let motion = self.clone();
        let extra = extra.child(
            canvas(
                move |bounds, window, _| {
                    if motion.phase.get() != phase {
                        return;
                    }
                    let natural = bounds.size.height;
                    motion.natural_height.set(natural);
                    let height = measured_sample.get();
                    // Use this frame's natural height, not a stale target after a feed update.
                    let settled = match phase {
                        GroupPhase::Opening if height == natural && natural == known_height => {
                            GroupPhase::Expanded
                        }
                        GroupPhase::Closing if height == px(0.) => GroupPhase::Collapsed,
                        _ => phase,
                    };
                    if settled != phase || phase == GroupPhase::Opening && natural != known_height {
                        motion.phase.set(settled);
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        if phase == GroupPhase::Expanded {
            return extra.into_any_element();
        }

        let target = if phase == GroupPhase::Opening {
            known_height
        } else {
            px(0.)
        };
        div()
            .relative()
            .with_spring(
                "group-rows-height",
                SpringAnimation::new(GROUP_SPRING)
                    .to(target)
                    .from(self.origin.get())
                    .with_epsilon(0.25),
                move |tail, height| {
                    sample.set(height);
                    let height = height.max(px(0.));
                    // The collapsed stack hints retain their original footprint, then
                    // yield that space as the real cards begin to appear.
                    let hint_height = (GROUP_GAP.to_pixels(rem) * 2. - px(7.)).max(px(0.));
                    let hint_opacity = (1. - height / (hint_height * 2.).max(px(16.))).max(0.);
                    tail.h(height + hint_height * hint_opacity)
                        .child(
                            div()
                                .relative()
                                .h(height)
                                .overflow_hidden()
                                .child(extra.absolute().top_0().left_0().w_full()),
                        )
                        .when(hint_opacity > 0., |tail| {
                            tail.child(
                                group_hint()
                                    .absolute()
                                    .top_0()
                                    .w_full()
                                    .opacity(hint_opacity),
                            )
                        })
                },
            )
            .into_any_element()
    }
}

fn group_hint() -> gpui::Div {
    group_rows()
        .child(
            div()
                .mx_3()
                .h(px(5.))
                .rounded_b_lg()
                .bg(rgba(0xcdd6f424))
                .mt(px(-7.)),
        )
        .child(
            div()
                .mx_6()
                .h(px(4.))
                .rounded_b_lg()
                .bg(rgba(0xcdd6f414))
                .mt(px(-9.)),
        )
}

fn group_rows() -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .gap(GROUP_GAP)
        .pt(GROUP_GAP)
}

pub(super) fn render(
    view: &mut NotificationView,
    notifications: &[bkss_integration::Notification],
    window: &mut Window,
    cx: &mut Context<NotificationView>,
) -> Stateful<Div> {
    let mut root = view
        .desktop
        .panel_root(Panel::Notifications, view.display, window);
    let body_width = notification_body_width(&mut root, Some(view.panel_limit.width), window);
    let mut content = div().flex().flex_col().gap_3().w_full();
    let close = view.desktop.clone();
    let clear = cx.entity();
    let ids: Vec<_> = notifications.iter().map(|n| n.id).collect();
    let display = view.display;
    content = content.child(
        row()
            .px_2()
            .justify_between()
            .child(
                label("通知")
                    .text_size(px(20.))
                    .font_weight(gpui::FontWeight::SEMIBOLD),
            )
            .child(
                row()
                    .gap_1()
                    .when(!ids.is_empty(), |view| {
                        view.child(
                            button(
                                "clear-notifications",
                                "通知をすべて削除",
                                move |_, cx| clear.update(cx, |view, cx| view.dismiss(&ids, cx)),
                            )
                            .child("すべて消去"),
                        )
                    })
                    .child(
                        button(
                            "close-notifications",
                            "通知一覧を閉じる",
                            move |_, cx| close.close(Panel::Notifications, display, cx),
                        )
                        .child(icons::icon(Icon::Close)),
                    ),
            ),
    );
    let groups = grouped(notifications);
    view.group_expansion.retain(|name, motion| {
        match groups.iter().find(|(group_name, _)| group_name == name) {
            Some((_, group)) => {
                if group.len() == 1 {
                    motion.without_tail();
                }
                true
            }
            None => false,
        }
    });
    let mut list = div()
        .id("notification-list")
        .overflow_y_scroll()
        .max_h((view.panel_limit.height - px(40.)).max(px(1.)))
        .flex()
        .flex_col()
        .gap_4()
        .pb_2();
    if groups.is_empty() {
        list = list.child(
            div()
                .w_full()
                .p_6()
                .h(px(180.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(icons::icon(Icon::Bell).size(px(28.)))
                .child(muted("通知はありません")),
        );
    }
    for (name, group) in groups {
        let count = group.len();
        let motion = (count > 1).then(|| {
            view.group_expansion
                .entry(name.clone())
                .or_default()
                .clone()
        });
        let clear = cx.entity();
        let ids: Vec<_> = group.iter().map(|n| n.id).collect();
        let mut stack = div().flex().flex_col().gap(GROUP_GAP);
        if let Some(motion) = &motion {
            let expanded = motion.phase.get().expanded();
            let toggle_motion = motion.clone();
            let toggle = cx.entity().downgrade();
            stack = stack.child(
                row()
                    .px_2()
                    .justify_between()
                    .child(
                        button(
                            gpui::ElementId::Name(format!("expand-group-{name}").into()),
                            format!(
                                "{name} の通知を{}",
                                if expanded {
                                    "まとめる"
                                } else {
                                    "展開する"
                                }
                            ),
                            move |_, cx| {
                                toggle_motion.toggle();
                                let _ = toggle.update(cx, |_, cx| cx.notify());
                            },
                        )
                        .aria_expanded(expanded)
                        .min_w_0()
                        .flex_1()
                        .justify_start()
                        .px_1()
                        .child(
                            label(format!("{name}  ·  {count}"))
                                .min_w_0()
                                .truncate()
                                .text_size(px(11.)),
                        )
                        .child(
                            muted(if expanded {
                                "まとめる"
                            } else {
                                "すべて表示"
                            })
                            .text_size(px(10.)),
                        ),
                    )
                    .child(
                        button(
                            gpui::ElementId::Name(format!("clear-group-{name}").into()),
                            format!("{name} の通知を削除"),
                            move |_, cx| clear.update(cx, |view, cx| view.dismiss(&ids, cx)),
                        )
                        .min_h(px(22.))
                        .child(icons::icon(Icon::Close).size(px(13.))),
                    ),
            );
        }
        let mut cards = group.into_iter();
        if let Some(first) = cards.next() {
            stack = stack.child(view.card(first, true, || true, body_width, cx));
        }
        let slot = div()
            .id(ElementId::Name(format!("notification-group-{name}").into()))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(stack);
        if let Some(motion) = motion {
            let extra = (motion.phase.get() != GroupPhase::Collapsed).then(|| {
                let mut extra = group_rows();
                for notification in cards {
                    let motion = motion.clone();
                    extra = extra.child(view.card(
                        notification,
                        true,
                        move || motion.phase.get() == GroupPhase::Expanded,
                        body_width,
                        cx,
                    ));
                }
                extra
            });
            list = list.child(slot.child(motion.tail(extra, window.rem_size())));
        } else {
            list = list.child(slot);
        }
    }
    content = content
        .child(list)
        .children(feedback::error_banner(&view.desktop.feedback, cx));
    let root = root.child(content);
    notification_surface(root, view.panel_limit)
}

pub(super) fn notification_surface(
    root: gpui::Stateful<gpui::Div>,
    limit: gpui::Size<Pixels>,
) -> gpui::Stateful<gpui::Div> {
    root.relative()
        // Nonzero alpha lets Hyprland's ignore_alpha mask follow the sliding panel.
        .bg(rgba(0x00000005))
        // A resized viewport must not become the next layout limit.
        .w(limit.width)
        .max_h(limit.height)
        .child(
            canvas(
                move |bounds, window, cx| {
                    let target = notification_surface_size(bounds.size, limit);
                    // An older Wayland configure can supersede an earlier resize request.
                    if window.viewport_size() != target {
                        window.defer(cx, move |window, _| {
                            if window.viewport_size() != target {
                                window.resize(target);
                            }
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
}

pub(in crate::ui) fn notification_panel_limit(display: gpui::Size<Pixels>) -> gpui::Size<Pixels> {
    // Wayland reports the whole output; subtract both shell bars' reservations.
    let available = gpui::size(
        display.width - px(24.),
        display.height - px(2. * (theme::BAR_HEIGHT + theme::MARGIN) + 24.),
    );
    gpui::size(
        available.width.min(px(theme::PANEL_WIDTH)),
        available.height.min(px(theme::PANEL_MAX_HEIGHT)),
    )
    .map(|value| px(f32::from(value).floor().max(1.)))
}

fn notification_surface_size(
    content: gpui::Size<Pixels>,
    limit: gpui::Size<Pixels>,
) -> gpui::Size<Pixels> {
    gpui::size(
        px(f32::from(content.width).ceil().max(1.)).min(limit.width),
        px(f32::from(content.height).ceil().max(1.)).min(limit.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::notification::card::card_button;
    use bkss_ui::effects::{GlassState, MountedGlass};
    use gpui::{Size, px, size};
    use std::{cell::Cell, rc::Rc, time::Duration};

    fn notification(id: u32, app: &str) -> Notification {
        Notification {
            id,
            app_name: app.into(),
            app_icon: String::new(),
            summary: String::new(),
            body: String::new(),
            actions: Vec::new(),
            urgency: None,
            resident: false,
        }
    }

    #[test]
    fn native_panel_fits_reserved_bars_and_grows_after_shrinking() {
        let limit = notification_panel_limit(size(px(1920.), px(1080.)));
        assert_eq!(limit, size(px(380.), px(760.)));
        assert_eq!(
            notification_panel_limit(size(px(320.), px(400.))),
            size(px(296.), px(280.))
        );
        assert_eq!(
            notification_panel_limit(size(px(12.), px(70.))),
            size(px(1.), px(1.))
        );
        for (height, expected) in [(220.2, 221.), (900., 760.), (180., 180.), (600., 600.)] {
            assert_eq!(
                notification_surface_size(size(limit.width, px(height)), limit),
                size(limit.width, px(expected))
            );
        }
        // Logical sizes remain integral even on fractionally scaled outputs.
        assert_eq!(
            notification_panel_limit(size(px(319.5), px(399.5))),
            size(px(295.), px(279.))
        );
    }

    struct PanelProbe {
        height: f32,
        limit: Size<Pixels>,
        glass: MountedGlass,
        panel: gpui::ScrollHandle,
        scroll: gpui::ScrollHandle,
    }

    impl PanelProbe {
        fn new(height: f32, limit: Size<Pixels>, cx: &mut Context<Self>) -> Self {
            Self {
                height,
                limit,
                glass: MountedGlass::new(theme::glass_style(22.).unwrap(), cx),
                panel: gpui::ScrollHandle::new(),
                scroll: gpui::ScrollHandle::new(),
            }
        }
    }

    impl Render for PanelProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut list = div()
                .id("probe-list")
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .max_h(self.limit.height - px(40.))
                .flex()
                .flex_col()
                .gap_4()
                .pb_2();
            if self.height == 180. {
                list = list.child(div().h(px(self.height)).flex_shrink_0());
            } else {
                let cards = if self.height > 760. { 10 } else { 4 };
                let height = (self.height - 16. * (cards - 1) as f32) / cards as f32;
                for _ in 0..cards {
                    list = list.child(
                        self.glass
                            .element(GlassState::default(), cx)
                            .w_full()
                            .h(px(height))
                            .flex_shrink_0(),
                    );
                }
            }
            // The test window stretches its root; the overlay normally measures a child.
            div().child(notification_surface(
                div()
                    .id("probe-root")
                    .overflow_y_scroll()
                    .track_scroll(&self.panel)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .w_full()
                            .child(div().h(px(28.)))
                            .child(list),
                    ),
                self.limit,
            ))
        }
    }

    #[test]
    fn center_blur_mask_fills_the_undecorated_panel_and_stays_fixed_when_scrolling() {
        fn assert_panel(
            visual: &mut gpui::VisualTestContext,
            panel: &gpui::ScrollHandle,
            expected: Size<Pixels>,
            populated: bool,
        ) {
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert_eq!(panel.bounds().size, expected);
                let quads = window.painted_quads();
                let bounds = panel.bounds().scale(window.scale_factor());
                let mut panel_quads = quads.iter().filter(|quad| quad.bounds == bounds);
                let mask = panel_quads
                    .next()
                    .expect("the panel has a native blur mask");
                assert!(panel_quads.next().is_none());
                assert_eq!(mask.background.as_solid(), Some(rgba(0x00000005).into()));
                assert_eq!(mask.corner_radii, gpui::Corners::default());
                assert_eq!(mask.border_widths, gpui::Edges::default());
                if populated {
                    assert!(quads.iter().any(|quad| {
                        quad.background
                            .as_solid()
                            .is_some_and(|color| (color.a - 0.30).abs() < 0.001)
                    }));
                } else {
                    assert_eq!(quads.len(), 1);
                }
            });
        }

        let mut cx = gpui::TestAppContext::single();
        let limit = size(px(380.), px(760.));
        let window = cx.add_window(|_, cx| PanelProbe::new(180., limit, cx));
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &cx);
        let (panel, scroll) = window
            .update(&mut cx, |view, _, _| {
                (view.panel.clone(), view.scroll.clone())
            })
            .unwrap();
        assert_panel(&mut visual, &panel, size(limit.width, px(228.)), false);
        for (height, expected) in [(420., px(468.)), (1100., limit.height)] {
            window
                .update(&mut cx, |view, _, cx| {
                    view.height = height;
                    cx.notify();
                })
                .unwrap();
            assert_panel(&mut visual, &panel, size(limit.width, expected), true);
        }
        assert_eq!(scroll.bounds().size.height, px(720.));
        assert!(scroll.max_offset().y > px(0.));
        visual.simulate_event(gpui::ScrollWheelEvent {
            position: scroll.bounds().center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-100.))),
            ..Default::default()
        });
        assert_panel(&mut visual, &panel, limit, true);
        assert_eq!(scroll.offset().y, px(-100.));
    }

    #[test]
    fn groups_preserve_newest_first_source_order_without_losing_items() {
        let notifications = vec![
            notification(5, "Mail"),
            notification(4, "Chat"),
            notification(3, "Mail"),
        ];
        let groups = grouped(&notifications);
        assert_eq!(
            groups
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["Mail", "Chat"]
        );
        assert_eq!(groups[0].1.iter().map(|n| n.id).collect::<Vec<_>>(), [5, 3]);
    }

    #[derive(Clone, Copy, Default)]
    struct GroupFrame {
        first_top: Pixels,
        following_top: Pixels,
        last_center: Option<gpui::Point<Pixels>>,
        clip_bottom: Option<Pixels>,
    }

    struct GroupProbe {
        motion: Rc<GroupExpansion>,
        extra_count: usize,
        frame: Rc<Cell<GroupFrame>>,
        activations: Rc<Cell<usize>>,
    }

    impl Render for GroupProbe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.frame.set(GroupFrame::default());
            let first = self.frame.clone();
            let following = self.frame.clone();
            let extra = (self.extra_count > 0 && self.motion.phase.get() != GroupPhase::Collapsed)
                .then(|| {
                    group_rows().children((0..self.extra_count).map(|index| {
                        let motion = self.motion.clone();
                        let activations = self.activations.clone();
                        let frame = self.frame.clone();
                        card_button(
                            ("test-extra-row", index),
                            "Extra row",
                            move || motion.phase.get() == GroupPhase::Expanded,
                            move |_, _| activations.set(activations.get() + 1),
                        )
                        .relative()
                        .h(px(48.))
                        .child(
                            canvas(
                                move |bounds, window, _| {
                                    let mut value = frame.get();
                                    value.last_center = Some(bounds.center());
                                    value.clip_bottom = Some(window.content_mask().bounds.bottom());
                                    frame.set(value);
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                    }))
                });
            div()
                .id("test-list")
                .overflow_y_scroll()
                .max_h(px(250.))
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .id("test-group")
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    let mut value = first.get();
                                    value.first_top = bounds.top();
                                    first.set(value);
                                },
                                |_, _, _, _| {},
                            )
                            .h(px(42.)),
                        )
                        .when(self.extra_count > 0, |group| {
                            group.child(self.motion.tail(extra, window.rem_size()))
                        }),
                )
                .child(
                    canvas(
                        move |bounds, _, _| {
                            let mut value = following.get();
                            value.following_top = bounds.top();
                            following.set(value);
                        },
                        |_, _, _, _| {},
                    )
                    .h(px(40.)),
                )
        }
    }

    #[gpui::test]
    fn group_rows_spring_open_and_closed_in_list_flow(cx: &mut gpui::TestAppContext) {
        let motion = Rc::new(GroupExpansion::default());
        let frame = Rc::new(Cell::new(GroupFrame::default()));
        let activations = Rc::new(Cell::new(0));
        let window = cx.open_window(size(px(280.), px(360.)), |_, _| GroupProbe {
            motion: motion.clone(),
            extra_count: 2,
            frame: frame.clone(),
            activations: activations.clone(),
        });
        cx.run_until_parked();
        let frames = |cx: &mut gpui::TestAppContext, ms| {
            // GPUI's spring reads a monotonic wall clock, not the test executor's clock.
            if ms > 0 {
                std::thread::sleep(Duration::from_millis(ms));
            }
            let callbacks = window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap();
            cx.run_until_parked();
            callbacks
        };
        let toggle = |cx: &mut gpui::TestAppContext| {
            window
                .update(cx, |view, _, cx| {
                    view.motion.toggle();
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
        };
        let read_frame = || {
            let value = frame.get();
            (value.first_top, value.following_top)
        };
        let click_last = |cx: &mut gpui::TestAppContext, clipped| {
            let value = frame.get();
            let center = value.last_center.unwrap();
            assert_eq!(value.clip_bottom.unwrap() < center.y, clipped);
            gpui::VisualTestContext::from_window(window.into(), cx)
                .simulate_click(center, gpui::Modifiers::default());
        };
        let change_extra_count = |cx: &mut gpui::TestAppContext, count| {
            window
                .update(cx, |view, _, cx| {
                    view.extra_count = count;
                    if count == 0 {
                        view.motion.without_tail();
                    }
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
        };

        let collapsed = (px(0.), px(67.));
        let expanded = (px(0.), px(170.));
        let between_endpoints = |height| collapsed.1 < height && height < expanded.1;
        assert_eq!(read_frame(), collapsed);
        assert_eq!(frames(cx, 0), 0);
        toggle(cx);
        assert_eq!(read_frame(), collapsed);
        assert!(frames(cx, 0) > 0);
        assert!(frames(cx, 70) > 0);
        let opening = read_frame();
        assert_eq!(opening.0, collapsed.0);
        assert!(
            between_endpoints(opening.1),
            "opening={opening:?}, natural={:?}",
            motion.natural_height.get(),
        );
        click_last(cx, true);
        assert_eq!(activations.get(), 0);

        toggle(cx);
        assert!(between_endpoints(read_frame().1));
        assert!(frames(cx, 90) > 0);
        let closing = read_frame();
        assert!(collapsed.1 < closing.1 && closing.1 < opening.1);
        toggle(cx);
        assert!(between_endpoints(read_frame().1));
        change_extra_count(cx, 3);
        assert!(between_endpoints(read_frame().1));
        assert!(frames(cx, 0) > 0);
        assert!(frames(cx, 900) > 0);
        assert!(frames(cx, 0) > 0);
        assert_eq!(read_frame(), (px(0.), px(226.)));
        assert_eq!(frames(cx, 0), 0);
        click_last(cx, false);
        assert_eq!(activations.get(), 1);

        change_extra_count(cx, 2);
        assert_eq!(read_frame(), expanded);
        assert_eq!(frames(cx, 0), 0);
        toggle(cx);
        assert_eq!(read_frame(), expanded);
        assert!(frames(cx, 80) > 0);
        let closing = read_frame();
        assert_eq!(closing.0, collapsed.0);
        assert!(collapsed.1 < closing.1 && closing.1 < expanded.1);
        click_last(cx, true);
        assert_eq!(activations.get(), 1);
        assert!(frames(cx, 900) > 0);
        assert!(frames(cx, 0) > 0);
        assert_eq!(read_frame(), collapsed);
        assert!(frame.get().last_center.is_none());
        assert_eq!(frames(cx, 0), 0);

        toggle(cx);
        assert!(frames(cx, 0) > 0);
        change_extra_count(cx, 0);
        assert_eq!(read_frame(), (px(0.), px(58.)));
        assert!(frame.get().last_center.is_none());
        frames(cx, 0);
        assert_eq!(frames(cx, 0), 0);
        change_extra_count(cx, 2);
        assert_eq!(read_frame(), expanded);
        assert_eq!(frames(cx, 0), 0);
    }
}

use super::super::banner_stack::{Banner, BannerPhase};
use super::{NotificationView, card::notification_body_width, feed::BannerFrameProgress};
use crate::ui::desktop::Panel;
use crate::ui::{shared::button, theme::ACCENT};
use gpui::{
    Animation, AnimationExt, App, Context, Div, Stateful, Window, canvas, div, ease_in_out,
    ease_out_quint, prelude::*, px, rgb,
};
use std::{cell::Cell, rc::Rc};

pub(super) fn banner_footer(
    count: usize,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> Option<Stateful<Div>> {
    (count > 0).then(|| {
        button("banner-show-all", "通知一覧を開く", action)
            .w_full()
            .text_color(rgb(ACCENT))
            .text_size(px(10.))
            .child("通知一覧を開く")
    })
}

pub(super) fn banner_frame_element(slot: Div, banner: &Banner, progress: f32) -> Div {
    let frame = banner.sample(progress);
    slot.left(px(frame.x))
        .opacity(frame.opacity)
        .when_some(frame.height, |slot, height| {
            slot.h(px(height)).overflow_hidden()
        })
}

pub(super) fn render(
    view: &mut NotificationView,
    notifications: &[bkss_integration::Notification],
    window: &mut Window,
    cx: &mut Context<NotificationView>,
) -> Stateful<Div> {
    let panel = Panel::Banners;
    let mut root = view.desktop.panel_root(panel, view.display, window);
    let body_width = notification_body_width(&mut root, None, window);
    let mut content = div().flex().flex_col().gap_3().w_full();
    content = content.relative().gap_0();
    let banners = view.feed.read(cx).banner_items();
    let banner_count = banners.len();
    let mut frames = Vec::new();
    for banner in banners {
        let progress = Rc::new(Cell::new(if banner.phase == BannerPhase::Visible {
            1.
        } else {
            0.
        }));
        let height = Rc::new(Cell::new(banner.natural_height));
        let measured_height = height.clone();
        let token = banner.token;
        let phase = banner.phase;
        let interactive = banner.live()
            && notifications
                .iter()
                .any(|notification| notification.id == banner.notification.id);
        let inner = div()
            .relative()
            .w_full()
            .flex_shrink_0()
            .pb(px(12.))
            .child(view.card(
                &banner.notification,
                false,
                move || interactive,
                body_width,
                cx,
            ))
            .child(
                canvas(
                    move |bounds, _, _| measured_height.set(f32::from(bounds.size.height)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .when(banner.sample(0.).height.is_some(), |inner| {
                inner.absolute().top_0().left_0()
            });
        let slot = div().relative().w_full().min_h_0().child(inner);
        let slot = if phase == BannerPhase::Visible {
            slot.into_any_element()
        } else {
            let frame_progress = progress.clone();
            let animation = if phase == BannerPhase::Fading {
                Animation::new(phase.duration()).with_easing(ease_in_out)
            } else {
                Animation::new(phase.duration()).with_easing(ease_out_quint())
            };
            slot.with_animation(
                gpui::ElementId::Name(format!("banner-{token}-{phase:?}").into()),
                animation,
                move |slot, value| {
                    frame_progress.set(value);
                    banner_frame_element(slot, &banner, value)
                },
            )
            .into_any_element()
        };
        content = content.child(slot);
        frames.push(BannerFrameProgress {
            token,
            phase,
            progress,
            height,
        });
    }
    let desktop = view.desktop.clone();
    let display = view.display;
    content = content.children(banner_footer(banner_count, move |_, cx| {
        desktop.show(Panel::Notifications, display, cx);
    }));
    let feed = view.feed.downgrade();
    // A stationary, non-fading sentinel delivers native animation completion,
    // including the final frame of a fully transparent or zero-height slot.
    content = content.child(
        canvas(
            move |_, _, cx| {
                cx.defer(move |cx| {
                    if let Some(feed) = feed.upgrade() {
                        feed.update(cx, |feed, cx| feed.complete_frames(frames, cx));
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size(px(1.)),
    );
    root.child(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::banner_stack::{BannerPhase, BannerStack};
    use crate::ui::{notification::feed::BANNER_TIMEOUT, theme};
    use gpui::{Animation, AnimationExt, div};
    use std::time::Duration;

    fn notification(id: u32, app: &str) -> bkss_integration::Notification {
        bkss_integration::Notification {
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
    fn native_banner_styles_slide_arrivals_but_only_fade_departing_cards() {
        assert_eq!(BANNER_TIMEOUT, Duration::from_secs(10));
        let mut stack = BannerStack::default();
        stack.upsert(&notification(1, "Mail"));
        let mut entering = banner_frame_element(div(), &stack.items[0], 0.);
        assert_eq!(
            entering.style().inset.left,
            Some(px(theme::PANEL_WIDTH).into())
        );
        assert_eq!(entering.style().inset.top, None);
        assert_eq!(entering.style().inset.bottom, None);
        let token = stack.items[0].token;
        stack.frame(token, BannerPhase::Entering, 1., 140.);
        stack.hide(1);
        let mut fading = banner_frame_element(div(), &stack.items[0], 0.5);
        assert_eq!(fading.style().inset.left, Some(px(0.).into()));
        assert_eq!(fading.style().inset.top, None);
        assert_eq!(fading.style().opacity, Some(0.5));
        assert_eq!(fading.style().size.height, Some(px(140.).into()));
        stack.frame(token, BannerPhase::Fading, 1., 140.);
        let mut collapsing = banner_frame_element(div(), &stack.items[0], 0.5);
        assert_eq!(collapsing.style().opacity, Some(0.));
        assert_eq!(collapsing.style().size.height, Some(px(70.).into()));
        drop(div().with_animation(
            "banner-construction",
            Animation::new(BannerPhase::Entering.duration()),
            |slot, _| slot,
        ));
    }

    #[test]
    fn popup_stack_has_one_footer_until_its_last_departing_snapshot_is_removed() {
        for count in [0, 1, 2, 3, 128] {
            let footer = banner_footer(count, |_, _| {});
            assert_eq!(footer.into_iter().count(), usize::from(count > 0));
        }
        let mut stack = BannerStack::default();
        stack.upsert(&notification(1, "Mail"));
        let token = stack.items[0].token;
        stack.frame(token, BannerPhase::Entering, 1., 100.);
        stack.hide(1);
        assert!(!stack.is_live(1));
        assert_eq!(
            banner_footer(stack.items.len(), |_, _| {})
                .into_iter()
                .count(),
            1
        );
        stack.frame(token, BannerPhase::Fading, 1., 100.);
        assert_eq!(
            banner_footer(stack.items.len(), |_, _| {})
                .into_iter()
                .count(),
            1
        );
        stack.frame(token, BannerPhase::Collapsing, 1., 100.);
        assert!(banner_footer(stack.items.len(), |_, _| {}).is_none());
    }
}

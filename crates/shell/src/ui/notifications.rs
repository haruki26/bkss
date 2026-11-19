use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use bkss_core::{MountedBinding, ReactiveSubscription, RuntimeHandle, ShellError, SourceState};
use bkss_integration::{Notification, NotificationsState};
use bkss_ui::effects::{GlassState, MountedGlass};
use chrono::{DateTime, Local};
use gpui::{
    Animation, AnimationExt, App, AppContext, Context, DisplayId, Entity, IntoElement, Pixels,
    Render, Size, Subscription, Task, Window, canvas, div, ease_in_out, ease_out_quint, prelude::*,
    px, rgb, rgba, size,
};

use crate::notification_model::active_notifications;

use super::{
    banner_stack::{Banner, BannerPhase, BannerStack},
    desktop::{Desktop, Panel, PanelLifetime},
    theme::{self, ACCENT, Icon, MUTED, button, icon, label, row},
};

const BANNER_TIMEOUT: Duration = Duration::from_secs(10);

pub struct NotificationFeed {
    binding: MountedBinding<NotificationsState>,
    _subscription: ReactiveSubscription,
    desktop: Weak<Desktop>,
    display: DisplayId,
    previous: HashMap<u32, Notification>,
    received: HashMap<u32, (DateTime<Local>, Instant)>,
    banners: BannerStack,
    hovered: HashSet<u32>,
    deadlines: HashMap<u32, Instant>,
    remaining: HashMap<u32, Duration>,
    timers: HashMap<u32, Task<()>>,
}

impl NotificationFeed {
    pub fn mount(
        desktop: &Rc<Desktop>,
        display: DisplayId,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<Self>, ShellError> {
        let binding = runtime.mount(&desktop.services.notifications, cx)?;
        let feed = cx.new(|cx: &mut Context<Self>| Self {
            _subscription: binding
                .reactive()
                .observe(cx, |feed, _, cx| feed.reconcile(cx)),
            binding,
            desktop: Rc::downgrade(desktop),
            display,
            previous: HashMap::new(),
            received: HashMap::new(),
            banners: BannerStack::default(),
            hovered: HashSet::new(),
            deadlines: HashMap::new(),
            remaining: HashMap::new(),
            timers: HashMap::new(),
        });
        feed.update(cx, |feed, cx| feed.reconcile(cx));
        Ok(feed)
    }

    pub fn state(&self, cx: &App) -> Option<NotificationsState> {
        match self.binding.reactive().get(cx) {
            SourceState::Ready(state) => Some(state),
            SourceState::Loading => None,
        }
    }

    pub fn notifications(&self, cx: &App) -> Vec<Notification> {
        self.state(cx)
            .map(|state| {
                let mut notifications: Vec<_> = active_notifications(&state.notifications)
                    .into_iter()
                    .cloned()
                    .collect();
                notifications.sort_by_key(|notification| {
                    std::cmp::Reverse(self.received.get(&notification.id).map(|(_, order)| *order))
                });
                notifications
            })
            .unwrap_or_default()
    }

    fn action_state(&self, id: u32, key: &str, cx: &App) -> Option<NotificationsState> {
        self.state(cx).filter(|state| {
            state.notifications.iter().any(|notification| {
                notification.id == id
                    && notification.actions.iter().any(|(action, _)| action == key)
            })
        })
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let notifications = self.notifications(cx);
        let active: HashSet<_> = notifications.iter().map(|n| n.id).collect();
        let removed: Vec<_> = self
            .banners
            .items
            .iter()
            .filter(|banner| !active.contains(&banner.notification.id))
            .map(|banner| banner.notification.id)
            .collect();
        for id in removed {
            self.banners.hide(id);
        }
        self.hovered.retain(|id| {
            self.banners
                .items
                .iter()
                .any(|banner| banner.notification.id == *id)
        });
        self.timers.retain(|id, _| active.contains(id));
        self.remaining.retain(|id, _| active.contains(id));
        self.deadlines.retain(|id, _| active.contains(id));
        self.received.retain(|id, _| {
            active.contains(id)
                || self
                    .banners
                    .items
                    .iter()
                    .any(|banner| banner.notification.id == *id)
        });
        let center_open = self
            .desktop
            .upgrade()
            .is_some_and(|desktop| desktop.is_open(Panel::Notifications, self.display));
        for notification in notifications.iter().rev() {
            if self.previous.get(&notification.id) != Some(notification) {
                self.received
                    .insert(notification.id, (Local::now(), Instant::now()));
                if !center_open {
                    self.banners.upsert(notification);
                    self.schedule(notification.id, BANNER_TIMEOUT, cx);
                }
            }
        }
        self.previous = notifications.into_iter().map(|n| (n.id, n)).collect();
        self.sync_surface(cx);
        cx.notify();
    }

    fn schedule(&mut self, id: u32, delay: Duration, cx: &mut Context<Self>) {
        self.timers.remove(&id);
        if self.hovered.contains(&id) {
            self.remaining.insert(id, delay);
            self.deadlines.remove(&id);
            return;
        }
        let deadline = Instant::now() + delay;
        self.deadlines.insert(id, deadline);
        self.remaining.remove(&id);
        let task = cx.spawn(async move |feed, cx| {
            cx.background_executor().timer(delay).await;
            let _ = feed.update(cx, |feed, cx| {
                if feed.deadlines.get(&id) == Some(&deadline) {
                    feed.hide_banner(id, cx);
                }
            });
        });
        self.timers.insert(id, task);
    }

    fn pause(&mut self, id: u32) {
        if !self.banners.is_live(id) {
            return;
        }
        self.hovered.insert(id);
        if let Some(deadline) = self.deadlines.remove(&id) {
            self.remaining
                .insert(id, deadline.saturating_duration_since(Instant::now()));
            self.timers.remove(&id);
        }
    }

    fn resume(&mut self, id: u32, cx: &mut Context<Self>) {
        self.hovered.remove(&id);
        if !self.banners.is_live(id) {
            return;
        }
        if let Some(remaining) = self.remaining.remove(&id) {
            self.schedule(id, remaining, cx);
        }
    }

    fn hide_banner(&mut self, id: u32, cx: &mut Context<Self>) {
        self.banners.hide(id);
        self.timers.remove(&id);
        self.deadlines.remove(&id);
        self.remaining.remove(&id);
        self.sync_surface(cx);
        cx.notify();
    }

    pub fn hide_all_banners(&mut self, cx: &mut Context<Self>) {
        self.banners.hide_all();
        self.timers.clear();
        self.deadlines.clear();
        self.remaining.clear();
        self.sync_surface(cx);
        cx.notify();
    }

    fn sync_surface(&self, cx: &mut App) {
        if let Some(desktop) = self.desktop.upgrade() {
            if self.banners.items.is_empty() {
                desktop.close(Panel::Banners, self.display, cx);
            } else {
                desktop.show(Panel::Banners, self.display, cx);
            }
        }
    }
}

pub struct NotificationView {
    desktop: Rc<Desktop>,
    display: DisplayId,
    feed: Entity<NotificationFeed>,
    banners: bool,
    expanded: HashSet<String>,
    glass: MountedGlass,
    panel_limit: Size<Pixels>,
    _feed_subscription: Subscription,
    _feedback_subscription: Subscription,
    _lifetime: PanelLifetime,
}

impl NotificationView {
    pub fn new(
        desktop: Rc<Desktop>,
        display: DisplayId,
        feed: Entity<NotificationFeed>,
        banners: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            display,
            banners,
            expanded: HashSet::new(),
            glass: MountedGlass::new(desktop.panel_style.clone(), cx),
            panel_limit: window.viewport_size(),
            _feed_subscription: cx.observe(&feed, |_, _, cx| cx.notify()),
            _feedback_subscription: cx.observe(&desktop.feedback, |_, _, cx| cx.notify()),
            _lifetime: desktop.lifetime(
                if banners {
                    Panel::Banners
                } else {
                    Panel::Notifications
                },
                display,
            ),
            feed,
            desktop,
        }
    }

    fn dismiss(&self, ids: &[u32], cx: &mut App) {
        if let Some(state) = self.feed.read(cx).state(cx) {
            let ids: Vec<_> = ids
                .iter()
                .copied()
                .filter(|id| {
                    state
                        .notifications
                        .iter()
                        .any(|notification| notification.id == *id)
                })
                .collect();
            let operation = async move {
                for id in ids {
                    match state.dismiss(id)?.await {
                        Ok(())
                        | Err(bkss_integration::OperationError::Backend(
                            bkss_integration::NotificationsError::UnknownNotification(_),
                        )) => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Ok::<_, Box<dyn std::error::Error>>(())
            };
            theme::dispatch(
                Ok::<_, std::io::Error>(operation),
                "通知を削除できません",
                &self.desktop.feedback,
                cx,
            );
        }
    }

    fn card(
        &self,
        notification: &Notification,
        show_actions: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = notification.id;
        let close = cx.entity();
        let open = self.feed.clone();
        let feedback = self.desktop.feedback.clone();
        let has_default = notification.actions.iter().any(|(key, _)| key == "default");
        let is_banner = self.banners;
        let received = self
            .feed
            .read(cx)
            .received
            .get(&id)
            .map(|(time, _)| time.format("%H:%M").to_string())
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
                actions = actions.child(
                    button(
                        ("notification-action", (u64::from(id) << 8) | index as u64),
                        text.clone(),
                        move |_, cx| {
                            if !interactive {
                                return;
                            }
                            if let Some(state) = feed.read(cx).action_state(id, &key, cx) {
                                theme::dispatch(
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
        let mut body = div().flex().flex_col().gap_1().child(
            label(notification.summary.clone())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .line_clamp(2),
        );
        if !notification.body.is_empty() {
            body = body.child(
                label(notification.body.clone())
                    .text_color(rgb(theme::TEXT))
                    .line_clamp(if show_actions { 5 } else { 2 }),
            );
        }
        let body = if has_default {
            button(
                ("notification-body", id),
                "通知を開く",
                move |_, cx| {
                    if !interactive || is_banner && !open.read(cx).banners.is_live(id) {
                        return;
                    }
                    if let Some(state) = open.read(cx).action_state(id, "default", cx) {
                        theme::dispatch(
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
            .when(!interactive, |view| view.tab_stop(false).cursor_default())
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
                    .p_4()
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
                                theme::muted(received)
                                    .font_family(theme::DATA_FONT)
                                    .text_size(px(10.)),
                            )
                            .child(
                                button(
                                    ("dismiss-notification", id),
                                    "通知を削除",
                                    move |_, cx| {
                                        if interactive {
                                            close.update(cx, |view, cx| {
                                                if !view.banners
                                                    || view.feed.read(cx).banners.is_live(id)
                                                {
                                                    view.dismiss(&[id], cx);
                                                }
                                            });
                                        }
                                    },
                                )
                                .when(!interactive, |view| view.tab_stop(false).cursor_default())
                                .min_h(px(22.))
                                .px_1()
                                .py_0()
                                .child(icon(Icon::Close).size(px(13.))),
                            ),
                    )
                    .child(body)
                    .when(!notification.actions.is_empty() && show_actions, |card| {
                        card.child(actions)
                    }),
            )
    }
}

impl Render for NotificationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notifications = self.feed.read(cx).notifications(cx);
        let mut content = div().flex().flex_col().gap_3().w_full();
        if self.banners {
            content = content.relative().gap_0();
            let banners = self.feed.read(cx).banners.items.clone();
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
                    .child(self.card(&banner.notification, false, interactive, cx))
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
                frames.push((token, phase, progress, height));
            }
            let desktop = self.desktop.clone();
            let display = self.display;
            content = content.children(banner_footer(banner_count, move |_, cx| {
                desktop.show(Panel::Notifications, display, cx);
            }));
            let feed = self.feed.downgrade();
            // A stationary, non-fading sentinel delivers native animation completion,
            // including the final frame of a fully transparent or zero-height slot.
            content = content.child(
                canvas(
                    move |_, _, cx| {
                        cx.defer(move |cx| {
                            if let Some(feed) = feed.upgrade() {
                                feed.update(cx, |feed, cx| {
                                    let mut changed = false;
                                    for (token, phase, progress, height) in frames {
                                        changed |= feed.banners.frame(
                                            token,
                                            phase,
                                            progress.get(),
                                            height.get(),
                                        );
                                    }
                                    if changed {
                                        feed.hovered.retain(|id| {
                                            feed.banners
                                                .items
                                                .iter()
                                                .any(|banner| banner.notification.id == *id)
                                        });
                                        feed.received.retain(|id, _| {
                                            feed.previous.contains_key(id)
                                                || feed
                                                    .banners
                                                    .items
                                                    .iter()
                                                    .any(|banner| banner.notification.id == *id)
                                        });
                                        feed.sync_surface(cx);
                                        cx.notify();
                                    }
                                });
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
        } else {
            let close = self.desktop.clone();
            let clear = cx.entity();
            let ids: Vec<_> = notifications.iter().map(|n| n.id).collect();
            let display = self.display;
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
                                        move |_, cx| {
                                            clear.update(cx, |view, cx| view.dismiss(&ids, cx))
                                        },
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
                                .child(icon(Icon::Close)),
                            ),
                    ),
            );
            let groups = grouped(&notifications);
            let mut list = div()
                .id("notification-list")
                .overflow_y_scroll()
                .max_h((self.panel_limit.height - px(40.)).max(px(1.)))
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
                        .child(icon(Icon::Bell).size(px(28.)))
                        .child(theme::muted("通知はありません")),
                );
            }
            for (name, group) in groups {
                let expanded = self.expanded.contains(&name);
                let count = group.len();
                let toggle = cx.entity();
                let toggle_name = name.clone();
                let clear = cx.entity();
                let ids: Vec<_> = group.iter().map(|n| n.id).collect();
                let mut stack = div().flex().flex_col().gap_2();
                if count > 1 {
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
                                        toggle.update(cx, |view, cx| {
                                            if !view.expanded.remove(&toggle_name) {
                                                view.expanded.insert(toggle_name.clone());
                                            }
                                            cx.notify();
                                        });
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
                                    theme::muted(if expanded {
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
                                    move |_, cx| {
                                        clear.update(cx, |view, cx| view.dismiss(&ids, cx))
                                    },
                                )
                                .min_h(px(22.))
                                .child(icon(Icon::Close).size(px(13.))),
                            ),
                    );
                }
                for notification in group.iter().take(if expanded { count } else { 1 }) {
                    stack = stack.child(self.card(notification, true, true, cx));
                }
                if !expanded && count > 1 {
                    stack = stack
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
                        );
                }
                list = list.child(stack);
            }
            content = content
                .child(list)
                .children(theme::error_banner(&self.desktop.feedback, cx));
        }
        let root = self
            .desktop
            .panel_root(
                if self.banners {
                    Panel::Banners
                } else {
                    Panel::Notifications
                },
                self.display,
                window,
            )
            .child(content);
        if self.banners {
            root
        } else {
            notification_surface(root, self.panel_limit)
        }
    }
}

fn notification_surface(
    root: gpui::Stateful<gpui::Div>,
    limit: Size<Pixels>,
) -> gpui::Stateful<gpui::Div> {
    root.relative()
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

pub(super) fn notification_panel_limit(display: Size<Pixels>) -> Size<Pixels> {
    // Wayland reports the whole output; subtract both shell bars' reservations.
    let available = size(
        display.width - px(24.),
        display.height - px(2. * (theme::BAR_HEIGHT + theme::MARGIN) + 24.),
    );
    size(
        available.width.min(px(theme::PANEL_WIDTH)),
        available.height.min(px(theme::PANEL_MAX_HEIGHT)),
    )
    .map(|value| px(f32::from(value).floor().max(1.)))
}

fn notification_surface_size(content: Size<Pixels>, limit: Size<Pixels>) -> Size<Pixels> {
    size(
        px(f32::from(content.width).ceil().max(1.)).min(limit.width),
        px(f32::from(content.height).ceil().max(1.)).min(limit.height),
    )
}

fn banner_footer(
    count: usize,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> Option<gpui::Stateful<gpui::Div>> {
    (count > 0).then(|| {
        button("banner-show-all", "通知一覧を開く", action)
            .w_full()
            .text_color(rgb(ACCENT))
            .text_size(px(10.))
            .child("通知一覧を開く")
    })
}

fn banner_frame_element(slot: gpui::Div, banner: &Banner, progress: f32) -> gpui::Div {
    let frame = banner.sample(progress);
    slot.left(px(frame.x))
        .opacity(frame.opacity)
        .when_some(frame.height, |slot, height| {
            slot.h(px(height)).overflow_hidden()
        })
}

fn app_name(notification: &Notification) -> String {
    if notification.app_name.trim().is_empty() {
        "アプリケーション".into()
    } else {
        notification.app_name.clone()
    }
}

fn grouped(notifications: &[Notification]) -> Vec<(String, Vec<&Notification>)> {
    let mut groups: Vec<(String, Vec<&Notification>)> = Vec::new();
    for notification in notifications {
        let name = app_name(notification);
        if let Some((_, items)) = groups.iter_mut().find(|(existing, _)| *existing == name) {
            items.push(notification);
        } else {
            groups.push((name, vec![notification]));
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    #[ignore = "requires live Hyprland and Wayland; creates a separate transparent test layer"]
    fn native_surface_follows_measured_content_after_initial_configures() {
        use std::{cell::RefCell, process::Command};

        struct PanelProbe {
            height: f32,
            limit: Size<Pixels>,
        }
        impl Render for PanelProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                notification_surface(
                    div().id("probe-root").overflow_y_scroll().child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .w_full()
                            .child(div().h(px(28.)))
                            .child(
                                div()
                                    .id("probe-list")
                                    .overflow_y_scroll()
                                    .max_h(self.limit.height - px(40.))
                                    .flex()
                                    .flex_col()
                                    .pb_2()
                                    .child(div().h(px(self.height)).flex_shrink_0()),
                            ),
                    ),
                    self.limit,
                )
            }
        }

        fn layer_size(namespace: &str) -> Result<Size<Pixels>, String> {
            let output = Command::new("hyprctl")
                .args(["-j", "layers"])
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).into_owned());
            }
            let layers: serde_json::Value =
                serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())?;
            layers
                .as_object()
                .into_iter()
                .flat_map(|outputs| outputs.values())
                .filter_map(|output| output["levels"].as_object())
                .flat_map(|levels| levels.values())
                .filter_map(|level| level.as_array())
                .flatten()
                .find(|layer| layer["namespace"] == namespace)
                .and_then(|layer| {
                    Some(size(
                        px(layer["w"].as_f64()? as f32),
                        px(layer["h"].as_f64()? as f32),
                    ))
                })
                .ok_or_else(|| format!("test layer not found: {namespace}"))
        }

        let outcome = Rc::new(RefCell::new(None::<Result<(), String>>));
        let captured = outcome.clone();
        let namespace = format!("notification-size-test-{}", std::process::id());
        bkss_core::Shell::run(move |runtime, cx| {
            bkss_ui::Overlay::new()
                .namespace(namespace.clone())
                .surface_size(size(px(380.), px(760.)))
                .anchor(bkss_ui::Anchor::TOP | bkss_ui::Anchor::RIGHT)
                .placement(bkss_ui::Placement::Anchor {
                    edges: bkss_ui::Anchor::TOP | bkss_ui::Anchor::LEFT,
                    offset: gpui::point(px(0.), px(0.)),
                })
                .initially_visible(true)
                .mount(
                    &runtime,
                    move |_, _, window, cx| {
                        let limit = window.viewport_size();
                        let probe = cx.new(|_| PanelProbe { height: 180., limit });
                        let probe_task = probe.clone();
                        let handle = window.window_handle();
                        let namespace = namespace.clone();
                        let outcome = captured.clone();
                        cx.spawn(async move |cx| {
                            let mut result = Ok(());
                            for (index, height) in
                                [180., 180., 420., 1100., 180.].into_iter().enumerate()
                            {
                                if index == 1 {
                                    // Reproduce a configure restoring the old native size.
                                    handle
                                        .update(cx, |_, window, _| window.resize(limit))
                                        .unwrap();
                                }
                                probe_task.update(cx, |probe, cx| {
                                    probe.height = height;
                                    cx.notify();
                                });
                                cx.background_executor()
                                    .timer(Duration::from_millis(500))
                                    .await;
                                let expected = size(
                                    limit.width,
                                    px(height + 48.).min(limit.height),
                                );
                                match layer_size(&namespace) {
                                    Ok(actual) if actual == expected => {}
                                    actual => {
                                        result = Err(format!(
                                            "content={height}, expected={expected:?}, native={actual:?}"
                                        ));
                                        break;
                                    }
                                }
                            }
                            *outcome.borrow_mut() = Some(result);
                            cx.update(|cx| cx.quit());
                        })
                        .detach();
                        Ok(probe)
                    },
                    cx,
                )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(*outcome.borrow(), Some(Ok(())));
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

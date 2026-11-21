use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use bkss_core::{MountedBinding, ReactiveSubscription, RuntimeHandle, ShellError, SourceState};
use bkss_integration::{Notification, NotificationsState};
use chrono::{DateTime, Local};
use gpui::{App, AppContext, Context, DisplayId, Entity, Task};

use crate::notification_model::active_notifications;

use super::super::{
    banner_stack::{BannerPhase, BannerStack},
    desktop::{Desktop, Panel},
};
pub(super) const BANNER_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct BannerFrameProgress {
    pub(super) token: u64,
    pub(super) phase: BannerPhase,
    pub(super) progress: Rc<Cell<f32>>,
    pub(super) height: Rc<Cell<f32>>,
}

pub(in crate::ui) struct NotificationFeed {
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
    pub(in crate::ui) fn mount(
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

    pub(super) fn state(&self, cx: &App) -> Option<NotificationsState> {
        match self.binding.reactive().get(cx) {
            SourceState::Ready(state) => Some(state),
            SourceState::Loading => None,
        }
    }

    pub(in crate::ui) fn notifications(&self, cx: &App) -> Vec<Notification> {
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

    pub(super) fn received_at(&self, id: u32) -> Option<DateTime<Local>> {
        self.received.get(&id).map(|(time, _)| *time)
    }

    pub(super) fn banner_items(&self) -> Vec<crate::ui::banner_stack::Banner> {
        self.banners.items.clone()
    }

    pub(super) fn is_live_banner(&self, id: u32) -> bool {
        self.banners.is_live(id)
    }

    pub(super) fn action_state(&self, id: u32, key: &str, cx: &App) -> Option<NotificationsState> {
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

    pub(super) fn pause(&mut self, id: u32) {
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

    pub(super) fn resume(&mut self, id: u32, cx: &mut Context<Self>) {
        self.hovered.remove(&id);
        if !self.banners.is_live(id) {
            return;
        }
        if let Some(remaining) = self.remaining.remove(&id) {
            self.schedule(id, remaining, cx);
        }
    }

    pub(super) fn hide_banner(&mut self, id: u32, cx: &mut Context<Self>) {
        self.banners.hide(id);
        self.timers.remove(&id);
        self.deadlines.remove(&id);
        self.remaining.remove(&id);
        self.sync_surface(cx);
        cx.notify();
    }

    pub(in crate::ui) fn hide_all_banners(&mut self, cx: &mut Context<Self>) {
        self.banners.hide_all();
        self.timers.clear();
        self.deadlines.clear();
        self.remaining.clear();
        self.sync_surface(cx);
        cx.notify();
    }

    pub(super) fn complete_frames(
        &mut self,
        frames: Vec<BannerFrameProgress>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for frame in frames {
            changed |= self.banners.frame(
                frame.token,
                frame.phase,
                frame.progress.get(),
                frame.height.get(),
            );
        }
        if changed {
            self.hovered.retain(|id| {
                self.banners
                    .items
                    .iter()
                    .any(|banner| banner.notification.id == *id)
            });
            self.received.retain(|id, _| {
                self.previous.contains_key(id)
                    || self
                        .banners
                        .items
                        .iter()
                        .any(|banner| banner.notification.id == *id)
            });
            self.sync_surface(cx);
            cx.notify();
        }
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

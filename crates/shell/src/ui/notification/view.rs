use std::{collections::HashMap, rc::Rc};

use bkss_ui::effects::MountedGlass;
use gpui::{
    App, Context, DisplayId, Entity, IntoElement, Pixels, Render, Size, Subscription, Window,
};

use super::super::{
    desktop::{Desktop, Panel, PanelLifetime},
    feedback,
};

use super::{center::GroupExpansion, feed::NotificationFeed};

pub(in crate::ui) struct NotificationView {
    pub(super) desktop: Rc<Desktop>,
    pub(super) display: DisplayId,
    pub(super) feed: Entity<NotificationFeed>,
    pub(super) banners: bool,
    pub(super) group_expansion: HashMap<String, Rc<GroupExpansion>>,
    pub(super) glass: MountedGlass,
    pub(super) panel_limit: Size<Pixels>,
    _feed_subscription: Subscription,
    _feedback_subscription: Subscription,
    _lifetime: PanelLifetime,
}

impl NotificationView {
    pub(in crate::ui) fn new(
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
            group_expansion: HashMap::new(),
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

    pub(super) fn dismiss(&self, ids: &[u32], cx: &mut App) {
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
            feedback::dispatch(
                Ok::<_, std::io::Error>(operation),
                "通知を削除できません",
                &self.desktop.feedback,
                cx,
            );
        }
    }
}

impl Render for NotificationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notifications = self.feed.read(cx).notifications(cx);
        if self.banners {
            super::popup::render(self, &notifications, window, cx)
        } else {
            super::center::render(self, &notifications, window, cx)
        }
    }
}

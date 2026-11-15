use gpui::{AnyElement, App, Window, div, prelude::*};

use super::NotificationsState;
use crate::IntegrationView;

impl IntegrationView for NotificationsState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div()
            .child(format!("Notifications: {}", self.notifications.len()))
            .into_any_element()
    }
}

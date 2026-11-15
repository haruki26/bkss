use gpui::{AnyElement, App, Window, div, prelude::*};

use super::{TrayState, TrayStatus};
use crate::IntegrationView;

impl IntegrationView for TrayState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let count = self
            .items
            .iter()
            .filter(|item| item.status != TrayStatus::Passive)
            .count();
        div().child(format!("Tray: {count}")).into_any_element()
    }
}

use crate::IntegrationView;
use gpui::{AnyElement, App, Window, div, prelude::*};

use super::state::ClockState;

impl IntegrationView for ClockState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div()
            .child(format!("{} {}", self.date, self.time))
            .into_any_element()
    }
}

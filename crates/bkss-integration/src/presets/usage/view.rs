use crate::IntegrationView;
use gpui::{AnyElement, App, Window, div, prelude::*};

use super::state::Usage;

impl IntegrationView for Usage {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div()
            .child(format!("{:.0}%", self.percent))
            .into_any_element()
    }
}

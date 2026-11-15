use crate::IntegrationView;
use gpui::{AnyElement, App, Window, div, prelude::*};

use super::state::HyprlandState;

impl IntegrationView for HyprlandState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        div()
            .child(
                self.active_workspace
                    .as_ref()
                    .map(|workspace| workspace.name.clone())
                    .unwrap_or_default(),
            )
            .into_any_element()
    }
}

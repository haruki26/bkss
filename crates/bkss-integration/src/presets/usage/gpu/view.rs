use gpui::{AnyElement, App, Window, div, prelude::*};

use super::state::GpuState;
use crate::IntegrationView;

impl IntegrationView for GpuState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let mut content = div().flex().gap_2();
        if self.devices.is_empty() {
            content = content.child("No GPU");
        }
        for device in &self.devices {
            let utilization = device
                .metrics
                .utilization_percent
                .filter(|value| value.is_finite())
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "Metrics unavailable".to_owned());
            content = content.child(format!("{}: {utilization}", device.name));
        }
        content.into_any_element()
    }
}

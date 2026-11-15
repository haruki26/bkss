use gpui::{AnyElement, App, Window, div, prelude::*};

use super::BacklightState;
use crate::IntegrationView;

impl IntegrationView for BacklightState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let mut content = div().flex().gap_2();
        if self.devices.is_empty() {
            content = content.child("No backlight");
        }
        for device in &self.devices {
            let brightness = if device.maximum == 0 {
                "Brightness unavailable".to_owned()
            } else {
                format!(
                    "{:.0}%",
                    device.brightness as f64 * 100. / device.maximum as f64
                )
            };
            content = content.child(format!("{}: {brightness}", device.name));
        }
        content.into_any_element()
    }
}

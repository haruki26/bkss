use gpui::{AnyElement, App, Window, div, prelude::*};

use super::WifiState;
use crate::IntegrationView;

impl IntegrationView for WifiState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let label = if !self.networking_enabled || !self.radio_enabled {
            "Wi-Fi off".to_owned()
        } else if self.devices.is_empty() {
            "No Wi-Fi device".to_owned()
        } else if let Some(access_point) = self.devices.iter().find_map(|device| {
            device.access_points.iter().find(|access_point| {
                Some(&access_point.path) == device.active_access_point.as_ref()
            })
        }) {
            format!(
                "{} · {}%",
                if access_point.display_name.is_empty() {
                    "Hidden network"
                } else {
                    &access_point.display_name
                },
                access_point.strength
            )
        } else {
            "Wi-Fi disconnected".to_owned()
        };
        div().child(label).into_any_element()
    }
}

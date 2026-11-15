use gpui::{AnyElement, App, Window, div, prelude::*};

use super::BluetoothState;
use crate::IntegrationView;

impl IntegrationView for BluetoothState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let label = if self.adapters.is_empty() {
            "No Bluetooth adapter".to_owned()
        } else if self.adapters.iter().all(|adapter| !adapter.powered) {
            "Bluetooth off".to_owned()
        } else {
            let connected = self
                .devices
                .iter()
                .filter(|device| device.connected)
                .count();
            format!("Bluetooth: {connected} connected")
        };
        div().child(label).into_any_element()
    }
}

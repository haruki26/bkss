use gpui::{AnyElement, App, Window, div, prelude::*};

use super::{BatteryState, ChargeState};
use crate::IntegrationView;

impl IntegrationView for BatteryState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let mut content = div().flex().gap_2();
        if self.batteries.is_empty() {
            content = content.child("No battery");
        }
        for battery in &self.batteries {
            let state = match battery.state {
                ChargeState::Charging => "Charging",
                ChargeState::Discharging => "Discharging",
                ChargeState::Empty => "Empty",
                ChargeState::FullyCharged => "Fully charged",
                ChargeState::PendingCharge => "Waiting to charge",
                ChargeState::PendingDischarge => "Waiting to discharge",
                ChargeState::Unknown(_) => "Status unavailable",
            };
            let charge = if battery.percentage.is_finite() {
                format!("{:.0}%", battery.percentage.clamp(0., 100.))
            } else {
                "Charge unavailable".to_owned()
            };
            content = content.child(format!(
                "{}: {charge} · {state}",
                battery.model.as_deref().unwrap_or("Battery")
            ));
        }
        content.into_any_element()
    }
}

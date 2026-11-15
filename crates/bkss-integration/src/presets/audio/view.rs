use gpui::{AnyElement, App, Window, div, prelude::*};

use super::AudioState;
use crate::IntegrationView;

impl IntegrationView for AudioState {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let output = self.default_sink.as_ref().map_or_else(
            || "No output device".to_owned(),
            |device| {
                if device.muted {
                    return "Output muted".to_owned();
                }
                let volume = if device.volume.is_empty() {
                    0.
                } else {
                    device.volume.iter().copied().sum::<f32>() / device.volume.len() as f32
                };
                format!("Output {:.0}%", volume * 100.)
            },
        );
        let input = self
            .default_source
            .as_ref()
            .map_or("No microphone", |device| {
                if device.muted {
                    "Microphone muted"
                } else {
                    "Microphone on"
                }
            });
        div()
            .flex()
            .gap_2()
            .child(output)
            .child(input)
            .into_any_element()
    }
}

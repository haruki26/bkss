//! Run `mise x -- cargo run -p bkss-ui --example source_display`.
//! Shows a live clock and a fixed demo temperature on each connected display;
//! stops after 30 seconds. The thermometer icon is GPUI geometry, not an SVG.
//! Applications can supply SVG assets through GPUI; BKSS bundles no icons.
use std::{io, time::Duration};

use bkss_core::{Shell, ShellError, Source, SourceDescriptor, SourcePublisher};
use bkss_integration::{Clock, ClockPrecision, ClockState, IntegrationView};
use bkss_ui::{KeyboardFocus, OverlayDefinition, OverlayRuntimeHandleExt, SourceDisplay};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, Render, Window, div, prelude::*, px, rgb,
};

#[derive(Clone)]
struct Temperature(f32);

impl IntegrationView for Temperature {
    fn render(&self, _: &mut Window, _: &mut App) -> AnyElement {
        let icon = div()
            .relative()
            .w(px(10.))
            .h(px(18.))
            .child(
                div()
                    .absolute()
                    .left(px(3.))
                    .top_0()
                    .w(px(4.))
                    .h(px(14.))
                    .rounded_full()
                    .bg(rgb(0xffffff)),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .size(px(10.))
                    .rounded_full()
                    .bg(rgb(0xffffff)),
            );
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(icon)
            .child(format!("Temperature {:.1} °C", self.0))
            .into_any_element()
    }
}

struct DemoThermometer;

impl Source for DemoThermometer {
    type Key = ();
    type Value = Temperature;
    type Error = io::Error;
    type Guard = ();

    fn key(&self) {}

    fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<()> {
        publisher.publish(Temperature(21.5), cx);
        Ok(())
    }
}

struct Readings {
    clock: Entity<SourceDisplay<ClockState>>,
    temperature: Entity<SourceDisplay<Temperature>>,
}

impl Render for Readings {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_6()
            .p_3()
            .text_color(rgb(0xffffff))
            .child(self.clock.clone())
            .child(self.temperature.clone())
    }
}

fn main() -> Result<(), ShellError> {
    let clock = SourceDescriptor::new(Clock::new(ClockPrecision::Minute));
    let thermometer = SourceDescriptor::new(DemoThermometer);
    Shell::run(move |runtime, cx| {
        let weak = runtime.downgrade();
        let overlay = runtime.mount_overlay(
            OverlayDefinition::new(KeyboardFocus::None),
            move |_, _, cx| {
                let runtime = weak.upgrade().ok_or_else(|| {
                    ShellError::setup(io::Error::other("runtime already stopped"))
                })?;
                let clock_binding = runtime.mount(&clock, cx)?;
                let temperature_binding = runtime.mount(&thermometer, cx)?;
                let clock = cx.new(|cx| {
                    SourceDisplay::new(clock_binding, cx)
                        .with_loading(|_, _| div().child("Loading clock…"))
                        .with_render(|state, _, _| div().child(state.time.clone()))
                });
                let temperature = cx.new(|cx| SourceDisplay::new(temperature_binding, cx));
                Ok(cx.new(|_| Readings { clock, temperature }))
            },
            cx,
        )?;
        for display in cx.displays() {
            overlay.open(display.id(), cx).map_err(ShellError::setup)?;
        }
        let weak = runtime.downgrade();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            cx.update(|cx| {
                if let Some(runtime) = weak.upgrade() {
                    runtime.shutdown(cx);
                }
            });
        })
        .detach();
        Ok(())
    })
}

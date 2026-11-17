//! Run `mise x -- cargo run -p bkss-ui --example source_display`.
//! Shows a live clock and a fixed demo temperature on each connected display;
//! stops after 30 seconds. The thermometer icon is GPUI geometry, not an SVG.
//! Applications can supply SVG assets through GPUI; BKSS bundles no icons.
use std::{io, time::Duration};

use bkss_core::{
    MountedBinding, ReactiveSubscription, Shell, ShellError, Source, SourceDescriptor,
    SourcePublisher, SourceState,
};
use bkss_integration::{Clock, ClockPrecision, ClockState, IntegrationView};
use bkss_ui::{DisplaySelection, Overlay};
use gpui::{AnyElement, App, AppContext, Context, Render, Window, div, prelude::*, px, rgb};

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
    clock: MountedBinding<ClockState>,
    temperature: MountedBinding<Temperature>,
    _clock_subscription: ReactiveSubscription,
    _temperature_subscription: ReactiveSubscription,
}

fn source_view<T: Clone + IntegrationView>(
    binding: &MountedBinding<T>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match binding.reactive().get(cx) {
        SourceState::Loading => div().child("Loading…").into_any_element(),
        SourceState::Ready(value) => value.render(window, cx),
    }
}

impl Render for Readings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_6()
            .p_3()
            .text_color(rgb(0xffffff))
            .child(source_view(&self.clock, window, cx))
            .child(source_view(&self.temperature, window, cx))
    }
}

fn main() -> Result<(), ShellError> {
    let clock = SourceDescriptor::new(Clock::new(ClockPrecision::Minute));
    let thermometer = SourceDescriptor::new(DemoThermometer);
    Shell::run(move |runtime, cx| {
        let clock = clock.clone();
        let thermometer = thermometer.clone();
        let weak = runtime.downgrade();
        let overlay = Overlay::new()
            .outputs(DisplaySelection::AllConnected)
            .mount(
                &runtime,
                move |_, _, _, cx| {
                    let runtime = weak
                        .upgrade()
                        .ok_or_else(|| ShellError::setup(io::Error::other("runtime stopped")))?;
                    let clock = runtime.mount(&clock, cx)?;
                    let temperature = runtime.mount(&thermometer, cx)?;
                    Ok(cx.new(|cx| Readings {
                        _clock_subscription: clock.reactive().observe(cx, |_, _, cx| cx.notify()),
                        _temperature_subscription: temperature
                            .reactive()
                            .observe(cx, |_, _, cx| cx.notify()),
                        clock,
                        temperature,
                    }))
                },
                cx,
            )?;
        for display in cx.displays() {
            overlay.show(display.id(), cx).map_err(ShellError::setup)?;
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

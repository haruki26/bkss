use std::{path::PathBuf, rc::Rc, sync::Arc};

use bkss_core::{RuntimeHandle, ShellError};
use bkss_integration::{
    ActiveWorkspace, AudioState, BacklightState, BatteryState, BluetoothState, ClockState,
    HyprlandClient, TrayState, TrayStatus, Usage, WifiState, Workspace,
};
use bkss_ui::effects::{GlassElement, GlassState, MountedGlass};
use gpui::{
    App, AppContext, Context, DisplayId, Entity, Image, ImageSource, IntoElement, MouseButton,
    Render, Subscription, Window, div, img, prelude::*, px, rgb, rgba,
};

use super::{
    commands::CommandListener,
    desktop::{Desktop, Panel},
    live::Live,
    notifications::NotificationFeed,
    theme::{self, ACCENT, DATA_FONT, FONT, Icon, MUTED, TEXT, button, divider, icon, label, row},
    workspace_strip::WorkspaceStrip,
};
use crate::sources::{GpuState, WeatherState, WeatherStatus};

const BAR_VERTICAL_PADDING: f32 = 4.;
const BAR_CONTROL_HEIGHT: f32 = theme::BAR_HEIGHT - 2. * BAR_VERTICAL_PADDING;

fn bar_island(element: GlassElement) -> GlassElement {
    element
        .h(px(theme::BAR_HEIGHT))
        .min_h(px(theme::BAR_HEIGHT))
        .max_h(px(theme::BAR_HEIGHT))
        .py(px(BAR_VERTICAL_PADDING))
        .flex()
        .items_center()
}

fn bar_control(element: gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div> {
    element
        .h(px(BAR_CONTROL_HEIGHT))
        .min_h(px(BAR_CONTROL_HEIGHT))
        .max_h(px(BAR_CONTROL_HEIGHT))
        .py_0()
}

pub struct TopBar {
    desktop: Rc<Desktop>,
    display: DisplayId,
    glass: MountedGlass,
    workspaces: Entity<WorkspaceStrip>,
    wifi: Live<WifiState>,
    bluetooth: Live<BluetoothState>,
    backlight: Live<BacklightState>,
    audio: Live<AudioState>,
    tray: Live<TrayState>,
}

impl TopBar {
    pub fn mount(
        desktop: Rc<Desktop>,
        display: DisplayId,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<Self>, ShellError> {
        let services = &desktop.services;
        let hyprland = runtime.mount(&services.hyprland, cx)?;
        let wifi = runtime.mount(&services.wifi, cx)?;
        let bluetooth = runtime.mount(&services.bluetooth, cx)?;
        let backlight = runtime.mount(&services.backlight, cx)?;
        let audio = runtime.mount(&services.audio, cx)?;
        let tray = runtime.mount(&services.tray, cx)?;
        let workspaces = cx.new(|cx| WorkspaceStrip::new(desktop.clone(), hyprland, cx));
        Ok(cx.new(|cx| Self {
            display,
            glass: MountedGlass::new(desktop.bar_style.clone(), cx),
            workspaces,
            wifi: Live::new(wifi, cx),
            bluetooth: Live::new(bluetooth, cx),
            backlight: Live::new(backlight, cx),
            audio: Live::new(audio, cx),
            tray: Live::new(tray, cx),
            desktop,
        }))
    }
}

impl Render for TopBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let strength = self
            .wifi
            .get(cx)
            .filter(|state| state.networking_enabled && state.radio_enabled)
            .and_then(|state| {
                state.devices.iter().find_map(|device| {
                    device.active_access_point.as_ref().and_then(|active| {
                        device
                            .access_points
                            .iter()
                            .find(|ap| ap.path == *active)
                            .map(|ap| ap.strength)
                    })
                })
            });
        let bluetooth_state = self.bluetooth.get(cx);
        let bluetooth_on = bluetooth_state
            .as_ref()
            .is_some_and(|state| state.adapters.iter().any(|adapter| adapter.powered));
        let brightness = self
            .backlight
            .get(cx)
            .and_then(|state| state.devices.into_iter().find(|device| device.maximum > 0))
            .map(|device| {
                format!(
                    "{:.0}%",
                    device.brightness as f64 / device.maximum as f64 * 100.
                )
            })
            .unwrap_or_else(|| "—".into());

        let mut audio = row().gap_2();
        if let Some(state) = self.audio.get(cx) {
            if let Some(sink) = state.default_sink {
                let mean = sink.volume.iter().sum::<f32>() / sink.volume.len().max(1) as f32;
                let muted = sink.muted || mean == 0.;
                audio = audio.child(
                    row()
                        .gap_1()
                        .child(theme::symbolic(
                            volume_glyph(mean, muted),
                            if muted { MUTED } else { ACCENT },
                            24.,
                        ))
                        .when(mean > 0., |view| {
                            view.child(label(format!("{:.0}%", mean * 100.)).text_color(rgb(MUTED)))
                        }),
                );
            }
            if let Some(source) = state.default_source {
                audio = audio.child(theme::symbolic(
                    if source.muted {
                        "yh-mic-off-symbolic"
                    } else {
                        "yh-mic-on-symbolic"
                    },
                    if source.muted { MUTED } else { ACCENT },
                    18.,
                ));
            }
        }

        let mut tray = row().gap_1().flex_shrink_0();
        if let Some(state) = self.tray.get(cx) {
            for item in state
                .items
                .into_iter()
                .filter(|i| i.status != TrayStatus::Passive)
            {
                let desktop = self.desktop.clone();
                let display = self.display;
                let action_item = item.clone();
                let menu_item = item.clone();
                let menu_desktop = desktop.clone();
                let title = if item.title.is_empty() {
                    item.id.owner.clone()
                } else {
                    item.title.clone()
                };
                let content = if item.status == TrayStatus::NeedsAttention {
                    let image = item.attention_icon.as_ref().or(item.icon.as_ref());
                    if let Some(image) = image.and_then(theme::tray_image) {
                        img(image).size(px(18.)).into_any_element()
                    } else {
                        desktop.app_icon(&item.icon_name, 18.)
                    }
                } else {
                    let (source, fallback) = main_tray_icon_sources(
                        desktop.icon_path(&item.icon_name),
                        item.icon.as_ref().and_then(theme::tray_image),
                    );
                    if let Some(source) = source {
                        img(source)
                            .size(px(18.))
                            .with_fallback(move || {
                                if let Some(image) = &fallback {
                                    img(image.clone()).size(px(18.)).into_any_element()
                                } else {
                                    icon(Icon::App).size(px(18.)).into_any_element()
                                }
                            })
                            .into_any_element()
                    } else {
                        icon(Icon::App).size(px(18.)).into_any_element()
                    }
                };
                tray = tray.child(
                    bar_control(button(
                        gpui::ElementId::Name(
                            format!("tray-{}{}", item.id.owner, item.id.path).into(),
                        ),
                        title,
                        move |_, cx| {
                            if action_item.item_is_menu {
                                desktop.open_tray(display, action_item.clone(), cx);
                            } else {
                                theme::dispatch(
                                    action_item.activate(0, 0),
                                    "トレイを開けません",
                                    &desktop.feedback,
                                    cx,
                                );
                            }
                        },
                    ))
                    .px_1()
                    .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                        cx.stop_propagation();
                        menu_desktop.open_tray(display, menu_item.clone(), cx);
                    })
                    .child(content),
                );
            }
        }

        let right = row().gap_3().flex_shrink_0().child(
            bar_island(self.glass.element(GlassState::default(), cx))
                .px_2()
                .gap_2()
                .child(
                    row()
                        .gap_2()
                        .px_1()
                        .child(theme::symbolic(
                            wifi_glyph(strength),
                            if strength.is_some() {
                                theme::SECONDARY
                            } else {
                                MUTED
                            },
                            24.,
                        ))
                        .child(theme::symbolic(
                            if bluetooth_on {
                                "yh-bluetooth-on-symbolic"
                            } else {
                                "yh-bluetooth-off-symbolic"
                            },
                            if bluetooth_on {
                                theme::SECONDARY
                            } else {
                                MUTED
                            },
                            24.,
                        )),
                )
                .child(divider())
                .child(
                    row()
                        .gap_2()
                        .px_1()
                        .child(
                            row()
                                .gap_1()
                                .child(theme::symbolic("yh-sun-symbolic", ACCENT, 24.))
                                .child(label(brightness).text_color(rgb(MUTED))),
                        )
                        .child(audio),
                )
                .child(divider())
                .child(tray.px_1()),
        );

        div()
            .w_full()
            .h_full()
            .font_family(FONT)
            .text_size(px(13.))
            .text_color(rgb(TEXT))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(theme::BAR_HEIGHT))
                    .child(self.workspaces.clone()),
            )
            .child(right)
    }
}

fn main_tray_icon_sources(
    named_path: Option<PathBuf>,
    pixmap: Option<Arc<Image>>,
) -> (Option<ImageSource>, Option<Arc<Image>>) {
    match named_path {
        Some(path) => (Some(path.into()), pixmap),
        None => (pixmap.map(ImageSource::from), None),
    }
}

pub(super) fn ordered_workspaces(workspaces: &[Workspace]) -> Vec<&Workspace> {
    let mut ordered: Vec<_> = workspaces.iter().collect();
    ordered.sort_by(|left, right| {
        (left.id.is_none(), left.id, &left.name, &left.address).cmp(&(
            right.id.is_none(),
            right.id,
            &right.name,
            &right.address,
        ))
    });
    ordered
}

pub(super) fn workspace_is_active(workspace: &Workspace, active: Option<&ActiveWorkspace>) -> bool {
    matches!(active, Some(ActiveWorkspace::Normal(active)) if active.address == workspace.address)
}

pub(super) fn client_is_on_workspace(client: &HyprlandClient, workspace: &Workspace) -> bool {
    client.workspace_address.as_deref() == Some(workspace.address.as_str())
}

pub(super) fn workspace_target(workspace: &Workspace) -> String {
    match workspace.id {
        Some(id) => id.to_string(),
        None => format!("name:{}", workspace.name),
    }
}

fn wifi_glyph(strength: Option<u8>) -> &'static str {
    match strength {
        None => "yh-wifi-off-symbolic",
        Some(80..) => "yh-wifi-1-symbolic",
        Some(60..) => "yh-wifi-2-symbolic",
        Some(40..) => "yh-wifi-3-symbolic",
        Some(20..) => "yh-wifi-4-symbolic",
        Some(_) => "yh-wifi-5-symbolic",
    }
}

fn volume_glyph(volume: f32, muted: bool) -> &'static str {
    if muted {
        "yh-volume-off-symbolic"
    } else if volume / 1.4 >= 0.66 {
        "yh-volume-1-symbolic"
    } else if volume / 1.4 >= 0.33 {
        "yh-volume-2-symbolic"
    } else {
        "yh-volume-3-symbolic"
    }
}

fn battery_glyph(percentage: f64, charging: bool) -> &'static str {
    if charging {
        "yh-battery-charging-symbolic"
    } else if percentage >= 75. {
        "yh-battery-1-symbolic"
    } else if percentage >= 50. {
        "yh-battery-2-symbolic"
    } else if percentage >= 25. {
        "yh-battery-3-symbolic"
    } else {
        "yh-battery-4-symbolic"
    }
}

fn weather_text(weather: &WeatherState) -> String {
    match weather.status {
        WeatherStatus::Loading => "…".into(),
        WeatherStatus::NotConfigured => "--".into(),
        WeatherStatus::Error => "ERR".into(),
        WeatherStatus::Ready => weather.temperature.clone(),
    }
}

fn weather_indicator(weather: WeatherState) -> gpui::Div {
    row()
        .h(px(BAR_CONTROL_HEIGHT))
        .gap_1()
        .child(theme::symbolic(&weather.icon_name, theme::PRIMARY, 18.))
        .child(
            label(weather_text(&weather))
                .text_size(px(16.))
                .text_color(rgb(if weather.status == WeatherStatus::Error {
                    theme::DANGER
                } else {
                    MUTED
                })),
        )
}

fn gpu_usage(state: Option<GpuState>) -> String {
    let Some(mut state) = state.filter(|state| !state.devices.is_empty()) else {
        return "—".into();
    };
    state
        .devices
        .sort_by(|left, right| left.name.cmp(&right.name));
    state
        .devices
        .iter()
        .map(|device| {
            device
                .metrics
                .utilization_percent
                .filter(|percent| percent.is_finite() && (0.0..=100.0).contains(percent))
                .map(|percent| format!("{percent:.1}%"))
                .unwrap_or_else(|| "—".into())
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

pub struct BottomBar {
    desktop: Rc<Desktop>,
    display: DisplayId,
    glass: MountedGlass,
    clock: Live<ClockState>,
    cpu: Live<Usage>,
    gpu: Live<GpuState>,
    memory: Live<Usage>,
    storage: Live<Usage>,
    battery: Live<BatteryState>,
    weather: Live<WeatherState>,
    feed: Entity<NotificationFeed>,
    _feed_subscription: Subscription,
    _feedback_subscription: Subscription,
    _commands: Entity<CommandListener>,
}

impl BottomBar {
    pub fn mount(
        desktop: Rc<Desktop>,
        display: DisplayId,
        feed: Entity<NotificationFeed>,
        runtime: &RuntimeHandle,
        cx: &mut App,
    ) -> Result<Entity<Self>, ShellError> {
        let services = &desktop.services;
        let clock = runtime.mount(&services.clock, cx)?;
        let cpu = runtime.mount(&services.cpu, cx)?;
        let gpu = runtime.mount(&services.gpu, cx)?;
        let memory = runtime.mount(&services.memory, cx)?;
        let storage = runtime.mount(&services.storage, cx)?;
        let battery = runtime.mount(&services.battery, cx)?;
        let weather = runtime.mount(&services.weather, cx)?;
        let commands = desktop.command_listener(runtime, cx)?;
        Ok(cx.new(|cx: &mut Context<Self>| Self {
            display,
            glass: MountedGlass::new(desktop.bar_style.clone(), cx),
            clock: Live::new(clock, cx),
            cpu: Live::new(cpu, cx),
            gpu: Live::new(gpu, cx),
            memory: Live::new(memory, cx),
            storage: Live::new(storage, cx),
            battery: Live::new(battery, cx),
            weather: Live::new(weather, cx),
            _feed_subscription: cx.observe(&feed, |_, _, cx| cx.notify()),
            _feedback_subscription: cx.observe(&desktop.feedback, |_, _, cx| cx.notify()),
            _commands: commands,
            feed,
            desktop,
        }))
    }
}

impl Render for BottomBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desktop = self.desktop.clone();
        let display = self.display;
        let lock = self.desktop.clone();
        let calendar = self.desktop.clone();
        let notifications = self.desktop.clone();
        let clock = self.clock.get(cx);
        let weather = self.weather.get(cx);
        let count = self.feed.read(cx).notifications(cx).len();
        let battery = self
            .battery
            .get(cx)
            .and_then(|state| state.batteries.into_iter().next());
        let date = clock
            .as_ref()
            .and_then(|clock| chrono::NaiveDate::parse_from_str(&clock.date, "%Y-%m-%d").ok())
            .map(|date| date.format("%Y/%m/%d %a").to_string())
            .unwrap_or_else(|| "----/--/-- ---".into());
        let calendar_open = self.desktop.is_open(Panel::Calendar, display);
        let notifications_open = self.desktop.is_open(Panel::Notifications, display);
        let mut start = row()
            .gap_1()
            .child(
                bar_control(button("power-menu", "電源メニュー", move |_, cx| {
                    desktop.toggle(Panel::Power, display, cx)
                }))
                .px_1()
                .child(icon(Icon::Power)),
            )
            .child(
                bar_control(button("lock", "画面をロック", move |_, cx| {
                    lock.lock(cx)
                }))
                .px_1()
                .child(icon(Icon::Lock)),
            )
            .child(divider());
        if let Some(error) = &self.desktop.feedback.read(cx).error {
            let feedback = self.desktop.feedback.clone();
            start = start.child(
                bar_control(button(
                    "bar-error",
                    format!("{error}。エラーを閉じる"),
                    move |_, cx| {
                        feedback.update(cx, |state, cx| {
                            state.error = None;
                            cx.notify();
                        });
                    },
                ))
                .max_w(px(500.))
                .text_color(rgb(theme::DANGER))
                .child(label(format!("{error}  ×")).truncate()),
            );
        } else {
            let gib = 1024_f64.powi(3);
            for (glyph, value) in [
                (
                    "yh-cpu-symbolic",
                    self.cpu
                        .get(cx)
                        .map(|usage| format!("{:.1}%", usage.percent)),
                ),
                ("yh-gpu-symbolic", Some(gpu_usage(self.gpu.get(cx)))),
                (
                    "yh-memory-symbolic",
                    self.memory.get(cx).map(|usage| {
                        format!(
                            "{:.1}/{:.0} GiB",
                            usage.used as f64 / gib,
                            usage.total as f64 / gib
                        )
                    }),
                ),
                (
                    "yh-storage-symbolic",
                    self.storage.get(cx).map(|usage| {
                        format!(
                            "{:.1}/{:.1} GiB",
                            usage.used as f64 / gib,
                            usage.total as f64 / gib
                        )
                    }),
                ),
            ] {
                start = start.child(
                    row()
                        .gap_1()
                        .px_2()
                        .child(theme::symbolic(glyph, ACCENT, 14.))
                        .child(label(value.unwrap_or_else(|| "N/A".into())).font_family(DATA_FONT)),
                );
            }
        }

        let end = row()
            .gap_2()
            .when_some(battery, |view, battery| {
                view.child(
                    row()
                        .gap_1()
                        .child(theme::symbolic(
                            battery_glyph(
                                battery.percentage,
                                battery.state == bkss_integration::ChargeState::Charging,
                            ),
                            if battery.percentage <= 25. {
                                theme::DANGER
                            } else {
                                theme::SAFE
                            },
                            24.,
                        ))
                        .child(label(format!("{:.0}%", battery.percentage)).text_color(rgb(MUTED))),
                )
            })
            .child(divider())
            .child(
                row()
                    .gap_1()
                    .child(
                        bar_control(button("calendar", "カレンダー", move |_, cx| {
                            calendar.toggle(Panel::Calendar, display, cx)
                        }))
                        .px_1()
                        .rounded_md()
                        .when(calendar_open, |view| view.bg(rgba(0xcba6f72e)))
                        .child(theme::symbolic("yh-calendar-symbolic", theme::PRIMARY, 18.))
                        .child(div().text_size(px(16.)).child(date)),
                    )
                    .child(
                        row()
                            .gap_1()
                            .child(theme::symbolic("yh-clock-symbolic", theme::PRIMARY, 18.))
                            .child(
                                div().text_size(px(16.)).child(
                                    clock
                                        .map(|clock| clock.time)
                                        .unwrap_or_else(|| "--:--:--".into()),
                                ),
                            ),
                    ),
            )
            .when_some(weather, |view, weather| {
                view.child(weather_indicator(weather))
            })
            .child(divider())
            .child(
                bar_control(button(
                    "notification-center",
                    format!("通知 {count} 件"),
                    move |_, cx| notifications.toggle(Panel::Notifications, display, cx),
                ))
                .relative()
                .px_1()
                .rounded_md()
                .when(notifications_open, |view| view.bg(rgba(0xcba6f72e)))
                .child(theme::symbolic(
                    "yh-bell-symbolic",
                    if notifications_open {
                        theme::PRIMARY
                    } else {
                        MUTED
                    },
                    24.,
                ))
                .when(count > 0, |view| {
                    view.child(
                        div()
                            .absolute()
                            .right_0()
                            .top(px(3.))
                            .min_w(px(13.))
                            .h(px(13.))
                            .px(px(2.))
                            .rounded_full()
                            .bg(rgb(theme::DANGER))
                            .text_color(rgb(0x1e1e2e))
                            .text_size(px(7.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if count > 9 {
                                "9+".into()
                            } else {
                                count.to_string()
                            }),
                    )
                }),
            );

        div()
            .w_full()
            .h_full()
            .font_family(FONT)
            .text_color(rgb(TEXT))
            .flex()
            .items_center()
            .gap_2()
            .child(
                bar_island(self.glass.element(GlassState::default(), cx))
                    .px_4()
                    .child(start),
            )
            .child(div().flex_1().min_w(px(32.)))
            .child(
                bar_island(self.glass.element(GlassState::default(), cx))
                    .px_4()
                    .child(end),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{Gpu, GpuMetrics, GpuSupport};
    use bkss_integration::SpecialWorkspace;

    fn workspace(address: &str, id: Option<i64>, name: &str) -> Workspace {
        Workspace {
            address: address.into(),
            id,
            name: name.into(),
        }
    }

    #[test]
    fn bar_islands_match_workspace_height_and_fit_their_controls() {
        assert_eq!(theme::BAR_HEIGHT, 40.);
        assert_eq!(BAR_CONTROL_HEIGHT, 32.);
        for mut island in [
            bar_island(GlassElement::new()).px_2(),
            bar_island(GlassElement::new()).px_4(),
            bar_island(GlassElement::new()).px_4(),
        ] {
            let style = island.style();
            let height = Some(px(40.).into());
            assert_eq!(style.size.height, height);
            assert_eq!(style.min_size.height, height);
            assert_eq!(style.max_size.height, height);
            assert_eq!(style.padding.top, Some(px(4.).into()));
            assert_eq!(style.padding.bottom, Some(px(4.).into()));
            assert_eq!(style.align_items, Some(gpui::AlignItems::Center));
        }
        let mut control = bar_control(button("bar-control", "バー操作", |_, _| {}));
        let style = control.style();
        let height = Some(px(32.).into());
        assert_eq!(style.size.height, height);
        assert_eq!(style.min_size.height, height);
        assert_eq!(style.max_size.height, height);
    }

    #[test]
    fn relocated_weather_preserves_ready_loading_missing_and_error_states() {
        for (status, expected) in [
            (WeatherStatus::Ready, "14°C"),
            (WeatherStatus::Loading, "…"),
            (WeatherStatus::NotConfigured, "--"),
            (WeatherStatus::Error, "ERR"),
        ] {
            let weather = WeatherState {
                icon_name: "yh-weather-cloudy-day-symbolic".into(),
                temperature: "14°C".into(),
                status,
            };
            assert_eq!(weather_text(&weather), expected);
            let mut indicator = weather_indicator(weather);
            assert_eq!(indicator.style().size.height, Some(px(32.).into()));
        }
    }

    #[test]
    fn gpu_usage_reports_only_real_percentages_and_keeps_missing_values_distinct_from_zero() {
        let device = |name: &str, utilization_percent| Gpu {
            name: name.into(),
            support: GpuSupport::Amd,
            metrics: GpuMetrics {
                utilization_percent,
            },
        };
        assert_eq!(gpu_usage(None), "—");
        assert_eq!(gpu_usage(Some(GpuState::default())), "—");
        for (percent, expected) in [
            (None, "—"),
            (Some(0.), "0.0%"),
            (Some(67.5), "67.5%"),
            (Some(f32::NAN), "—"),
            (Some(f32::INFINITY), "—"),
            (Some(-1.), "—"),
            (Some(101.), "—"),
        ] {
            assert_eq!(
                gpu_usage(Some(GpuState {
                    devices: vec![device("card0", percent)]
                })),
                expected
            );
        }
        assert_eq!(
            gpu_usage(Some(GpuState {
                devices: vec![device("card1", None), device("card0", Some(42.))],
            })),
            "42.0% / —"
        );
    }

    #[test]
    fn ags_status_glyphs_follow_the_original_thresholds() {
        assert_eq!(wifi_glyph(None), "yh-wifi-off-symbolic");
        assert_eq!(wifi_glyph(Some(80)), "yh-wifi-1-symbolic");
        assert_eq!(wifi_glyph(Some(60)), "yh-wifi-2-symbolic");
        assert_eq!(wifi_glyph(Some(40)), "yh-wifi-3-symbolic");
        assert_eq!(wifi_glyph(Some(20)), "yh-wifi-4-symbolic");
        assert_eq!(wifi_glyph(Some(19)), "yh-wifi-5-symbolic");
        assert_eq!(volume_glyph(1., true), "yh-volume-off-symbolic");
        assert_eq!(volume_glyph(1., false), "yh-volume-1-symbolic");
        assert_eq!(volume_glyph(0.5, false), "yh-volume-2-symbolic");
        assert_eq!(volume_glyph(0.2, false), "yh-volume-3-symbolic");
        assert_eq!(battery_glyph(10., true), "yh-battery-charging-symbolic");
        assert_eq!(battery_glyph(75., false), "yh-battery-1-symbolic");
        assert_eq!(battery_glyph(50., false), "yh-battery-2-symbolic");
        assert_eq!(battery_glyph(25., false), "yh-battery-3-symbolic");
        assert_eq!(battery_glyph(24., false), "yh-battery-4-symbolic");
    }

    #[test]
    fn workspace_badges_use_only_clients_with_the_same_workspace_address() {
        let normal = workspace("normal", Some(1), "1");
        let clients = [
            HyprlandClient {
                address: "a".into(),
                class: "app-a".into(),
                workspace_address: Some("normal".into()),
            },
            HyprlandClient {
                address: "d".into(),
                class: "app-a".into(),
                workspace_address: Some("normal".into()),
            },
            HyprlandClient {
                address: "b".into(),
                class: "app-a".into(),
                workspace_address: Some("special".into()),
            },
            HyprlandClient {
                address: "c".into(),
                class: "app-a".into(),
                workspace_address: None,
            },
        ];
        assert_eq!(
            clients
                .iter()
                .filter(|client| client_is_on_workspace(client, &normal))
                .map(|client| client.address.as_str())
                .collect::<Vec<_>>(),
            ["a", "d"]
        );
    }

    #[test]
    fn main_tray_name_wins_and_keeps_pixmap_for_load_failure() {
        let pixmap = theme::tray_image(&bkss_integration::TrayIcon {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 255],
        })
        .unwrap();
        let resolved_path = PathBuf::from("/icon-theme/resolved.svg");
        let (source, fallback) =
            main_tray_icon_sources(Some(resolved_path.clone()), Some(pixmap.clone()));
        assert!(matches!(
            source,
            Some(ImageSource::Resource(gpui::Resource::Path(path)))
                if path.as_ref() == resolved_path.as_path()
        ));
        assert!(Arc::ptr_eq(fallback.as_ref().unwrap(), &pixmap));

        let (source, fallback) = main_tray_icon_sources(None, Some(pixmap.clone()));
        assert!(matches!(source, Some(ImageSource::Image(image)) if Arc::ptr_eq(&image, &pixmap)));
        assert!(fallback.is_none());

        let (source, fallback) = main_tray_icon_sources(Some(resolved_path), None);
        assert!(matches!(
            source,
            Some(ImageSource::Resource(gpui::Resource::Path(_)))
        ));
        assert!(fallback.is_none());

        let (source, fallback) = main_tray_icon_sources(None, None);
        assert!(source.is_none());
        assert!(fallback.is_none());
    }

    #[test]
    fn normal_workspaces_sort_numbered_then_named_without_inventing_ids() {
        let workspaces = [
            workspace("beta", None, "Beta"),
            workspace("ten", Some(10), "10"),
            workspace("alpha-z", None, "Alpha"),
            workspace("two", Some(2), "renamed"),
            workspace("alpha-a", None, "Alpha"),
        ];
        let ordered = ordered_workspaces(&workspaces);
        assert_eq!(
            ordered
                .iter()
                .map(|workspace| workspace.address.as_str())
                .collect::<Vec<_>>(),
            ["two", "ten", "alpha-a", "alpha-z", "beta"],
        );
        assert_eq!(ordered[0].name, "renamed");
        assert_eq!(ordered[2].id, None);
    }

    #[test]
    fn only_an_active_normal_with_the_same_address_is_selected() {
        let first = workspace("first", None, "Alpha");
        let second = workspace("second", None, "Beta");
        let active = ActiveWorkspace::Normal(first.clone());
        assert!(workspace_is_active(&first, Some(&active)));
        assert!(!workspace_is_active(&second, Some(&active)));
        assert!(!workspace_is_active(&first, None));

        let special = ActiveWorkspace::Special(SpecialWorkspace {
            address: first.address.clone(),
            name: "scratchpad".into(),
        });
        assert!(!workspace_is_active(&first, Some(&special)));
        assert!(!workspace_is_active(&second, Some(&special)));

        let numbered = workspace("old-address", Some(2), "2");
        let replaced = ActiveWorkspace::Normal(workspace("new-address", Some(2), "2"));
        assert!(!workspace_is_active(&numbered, Some(&replaced)));
    }

    #[test]
    fn switch_targets_distinguish_numbered_and_named_normals() {
        assert_eq!(workspace_target(&workspace("two", Some(2), "renamed")), "2");
        assert_eq!(
            workspace_target(&workspace("chat", None, "chat")),
            "name:chat"
        );
        assert_eq!(
            workspace_target(&workspace("named-two", None, "2")),
            "name:2"
        );
        assert_eq!(
            workspace_target(&workspace("literal-name", None, "name:chat")),
            "name:name:chat",
        );
    }
}

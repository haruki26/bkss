use gpui::{
    App, FontWeight, IntoElement, ParentElement, Pixels, RenderOnce, Styled, Window, div, px, rgb,
};

use super::theme::{ACCENT, DANGER, MUTED, SECONDARY};

const ICON_FONT: &str = "Symbols Nerd Font";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Icon {
    Bell,
    Clock,
    Calendar,
    Power,
    Reboot,
    Lock,
    Close,
    Left,
    Right,
    App,
    Cpu,
    Memory,
    Storage,
    Sun,
    VolumeOff,
    VolumeLow,
    VolumeMedium,
    VolumeHigh,
    MicOn,
    MicOff,
    BluetoothOn,
    BluetoothOff,
    WifiOff,
    Wifi1,
    Wifi2,
    Wifi3,
    Wifi4,
    Wifi5,
    BatteryCharging,
    BatteryLow,
    BatteryMedium,
    BatteryHigh,
    BatteryVeryLow,
    WeatherClearDay,
    WeatherClearNight,
    WeatherCloudyDay,
    WeatherCloudyNight,
    WeatherShowerRain,
    WeatherRainDay,
    WeatherRainNight,
    WeatherThunder,
    WeatherSnow,
    WeatherFog,
    WeatherUnknown,
    Gpu,
}

impl Icon {
    pub fn render(self, color: u32, size: f32) -> GlyphIcon {
        let glyph = match self {
            Icon::Bell => "\u{f009c}",
            Icon::Clock => "\u{f0150}",
            Icon::Calendar => "\u{f0e18}",
            Icon::Power => "\u{f0425}",
            Icon::Reboot => "\u{f0709}",
            Icon::Lock => "\u{f0341}",
            Icon::Close => "\u{f0156}",
            Icon::Left => "\u{f0141}",
            Icon::Right => "\u{f0142}",
            Icon::App => "\u{f0614}",
            Icon::Cpu => "\u{f4bc}",
            Icon::Memory => "\u{efc5}",
            Icon::Storage => "\u{f0a0}",
            Icon::Sun => "\u{f0599}",
            Icon::VolumeOff => "\u{f075f}",
            Icon::VolumeLow => "\u{f057e}",
            Icon::VolumeMedium => "\u{f0580}",
            Icon::VolumeHigh => "\u{f057f}",
            Icon::MicOn => "\u{f036e}",
            Icon::MicOff => "\u{f036d}",
            Icon::BluetoothOn => "\u{f00af}",
            Icon::BluetoothOff => "\u{f00b2}",
            Icon::WifiOff => "\u{f092e}",
            Icon::Wifi1 => "\u{f0928}",
            Icon::Wifi2 => "\u{f0925}",
            Icon::Wifi3 => "\u{f0922}",
            Icon::Wifi4 => "\u{f091f}",
            Icon::Wifi5 => "\u{f092f}",
            Icon::BatteryCharging => "\u{f089f}",
            Icon::BatteryHigh => "\u{f0082}",
            Icon::BatteryMedium => "\u{f007f}",
            Icon::BatteryLow => "\u{f007d}",
            Icon::BatteryVeryLow => "\u{f007a}",
            Icon::WeatherClearDay => "\u{e30d}",
            Icon::WeatherClearNight => "\u{e32b}",
            Icon::WeatherCloudyDay => "\u{e302}",
            Icon::WeatherCloudyNight => "\u{e37e}",
            Icon::WeatherShowerRain => "\u{e319}",
            Icon::WeatherRainDay => "\u{e308}",
            Icon::WeatherRainNight => "\u{e325}",
            Icon::WeatherThunder => "\u{e31d}",
            Icon::WeatherSnow => "\u{e31a}",
            Icon::WeatherFog => "\u{e313}",
            Icon::WeatherUnknown => "\u{e312}",
            Icon::Gpu => "\u{f08ae}",
        };
        GlyphIcon {
            glyph,
            color,
            size: px(size),
        }
    }
}

pub fn icon(kind: Icon) -> GlyphIcon {
    match kind {
        Icon::Bell => kind.render(MUTED, 24.),
        Icon::Power => kind.render(DANGER, 18.),
        Icon::Reboot => kind.render(ACCENT, 24.),
        Icon::Lock => kind.render(SECONDARY, 18.),
        _ => kind.render(MUTED, 16.),
    }
}

#[derive(IntoElement)]
pub struct GlyphIcon {
    glyph: &'static str,
    color: u32,
    size: Pixels,
}

impl GlyphIcon {
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for GlyphIcon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .size(self.size)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .font_family(ICON_FONT)
            .font_weight(FontWeight::NORMAL)
            .text_color(rgb(self.color))
            .text_size(self.size)
            .line_height(self.size)
            .whitespace_nowrap()
            .child(self.glyph)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_glyph_keeps_requested_color_and_size() {
        let gpu = Icon::Gpu.render(ACCENT, 14.);
        assert_eq!(gpu.glyph, "\u{f08ae}");
        assert_eq!(gpu.size, px(14.));
        assert_eq!(gpu.color, ACCENT);
    }
}

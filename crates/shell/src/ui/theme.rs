use bkss_ui::effects::{
    GlassBorder, GlassColor, GlassInnerEffect, GlassRadius, GlassRim, GlassStyle, GlassStyleError,
};

pub const TEXT: u32 = 0xcdd6f4;
pub const MUTED: u32 = 0xa6adc8;
pub const ACCENT: u32 = 0x94e2d5;
pub const PRIMARY: u32 = 0xcba6f7;
pub const SECONDARY: u32 = 0x89dceb;
pub const SAFE: u32 = 0xa6e3a1;
pub const BLUE: u32 = 0x89b4fa;
pub const DANGER: u32 = 0xf38ba8;
pub const FONT: &str = "HackGen";
pub const DATA_FONT: &str = "HackGen";
pub const BAR_HEIGHT: f32 = 40.;
pub const MARGIN: f32 = 8.;
pub const PANEL_WIDTH: f32 = 380.;
pub const PANEL_MAX_HEIGHT: f32 = 760.;
const GLASS_TINT_ALPHA: f32 = 0.30;

pub fn glass_style(radius: f32) -> Result<GlassStyle, GlassStyleError> {
    Ok(GlassStyle::default()
        .with_radius(GlassRadius::new(radius)?)
        .with_tint(GlassColor::rgba(0.118, 0.118, 0.18, GLASS_TINT_ALPHA)?)
        .with_border(Some(GlassBorder::new(
            0.75,
            GlassColor::rgba(0.88, 0.95, 1., 0.25)?,
        )?))
        .with_rim(Some(GlassRim::new(
            1.5,
            0.5,
            GlassColor::rgba(0.88, 0.95, 1., 0.06)?,
        )?))
        .with_inner_highlight(Some(GlassInnerEffect::new(
            GlassColor::rgba(0.9, 0.97, 1., 0.12)?,
            [0., 1.],
            3.,
            0.,
        )?))
        .with_inner_shadow(Some(GlassInnerEffect::new(
            GlassColor::rgba(0., 0.025, 0.05, 0.16)?,
            [0., -2.],
            7.,
            0.,
        )?)))
}

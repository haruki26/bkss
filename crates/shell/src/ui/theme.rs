use std::{fmt::Display, future::Future, sync::Arc};

use bkss_ui::effects::{
    GlassBorder, GlassColor, GlassInnerEffect, GlassRadius, GlassRim, GlassStyle, GlassStyleError,
};
use gpui::{
    App, Div, Entity, FontWeight, Image, ImageFormat, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, Stateful, StatefulInteractiveElement, Styled, Window, div,
    px, rgb, rgba,
};

pub struct IconIndex {
    paths: std::collections::HashMap<String, std::path::PathBuf>,
    app_icons: std::collections::HashMap<String, Option<String>>,
}

impl IconIndex {
    pub fn load() -> Self {
        let mut paths = std::collections::HashMap::new();
        let mut roots = vec![
            std::path::PathBuf::from("/usr/share/pixmaps"),
            std::path::PathBuf::from("/usr/share/icons"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(std::path::PathBuf::from(&home).join(".local/share/icons"));
            roots.push(std::path::PathBuf::from(home).join(".icons"));
        }
        for root in roots {
            index_icons(&root, 0, &mut paths);
        }
        let mut app_icons = std::collections::HashMap::new();
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            });
        let data_dirs = std::env::var_os("XDG_DATA_DIRS")
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        for root in data_home
            .into_iter()
            .chain(std::env::split_paths(&data_dirs).filter(|path| path.is_absolute()))
        {
            index_desktop_entries(&root.join("applications"), "", &mut app_icons);
        }
        Self { paths, app_icons }
    }

    pub fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        let path = std::path::Path::new(name);
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            self.paths.get(name).cloned()
        }
    }

    pub fn app_icon_name(&self, class: &str) -> &str {
        self.app_icons
            .get(class)
            .or_else(|| self.app_icons.get(&class.to_lowercase()))
            .and_then(|icon| icon.as_deref())
            .unwrap_or("application-x-executable")
    }
}

fn index_desktop_entries(
    root: &std::path::Path,
    prefix: &str,
    icons: &mut std::collections::HashMap<String, Option<String>>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            index_desktop_entries(&path, &format!("{prefix}{name}-"), icons);
        } else if let Some(id) = name.strip_suffix(".desktop") {
            // A higher-priority entry without an icon still masks the same system ID.
            icons
                .entry(format!("{prefix}{id}"))
                .or_insert_with(|| read_desktop_entry_icon(&path));
        }
    }
}

const MAX_DESKTOP_ENTRY_BYTES: u64 = 1024 * 1024;

fn read_desktop_entry_icon(path: &std::path::Path) -> Option<String> {
    use std::io::Read;

    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DESKTOP_ENTRY_BYTES {
        return None;
    }
    let mut contents = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_DESKTOP_ENTRY_BYTES + 1)
        .read_to_end(&mut contents)
        .ok()?;
    if contents.len() as u64 > MAX_DESKTOP_ENTRY_BYTES {
        return None;
    }
    desktop_entry_icon(std::str::from_utf8(&contents).ok()?)
}

fn desktop_entry_icon(contents: &str) -> Option<String> {
    let mut in_entry = false;
    let mut icon = None;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            match (key.trim(), value.trim()) {
                ("Hidden", "true") => return None,
                ("Icon", value) if !value.is_empty() => icon = Some(value.to_owned()),
                _ => {}
            }
        }
    }
    icon
}

fn index_icons(
    root: &std::path::Path,
    depth: usize,
    paths: &mut std::collections::HashMap<String, std::path::PathBuf>,
) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            index_icons(&path, depth + 1, paths);
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| matches!(s, "svg" | "png"))
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
        {
            paths.insert(name.to_owned(), path);
        }
    }
}

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
const ICON_FONT: &str = "Symbols Nerd Font";
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

pub fn row() -> Div {
    div().flex().items_center().gap_2()
}

pub fn label(text: impl Into<gpui::SharedString>) -> Div {
    div().text_size(px(13.)).child(text.into())
}

pub fn muted(text: impl Into<gpui::SharedString>) -> Div {
    label(text).text_color(rgb(MUTED))
}

pub fn divider() -> Div {
    div()
        .w(px(1.))
        .h(px(18.))
        .mx_1()
        .flex_shrink_0()
        .bg(rgba(0x585b70e6))
}

pub fn button(
    id: impl Into<gpui::ElementId>,
    accessible_label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    button_with_hover(id, accessible_label, action, |style| {
        style.bg(rgba(0xffffff16))
    })
}

pub fn button_with_hover(
    id: impl Into<gpui::ElementId>,
    accessible_label: impl Into<gpui::SharedString>,
    action: impl Fn(&mut Window, &mut App) + 'static,
    hover: impl FnOnce(gpui::StyleRefinement) -> gpui::StyleRefinement,
) -> Stateful<Div> {
    let action = std::rc::Rc::new(action);
    let keyboard_action = action.clone();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(accessible_label)
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        .min_h(px(28.))
        .px_2()
        .py_1()
        .rounded_lg()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .text_size(px(12.))
        .hover(hover)
        .focus_visible(|s| s.bg(rgba(0xcba6f740)).text_color(rgb(PRIMARY)))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            action(window, cx);
        })
        .on_key_down(move |event, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                keyboard_action(window, cx);
            }
        })
}

#[derive(Default)]
pub struct Feedback {
    pub error: Option<String>,
    pub tray_refresh: super::tray::RefreshFeedback,
}

pub fn fail(feedback: &Entity<Feedback>, message: impl Into<String>, cx: &mut App) {
    feedback.update(cx, |state, cx| {
        state.error = Some(message.into());
        cx.notify();
    });
}

pub fn dispatch<F, E, D>(
    operation: Result<F, D>,
    context: &'static str,
    feedback: &Entity<Feedback>,
    cx: &mut App,
) where
    F: Future<Output = Result<(), E>> + 'static,
    E: Display,
    D: Display,
{
    match operation {
        Err(error) => fail(feedback, format!("{context}: {error}"), cx),
        Ok(operation) => {
            let feedback = feedback.downgrade();
            cx.spawn(async move |cx| {
                if let Err(error) = operation.await {
                    let message = format!("{context}: {error}");
                    cx.update(|cx| {
                        if let Some(feedback) = feedback.upgrade() {
                            fail(&feedback, message, cx);
                        }
                    });
                }
            })
            .detach();
        }
    }
}

pub fn error_banner(feedback: &Entity<Feedback>, cx: &App) -> Option<impl IntoElement> {
    feedback.read(cx).error.as_ref().map(|message| {
        let feedback = feedback.clone();
        row()
            .w_full()
            .rounded_lg()
            .px_3()
            .py_2()
            .bg(rgba(0xf38ba824))
            .text_color(rgb(DANGER))
            .child(label(message.clone()).flex_1().min_w_0())
            .child(
                button("dismiss-error", "エラーを閉じる", move |_, cx| {
                    feedback.update(cx, |state, cx| {
                        state.error = None;
                        cx.notify();
                    });
                })
                .child("×"),
            )
    })
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "yh-calendar-symbolic" => "\u{f0e18}",
        "yh-clock-symbolic" => "\u{f0150}",
        "yh-bell-symbolic" => "\u{f009c}",
        "yh-power-symbolic" => "\u{f0425}",
        "yh-lock-symbolic" => "\u{f0341}",
        "yh-cpu-symbolic" => "\u{f4bc}",
        "yh-memory-symbolic" => "\u{efc5}",
        "yh-storage-symbolic" => "\u{f0a0}",
        "yh-sun-symbolic" => "\u{f0599}",
        "yh-volume-off-symbolic" => "\u{f075f}",
        "yh-volume-1-symbolic" => "\u{f057e}",
        "yh-volume-2-symbolic" => "\u{f0580}",
        "yh-volume-3-symbolic" => "\u{f057f}",
        "yh-mic-on-symbolic" => "\u{f036e}",
        "yh-mic-off-symbolic" => "\u{f036d}",
        "yh-bluetooth-on-symbolic" => "\u{f00af}",
        "yh-bluetooth-off-symbolic" => "\u{f00b2}",
        "yh-wifi-off-symbolic" => "\u{f092e}",
        "yh-wifi-1-symbolic" => "\u{f0928}",
        "yh-wifi-2-symbolic" => "\u{f0925}",
        "yh-wifi-3-symbolic" => "\u{f0922}",
        "yh-wifi-4-symbolic" => "\u{f091f}",
        "yh-wifi-5-symbolic" => "\u{f092f}",
        "yh-battery-charging-symbolic" => "\u{f089f}",
        "yh-battery-1-symbolic" => "\u{f0082}",
        "yh-battery-2-symbolic" => "\u{f007f}",
        "yh-battery-3-symbolic" => "\u{f007d}",
        "yh-battery-4-symbolic" => "\u{f007a}",
        "yh-weather-clear-day-symbolic" => "\u{e30d}",
        "yh-weather-clear-night-symbolic" => "\u{e32b}",
        "yh-weather-cloudy-day-symbolic" => "\u{e302}",
        "yh-weather-cloudy-night-symbolic" => "\u{e37e}",
        "yh-weather-shower-rain-symbolic" => "\u{e319}",
        "yh-weather-rain-day-symbolic" => "\u{e308}",
        "yh-weather-rain-night-symbolic" => "\u{e325}",
        "yh-weather-thunder-symbolic" => "\u{e31d}",
        "yh-weather-snow-symbolic" => "\u{e31a}",
        "yh-weather-fog-symbolic" => "\u{e313}",
        "yh-weather-unknown-symbolic" => "\u{e312}",
        "yh-gpu-symbolic" => "\u{f08ae}",
        _ => return None,
    })
}

pub fn symbolic(name: &str, color: u32, size: f32) -> GlyphIcon {
    let Some(glyph) = symbol(name) else {
        return icon(Icon::App).size(px(size));
    };
    GlyphIcon {
        glyph,
        color,
        size: px(size),
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

#[derive(Clone, Copy)]
pub enum Icon {
    Bell,
    Power,
    Reboot,
    Lock,
    Close,
    Left,
    Right,
    App,
}

pub fn icon(kind: Icon) -> GlyphIcon {
    let glyph = match kind {
        Icon::Bell => return symbolic("yh-bell-symbolic", MUTED, 24.),
        Icon::Power => return symbolic("yh-power-symbolic", DANGER, 18.),
        Icon::Reboot => {
            return GlyphIcon {
                glyph: "\u{f0709}", // md-restart
                ..symbolic("yh-power-symbolic", ACCENT, 24.)
            };
        }
        Icon::Lock => return symbolic("yh-lock-symbolic", SECONDARY, 18.),
        Icon::Close => "\u{f0156}", // md-close
        Icon::Left => "\u{f0141}",  // md-chevron_left
        Icon::Right => "\u{f0142}", // md-chevron_right
        Icon::App => "\u{f0614}",   // md-application_outline
    };
    GlyphIcon {
        glyph,
        color: MUTED,
        size: px(16.),
    }
}

pub fn tray_image(icon: &bkss_integration::TrayIcon) -> Option<Arc<Image>> {
    let pixels = icon.width.checked_mul(icon.height)?;
    if pixels == 0 || pixels > 1024 * 1024 || icon.rgba.len() != pixels as usize * 4 {
        return None;
    }
    // A V4 BMP preserves the alpha channel without another image-codec dependency.
    let mut bytes = vec![0_u8; 122];
    bytes[0..2].copy_from_slice(b"BM");
    bytes[2..6].copy_from_slice(&(122 + pixels * 4).to_le_bytes());
    bytes[10..14].copy_from_slice(&122_u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&108_u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&icon.width.to_le_bytes());
    bytes[22..26].copy_from_slice(&(-(icon.height as i32)).to_le_bytes());
    bytes[26..28].copy_from_slice(&1_u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&32_u16.to_le_bytes());
    bytes[30..34].copy_from_slice(&3_u32.to_le_bytes());
    for (offset, mask) in [
        (54, 0x00ff0000_u32),
        (58, 0x0000ff00),
        (62, 0x000000ff),
        (66, 0xff000000),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&mask.to_le_bytes());
    }
    bytes[70..74].copy_from_slice(&0x73524742_u32.to_le_bytes());
    for pixel in icon.rgba.as_chunks::<4>().0 {
        bytes.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    Some(Arc::new(Image::from_bytes(ImageFormat::Bmp, bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_divider_has_a_single_height_that_fits_the_bar_content() {
        let mut divider = divider();
        assert_eq!(divider.style().size.height, Some(px(18.).into()));
        assert_eq!(divider.style().size.width, Some(px(1.).into()));
        assert_eq!(BAR_HEIGHT, 40.);
    }

    #[test]
    fn gpu_symbol_keeps_its_glyph_color_and_size() {
        let gpu = symbolic("yh-gpu-symbolic", ACCENT, 14.);
        assert_eq!(gpu.glyph, "\u{f08ae}");
        assert_eq!(gpu.size, px(14.));
        assert_eq!(gpu.color, ACCENT);
    }

    #[test]
    fn unknown_symbolic_names_use_the_generic_app_glyph() {
        let unknown = symbolic("not-a-symbolic-name", DANGER, 22.);
        assert_eq!(unknown.glyph, "\u{f0614}");
        assert_eq!(unknown.size, px(22.));
        assert_eq!(unknown.color, MUTED);
    }

    #[test]
    fn app_classes_use_desktop_entry_icons_with_ags_case_rules_and_xdg_precedence() {
        let root = std::env::temp_dir().join(format!(
            "shell-app-icons-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let user = root.join("user/applications");
        let system = root.join("system/applications");
        std::fs::create_dir_all(user.join("vendor")).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        for (path, contents) in [
            (
                user.join("foo.desktop"),
                "[Desktop Entry]\nType=Application\nIcon=foo-icon\n[Desktop Action New]\nIcon=wrong-action-icon\n",
            ),
            (
                user.join("ExactCase.desktop"),
                "[Desktop Entry]\nType=Application\nIcon=exact-icon\n",
            ),
            (
                user.join("hidden.desktop"),
                "[Desktop Entry]\nHidden=true\nIcon=hidden-icon\n",
            ),
            (
                user.join("no-icon.desktop"),
                "[Desktop Entry]\nType=Application\n",
            ),
            (
                user.join("bad-utf8.desktop"),
                "[Desktop Entry]\nIcon=bad-icon\n",
            ),
            (
                user.join("vendor/nested.desktop"),
                "[Desktop Entry]\nIcon=/absolute/app.svg\n",
            ),
            (
                system.join("foo.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("hidden.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("no-icon.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
            (
                system.join("bad-utf8.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            ),
        ] {
            std::fs::write(path, contents).unwrap();
        }
        std::fs::write(user.join("bad-utf8.desktop"), b"\xff").unwrap();
        std::fs::write(
            user.join("oversized.desktop"),
            vec![b'x'; MAX_DESKTOP_ENTRY_BYTES as usize + 1],
        )
        .unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(user.join("foo.desktop"), user.join("linked.desktop"))
                .unwrap();
            std::os::unix::fs::symlink("/dev/zero", user.join("special.desktop")).unwrap();
            std::fs::write(
                system.join("special.desktop"),
                "[Desktop Entry]\nIcon=wrong-system-icon\n",
            )
            .unwrap();
        }
        let mut index = IconIndex {
            paths: [
                ("foo-icon".into(), root.join("foo-icon.svg")),
                ("unmatched".into(), root.join("unmatched.svg")),
                (
                    "application-x-executable".into(),
                    root.join("executable.svg"),
                ),
            ]
            .into(),
            app_icons: Default::default(),
        };
        for applications in [&user, &system] {
            index_desktop_entries(applications, "", &mut index.app_icons);
        }
        for class in ["foo", "Foo", "FOO"] {
            assert_eq!(index.app_icon_name(class), "foo-icon");
            assert_eq!(
                index.path(index.app_icon_name(class)),
                Some(root.join("foo-icon.svg"))
            );
        }
        assert_eq!(index.app_icon_name("ExactCase"), "exact-icon");
        assert_eq!(index.app_icon_name("vendor-nested"), "/absolute/app.svg");
        for class in [
            "exactcase",
            "unmatched",
            "hidden",
            "no-icon",
            "bad-utf8",
            "oversized",
        ] {
            assert_eq!(index.app_icon_name(class), "application-x-executable");
            assert_eq!(
                index.path(index.app_icon_name(class)),
                Some(root.join("executable.svg"))
            );
        }
        #[cfg(unix)]
        {
            assert_eq!(index.app_icon_name("linked"), "foo-icon");
            assert_eq!(index.app_icon_name("special"), "application-x-executable");
        }
        // Notification and tray icon names must not be treated as client classes.
        assert_eq!(index.path("unmatched"), Some(root.join("unmatched.svg")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn weather_glyphs_distinguish_day_and_night() {
        for condition in ["clear", "cloudy", "rain"] {
            let day = symbol(&format!("yh-weather-{condition}-day-symbolic")).unwrap();
            let night = symbol(&format!("yh-weather-{condition}-night-symbolic")).unwrap();
            assert_ne!(day, night, "{condition}");
        }
    }

    #[test]
    fn tray_pixels_keep_alpha_and_reject_bad_dimensions() {
        let mut icon = bkss_integration::TrayIcon {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 40],
        };
        let image = tray_image(&icon).unwrap();
        assert_eq!(&image.bytes[122..], &[30, 20, 10, 40]);
        icon.rgba.pop();
        assert!(tray_image(&icon).is_none());
        icon.width = u32::MAX;
        assert!(tray_image(&icon).is_none());
    }
}

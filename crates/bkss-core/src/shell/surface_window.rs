use gpui::{
    AnyWindowHandle, AppContext, AsyncApp, DisplayId, Pixels, Size, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, layer_shell::LayerShellOptions, px,
};

pub(super) fn layer_window_options(
    display: DisplayId,
    size: Size<Pixels>,
    options: LayerShellOptions,
) -> WindowOptions {
    WindowOptions {
        display_id: Some(display),
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
            origin: gpui::point(px(0.), px(0.)),
            size,
        })),
        focus: false,
        window_background: WindowBackgroundAppearance::Transparent,
        kind: WindowKind::LayerShell(options),
        ..Default::default()
    }
}

pub(super) fn remove_window(cx: &mut impl AppContext, window: AnyWindowHandle) {
    let _ = cx.update_window(window, |_, window, _| window.remove_window());
}

pub(super) fn remove_window_later(cx: &AsyncApp, window: AnyWindowHandle) {
    let app = cx.clone();
    app.spawn(async move |cx| remove_window(cx, window))
        .detach();
}

pub(super) fn remove_windows_later(cx: &AsyncApp, windows: Vec<AnyWindowHandle>) {
    if windows.is_empty() {
        return;
    }
    let app = cx.clone();
    app.spawn(async move |cx| {
        for window in windows {
            remove_window(cx, window);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds,
        layer_shell::{Anchor, KeyboardInteractivity, Layer},
        point, px,
    };

    #[test]
    fn layer_options_keep_surface_settings_and_caller_layer_options() {
        let display = DisplayId::new(7);
        let size = Size::new(px(0.), px(32.));
        let options = LayerShellOptions {
            namespace: "caller".to_owned(),
            layer: Layer::Overlay,
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            anchor: Anchor::LEFT | Anchor::RIGHT,
            exclusive_zone: Some(px(12.)),
            exclusive_edge: Some(Anchor::BOTTOM),
            margin: Some((px(1.), px(2.), px(3.), px(4.))),
        };
        let window = layer_window_options(display, size, options);

        assert_eq!(window.display_id, Some(display));
        assert!(matches!(
            window.window_bounds,
            Some(WindowBounds::Windowed(Bounds { origin, size: actual }))
                if origin == point(px(0.), px(0.)) && actual == size
        ));
        assert!(!window.focus);
        assert_eq!(
            window.window_background,
            WindowBackgroundAppearance::Transparent
        );
        let WindowKind::LayerShell(layer_options) = window.kind else {
            panic!("expected layer shell options");
        };
        assert_eq!(layer_options.namespace, "caller");
        assert_eq!(layer_options.layer, Layer::Overlay);
        assert_eq!(
            layer_options.keyboard_interactivity,
            KeyboardInteractivity::OnDemand
        );
        assert_eq!(layer_options.anchor, Anchor::LEFT | Anchor::RIGHT);
        assert_eq!(layer_options.exclusive_zone, Some(px(12.)));
        assert_eq!(layer_options.exclusive_edge, Some(Anchor::BOTTOM));
        assert_eq!(layer_options.margin, Some((px(1.), px(2.), px(3.), px(4.))));
        let default_options = layer_window_options(
            display,
            size,
            LayerShellOptions {
                keyboard_interactivity: KeyboardInteractivity::None,
                ..Default::default()
            },
        );
        let WindowKind::LayerShell(default_options) = default_options.kind else {
            panic!("expected layer shell options");
        };
        assert_eq!(
            default_options.keyboard_interactivity,
            KeyboardInteractivity::None
        );
    }
}

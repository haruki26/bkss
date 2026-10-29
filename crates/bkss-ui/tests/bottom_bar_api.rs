use std::{cell::Cell, rc::Rc};

use bkss_core::{GlassStyle, Shell, ShellError};
use bkss_ui::{
    BottomBar, BottomBarAlignment, BottomBarGeometry, BottomBarMargins, BottomBarWidth,
    DisplaySelection, RuntimeHandleExt,
};
use gpui::{AppContext, DisplayId, EmptyView, TestAppContext};

#[test]
fn public_declaration_and_runtime_extension_keep_content_inert_until_mount() {
    let calls = Rc::new(Cell::new(0));
    let bar = BottomBar::new(BottomBarGeometry {
        width: BottomBarWidth::FloatingContentWidth {
            max_logical_width: Some(640.),
            horizontal_alignment: BottomBarAlignment::Right,
        },
        logical_height: 44.,
        horizontal_padding: 16.,
        margins: BottomBarMargins {
            left: 8.,
            right: 20.,
            bottom: 10.,
        },
        reserve: false,
    })
    .unwrap()
    .with_style(GlassStyle::default())
    .with_displays(DisplaySelection::Explicit(vec![DisplayId::new(7)]));
    assert_eq!(calls.get(), 0);

    let cx = TestAppContext::single();
    cx.update(|cx| {
        let runtime = Shell::attach(cx, |_, _| Ok(())).unwrap();
        runtime.shutdown(cx);
        let factory_calls = calls.clone();
        let result: Result<(), ShellError> = runtime.mount_bottom_bar(
            bar,
            move |display, _window, cx| {
                assert_eq!(display, DisplayId::new(7));
                factory_calls.set(factory_calls.get() + 1);
                cx.new(|_| EmptyView)
            },
            cx,
        );
        assert!(
            result.is_err(),
            "a stopped runtime must reject registration"
        );
    });
    assert_eq!(calls.get(), 0);
}

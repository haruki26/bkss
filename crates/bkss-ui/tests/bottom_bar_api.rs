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

#[test]
fn invalid_geometry_is_rejected_before_any_runtime_or_factory_exists() {
    let defaults = BottomBarGeometry::default();
    assert!(BottomBar::new(defaults).is_ok());
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1., f32::MAX] {
        for geometry in [
            BottomBarGeometry {
                logical_height: value,
                ..defaults
            },
            BottomBarGeometry {
                horizontal_padding: value,
                ..defaults
            },
            BottomBarGeometry {
                margins: BottomBarMargins {
                    left: value,
                    ..defaults.margins
                },
                ..defaults
            },
            BottomBarGeometry {
                margins: BottomBarMargins {
                    right: value,
                    ..defaults.margins
                },
                ..defaults
            },
            BottomBarGeometry {
                margins: BottomBarMargins {
                    bottom: value,
                    ..defaults.margins
                },
                ..defaults
            },
            BottomBarGeometry {
                width: BottomBarWidth::FloatingContentWidth {
                    max_logical_width: Some(value),
                    horizontal_alignment: BottomBarAlignment::Center,
                },
                ..defaults
            },
        ] {
            assert!(BottomBar::new(geometry).is_err(), "{geometry:?}");
        }
    }
    for value in [0., 0.5] {
        assert!(
            BottomBar::new(BottomBarGeometry {
                logical_height: value,
                ..defaults
            })
            .is_err()
        );
        assert!(
            BottomBar::new(BottomBarGeometry {
                width: BottomBarWidth::FloatingContentWidth {
                    max_logical_width: Some(value),
                    horizontal_alignment: BottomBarAlignment::Center
                },
                ..defaults
            })
            .is_err()
        );
    }
    assert!(
        BottomBar::new(BottomBarGeometry {
            logical_height: 1_500_000_000.,
            margins: BottomBarMargins {
                bottom: 1_500_000_000.,
                ..defaults.margins
            },
            ..defaults
        })
        .is_err()
    );
    assert!(
        BottomBar::new(BottomBarGeometry {
            horizontal_padding: 1_500_000_000.,
            ..defaults
        })
        .is_err()
    );
    assert!(
        BottomBar::new(BottomBarGeometry {
            logical_height: 1.,
            horizontal_padding: 0.,
            margins: BottomBarMargins {
                left: 0.,
                right: 0.,
                bottom: 0.
            },
            ..defaults
        })
        .is_ok()
    );
}

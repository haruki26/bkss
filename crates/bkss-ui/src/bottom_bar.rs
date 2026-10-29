use std::{error::Error, fmt};

use bkss_core::{
    DisplaySelection, Glass, GlassState, GlassStyle, LayerSurfaceConfig, LayerSurfaceDeclaration,
    RuntimeHandle, ShellError,
};
use gpui::{
    App, AppContext, Bounds, Context, DisplayId, Entity, IntoElement, Pixels, Render, Size, Window,
    canvas, div,
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
    point,
    prelude::*,
    px, size,
};

/// Horizontal placement inside the left and right margins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BottomBarAlignment {
    #[default]
    Center,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BottomBarWidth {
    /// Fill the display width remaining after the horizontal margins.
    FullDisplayWidth,
    /// Measure the content and add horizontal padding, limited to the available width.
    FloatingContentWidth {
        max_logical_width: Option<f32>,
        horizontal_alignment: BottomBarAlignment,
    },
}

impl Default for BottomBarWidth {
    fn default() -> Self {
        Self::FloatingContentWidth {
            max_logical_width: None,
            horizontal_alignment: BottomBarAlignment::Center,
        }
    }
}

/// Visual spacing in logical pixels, not native layer-shell margins.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BottomBarMargins {
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for BottomBarMargins {
    fn default() -> Self {
        Self {
            left: 12.,
            right: 12.,
            bottom: 12.,
        }
    }
}

/// All measurements are logical pixels. The default is a centered floating bar,
/// 40 pixels high, with 12 pixels of horizontal padding and margins, reserving 52 pixels.
/// Content is vertically centered and clipped to the Glass box; it is never scaled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BottomBarGeometry {
    pub width: BottomBarWidth,
    pub logical_height: f32,
    pub horizontal_padding: f32,
    pub margins: BottomBarMargins,
    pub reserve: bool,
}

impl Default for BottomBarGeometry {
    fn default() -> Self {
        Self {
            width: BottomBarWidth::default(),
            logical_height: 40.,
            horizontal_padding: 12.,
            margins: BottomBarMargins::default(),
            reserve: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BottomBarGeometryError {
    field: &'static str,
    requirement: &'static str,
}

impl fmt::Display for BottomBarGeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} must be {}", self.field, self.requirement)
    }
}

impl Error for BottomBarGeometryError {}

impl BottomBarGeometry {
    /// Rejects invalid values before a declaration can be registered with a runtime.
    pub fn validate(&self) -> Result<(), BottomBarGeometryError> {
        let check = |value: f32, field, minimum| {
            if !value.is_finite() || value < minimum || f64::from(value) > f64::from(i32::MAX) {
                Err(BottomBarGeometryError {
                    field,
                    requirement: if minimum == 1. {
                        "finite, at least 1, and representable by layer-shell"
                    } else {
                        "finite, nonnegative, and representable by layer-shell"
                    },
                })
            } else {
                Ok(())
            }
        };
        check(self.logical_height, "bar height", 1.)?;
        check(self.horizontal_padding, "horizontal padding", 0.)?;
        check(self.margins.left, "left margin", 0.)?;
        check(self.margins.right, "right margin", 0.)?;
        check(self.margins.bottom, "bottom margin", 0.)?;
        check(self.allocation_height(), "height plus bottom margin", 1.)?;
        check(
            self.margins.left + self.margins.right,
            "horizontal margins",
            0.,
        )?;
        check(
            self.horizontal_padding * 2.,
            "combined horizontal padding",
            0.,
        )?;
        if let BottomBarWidth::FloatingContentWidth {
            max_logical_width: Some(width),
            ..
        } = self.width
        {
            check(width, "maximum width", 1.)?;
        }
        Ok(())
    }

    fn allocation_height(self) -> f32 {
        self.logical_height + self.margins.bottom
    }

    fn available_width(self, viewport: Size<Pixels>) -> Pixels {
        (viewport.width - px(self.margins.left) - px(self.margins.right)).max(px(0.))
    }

    fn maximum_width(self, viewport: Size<Pixels>) -> Pixels {
        let available = self.available_width(viewport);
        match self.width {
            BottomBarWidth::FloatingContentWidth {
                max_logical_width: Some(maximum),
                ..
            } => available.min(px(maximum)),
            _ => available,
        }
    }

    fn surface_config(self, display_bounds: Bounds<Pixels>) -> LayerSurfaceConfig {
        LayerSurfaceConfig {
            size: size(display_bounds.size.width, px(self.allocation_height())),
            options: LayerShellOptions {
                namespace: "bkss-bottom-bar".into(),
                layer: Layer::Top,
                anchor: Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                exclusive_zone: Some(px(if self.reserve {
                    self.allocation_height()
                } else {
                    0.
                })),
                exclusive_edge: Some(Anchor::BOTTOM),
                // Visual margins already live inside the allocation and reservation.
                margin: None,
                keyboard_interactivity: KeyboardInteractivity::None,
            },
        }
    }
}

/// An inert declaration. Creating or configuring it does not create a window.
#[derive(Clone, Default)]
pub struct BottomBar {
    geometry: BottomBarGeometry,
    style: GlassStyle,
    displays: DisplaySelection,
}

impl BottomBar {
    pub fn new(geometry: BottomBarGeometry) -> Result<Self, BottomBarGeometryError> {
        geometry.validate()?;
        Ok(Self {
            geometry,
            ..Self::default()
        })
    }

    pub fn with_style(mut self, style: GlassStyle) -> Self {
        self.style = style;
        self
    }

    /// Targets the registration-time display snapshot, or the given IDs.
    /// Later hotplug is not followed. Core owns readiness, failure, and cleanup.
    pub fn with_displays(mut self, displays: DisplaySelection) -> Self {
        self.displays = displays;
        self
    }
}

pub trait RuntimeHandleExt {
    /// Creates one independent content entity per selected display, exactly once.
    /// Keyboard focus is never requested. Ordinary pointer handlers work inside Glass.
    fn mount_bottom_bar<V: Render + 'static>(
        &self,
        bar: BottomBar,
        content: impl FnMut(DisplayId, &mut Window, &mut App) -> Entity<V> + 'static,
        cx: &mut App,
    ) -> Result<(), ShellError>;
}

impl RuntimeHandleExt for RuntimeHandle {
    fn mount_bottom_bar<V: Render + 'static>(
        &self,
        bar: BottomBar,
        mut content: impl FnMut(DisplayId, &mut Window, &mut App) -> Entity<V> + 'static,
        cx: &mut App,
    ) -> Result<(), ShellError> {
        let geometry = bar.geometry;
        self.mount_layer_surface(
            LayerSurfaceDeclaration::new(
                bar.displays,
                move |_, bounds| geometry.surface_config(bounds),
                move |display, window, cx| {
                    let content = content(display, window, cx);
                    cx.new(|cx| BottomBarView {
                        geometry,
                        glass: Glass::new(bar.style.clone(), cx),
                        content,
                        #[cfg(test)]
                        last_input: Default::default(),
                    })
                },
            ),
            cx,
        )
    }
}

struct BottomBarView<V> {
    geometry: BottomBarGeometry,
    glass: Glass,
    content: Entity<V>,
    #[cfg(test)]
    last_input: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}

/// Wayland regions use integer logical coordinates. Round inward, never expanding
/// into transparent margins. Display origins and output scale are not local offsets.
fn input_bounds(bounds: Bounds<Pixels>, viewport: Size<Pixels>) -> Option<Bounds<Pixels>> {
    let left = f32::from(bounds.left().max(px(0.))).ceil();
    let top = f32::from(bounds.top().max(px(0.))).ceil();
    let right = f32::from(bounds.right().min(viewport.width)).floor();
    let bottom = f32::from(bounds.bottom().min(viewport.height)).floor();
    (right > left && bottom > top).then(|| {
        Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        )
    })
}

impl<V: Render + 'static> Render for BottomBarView<V> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let geometry = self.geometry;
        let viewport = window.viewport_size();
        #[cfg(test)]
        let last_input = self.last_input.clone();
        let mut glass = self
            .glass
            .element(GlassState::default(), cx)
            .flex()
            .items_center()
            .flex_shrink_0()
            .min_w_0()
            .h(px(geometry.logical_height))
            .max_w(geometry.maximum_width(viewport))
            .overflow_hidden()
            // Padding belongs to the inner row so even a viewport narrower than
            // the padding can limit the visible Glass box without expanding it.
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .h_full()
                    .px(px(geometry.horizontal_padding))
                    .child(self.content.clone()),
            )
            .child(
                canvas(
                    move |bounds, window, _| {
                        let region = input_bounds(bounds, window.viewport_size());
                        window.set_input_region(Some(region.as_slice()));
                        #[cfg(test)]
                        last_input.set(region);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        if geometry.width == BottomBarWidth::FullDisplayWidth {
            glass = glass.w(geometry.available_width(viewport));
        }
        let row = div()
            .absolute()
            .left(px(geometry.margins.left))
            .bottom(px(geometry.margins.bottom))
            .w(geometry.available_width(viewport))
            .h(px(geometry.logical_height))
            .flex();
        let row = match geometry.width {
            BottomBarWidth::FloatingContentWidth {
                horizontal_alignment: BottomBarAlignment::Left,
                ..
            } => row.justify_start(),
            BottomBarWidth::FloatingContentWidth {
                horizontal_alignment: BottomBarAlignment::Right,
                ..
            } => row.justify_end(),
            _ => row.justify_center(),
        };
        div()
            .w(viewport.width)
            .h(viewport.height)
            .overflow_hidden()
            .child(row.child(glass))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowBounds, WindowOptions};
    use std::{cell::Cell, rc::Rc};

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    fn assert_native_layout(actual: Bounds<Pixels>, logical: Bounds<Pixels>, scale: f32) {
        // GPUI snaps layout to device pixels. Compare logical placement within
        // that native precision, then compare input against the exact painted box.
        for (actual, expected) in [
            (actual.left(), logical.left()),
            (actual.top(), logical.top()),
            (actual.right(), logical.right()),
            (actual.bottom(), logical.bottom()),
        ] {
            assert!(
                f32::from(actual - expected).abs() <= 1. / scale,
                "{actual:?} != {expected:?} at scale {scale}"
            );
        }
    }

    #[test]
    fn surface_uses_logical_dimensions_bottom_reservation_and_no_native_margins() {
        for (width, height, scale) in [(1920., 1080., 1.), (1280., 720., 1.5), (960., 540., 2.)] {
            for reserve in [true, false] {
                let geometry = BottomBarGeometry {
                    reserve,
                    ..Default::default()
                };
                // Origins can be nonzero or negative; only the logical size is used.
                let config = geometry.surface_config(rect(-1920., 240., width, height));
                assert_eq!(config.size, size(px(width), px(52.)));
                assert_eq!(f32::from(config.size.width) * scale, width * scale);
                assert_eq!(
                    config.options.anchor,
                    Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT
                );
                assert_eq!(config.options.exclusive_edge, Some(Anchor::BOTTOM));
                assert_eq!(
                    config.options.exclusive_zone,
                    Some(px(if reserve { 52. } else { 0. }))
                );
                assert_eq!(
                    config.options.keyboard_interactivity,
                    KeyboardInteractivity::None
                );
                assert_eq!(config.options.margin, None);
            }
        }
    }

    #[test]
    fn input_rounds_inward_and_clips_without_scaling_or_display_origins() {
        let viewport = size(px(100.), px(52.));
        for (bounds, expected) in [
            (rect(10.2, 0.2, 30.6, 40.6), Some(rect(11., 1., 29., 39.))),
            (rect(-10.2, -3., 120.8, 60.), Some(rect(0., 0., 100., 52.))),
            (rect(99.2, 0., 2., 40.), None),
            (rect(0., 52., 40., 10.), None),
            (rect(10., 0., 0., 40.), None),
            (rect(200., 0., 10., 40.), None),
            (rect(12., 0., 76., 40.), Some(rect(12., 0., 76., 40.))),
        ] {
            assert_eq!(input_bounds(bounds, viewport), expected);
        }
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

    struct Content {
        width: f32,
    }

    impl Render for Content {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).h(px(20.))
        }
    }

    fn layout_frame(
        geometry: BottomBarGeometry,
        content_width: f32,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> (Bounds<Pixels>, Option<Bounds<Pixels>>) {
        let cx = TestAppContext::single();
        let input = Rc::new(Cell::new(None));
        let recorded_input = input.clone();
        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(800.), px(-400.)),
                        viewport,
                    ))),
                    ..Default::default()
                },
                |_, cx| {
                    let content = cx.new(|_| Content {
                        width: content_width,
                    });
                    cx.new(|cx| BottomBarView {
                        geometry,
                        glass: Glass::new(GlassStyle::default(), cx),
                        content,
                        last_input: recorded_input,
                    })
                },
            )
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), &cx);
        visual.simulate_scale_factor_change(scale);
        visual.run_until_parked();
        let painted = visual.update(|window, _| {
            let quads = window.painted_quads();
            assert_eq!(quads.len(), 1, "only the Glass background should paint");
            let bounds = quads[0].bounds;
            rect(
                bounds.origin.x.0 / scale,
                bounds.origin.y.0 / scale,
                bounds.size.width.0 / scale,
                bounds.size.height.0 / scale,
            )
        });
        (painted, input.get())
    }

    #[test]
    fn actual_layout_and_input_share_the_glass_box_for_widths_alignment_and_scale() {
        for (viewport_width, scale) in [(1920., 1.), (1280., 1.5), (960., 2.)] {
            let viewport = size(px(viewport_width), px(70.));
            for reserve in [true, false] {
                for (alignment, expected_x) in [
                    (BottomBarAlignment::Left, 10.),
                    (BottomBarAlignment::Center, (viewport_width - 150.) / 2.),
                    (BottomBarAlignment::Right, viewport_width - 160.),
                ] {
                    let geometry = BottomBarGeometry {
                        width: BottomBarWidth::FloatingContentWidth {
                            max_logical_width: None,
                            horizontal_alignment: alignment,
                        },
                        logical_height: 40.,
                        horizontal_padding: 12.,
                        margins: BottomBarMargins {
                            left: 10.,
                            right: 30.,
                            bottom: 15.,
                        },
                        reserve,
                    };
                    let (painted, input) = layout_frame(geometry, 106., viewport, scale);
                    let expected = rect(expected_x, 15., 130., 40.);
                    assert_native_layout(painted, expected, scale);
                    assert_eq!(input, input_bounds(painted, viewport));
                    let full = BottomBarGeometry {
                        width: BottomBarWidth::FullDisplayWidth,
                        ..geometry
                    };
                    let (painted, input) = layout_frame(full, 106., viewport, scale);
                    let expected = rect(10., 15., viewport_width - 40., 40.);
                    assert_native_layout(painted, expected, scale);
                    assert_eq!(input, input_bounds(painted, viewport));
                }
            }
        }
    }

    #[test]
    fn actual_layout_caps_content_width_and_clips_the_input_to_the_viewport() {
        for (content_width, cap, expected_width) in [
            (10., None, 34.),
            (10., Some(10.), 10.),
            (500., Some(160.), 160.),
            (500., None, 280.),
        ] {
            let geometry = BottomBarGeometry {
                width: BottomBarWidth::FloatingContentWidth {
                    max_logical_width: cap,
                    horizontal_alignment: BottomBarAlignment::Left,
                },
                margins: BottomBarMargins {
                    left: 8.5,
                    right: 11.5,
                    bottom: 12.,
                },
                ..Default::default()
            };
            let viewport = size(px(300.), px(45.));
            let (painted, input) = layout_frame(geometry, content_width, viewport, 1.);
            let expected = rect(8.5, -7., expected_width, 40.);
            assert_native_layout(painted, expected, 1.);
            assert_eq!(input, input_bounds(painted, viewport));
        }
    }
}

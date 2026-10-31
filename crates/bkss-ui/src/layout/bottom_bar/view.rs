use super::*;

pub(super) struct BottomBarView<V> {
    pub(super) geometry: BottomBarGeometry,
    pub(super) glass: Glass,
    pub(super) content: Entity<V>,
    #[cfg(test)]
    pub(super) last_input: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
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

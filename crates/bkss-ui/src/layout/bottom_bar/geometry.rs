use super::*;

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

    pub(super) fn available_width(self, viewport: Size<Pixels>) -> Pixels {
        (viewport.width - px(self.margins.left) - px(self.margins.right)).max(px(0.))
    }

    pub(super) fn maximum_width(self, viewport: Size<Pixels>) -> Pixels {
        let available = self.available_width(viewport);
        match self.width {
            BottomBarWidth::FloatingContentWidth {
                max_logical_width: Some(maximum),
                ..
            } => available.min(px(maximum)),
            _ => available,
        }
    }

    pub(super) fn surface_config(self, display_bounds: Bounds<Pixels>) -> LayerSurfaceConfig {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
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
}

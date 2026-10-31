use super::*;

/// Creation-time keyboard policy. Neither mode requests native activation.
/// `None` still permits ordinary pointer interaction with content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardFocus {
    /// Native keyboard interactivity is disabled; pointer controls still work.
    None,
    /// Creation-time OnDemand interactivity, with a default internal key route.
    /// Native focus still depends on the compositor and user interaction.
    Interactive,
}

/// Opt-in dismissal within the current configured surface viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerDismiss {
    /// Request pointer input only for the clipped content box.
    #[default]
    Disabled,
    /// Paint a dim backdrop, consume outside pointer-down, and close this instance.
    Backdrop,
}

/// At most one edge on each axis; an unspecified axis is centered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayAnchors {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OverlayPlacement {
    #[default]
    Center,
    /// Signed logical offsets use window coordinates: right/down are positive.
    /// The result is clamped, never flipped to a different anchor.
    Anchor {
        edges: OverlayAnchors,
        offset: [f32; 2],
    },
}

/// Natural content size, capped and clipped to the configured local viewport.
/// No outer scroll container, typography scaling, or smart repositioning is added.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayGeometry {
    pub placement: OverlayPlacement,
    /// Optional `[width, height]` cap in logical pixels, not a requested fixed size.
    pub max_logical_size: Option<[f32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayGeometryError {
    ContradictoryAnchors,
    InvalidOffset,
    InvalidMaximumSize,
}

impl fmt::Display for OverlayGeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ContradictoryAnchors => "overlay cannot anchor to opposing edges on one axis",
            Self::InvalidOffset => "overlay offsets must be finite and representable logical coordinates",
            Self::InvalidMaximumSize => "overlay maximum dimensions must be finite, at least one logical pixel, and representable",
        })
    }
}

impl Error for OverlayGeometryError {}

impl OverlayGeometry {
    /// Validates before any runtime registration or content factory invocation.
    pub fn validate(self) -> Result<(), OverlayGeometryError> {
        if let OverlayPlacement::Anchor { edges, offset } = self.placement {
            if (edges.top && edges.bottom) || (edges.left && edges.right) {
                return Err(OverlayGeometryError::ContradictoryAnchors);
            }
            if offset
                .into_iter()
                .any(|value| !value.is_finite() || f64::from(value).abs() > f64::from(i32::MAX))
            {
                return Err(OverlayGeometryError::InvalidOffset);
            }
        }
        if self.max_logical_size.is_some_and(|dimensions| {
            dimensions.into_iter().any(|value| {
                !value.is_finite() || value < 1. || f64::from(value) > f64::from(i32::MAX)
            })
        }) {
            return Err(OverlayGeometryError::InvalidMaximumSize);
        }
        Ok(())
    }

    pub(super) fn limit(self, viewport: Size<Pixels>, scale: f32) -> Size<Pixels> {
        let axis = |view: Pixels, maximum: Option<f32>| {
            let view = f64::from(f32::from(view));
            let scale = f64::from(scale);
            if !view.is_finite() || view <= 0. || !scale.is_finite() || scale <= 0. {
                return px(0.);
            }
            let physical = (view * scale).floor();
            let maximum = maximum.map_or(physical, |v| (f64::from(v) * scale).floor());
            px((physical.min(maximum).max(0.) / scale) as f32)
        };
        size(
            axis(viewport.width, self.max_logical_size.map(|s| s[0])),
            axis(viewport.height, self.max_logical_size.map(|s| s[1])),
        )
    }

    pub(super) fn bounds(
        self,
        natural: Size<Pixels>,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> Bounds<Pixels> {
        let limit = self.limit(viewport, scale);
        let (edges, offset) = match self.placement {
            OverlayPlacement::Center => (OverlayAnchors::default(), [0., 0.]),
            OverlayPlacement::Anchor { edges, offset } => (edges, offset),
        };
        let axis =
            |natural: Pixels, view: Pixels, limit: Pixels, start: bool, end: bool, offset: f32| {
                let scale = f64::from(scale);
                if !scale.is_finite() || scale <= 0. {
                    return (px(0.), px(0.));
                }
                let view = (f64::from(f32::from(view)) * scale).floor().max(0.);
                let limit = (f64::from(f32::from(limit)) * scale).round();
                let length = (f64::from(f32::from(natural)) * scale)
                    .round()
                    .clamp(0., limit);
                let free = (view - length).max(0.);
                let origin = if start {
                    0.
                } else if end {
                    free
                } else {
                    free / 2.
                };
                let origin = (origin + f64::from(offset) * scale).clamp(0., free).floor();
                (px((origin / scale) as f32), px((length / scale) as f32))
            };
        let (x, width) = axis(
            natural.width,
            viewport.width,
            limit.width,
            edges.left,
            edges.right,
            offset[0],
        );
        let (y, height) = axis(
            natural.height,
            viewport.height,
            limit.height,
            edges.top,
            edges.bottom,
            offset[1],
        );
        Bounds::new(point(x, y), size(width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_offsets_and_maximum_sizes_are_typed() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let geometry = OverlayGeometry {
                placement: OverlayPlacement::Anchor {
                    edges: OverlayAnchors::default(),
                    offset: [bad, 0.],
                },
                ..Default::default()
            };
            assert_eq!(
                geometry.validate(),
                Err(OverlayGeometryError::InvalidOffset)
            );
        }
        for bad in [f32::NAN, f32::INFINITY, -1., 0., 0.5, f32::MAX] {
            let geometry = OverlayGeometry {
                max_logical_size: Some([bad, 10.]),
                ..Default::default()
            };
            assert_eq!(
                geometry.validate(),
                Err(OverlayGeometryError::InvalidMaximumSize)
            );
        }
    }
}

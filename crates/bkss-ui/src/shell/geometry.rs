use gpui::{Bounds, Edges, Pixels, Point, Size, layer_shell::Anchor, point, px, size};
use std::{error::Error, fmt};

/// A fixed logical-pixel surface thickness, independent of GPUI content styling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thickness(Pixels);

impl Thickness {
    pub fn new(value: Pixels) -> Result<Self, SurfaceConfigError> {
        validate_number(value, px(1.), "thickness")?;
        Ok(Self(value))
    }

    pub fn get(self) -> Pixels {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceConfigError(pub(super) &'static str);

impl fmt::Display for SurfaceConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for SurfaceConfigError {}

pub(super) fn validate_number(
    value: Pixels,
    minimum: Pixels,
    field: &'static str,
) -> Result<(), SurfaceConfigError> {
    let value = f32::from(value);
    if !value.is_finite() || value < f32::from(minimum) || f64::from(value) > f64::from(i32::MAX) {
        return Err(SurfaceConfigError(field));
    }
    Ok(())
}

/// Content placement inside the configured local viewport, not native anchoring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Placement {
    #[default]
    Center,
    /// At most one edge per axis. Offsets are logical pixels; right/down are positive.
    /// Placement is clamped to the viewport, never flipped.
    Anchor {
        edges: Anchor,
        offset: Point<Pixels>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarAlignment {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Clone, Copy)]
pub(super) struct BarGeometry {
    pub edge: Anchor,
    pub thickness: Thickness,
    pub margins: Edges<Pixels>,
    pub full_length: bool,
    pub max_length: Option<Pixels>,
    pub alignment: BarAlignment,
}

impl BarGeometry {
    pub fn horizontal(self) -> bool {
        self.edge == Anchor::TOP || self.edge == Anchor::BOTTOM
    }

    pub fn edge_margin(self) -> Pixels {
        match self.edge {
            Anchor::TOP => self.margins.top,
            Anchor::BOTTOM => self.margins.bottom,
            Anchor::LEFT => self.margins.left,
            _ => self.margins.right,
        }
    }

    pub fn allocation(self) -> Pixels {
        self.thickness.get() + self.edge_margin()
    }

    pub fn validate(self) -> Result<(), SurfaceConfigError> {
        if ![Anchor::TOP, Anchor::BOTTOM, Anchor::LEFT, Anchor::RIGHT].contains(&self.edge) {
            return Err(SurfaceConfigError("bar edge must be one screen edge"));
        }
        for margin in [
            self.margins.top,
            self.margins.right,
            self.margins.bottom,
            self.margins.left,
        ] {
            validate_number(
                margin,
                px(0.),
                "bar margins must be finite nonnegative logical pixels",
            )?;
        }
        validate_number(
            self.allocation(),
            px(1.),
            "bar thickness plus edge margin is too large",
        )?;
        validate_number(
            self.margins.left + self.margins.right,
            px(0.),
            "bar horizontal margins are too large",
        )?;
        validate_number(
            self.margins.top + self.margins.bottom,
            px(0.),
            "bar vertical margins are too large",
        )?;
        if let Some(maximum) = self.max_length {
            validate_number(
                maximum,
                px(1.),
                "bar maximum length must be positive logical pixels",
            )?;
        }
        Ok(())
    }

    fn available_length(self, viewport: Size<Pixels>) -> Pixels {
        if self.horizontal() {
            (viewport.width - self.margins.left - self.margins.right).max(px(0.))
        } else {
            (viewport.height - self.margins.top - self.margins.bottom).max(px(0.))
        }
    }

    pub fn limit(self, viewport: Size<Pixels>) -> Size<Pixels> {
        let available = self.available_length(viewport);
        let length = self
            .max_length
            .map_or(available, |maximum| available.min(maximum));
        if self.horizontal() {
            size(length, self.thickness.get())
        } else {
            size(self.thickness.get(), length)
        }
    }

    pub fn bounds(
        self,
        natural: Size<Pixels>,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> Bounds<Pixels> {
        let limit = self.limit(viewport);
        let length = if self.horizontal() {
            natural.width.min(limit.width)
        } else {
            natural.height.min(limit.height)
        };
        let length = if self.full_length {
            if self.horizontal() {
                limit.width
            } else {
                limit.height
            }
        } else {
            length
        };
        let available = self.available_length(viewport);
        let offset = match self.alignment {
            BarAlignment::Start => px(0.),
            BarAlignment::Center => (available - length) / 2.,
            BarAlignment::End => available - length,
        };
        let content_size = if self.horizontal() {
            size(length, natural.height.min(limit.height))
        } else {
            size(natural.width.min(limit.width), length)
        };
        let origin = match self.edge {
            Anchor::TOP => point(self.margins.left + offset, self.margins.top),
            Anchor::BOTTOM => point(
                self.margins.left + offset,
                viewport.height - self.margins.bottom - content_size.height,
            ),
            Anchor::LEFT => point(self.margins.left, self.margins.top + offset),
            _ => point(
                viewport.width - self.margins.right - content_size.width,
                self.margins.top + offset,
            ),
        };
        Bounds::new(
            origin.map(|v| px((f32::from(v) * scale).floor() / scale)),
            content_size,
        )
    }
}

#[derive(Clone, Copy)]
pub(super) enum Geometry {
    Content {
        placement: Placement,
        maximum: Option<Size<Pixels>>,
    },
    Bar(BarGeometry),
}

impl Geometry {
    pub fn validate(self) -> Result<(), SurfaceConfigError> {
        match self {
            Self::Bar(bar) => bar.validate(),
            Self::Content { placement, maximum } => {
                if let Placement::Anchor { edges, offset } = placement {
                    if edges.bits() & !Anchor::all().bits() != 0
                        || edges.contains(Anchor::TOP | Anchor::BOTTOM)
                        || edges.contains(Anchor::LEFT | Anchor::RIGHT)
                    {
                        return Err(SurfaceConfigError(
                            "content placement cannot use opposing edges",
                        ));
                    }
                    for value in [offset.x, offset.y] {
                        if !f32::from(value).is_finite()
                            || f64::from(f32::from(value)).abs() > f64::from(i32::MAX)
                        {
                            return Err(SurfaceConfigError(
                                "content offsets must be finite representable logical pixels",
                            ));
                        }
                    }
                }
                if let Some(maximum) = maximum {
                    for dimension in [maximum.width, maximum.height] {
                        validate_number(
                            dimension,
                            px(1.),
                            "content limits must be positive logical pixels",
                        )?;
                    }
                }
                Ok(())
            }
        }
    }

    pub fn limit(self, viewport: Size<Pixels>, scale: f32) -> Size<Pixels> {
        let maximum = match self {
            Self::Bar(bar) => Some(bar.limit(viewport)),
            Self::Content { maximum, .. } => maximum,
        };
        let axis = |view: Pixels, maximum: Option<Pixels>| {
            let view = f64::from(f32::from(view));
            let scale = f64::from(scale);
            if !view.is_finite() || view <= 0. || !scale.is_finite() || scale <= 0. {
                return px(0.);
            }
            let physical = (view * scale).floor();
            let maximum = maximum.map_or(physical, |v| (f64::from(f32::from(v)) * scale).floor());
            px((physical.min(maximum).max(0.) / scale) as f32)
        };
        size(
            axis(viewport.width, maximum.map(|s| s.width)),
            axis(viewport.height, maximum.map(|s| s.height)),
        )
    }

    pub fn bounds(
        self,
        natural: Size<Pixels>,
        viewport: Size<Pixels>,
        scale: f32,
    ) -> Bounds<Pixels> {
        if !scale.is_finite() || scale <= 0. {
            return Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.)));
        }
        let placement = match self {
            Self::Bar(bar) => return bar.bounds(natural, viewport, scale),
            Self::Content { placement, .. } => placement,
        };
        let limit = self.limit(viewport, scale);
        let (edges, offset) = match placement {
            Placement::Center => (Anchor::empty(), point(px(0.), px(0.))),
            Placement::Anchor { edges, offset } => (edges, offset),
        };
        let axis = |natural: Pixels,
                    view: Pixels,
                    limit: Pixels,
                    start: bool,
                    end: bool,
                    offset: Pixels| {
            let scale = f64::from(scale);
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
            let origin = (origin + f64::from(f32::from(offset)) * scale)
                .clamp(0., free)
                .floor();
            (px((origin / scale) as f32), px((length / scale) as f32))
        };
        let (x, width) = axis(
            natural.width,
            viewport.width,
            limit.width,
            edges.contains(Anchor::LEFT),
            edges.contains(Anchor::RIGHT),
            offset.x,
        );
        let (y, height) = axis(
            natural.height,
            viewport.height,
            limit.height,
            edges.contains(Anchor::TOP),
            edges.contains(Anchor::BOTTOM),
            offset.y,
        );
        Bounds::new(point(x, y), size(width, height))
    }
}

/// Native regions use integer logical coordinates, not device pixels.
pub(super) fn input_region(
    bounds: Bounds<Pixels>,
    viewport: Size<Pixels>,
) -> Option<Bounds<Pixels>> {
    if [
        bounds.left(),
        bounds.top(),
        bounds.right(),
        bounds.bottom(),
        viewport.width,
        viewport.height,
    ]
    .into_iter()
    .any(|v| !f32::from(v).is_finite())
    {
        return None;
    }
    let maximum = px(2_147_483_520.);
    let left = f32::from(bounds.left().max(px(0.)).min(maximum)).ceil();
    let top = f32::from(bounds.top().max(px(0.)).min(maximum)).ceil();
    let right = f32::from(bounds.right().min(viewport.width).min(maximum)).floor();
    let bottom = f32::from(bounds.bottom().min(viewport.height).min(maximum)).floor();
    (right > left && bottom > top).then(|| {
        Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn native_input_clips_and_rounds_inward_in_logical_coordinates() {
        let viewport = size(px(100.), px(52.));
        for (bounds, expected) in [
            (rect(10.2, 0.2, 30.6, 40.6), Some(rect(11., 1., 29., 39.))),
            (rect(-10.2, -3., 120.8, 60.), Some(rect(0., 0., 100., 52.))),
            (rect(99.2, 0., 2., 40.), None),
            (rect(0., 52., 40., 10.), None),
            (rect(f32::NAN, 0., 40., 10.), None),
            (rect(f32::INFINITY, 0., 40., 10.), None),
        ] {
            assert_eq!(input_region(bounds, viewport), expected);
        }
        assert_eq!(
            input_region(
                rect(0., 0., f32::MAX, f32::MAX),
                size(px(f32::MAX), px(f32::MAX))
            ),
            Some(rect(0., 0., 2_147_483_520., 2_147_483_520.))
        );
    }

    #[test]
    fn content_placement_preserves_physical_clamping_and_local_coordinates() {
        let viewport = size(px(301.), px(201.));
        for scale in [1., 1.5, 2.] {
            let geometry = Geometry::Content {
                placement: Placement::Anchor {
                    edges: Anchor::BOTTOM | Anchor::RIGHT,
                    offset: point(px(-12.2), px(800.)),
                },
                maximum: Some(size(px(120.8), px(80.8))),
            };
            let bounds = geometry.bounds(size(px(1000.), px(1000.)), viewport, scale);
            let limit = geometry.limit(viewport, scale);
            assert_eq!(bounds.size, limit);
            assert!(bounds.left() >= px(0.));
            assert!(bounds.right() <= viewport.width);
            assert!(bounds.bottom() <= viewport.height);
            assert!(f32::from(viewport.height - bounds.bottom()) <= 1. / scale);
        }
    }
}

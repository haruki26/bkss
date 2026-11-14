use super::{GlassColor, GlassInnerEffect, style::ResolvedGlassStyle};
use gpui::{
    AnyElement, App, BorderStyle, Bounds, BoxShadow, Corners, Div, Edges, Hsla, IntoElement,
    Pixels, RenderOnce, Rgba, StyleRefinement, Window, canvas, div, point, prelude::*, px, quad,
};

/// A fresh Glass frame with ordinary GPUI layout and child composition.
///
/// GPUI paints tint and explicitly requested borders, rims, and inner effects.
/// Background refraction and blur are supplied separately by the compositor,
/// not by this element. Decoration opacity does not change child opacity.
#[derive(IntoElement)]
pub struct GlassElement {
    container: Div,
    children: Vec<AnyElement>,
    decoration: ResolvedGlassStyle,
}

pub(super) fn element(decoration: ResolvedGlassStyle) -> GlassElement {
    GlassElement {
        container: div().relative(),
        children: Vec::new(),
        decoration,
    }
}

impl Styled for GlassElement {
    fn style(&mut self) -> &mut StyleRefinement {
        self.container.style()
    }
}

impl ParentElement for GlassElement {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for GlassElement {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = self.decoration;
        // Absolute canvases do not reserve layout space or change child opacity.
        // The last canvas paints the edge after content, matching GPUI's border order.
        self.container
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        paint_background(style, bounds, window);
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .children(self.children)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        paint_edges(style, bounds, window);
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
    }
}

fn color(value: GlassColor, opacity: f32) -> Hsla {
    Rgba {
        r: value.red(),
        g: value.green(),
        b: value.blue(),
        a: value.alpha() * opacity,
    }
    .into()
}

fn radius(style: ResolvedGlassStyle, bounds: Bounds<Pixels>) -> Pixels {
    px(style.radius.get())
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.)
}

fn shadow(effect: GlassInnerEffect, opacity: f32) -> BoxShadow {
    BoxShadow {
        color: color(effect.color(), opacity),
        offset: point(px(effect.offset()[0]), px(effect.offset()[1])),
        blur_radius: px(effect.blur()),
        spread_radius: px(effect.spread()),
        inset: true,
    }
}

fn paint_background(style: ResolvedGlassStyle, bounds: Bounds<Pixels>, window: &mut Window) {
    let corners = Corners::all(radius(style, bounds));
    window.paint_quad(quad(
        bounds,
        corners,
        color(style.tint, style.opacity.get()),
        Edges::default(),
        Hsla::transparent_black(),
        BorderStyle::default(),
    ));
    // Native inset shadows clip to the rounded interior, including signed offsets
    // and spread. No outer shadow or translucent child layer approximates them.
    for effect in [style.inner_highlight, style.inner_shadow]
        .into_iter()
        .flatten()
    {
        window.paint_inset_shadows(bounds, corners, &[shadow(effect, style.opacity.get())]);
    }
}

fn ring(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    inset: f32,
    width: f32,
    color: Hsla,
) -> Option<gpui::PaintQuad> {
    let half = bounds.size.width.min(bounds.size.height) / 2.;
    if px(inset) >= half || width == 0. {
        return None;
    }
    let bounds = bounds.dilate(-px(inset));
    let radius = (radius - px(inset)).max(px(0.));
    let width = px(width).min(bounds.size.width.min(bounds.size.height) / 2.);
    Some(quad(
        bounds,
        Corners::all(radius),
        Hsla::transparent_black(),
        Edges::all(width),
        color,
        BorderStyle::default(),
    ))
}

fn paint_edges(style: ResolvedGlassStyle, bounds: Bounds<Pixels>, window: &mut Window) {
    let radius = radius(style, bounds);
    if let Some(border) = style.border
        && let Some(quad) = ring(
            bounds,
            radius,
            0.,
            border.width(),
            color(border.color(), style.opacity.get()),
        )
    {
        window.paint_quad(quad);
    }
    if let Some(rim) = style.rim
        && let Some(quad) = ring(
            bounds,
            radius,
            rim.inset(),
            rim.width(),
            color(rim.color(), style.opacity.get()),
        )
    {
        window.paint_quad(quad);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GlassOpacity, GlassRadius};
    use super::*;
    use gpui::size;

    #[test]
    fn decoration_alpha_multiplies_and_native_effect_keeps_signed_geometry() {
        let tint = GlassColor::rgba(0.2, 0.4, 0.6, 0.5).unwrap();
        assert_eq!(color(tint, 0.4).a, 0.2);
        let effect = GlassInnerEffect::new(tint, [-3., 2.], 4., -1.).unwrap();
        let effect = shadow(effect, 0.4);
        assert!(effect.inset);
        assert_eq!(effect.color.a, 0.2);
        assert_eq!(effect.offset, point(px(-3.), px(2.)));
        assert_eq!(effect.blur_radius, px(4.));
        assert_eq!(effect.spread_radius, px(-1.));
    }

    #[test]
    fn radius_and_rings_fit_bounds_without_changing_style_values() {
        let style = ResolvedGlassStyle {
            radius: GlassRadius::new(100.).unwrap(),
            opacity: GlassOpacity::new(1.).unwrap(),
            ..Default::default()
        };
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(100.), px(40.)));
        assert_eq!(radius(style, bounds), px(20.));
        assert_eq!(style.radius.get(), 100.);
        let rim = ring(bounds, px(20.), 4., 2., Hsla::white()).unwrap();
        assert_eq!(rim.bounds, bounds.dilate(px(-4.)));
        assert_eq!(rim.corner_radii, Corners::all(px(16.)));
        assert_eq!(rim.border_widths, Edges::all(px(2.)));
        assert!(ring(bounds, px(20.), 20., 2., Hsla::white()).is_none());
        assert!(ring(bounds, px(20.), 0., 0., Hsla::white()).is_none());
    }
}

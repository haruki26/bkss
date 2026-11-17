use super::geometry::{BarGeometry, Geometry, validate_number};
use super::view::SurfaceView;
use super::{
    Anchor, BarAlignment, KeyboardInteractivity, Layer, Output, Placement, SurfaceConfigError,
    Thickness,
};
use crate::animation::{SurfaceTransitions, Transition};
use bkss_core::{
    AnimationPlan, DisplaySelection, DynamicSurfaceHandle, LayerSurfaceConfig, RuntimeHandle,
    ShellError, SurfaceInstanceHandle, SurfaceRequestError, WeakRuntimeHandle,
};
use gpui::{
    App, AppContext, Edges, Entity, Pixels, Render, Size, StyleRefinement, Styled, Window,
    layer_shell::LayerShellOptions, px, size,
};
use std::time::Duration;

/// A reusable inert declaration. Native configuration is fixed when mounted;
/// `Styled` methods configure a fresh content root, not the native surface.
#[derive(Clone)]
pub struct ShellSurface {
    pub(super) style: StyleRefinement,
    pub(super) geometry: Geometry,
    pub(super) transitions: SurfaceTransitions,
    pub(super) backdrop: bool,
    options: LayerShellOptions,
    native_size: Size<Pixels>,
    outputs: DisplaySelection,
    visible: bool,
    manual_zone: Option<Pixels>,
}

impl Default for ShellSurface {
    fn default() -> Self {
        Self {
            style: StyleRefinement::default(),
            geometry: Geometry::Content {
                placement: Placement::Center,
                maximum: None,
            },
            transitions: SurfaceTransitions::default(),
            backdrop: false,
            options: LayerShellOptions {
                namespace: "bkss-surface".into(),
                layer: Layer::Top,
                anchor: Anchor::all(),
                exclusive_zone: Some(px(0.)),
                exclusive_edge: None,
                margin: None,
                keyboard_interactivity: KeyboardInteractivity::None,
            },
            native_size: size(px(0.), px(0.)),
            outputs: DisplaySelection::default(),
            visible: false,
            manual_zone: None,
        }
    }
}

impl ShellSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets native dimensions. Zero stretches only along an axis anchored to both edges.
    /// Ordinary GPUI `w` / `h` / `size` still style content only.
    pub fn surface_size(mut self, value: Size<Pixels>) -> Self {
        self.native_size = value;
        self
    }

    pub fn anchor(mut self, value: Anchor) -> Self {
        self.options.anchor = value;
        self
    }

    pub fn exclusive_edge(mut self, value: Anchor) -> Self {
        self.options.exclusive_edge = Some(value);
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        if let Geometry::Content {
            placement: current, ..
        } = &mut self.geometry
        {
            *current = placement;
        }
        self
    }

    pub fn content_limit(mut self, value: Size<Pixels>) -> Self {
        if let Geometry::Content { maximum, .. } = &mut self.geometry {
            *maximum = Some(value);
        }
        self
    }

    /// Registers without invoking the factory until this instance is shown.
    /// Keep mounted resource bindings in `V`; the supplied runtime is weak.
    /// Hotplug and native focus/stacking guarantees are outside this contract.
    pub fn mount<V: Render + 'static>(
        self,
        runtime: &RuntimeHandle,
        mut factory: impl FnMut(
            SurfaceInstanceHandle,
            WeakRuntimeHandle,
            &mut Window,
            &mut App,
        ) -> Result<Entity<V>, ShellError>
        + 'static,
        cx: &mut App,
    ) -> Result<SurfaceHandle, ShellError> {
        let config = self.config().map_err(ShellError::setup)?;
        let plan = self
            .transitions
            .resolve(size(px(1.), px(1.)))
            .map_err(ShellError::setup)?;
        let animated = self.transitions.requires_measurement() || plan != AnimationPlan::default();
        let outputs = self.outputs.clone();
        let visible = self.visible;
        let weak = runtime.downgrade();
        let mount =
            move |instance: SurfaceInstanceHandle, window: &mut Window, cx: &mut App| {
                let content = factory(instance.clone(), weak.clone(), window, cx)?;
                Ok(cx.new(|cx| {
                    SurfaceView::new(self.clone(), content, instance, animated, window, cx)
                }))
            };
        let handle = if animated {
            runtime.register_deferred_animated_dynamic_layer_surface(
                config,
                Duration::from_secs(5),
                mount,
                cx,
            )?
        } else {
            runtime.register_dynamic_layer_surface(config, Duration::from_secs(5), mount, cx)?
        };
        if visible {
            handle.open_on_startup(outputs, cx)?;
        }
        Ok(SurfaceHandle(handle))
    }

    pub(super) fn keyboard(&self) -> KeyboardInteractivity {
        self.options.keyboard_interactivity
    }

    pub(super) fn config(&self) -> Result<LayerSurfaceConfig, SurfaceConfigError> {
        self.geometry.validate()?;
        if matches!(&self.outputs, DisplaySelection::Explicit(ids) if ids.is_empty()) {
            return Err(SurfaceConfigError("at least one output must be selected"));
        }
        let mut options = self.options.clone();
        let native_size = if let Geometry::Bar(bar) = self.geometry {
            options.anchor = if bar.horizontal() {
                bar.edge | Anchor::LEFT | Anchor::RIGHT
            } else {
                bar.edge | Anchor::TOP | Anchor::BOTTOM
            };
            options.exclusive_edge = Some(bar.edge);
            options.exclusive_zone = Some(self.manual_zone.unwrap_or(bar.allocation()));
            if bar.horizontal() {
                size(px(0.), bar.allocation())
            } else {
                size(bar.allocation(), px(0.))
            }
        } else {
            self.native_size
        };
        if options.anchor.bits() & !Anchor::all().bits() != 0 {
            return Err(SurfaceConfigError("surface anchors must be screen edges"));
        }
        for (dimension, opposing) in [
            (native_size.width, Anchor::LEFT | Anchor::RIGHT),
            (native_size.height, Anchor::TOP | Anchor::BOTTOM),
        ] {
            validate_number(
                dimension,
                px(0.),
                "surface dimensions must be finite nonnegative logical pixels",
            )?;
            if dimension == px(0.) && !options.anchor.contains(opposing) {
                return Err(SurfaceConfigError(
                    "a zero surface dimension requires opposing anchors",
                ));
            }
        }
        if let Some(zone) = self.manual_zone {
            if zone != px(-1.) {
                validate_number(
                    zone,
                    px(0.),
                    "exclusive zone must be -1 or finite nonnegative representable logical pixels",
                )?;
            }
            options.exclusive_zone = Some(zone);
        }
        if let Some(edge) = options.exclusive_edge
            && (![Anchor::TOP, Anchor::BOTTOM, Anchor::LEFT, Anchor::RIGHT].contains(&edge)
                || !options.anchor.contains(edge))
        {
            return Err(SurfaceConfigError(
                "exclusive edge must be one anchored screen edge",
            ));
        }
        Ok(LayerSurfaceConfig {
            size: native_size,
            options,
        })
    }
}

/// An initially visible edge bar. Thickness and reservation never follow content styling.
#[derive(Clone)]
pub struct Bar(pub(super) ShellSurface);

impl Bar {
    pub fn new(thickness: Thickness) -> Self {
        let geometry = BarGeometry {
            edge: Anchor::BOTTOM,
            thickness,
            margins: Edges::all(px(12.)),
            full_length: false,
            max_length: None,
            alignment: BarAlignment::Center,
        };
        let mut surface = ShellSurface::new();
        surface.options.namespace = "bkss-bottom-bar".into();
        surface.visible = true;
        surface.geometry = Geometry::Bar(geometry);
        Self(surface)
    }

    pub fn edge(mut self, edge: Anchor) -> Self {
        if let Geometry::Bar(bar) = &mut self.0.geometry {
            bar.edge = edge;
        }
        self
    }

    pub fn margins(mut self, margins: Edges<Pixels>) -> Self {
        if let Geometry::Bar(bar) = &mut self.0.geometry {
            bar.margins = margins;
        }
        self
    }

    pub fn full_length(mut self) -> Self {
        if let Geometry::Bar(bar) = &mut self.0.geometry {
            bar.full_length = true;
        }
        self
    }

    pub fn max_length(mut self, maximum: Pixels) -> Self {
        if let Geometry::Bar(bar) = &mut self.0.geometry {
            bar.max_length = Some(maximum);
        }
        self
    }

    pub fn alignment(mut self, alignment: BarAlignment) -> Self {
        if let Geometry::Bar(bar) = &mut self.0.geometry {
            bar.alignment = alignment;
        }
        self
    }
}

/// Initially hidden, transparent, non-keyboard overlay with content-only pointer input.
/// Effects such as Glass are explicitly composed by the user's fresh content view.
#[derive(Clone)]
pub struct Overlay(pub(super) ShellSurface);

impl Default for Overlay {
    fn default() -> Self {
        let mut surface = ShellSurface::new();
        surface.options.layer = Layer::Overlay;
        surface.options.namespace = "bkss-overlay".into();
        Self(surface)
    }
}

impl Overlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fixes the native viewport size at registration. GPUI `w` / `h` still
    /// style content only; zero dimensions require opposing native anchors.
    pub fn surface_size(mut self, size: Size<Pixels>) -> Self {
        self.0 = self.0.surface_size(size);
        self
    }

    /// Sets creation-time native anchors, independently of content placement.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.0 = self.0.anchor(anchor);
        self
    }

    pub fn exclusive_edge(mut self, edge: Anchor) -> Self {
        self.0 = self.0.exclusive_edge(edge);
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.0 = self.0.placement(placement);
        self
    }

    pub fn content_limit(mut self, value: Size<Pixels>) -> Self {
        self.0 = self.0.content_limit(value);
        self
    }

    /// A stationary dim backdrop consumes exterior pointer-down within the local viewport.
    pub fn dismiss_on_backdrop(mut self) -> Self {
        self.0.backdrop = true;
        self
    }
}

macro_rules! shared_builders {
    ($type:ty, $field:tt) => {
        impl Styled for $type {
            fn style(&mut self) -> &mut StyleRefinement {
                &mut self.$field.style
            }
        }
        impl $type {
            pub fn layer(mut self, layer: Layer) -> Self {
                self.$field.options.layer = layer;
                self
            }
            pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
                self.$field.options.namespace = namespace.into();
                self
            }
            /// Native keyboard policy does not change pointer input regions.
            /// OnDemand does not guarantee activation or focus restoration.
            pub fn keyboard_interactivity(mut self, keyboard: KeyboardInteractivity) -> Self {
                self.$field.options.keyboard_interactivity = keyboard;
                self
            }
            pub fn exclusive_zone(mut self, zone: Pixels) -> Self {
                self.$field.manual_zone = Some(zone);
                self
            }
            pub fn output(self, output: Output) -> Self {
                self.outputs(DisplaySelection::Explicit(vec![output]))
            }
            pub fn outputs(mut self, outputs: DisplaySelection) -> Self {
                self.$field.outputs = outputs;
                self
            }
            pub fn initially_visible(mut self, visible: bool) -> Self {
                self.$field.visible = visible;
                self
            }
            pub fn transition(mut self, transition: Transition) -> Self {
                self.$field.transitions.set_transition(transition);
                self
            }
            pub fn enter_transition(mut self, transition: Transition) -> Self {
                self.$field.transitions.set_enter(transition);
                self
            }
            pub fn exit_transition(mut self, transition: Transition) -> Self {
                self.$field.transitions.set_exit(transition);
                self
            }
            pub fn mount<V: Render + 'static>(
                self,
                runtime: &RuntimeHandle,
                factory: impl FnMut(
                    SurfaceInstanceHandle,
                    WeakRuntimeHandle,
                    &mut Window,
                    &mut App,
                ) -> Result<Entity<V>, ShellError>
                + 'static,
                cx: &mut App,
            ) -> Result<SurfaceHandle, ShellError> {
                self.$field.mount(runtime, factory, cx)
            }
        }
    };
}

shared_builders!(Bar, 0);
shared_builders!(Overlay, 0);

impl Styled for ShellSurface {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ShellSurface {
    pub fn layer(mut self, layer: Layer) -> Self {
        self.options.layer = layer;
        self
    }
    pub fn namespace(mut self, namespace: impl Into<String>) -> Self {
        self.options.namespace = namespace.into();
        self
    }
    pub fn keyboard_interactivity(mut self, keyboard: KeyboardInteractivity) -> Self {
        self.options.keyboard_interactivity = keyboard;
        self
    }
    pub fn exclusive_zone(mut self, zone: Pixels) -> Self {
        self.manual_zone = Some(zone);
        self
    }
    pub fn output(self, output: Output) -> Self {
        self.outputs(DisplaySelection::Explicit(vec![output]))
    }
    pub fn outputs(mut self, outputs: DisplaySelection) -> Self {
        self.outputs = outputs;
        self
    }
    pub fn initially_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
    pub fn transition(mut self, transition: Transition) -> Self {
        self.transitions.set_transition(transition);
        self
    }
    pub fn enter_transition(mut self, transition: Transition) -> Self {
        self.transitions.set_enter(transition);
        self
    }
    pub fn exit_transition(mut self, transition: Transition) -> Self {
        self.transitions.set_exit(transition);
        self
    }
}

/// Weak requests to the core-owned per-display controller.
#[derive(Clone)]
pub struct SurfaceHandle(DynamicSurfaceHandle);

impl SurfaceHandle {
    pub fn show(&self, output: Output, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.0.open(output, cx)
    }
    pub fn hide(&self, output: Output, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.0.close(output, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bar_edges_fix_allocation_and_reservation_independently_of_content_style() {
        let margins = Edges {
            top: px(11.),
            right: px(13.),
            bottom: px(17.),
            left: px(19.),
        };
        for (edge, margin, horizontal) in [
            (Anchor::TOP, 11., true),
            (Anchor::BOTTOM, 17., true),
            (Anchor::LEFT, 19., false),
            (Anchor::RIGHT, 13., false),
        ] {
            let bar = Bar::new(Thickness::new(px(40.)).unwrap())
                .h(px(500.))
                .w(px(900.))
                .edge(edge)
                .margins(margins);
            let config = bar.0.config().unwrap();
            assert_eq!(config.options.namespace, "bkss-bottom-bar");
            assert_eq!(config.options.layer, Layer::Top);
            assert_eq!(
                config.options.keyboard_interactivity,
                KeyboardInteractivity::None
            );
            assert_eq!(config.options.exclusive_edge, Some(edge));
            assert_eq!(config.options.exclusive_zone, Some(px(40. + margin)));
            assert_eq!(
                config.options.margin, None,
                "visual margins are already in the allocation"
            );
            assert_eq!(
                config.size,
                if horizontal {
                    size(px(0.), px(40. + margin))
                } else {
                    size(px(40. + margin), px(0.))
                }
            );
            let overridden = bar.exclusive_zone(px(8.)).0.config().unwrap();
            assert_eq!(overridden.options.exclusive_zone, Some(px(8.)));
            assert_eq!(overridden.size, config.size);
        }
    }

    #[test]
    fn overlay_native_defaults_are_transparent_nonkeyboard_and_nonreserving() {
        let overlay = Overlay::new();
        let config = overlay.0.config().unwrap();
        assert!(!overlay.0.visible);
        assert!(!overlay.0.backdrop);
        assert_eq!(config.options.layer, Layer::Overlay);
        assert_eq!(config.options.exclusive_zone, Some(px(0.)));
        assert_eq!(
            config.options.keyboard_interactivity,
            KeyboardInteractivity::None
        );
        assert_eq!(config.options.anchor, Anchor::all());
        assert_eq!(config.size, size(px(0.), px(0.)));
        assert_eq!(overlay.0.style, StyleRefinement::default());
        assert_eq!(
            overlay.0.transitions.resolve(size(px(1.), px(1.))).unwrap(),
            AnimationPlan::default()
        );
    }

    #[test]
    fn overlay_native_configuration_is_explicit_and_independent_of_content_style() {
        let overlay = Overlay::new()
            .anchor(Anchor::TOP | Anchor::RIGHT)
            .surface_size(size(px(320.), px(240.)))
            .exclusive_edge(Anchor::TOP)
            .exclusive_zone(px(-1.))
            .w(px(900.))
            .h(px(800.));
        let config = overlay.0.config().unwrap();
        assert_eq!(config.size, size(px(320.), px(240.)));
        assert_eq!(config.options.anchor, Anchor::TOP | Anchor::RIGHT);
        assert_eq!(config.options.exclusive_edge, Some(Anchor::TOP));
        assert_eq!(config.options.exclusive_zone, Some(px(-1.)));
        assert!(!overlay.0.visible);
        assert!(!overlay.0.backdrop);
    }

    #[test]
    fn manual_zone_accepts_protocol_minus_one_but_rejects_invalid_values() {
        for zone in [-1., 0., 100.] {
            let bar = Bar::new(Thickness::new(px(40.)).unwrap()).exclusive_zone(px(zone));
            assert_eq!(
                bar.0.config().unwrap().options.exclusive_zone,
                Some(px(zone))
            );
        }
        for zone in [
            -2.,
            -0.5,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
        ] {
            assert!(Overlay::new().exclusive_zone(px(zone)).0.config().is_err());
            assert!(
                Bar::new(Thickness::new(px(40.)).unwrap())
                    .exclusive_zone(px(zone))
                    .0
                    .config()
                    .is_err()
            );
        }
    }
}

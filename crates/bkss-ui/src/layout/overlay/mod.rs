use std::{error::Error, fmt, time::Duration};

use crate::animation::{Animation, AnimationError};
use bkss_core::{
    DynamicSurfaceHandle, Glass, GlassState, GlassStyle, LayerSurfaceConfig, RuntimeHandle,
    ShellError, SurfaceInstanceHandle, SurfaceRequestError,
};
use gpui::{
    AnyElement, App, AppContext, AvailableSpace, Bounds, ContentMask, Context, Element, ElementId,
    Entity, FocusHandle, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels,
    Render, Size, Style, Window, div,
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
    point,
    prelude::*,
    px, rgba, size,
};

mod geometry;
mod view;

pub use geometry::{
    KeyboardFocus, OverlayAnchors, OverlayGeometry, OverlayGeometryError, OverlayPlacement,
    PointerDismiss,
};
use view::OverlayView;

/// An inert configuration, reusable with independent per-mount content factories.
/// Style inputs are validated by the existing Glass value constructors.
#[derive(Clone)]
pub struct OverlayDefinition {
    keyboard: KeyboardFocus,
    pointer_dismiss: PointerDismiss,
    geometry: OverlayGeometry,
    style: GlassStyle,
    animation: Option<Animation>,
}

impl OverlayDefinition {
    /// Keyboard policy is intentionally explicit, with no default implementation.
    pub fn new(keyboard: KeyboardFocus) -> Self {
        Self {
            keyboard,
            pointer_dismiss: PointerDismiss::Disabled,
            geometry: OverlayGeometry::default(),
            style: GlassStyle::default(),
            animation: None,
        }
    }

    pub fn with_geometry(
        mut self,
        geometry: OverlayGeometry,
    ) -> Result<Self, OverlayGeometryError> {
        geometry.validate()?;
        self.geometry = geometry;
        Ok(self)
    }

    pub fn with_pointer_dismiss(mut self, pointer_dismiss: PointerDismiss) -> Self {
        self.pointer_dismiss = pointer_dismiss;
        self
    }

    pub fn with_style(mut self, style: GlassStyle) -> Self {
        self.style = style;
        self
    }

    /// Adds opacity motion without starting a clock or mounting content.
    /// Later assignments override the same phase, preserving other assignments.
    pub fn with_animation(mut self, animation: Animation) -> Result<Self, AnimationError> {
        let animation = self
            .animation
            .map_or(animation, |current| current.merge(animation));
        animation.compile()?;
        self.animation = Some(animation);
        Ok(self)
    }

    fn surface_config(&self) -> LayerSurfaceConfig {
        LayerSurfaceConfig {
            size: size(px(0.), px(0.)),
            options: LayerShellOptions {
                namespace: "bkss-overlay".into(),
                layer: Layer::Overlay,
                anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                exclusive_zone: Some(px(0.)),
                exclusive_edge: None,
                margin: None,
                keyboard_interactivity: match self.keyboard {
                    KeyboardFocus::None => KeyboardInteractivity::None,
                    KeyboardFocus::Interactive => KeyboardInteractivity::OnDemand,
                },
            },
        }
    }
}

/// Weak runtime-owned open/close requests, independently targeted by display.
/// Redundant open does not raise, activate, or reorder an existing instance.
#[derive(Clone)]
pub struct OverlayHandle(DynamicSurfaceHandle);

impl OverlayHandle {
    pub fn open(&self, display: gpui::DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.0.open(display, cx)
    }

    pub fn close(&self, display: gpui::DisplayId, cx: &mut App) -> Result<(), SurfaceRequestError> {
        self.0.close(display, cx)
    }
}

/// Additive extension; the existing BottomBar extension remains unchanged.
pub trait OverlayRuntimeHandleExt {
    /// Registers without opening a surface. An open after a completed close calls
    /// the fallible factory again; reopening during exit reuses the mounted content.
    /// Capture descriptors and a weak runtime, not mounted leases or a strong
    /// runtime; keep leases inside the returned view.
    ///
    /// Interactive Escape bubbles from content: a child may consume it; otherwise
    /// it dismisses only the receiving generation. No cross-overlay keyboard
    /// arbitration, native activation, order, or focus restoration is promised.
    ///
    /// Content and backdrop share one Layer::Overlay surface per display. Both
    /// use its current configured local viewport, not full display bounds.
    /// Only stable initial positive reservations are supported; live reservation
    /// tracking is not. Non-reserving bars and same-layer overlays may cover or
    /// block each other according to the compositor's ordering.
    fn mount_overlay<V: Render + 'static>(
        &self,
        definition: OverlayDefinition,
        content: impl FnMut(gpui::DisplayId, &mut Window, &mut App) -> Result<Entity<V>, ShellError>
        + 'static,
        cx: &mut App,
    ) -> Result<OverlayHandle, ShellError>;
}

impl OverlayRuntimeHandleExt for RuntimeHandle {
    fn mount_overlay<V: Render + 'static>(
        &self,
        definition: OverlayDefinition,
        mut content: impl FnMut(gpui::DisplayId, &mut Window, &mut App) -> Result<Entity<V>, ShellError>
        + 'static,
        cx: &mut App,
    ) -> Result<OverlayHandle, ShellError> {
        definition.geometry.validate().map_err(ShellError::setup)?;
        let plan = definition
            .animation
            .map(Animation::compile)
            .transpose()
            .map_err(ShellError::setup)?;
        let config = definition.surface_config();
        let factory = move |instance: SurfaceInstanceHandle, window: &mut Window, cx: &mut App| {
            let content = content(instance.display_id(), window, cx)?;
            Ok(cx.new(|cx| OverlayView::new(&definition, content, instance, window, cx)))
        };
        match plan {
            Some(plan) => self.register_animated_dynamic_layer_surface(
                config,
                Duration::from_secs(5),
                plan,
                factory,
                cx,
            ),
            None => {
                self.register_dynamic_layer_surface(config, Duration::from_secs(5), factory, cx)
            }
        }
        .map(OverlayHandle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn explicit_modes_configure_one_zero_sized_overlay_surface() {
        for (mode, keyboard) in [
            (KeyboardFocus::None, KeyboardInteractivity::None),
            (KeyboardFocus::Interactive, KeyboardInteractivity::OnDemand),
        ] {
            let definition = OverlayDefinition::new(mode);
            assert_eq!(definition.pointer_dismiss, PointerDismiss::Disabled);
            let config = definition.surface_config();
            assert_eq!(config.size, size(px(0.), px(0.)));
            assert_eq!(config.options.namespace, "bkss-overlay");
            assert_eq!(config.options.layer, Layer::Overlay);
            assert_eq!(
                config.options.anchor,
                Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT
            );
            assert_eq!(config.options.exclusive_zone, Some(px(0.)));
            assert_eq!(config.options.keyboard_interactivity, keyboard);
        }
    }

    #[test]
    fn contradictory_anchor_geometry_is_rejected_by_definition_builder() {
        for (edges, expected) in [
            (
                OverlayAnchors {
                    left: true,
                    right: true,
                    ..Default::default()
                },
                OverlayGeometryError::ContradictoryAnchors,
            ),
            (
                OverlayAnchors {
                    top: true,
                    bottom: true,
                    ..Default::default()
                },
                OverlayGeometryError::ContradictoryAnchors,
            ),
        ] {
            let geometry = OverlayGeometry {
                placement: OverlayPlacement::Anchor {
                    edges,
                    offset: [0., 0.],
                },
                ..Default::default()
            };
            assert_eq!(
                OverlayDefinition::new(KeyboardFocus::None)
                    .with_geometry(geometry)
                    .err(),
                Some(expected)
            );
        }
    }

    #[test]
    fn invalid_geometry_is_rejected_before_registration() {
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let runtime = bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap();
            let mut definition = OverlayDefinition::new(KeyboardFocus::None);
            definition.geometry.max_logical_size = Some([f32::NAN, 10.]);
            let result =
                runtime.mount_overlay(definition, |_, _, cx| Ok(cx.new(|_| gpui::EmptyView)), cx);
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("invalid geometry registered"),
            };
            assert_eq!(
                error
                    .source()
                    .and_then(|e| e.downcast_ref::<OverlayGeometryError>()),
                Some(&OverlayGeometryError::InvalidMaximumSize)
            );
            assert!(cx.windows().is_empty());
            runtime.shutdown(cx);
        });
    }

    #[test]
    fn animation_builder_is_inert_validated_and_merges_phases() {
        use crate::Timing;
        let definition = OverlayDefinition::new(KeyboardFocus::None)
            .with_animation(Animation::transition(Timing::default()).opacity(0., 1.))
            .unwrap()
            .with_animation(Animation::exit(Timing::spring_default()).opacity(1., 0.2))
            .unwrap();
        let channel = definition
            .animation
            .unwrap()
            .compile()
            .unwrap()
            .opacity
            .unwrap();
        assert_eq!(channel.enter.unwrap().endpoints, [0., 1.]);
        assert_eq!(channel.exit.unwrap().endpoints, [1., 0.2]);
        assert!(
            OverlayDefinition::new(KeyboardFocus::None)
                .with_animation(Animation::enter(Timing::default()).opacity(f32::NAN, 1.))
                .is_err()
        );
    }

    #[test]
    fn invalid_animation_is_rejected_before_registration_or_factory() {
        use crate::Timing;
        let cx = TestAppContext::single();
        cx.update(|cx| {
            let runtime = bkss_core::Shell::attach(cx, |_, _| Ok(())).unwrap();
            let mut definition = OverlayDefinition::new(KeyboardFocus::None);
            definition.animation = Some(Animation::enter(Timing::default()).opacity(-1., 1.));
            let error = runtime
                .mount_overlay(
                    definition,
                    |_, _, _| -> Result<Entity<gpui::EmptyView>, ShellError> {
                        panic!("invalid animation must not invoke the factory")
                    },
                    cx,
                )
                .err()
                .unwrap();
            assert_eq!(
                error
                    .source()
                    .and_then(|error| error.downcast_ref::<AnimationError>()),
                Some(&AnimationError::InvalidEndpoint)
            );
            assert!(cx.windows().is_empty());
            runtime.shutdown(cx);
        });
    }
}

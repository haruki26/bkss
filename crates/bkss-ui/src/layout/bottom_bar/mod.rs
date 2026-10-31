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

mod geometry;
mod view;

pub use geometry::{
    BottomBarAlignment, BottomBarGeometry, BottomBarGeometryError, BottomBarMargins, BottomBarWidth,
};
use view::BottomBarView;

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

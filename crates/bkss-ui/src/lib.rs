//! Shell widgets built on BKSS core and ordinary GPUI content.

pub mod animation;
pub mod effects;
pub mod shell;

pub use animation::{AnimationError, SpringPreset, Transition};
pub use bkss_core::DisplaySelection;
pub use effects::Glass;
pub use shell::{
    Anchor, Bar, BarAlignment, KeyboardInteractivity, Layer, Output, Overlay, Placement,
    ShellSurface, SurfaceConfigError, SurfaceHandle, Thickness,
};

//! Static shell widgets built on BKSS core and ordinary GPUI content.

mod animation;
mod layout;

pub use animation::{Animation, AnimationError, EasingCurve, Timing};
pub use bkss_core::DisplaySelection;
pub use layout::{
    BottomBar, BottomBarAlignment, BottomBarGeometry, BottomBarGeometryError, BottomBarMargins,
    BottomBarWidth, KeyboardFocus, OverlayAnchors, OverlayDefinition, OverlayGeometry,
    OverlayGeometryError, OverlayHandle, OverlayPlacement, OverlayRuntimeHandleExt, PointerDismiss,
    RuntimeHandleExt,
};

//! Static shell widgets built on BKSS core and ordinary GPUI content.

mod bottom_bar;
mod overlay;

pub use bkss_core::DisplaySelection;
pub use bottom_bar::{
    BottomBar, BottomBarAlignment, BottomBarGeometry, BottomBarGeometryError, BottomBarMargins,
    BottomBarWidth, RuntimeHandleExt,
};
pub use overlay::{
    KeyboardFocus, OverlayAnchors, OverlayDefinition, OverlayGeometry, OverlayGeometryError,
    OverlayHandle, OverlayPlacement, OverlayRuntimeHandleExt, PointerDismiss,
};

mod descriptor;
mod error;
mod layer;
mod readiness;
pub(crate) use layer::{LayerContentFactory, StartupTargetError, StartupTargets};
pub(crate) use readiness::{DisplayReadiness, Readiness};

pub use descriptor::{SurfaceDescriptor, SurfaceDescriptorError};
pub use error::SurfaceError;
pub use layer::{DisplaySelection, LayerSurfaceConfig, LayerSurfaceDeclaration, LayerSurfaceError};

pub const DEFAULT_DISPLAY_READINESS_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(5);
pub(crate) const DISPLAY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
pub(crate) const SURFACE_NAMESPACE: &str = "bkss-core-surface";

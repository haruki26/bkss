mod config;
mod descriptor;
mod error;
mod layer;
mod readiness;

pub(crate) use config::{DISPLAY_POLL_INTERVAL, SURFACE_NAMESPACE};
pub(crate) use layer::{LayerContentFactory, StartupTargetError, StartupTargets};
pub(crate) use readiness::{DisplayReadiness, Readiness};

pub use config::DEFAULT_DISPLAY_READINESS_TIMEOUT;
pub use descriptor::{SurfaceDescriptor, SurfaceDescriptorError};
pub use error::SurfaceError;
pub use layer::{DisplaySelection, LayerSurfaceConfig, LayerSurfaceDeclaration, LayerSurfaceError};

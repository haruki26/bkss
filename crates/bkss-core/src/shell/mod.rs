mod binding;
mod entry;
mod error;
mod runtime;
mod surface;
#[cfg(test)]
pub(super) mod test_support;

pub use binding::MountedBinding;
pub use entry::Shell;
pub use error::ShellError;
pub use runtime::{RuntimeHandle, WeakRuntimeHandle};
pub use surface::{
    DynamicSurfaceConfigError, DynamicSurfaceHandle, SurfaceInstanceHandle, SurfaceRequestError,
    validate_dynamic_surface_config_for_ui,
};

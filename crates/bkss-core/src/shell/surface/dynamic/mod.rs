mod animation_driver;
mod controller;

pub(in crate::shell) use controller::{Controller, drop_controllers, register, stop_controllers};
pub use controller::{
    DynamicSurfaceConfigError, DynamicSurfaceHandle, SurfaceInstanceHandle, SurfaceRequestError,
    validate_dynamic_surface_config_for_ui,
};

mod animation;
mod dynamic;
mod startup;
mod window;

pub(super) use dynamic::{Controller, drop_controllers, register, stop_controllers};
pub use dynamic::{DynamicSurfaceHandle, SurfaceInstanceHandle, SurfaceRequestError};
pub(super) use startup::{
    SurfaceWork, drop_surface_works, mount_layer_surface, mount_surface, stop_surface_works,
};
pub(super) use window::{
    layer_window_options, remove_window, remove_window_later, remove_windows_later,
};

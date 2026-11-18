mod backend;
mod error;
mod source;
mod state;
mod view;

pub use error::{HyprlandError, WorkspaceSwitch};
pub use source::Hyprland;
pub use state::{
    ActiveWindow, ActiveWorkspace, HyprlandClient, HyprlandState, SpecialWorkspace, Workspace,
};

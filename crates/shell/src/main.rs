mod actions;
mod calendar_model;
mod notification_model;
mod services;
mod settings;
mod sources;
mod ui;

use std::{error::Error, ffi::OsStr};

use actions::Actions;
use services::Services;

fn main() -> Result<(), Box<dyn Error>> {
    let services = Services::new()?;
    let actions = Actions::new(
        services.hyprland_socket_dir.clone(),
        services.powermenu_socket.clone(),
    );
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.as_slice() == [OsStr::new("toggle"), OsStr::new("powermenu")] {
        async_io::block_on(actions.toggle_powermenu())?;
        return Ok(());
    }
    if !args.is_empty() {
        return Err("usage: shell [toggle powermenu]".into());
    }
    ui::run(services, actions)?;
    Ok(())
}

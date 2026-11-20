mod actions;
mod calendar_model;
mod notification_model;
mod services;
mod settings;
mod sources;
mod ui;

use std::{error::Error, ffi::OsStr};

use actions::{Actions, toggle_powermenu};
use services::Services;

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(std::env::args_os().skip(1))? {
        Command::TogglePowermenu => {
            let socket = services::powermenu_socket(&services::runtime_dir()?);
            async_io::block_on(toggle_powermenu(socket))?;
        }
        Command::Run => {
            let services = Services::new()?;
            let actions = Actions::new(services.hyprland_socket_dir.clone());
            ui::run(services, actions)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Run,
    TogglePowermenu,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Command, &'static str> {
    let args: Vec<_> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Command::Run),
        [command, action]
            if command == OsStr::new("toggle") && action == OsStr::new("powermenu") =>
        {
            Ok(Command::TogglePowermenu)
        }
        _ => Err("usage: shell [toggle powermenu]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_before_services_are_initialized() {
        assert_eq!(
            parse_args(Vec::<std::ffi::OsString>::new()),
            Ok(Command::Run)
        );
        assert_eq!(
            parse_args(["toggle", "powermenu"].map(Into::into)),
            Ok(Command::TogglePowermenu)
        );
        assert_eq!(
            parse_args(["unknown"].map(Into::into)),
            Err("usage: shell [toggle powermenu]")
        );
    }
}

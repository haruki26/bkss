mod actions;
mod calendar_model;
mod notification_model;
mod services;
mod settings;
mod sources;
mod ui;

use std::{error::Error, ffi::OsStr};

use actions::toggle_powermenu;

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(std::env::args_os().skip(1))? {
        Command::TogglePowermenu => {
            let socket = services::powermenu_socket(&services::runtime_dir()?);
            async_io::block_on(toggle_powermenu(socket))?;
        }
        Command::Run | Command::Quit => ui::run()?,
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Run,
    Quit,
    TogglePowermenu,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Command, &'static str> {
    let args: Vec<_> = args.into_iter().collect();
    match args.as_slice() {
        [command] if command == OsStr::new("run") => Ok(Command::Run),
        [command] if command == OsStr::new("quit") => Ok(Command::Quit),
        [command, action]
            if command == OsStr::new("toggle") && action == OsStr::new("powermenu") =>
        {
            Ok(Command::TogglePowermenu)
        }
        _ => Err("usage: bkss {run|quit|toggle powermenu}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_before_services_are_initialized() {
        assert_eq!(
            parse_args(Vec::<std::ffi::OsString>::new()),
            Err("usage: bkss {run|quit|toggle powermenu}")
        );
        assert_eq!(parse_args(["run"].map(Into::into)), Ok(Command::Run));
        assert_eq!(parse_args(["quit"].map(Into::into)), Ok(Command::Quit));
        assert_eq!(
            parse_args(["toggle", "powermenu"].map(Into::into)),
            Ok(Command::TogglePowermenu)
        );
        assert_eq!(
            parse_args(["unknown"].map(Into::into)),
            Err("usage: bkss {run|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["run", "extra"].map(Into::into)),
            Err("usage: bkss {run|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["quit", "extra"].map(Into::into)),
            Err("usage: bkss {run|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["toggle", "powermenu", "extra"].map(Into::into)),
            Err("usage: bkss {run|quit|toggle powermenu}")
        );
    }
}

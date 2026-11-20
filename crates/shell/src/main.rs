mod actions;
mod calendar_model;
mod notification_model;
mod services;
mod settings;
mod sources;
mod ui;

use std::{
    error::Error,
    ffi::OsStr,
    io,
    process::{Child, Command as ProcessCommand, Stdio},
};

use actions::toggle_powermenu;

fn main() -> Result<(), Box<dyn Error>> {
    match parse_args(std::env::args_os().skip(1))? {
        Command::TogglePowermenu => {
            let socket = services::powermenu_socket(&services::runtime_dir()?);
            async_io::block_on(toggle_powermenu(socket))?;
        }
        Command::Run => {
            let mut command = ProcessCommand::new(std::env::current_exe()?);
            command.args(["run", "--foreground"]);
            spawn_background(&mut command)?;
        }
        Command::RunForeground | Command::Quit => ui::run()?,
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Run,
    RunForeground,
    Quit,
    TogglePowermenu,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Command, &'static str> {
    let args: Vec<_> = args.into_iter().collect();
    match args.as_slice() {
        [command] if command == OsStr::new("run") => Ok(Command::Run),
        [command, option]
            if command == OsStr::new("run") && option == OsStr::new("--foreground") =>
        {
            Ok(Command::RunForeground)
        }
        [command] if command == OsStr::new("quit") => Ok(Command::Quit),
        [command, action]
            if command == OsStr::new("toggle") && action == OsStr::new("powermenu") =>
        {
            Ok(Command::TogglePowermenu)
        }
        _ => Err("usage: bkss {run [--foreground]|quit|toggle powermenu}"),
    }
}

fn spawn_background(command: &mut ProcessCommand) -> io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_before_services_are_initialized() {
        assert_eq!(
            parse_args(Vec::<std::ffi::OsString>::new()),
            Err("usage: bkss {run [--foreground]|quit|toggle powermenu}")
        );
        assert_eq!(parse_args(["run"].map(Into::into)), Ok(Command::Run));
        assert_eq!(
            parse_args(["run", "--foreground"].map(Into::into)),
            Ok(Command::RunForeground)
        );
        assert_eq!(parse_args(["quit"].map(Into::into)), Ok(Command::Quit));
        assert_eq!(
            parse_args(["toggle", "powermenu"].map(Into::into)),
            Ok(Command::TogglePowermenu)
        );
        assert_eq!(
            parse_args(["unknown"].map(Into::into)),
            Err("usage: bkss {run [--foreground]|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["run", "extra"].map(Into::into)),
            Err("usage: bkss {run [--foreground]|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["quit", "extra"].map(Into::into)),
            Err("usage: bkss {run [--foreground]|quit|toggle powermenu}")
        );
        assert_eq!(
            parse_args(["toggle", "powermenu", "extra"].map(Into::into)),
            Err("usage: bkss {run [--foreground]|quit|toggle powermenu}")
        );
    }

    #[test]
    fn background_child_uses_dev_null_for_standard_streams() {
        let mut command = ProcessCommand::new("sh");
        command.args([
            "-c",
            r#"for fd in 0 1 2; do test "$(readlink "/proc/$$/fd/$fd")" = /dev/null || exit 1; done"#,
        ]);

        let mut child = spawn_background(&mut command).expect("spawn short-lived child");
        assert!(child.wait().expect("wait for short-lived child").success());
    }
}

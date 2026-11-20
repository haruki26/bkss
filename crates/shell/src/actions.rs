use std::{
    future::Future,
    io,
    pin::Pin,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use bkss_integration::{CommandIpcClient, Hyprland, WorkspaceSwitch};

pub struct Actions {
    hyprland_socket_dir: std::path::PathBuf,
}

impl Actions {
    pub fn new(hyprland_socket_dir: std::path::PathBuf) -> Self {
        Self {
            hyprland_socket_dir,
        }
    }

    pub fn lock(&self) -> ActionResult {
        spawn_action(|| checked_command("hyprlock", &[]))
    }

    pub fn reboot(&self) -> ActionResult {
        spawn_action(|| checked_command("systemctl", &["reboot"]))
    }

    pub fn poweroff(&self) -> ActionResult {
        spawn_action(|| checked_command("systemctl", &["poweroff"]))
    }

    pub fn switch_workspace(&self, workspace: &str) -> Result<WorkspaceSwitch, std::io::Error> {
        Hyprland::switch_workspace(&self.hyprland_socket_dir, workspace)
    }
}

pub(crate) fn toggle_powermenu(socket: std::path::PathBuf) -> ActionResult {
    spawn_action(move || {
        let response = CommandIpcClient::send(socket, "toggle", b"powermenu")?;
        if response == b"ok" {
            Ok(())
        } else {
            Err(io::Error::other("power-menu IPC rejected the command"))
        }
    })
}

pub type ActionResult = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

fn spawn_action(action: impl FnOnce() -> io::Result<()> + Send + 'static) -> ActionResult {
    let (sender, receiver) = mpsc::sync_channel(1);
    let spawn = thread::Builder::new()
        .name("shell-action".into())
        .spawn(move || {
            let _ = sender.send(action());
        });
    let result = if let Err(error) = spawn {
        let (sender, receiver) = mpsc::sync_channel(1);
        let _ = sender.send(Err(error));
        receiver
    } else {
        receiver
    };
    Box::pin(async move {
        loop {
            match result.try_recv() {
                Ok(value) => return value,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::other("background action ended without a result"));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    async_io::Timer::after(Duration::from_millis(10)).await;
                }
            }
        }
    })
}

fn checked_command(program: &str, args: &[&str]) -> io::Result<()> {
    let status = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited with {status}")))
    }
}

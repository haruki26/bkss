use crate::base::worker;
use crate::{Source, SourceDescriptor, SourcePublisher};
use gpui::App;
use std::{
    io,
    io::BufReader,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    thread,
};

use super::{
    backend::{IO_TIMEOUT, command, protocol, read_event_line, snapshot},
    error::{HyprlandError, WorkspaceSwitch},
    state::HyprlandState,
};

pub struct Hyprland {
    socket_dir: PathBuf,
}

impl Hyprland {
    /// Declares a Hyprland source for the directory containing `.socket.sock`.
    ///
    /// The descriptor is inert until mounted; its event listener is connected
    /// on a worker before the initial command snapshot is requested.
    pub fn new(socket_dir: impl Into<PathBuf>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            socket_dir: socket_dir.into(),
        })
    }

    /// Runs the workspace command on a worker; awaiting its result does not block the caller.
    ///
    /// Invalid workspace names are rejected without connecting to the backend.
    pub fn switch_workspace(
        socket_dir: impl AsRef<Path>,
        workspace: impl Into<String>,
    ) -> io::Result<WorkspaceSwitch> {
        let socket = socket_dir.as_ref().join(".socket.sock");
        let workspace = workspace.into();
        let (sender, receiver) = async_channel::bounded(1);
        thread::Builder::new()
            .name("bkss-hyprland-command".into())
            .spawn(move || {
                let result = match workspace_dispatch_request(&workspace) {
                    Err(message) => Err(protocol(
                        "switch Hyprland workspace",
                        message,
                        Some(socket.clone()),
                    )),
                    Ok(request) => command(&socket, &request).and_then(|response| {
                        if response.trim().starts_with("ok") {
                            Ok(())
                        } else {
                            Err(protocol(
                                "switch Hyprland workspace",
                                &format!("Hyprland rejected the workspace: {}", response.trim()),
                                Some(socket.clone()),
                            ))
                        }
                    }),
                };
                let _ = sender.try_send(result);
            })?;
        Ok(Box::pin(async move {
            receiver.recv().await.unwrap_or_else(|_| {
                Err(protocol(
                    "switch Hyprland workspace",
                    "background operation ended without a result",
                    None,
                ))
            })
        }))
    }
}

fn workspace_dispatch_request(workspace: &str) -> Result<String, &'static str> {
    if workspace.is_empty() || workspace.len() > 512 || workspace.contains(['\n', '\r', '\0', ';'])
    {
        return Err("workspace name is empty or contains invalid characters");
    }

    if let Ok(id) = workspace.parse::<i64>() {
        return Ok(format!("/dispatch hl.dsp.focus({{ workspace = {id} }})"));
    }

    let name = workspace.strip_prefix("name:").unwrap_or(workspace);
    if name.is_empty() {
        return Err("workspace name is empty or contains invalid characters");
    }
    Ok(format!(
        "/dispatch hl.dsp.focus({{ workspace = {} }})",
        lua_string(&format!("name:{name}"))
    ))
}

fn lua_string(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() + 2);
    encoded.push('"');
    for character in value.chars() {
        match character {
            '"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character if character.is_control() => {
                for byte in character.to_string().bytes() {
                    encoded.push_str(&format!("\\{byte:03}"));
                }
            }
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

impl Source for Hyprland {
    type Key = PathBuf;
    type Value = HyprlandState;
    type Error = HyprlandError;
    type Guard = HyprlandGuard;

    fn key(&self) -> Self::Key {
        self.socket_dir.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let event_path = self.socket_dir.join(".socket2.sock");
        let command_path = self.socket_dir.join(".socket.sock");
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        worker::spawn_worker(
            "bkss-hyprland-events",
            bridge,
            sender,
            move |publisher, cancellation| run(event_path, command_path, publisher, cancellation),
        )
        .map_err(|source| HyprlandError::Io {
            operation: "start Hyprland event worker",
            endpoint: self.socket_dir.join(".socket2.sock"),
            source,
        })
        .map(|worker| HyprlandGuard { _worker: worker })
    }
}

pub struct HyprlandGuard {
    _worker: worker::WorkerGuard,
}

fn run(
    event_path: PathBuf,
    command_path: PathBuf,
    publisher: crate::ThreadPublisher<HyprlandState, HyprlandError>,
    cancellation: worker::Cancellation,
) -> Result<(), HyprlandError> {
    let event_stream = UnixStream::connect(&event_path).map_err(|source| HyprlandError::Io {
        operation: "connect to Hyprland event socket",
        endpoint: event_path.clone(),
        source,
    })?;
    event_stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|source| HyprlandError::Io {
            operation: "configure Hyprland event socket",
            endpoint: event_path.clone(),
            source,
        })?;
    let initial = snapshot(&command_path)?;
    publisher.publish(initial).map_err(|_| {
        protocol(
            "publish Hyprland initial state",
            "source is closed",
            Some(event_path.clone()),
        )
    })?;
    let mut events = BufReader::new(event_stream);
    let mut line = Vec::new();
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        match read_event_line(&mut events, &mut line) {
            Ok(0) => {
                return Err(protocol(
                    "read Hyprland events",
                    "unexpected EOF",
                    Some(event_path.clone()),
                ));
            }
            Ok(_) if line.ends_with(b"\n") => {
                let event =
                    std::str::from_utf8(&line).map_err(|error| HyprlandError::Protocol {
                        operation: "parse Hyprland event",
                        endpoint: Some(event_path.clone()),
                        message: error.to_string(),
                    })?;
                let well_formed = event.trim_end().contains(">>");
                line.clear();
                if !well_formed {
                    return Err(protocol(
                        "parse Hyprland event",
                        "event is missing separator",
                        Some(event_path.clone()),
                    ));
                }
                let state = snapshot(&command_path)?;
                if publisher.publish(state).is_err() {
                    return Ok(());
                }
            }
            Ok(_) => {
                return Err(protocol(
                    "read Hyprland events",
                    "event too large",
                    Some(event_path.clone()),
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(source) => {
                return Err(HyprlandError::Io {
                    operation: "read Hyprland events",
                    endpoint: event_path.clone(),
                    source,
                });
            }
        }
    }
}

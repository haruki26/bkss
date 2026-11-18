use serde::Deserialize;
use std::{
    io::{self, BufRead, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    error::HyprlandError,
    state::{
        ActiveWindow, ActiveWorkspace, HyprlandClient, HyprlandState, SpecialWorkspace, Workspace,
    },
};

const MAX_RESPONSE: u64 = 1024 * 1024;
const MAX_EVENT_LINE: usize = 64 * 1024;
pub(super) const IO_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WorkspaceType {
    Normal,
    Special,
}

#[derive(Deserialize)]
struct WorkspaceResponse {
    address: String,
    #[serde(rename = "type")]
    kind: WorkspaceType,
    id: Option<i64>,
    name: String,
}

#[derive(Deserialize)]
struct ClientWorkspaceResponse {
    address: String,
}

#[derive(Deserialize)]
struct ClientResponse {
    address: String,
    #[serde(rename = "class")]
    class_name: String,
    #[serde(default)]
    workspace: Option<ClientWorkspaceResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MonitorResponse {
    focused: bool,
    #[serde(default)]
    special_workspace: Option<MonitorSpecialWorkspaceResponse>,
}

#[derive(Deserialize)]
struct MonitorSpecialWorkspaceResponse {
    #[serde(default)]
    address: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: String,
}

pub(super) fn snapshot(socket: &Path) -> Result<HyprlandState, HyprlandError> {
    #[derive(Deserialize)]
    struct WindowResponse {
        #[serde(default)]
        address: String,
        #[serde(default)]
        title: String,
        #[serde(default, rename = "class")]
        class_name: String,
    }
    let workspaces: Vec<WorkspaceResponse> = parse_json(
        "list Hyprland workspaces",
        socket,
        &command(socket, "j/workspaces")?,
    )?;
    let active_workspace: Option<WorkspaceResponse> = parse_json(
        "get active Hyprland workspace",
        socket,
        &command(socket, "j/activeworkspace")?,
    )?;
    let active_window: WindowResponse = parse_json(
        "get active Hyprland window",
        socket,
        &command(socket, "j/activewindow")?,
    )?;
    let clients: Vec<ClientResponse> = parse_json(
        "list Hyprland clients",
        socket,
        &command(socket, "j/clients")?,
    )?;
    let monitors: Vec<MonitorResponse> = parse_json(
        "list Hyprland monitors",
        socket,
        &command(socket, "j/monitors")?,
    )?;
    let mut normal_workspaces = Vec::new();
    let mut special_workspaces = Vec::new();
    for workspace in workspaces {
        match workspace.kind {
            WorkspaceType::Normal => normal_workspaces.push(Workspace {
                address: workspace.address,
                id: workspace.id,
                name: workspace.name,
            }),
            WorkspaceType::Special => special_workspaces.push(SpecialWorkspace {
                address: workspace.address,
                name: workspace.name,
            }),
        }
    }
    let active_workspace = active_workspace.map(|workspace| match workspace.kind {
        WorkspaceType::Normal => ActiveWorkspace::Normal(Workspace {
            address: workspace.address,
            id: workspace.id,
            name: workspace.name,
        }),
        WorkspaceType::Special => ActiveWorkspace::Special(SpecialWorkspace {
            address: workspace.address,
            name: workspace.name,
        }),
    });
    let focused_special_workspace = monitors
        .into_iter()
        .find(|monitor| monitor.focused)
        .and_then(|monitor| monitor.special_workspace)
        .filter(|workspace| !workspace.address.is_empty())
        .map(|workspace| {
            if workspace.kind != "special" {
                return Err(protocol(
                    "get focused Hyprland special workspace",
                    "focused monitor special workspace has a non-special type",
                    Some(socket.into()),
                ));
            }
            Ok(ActiveWorkspace::Special(SpecialWorkspace {
                address: workspace.address,
                name: workspace.name,
            }))
        })
        .transpose()?;
    Ok(HyprlandState {
        workspaces: normal_workspaces,
        special_workspaces,
        clients: clients
            .into_iter()
            .map(|client| HyprlandClient {
                address: client.address,
                class: client.class_name,
                workspace_address: client.workspace.map(|workspace| workspace.address),
            })
            .collect(),
        active_workspace: focused_special_workspace.or(active_workspace),
        active_window: (!active_window.address.is_empty()).then_some(ActiveWindow {
            address: active_window.address,
            title: active_window.title,
            class: active_window.class_name,
        }),
    })
}

pub(super) fn command(socket: &Path, request: &str) -> Result<String, HyprlandError> {
    let mut stream = UnixStream::connect(socket).map_err(|source| HyprlandError::Io {
        operation: "connect to Hyprland command socket",
        endpoint: socket.into(),
        source,
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|source| HyprlandError::Io {
            operation: "configure Hyprland command socket",
            endpoint: socket.into(),
            source,
        })?;
    stream
        .write_all(request.as_bytes())
        .map_err(|source| HyprlandError::Io {
            operation: "send Hyprland command",
            endpoint: socket.into(),
            source,
        })?;
    let mut response = String::new();
    stream
        .take(MAX_RESPONSE)
        .read_to_string(&mut response)
        .map_err(|source| HyprlandError::Io {
            operation: "read Hyprland response",
            endpoint: socket.into(),
            source,
        })?;
    if response.len() as u64 >= MAX_RESPONSE {
        return Err(protocol(
            "read Hyprland response",
            "response exceeds size limit",
            Some(socket.into()),
        ));
    }
    Ok(response)
}

pub(super) fn read_event_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
    let remaining = MAX_EVENT_LINE.saturating_sub(line.len());
    if remaining == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Hyprland event exceeds size limit",
        ));
    }
    let mut limited = reader.take(remaining as u64);
    let count = limited.read_until(b'\n', line)?;
    if count > 0 && !line.ends_with(b"\n") && line.len() == MAX_EVENT_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Hyprland event exceeds size limit",
        ));
    }
    Ok(count)
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    operation: &'static str,
    endpoint: &Path,
    response: &str,
) -> Result<T, HyprlandError> {
    serde_json::from_str(response).map_err(|source| HyprlandError::Json {
        operation,
        endpoint: endpoint.into(),
        source,
    })
}

pub(super) fn protocol(
    operation: &'static str,
    message: &str,
    endpoint: Option<PathBuf>,
) -> HyprlandError {
    HyprlandError::Protocol {
        operation,
        endpoint,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::hyprland::Hyprland;
    use std::thread::JoinHandle;
    use std::{
        error::Error,
        fs,
        os::unix::net::UnixListener,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
        thread,
        time::{Duration, Instant},
    };

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn socket_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bkss-hyprland-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).expect("test socket directory");
        dir
    }

    fn accept_bounded(listener: &UnixListener) -> std::os::unix::net::UnixStream {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(1)))
                        .expect("read timeout");
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .expect("write timeout");
                    return stream;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for socket client"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("socket accept failed: {error}"),
            }
        }
    }

    fn serve_commands(listener: UnixListener, responses: Vec<&'static str>) -> JoinHandle<()> {
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        thread::spawn(move || {
            for response in responses {
                let mut stream = accept_bounded(&listener);
                let mut request = Vec::new();
                let mut buffer = [0; 128];
                let count = stream.read(&mut buffer).expect("command read");
                request.extend_from_slice(&buffer[..count]);
                stream
                    .write_all(response.as_bytes())
                    .expect("response sent");
            }
        })
    }

    #[test]
    fn snapshot_reads_workspaces_and_active_items_from_socket() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                r#"[{"address":"0x1","type":"normal","id":1,"name":"one"}]"#,
                r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
                r#"{"address":"0x1","title":"Terminal","class":"kitty"}"#,
                r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x1","type":"normal","name":"one"}}]"#,
                "[]",
            ],
        );

        let state = snapshot(&socket).expect("snapshot");
        assert_eq!(
            state.workspaces,
            [Workspace {
                address: "0x1".into(),
                id: Some(1),
                name: "one".into()
            }]
        );
        assert_eq!(
            state.active_window,
            Some(ActiveWindow {
                address: "0x1".into(),
                title: "Terminal".into(),
                class: "kitty".into(),
            })
        );
        assert_eq!(
            state.clients,
            [HyprlandClient {
                address: "0xa".into(),
                class: "kitty".into(),
                workspace_address: Some("0x1".into()),
            }]
        );
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn snapshot_separates_special_and_named_normal_workspaces() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                r#"[{"address":"0x1","type":"normal","id":1,"name":"one"},{"address":"0x2","type":"normal","name":"named"},{"address":"0x3","type":"special","name":"special"}]"#,
                r#"{"address":"0x2","type":"normal","name":"named"}"#,
                "{}",
                r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x1","type":"normal","name":"one"}},{"address":"0xb","class":"foot","workspace":{"address":"0x3","type":"special","name":"special"}},{"address":"0xc","class":"term","workspace":{"address":"0x9","type":"normal","name":"elsewhere"}}]"#,
                "[]",
            ],
        );
        let state = snapshot(&socket).expect("snapshot");
        assert_eq!(state.workspaces.len(), 2);
        assert_eq!(state.workspaces[0].id, Some(1));
        assert_eq!(state.workspaces[1].id, None);
        assert_eq!(state.workspaces[1].name, "named");
        assert_eq!(state.special_workspaces.len(), 1);
        assert_eq!(state.special_workspaces[0].name, "special");
        assert!(matches!(
            state.active_workspace,
            Some(ActiveWorkspace::Normal(Workspace { id: None, .. }))
        ));
        assert_eq!(state.active_window, None);
        assert_eq!(state.clients.len(), 3);
        assert_eq!(
            state
                .clients
                .iter()
                .map(|client| client.workspace_address.as_deref())
                .collect::<Vec<_>>(),
            [Some("0x1"), Some("0x3"), Some("0x9")]
        );
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn active_special_workspace_is_typed_separately() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                r#"[{"address":"0x3","type":"special","name":"special"}]"#,
                r#"{"address":"0x3","type":"special","name":"special"}"#,
                "{}",
                "[]",
                "[]",
            ],
        );
        let state = snapshot(&socket).expect("snapshot");
        assert!(matches!(
            state.active_workspace,
            Some(ActiveWorkspace::Special(SpecialWorkspace { ref address, .. })) if address == "0x3"
        ));
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn focused_monitor_special_workspace_overrides_normal_active_workspace_without_window() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                r#"[{"address":"0x1","type":"normal","id":1,"name":"one"},{"address":"0x3","type":"special","name":"special"}]"#,
                r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
                "{}",
                "[]",
                r#"[{"focused":true,"specialWorkspace":{"address":"0x3","type":"special","name":"special"}}]"#,
            ],
        );
        let state = snapshot(&socket).expect("snapshot");
        assert_eq!(
            state.active_workspace,
            Some(ActiveWorkspace::Special(SpecialWorkspace {
                address: "0x3".into(),
                name: "special".into(),
            }))
        );
        assert_eq!(state.active_window, None);
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn empty_focused_special_workspace_falls_back_and_nonfocused_special_is_ignored() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                "[]",
                r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
                "{}",
                "[]",
                r#"[{"focused":false,"specialWorkspace":{"address":"0xa","type":"special","name":"other"}},{"focused":true,"specialWorkspace":{"address":"","type":"","name":""}}]"#,
            ],
        );
        let state = snapshot(&socket).expect("snapshot");
        assert_eq!(
            state.active_workspace,
            Some(ActiveWorkspace::Normal(Workspace {
                address: "0x1".into(),
                id: Some(1),
                name: "one".into(),
            }))
        );
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn populated_focused_monitor_special_workspace_rejects_non_special_type() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        let server = serve_commands(
            listener,
            vec![
                "[]",
                r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
                "{}",
                "[]",
                r#"[{"focused":true,"specialWorkspace":{"address":"0x3","type":"normal","name":"wrong"}}]"#,
            ],
        );
        assert!(
            snapshot(&socket)
                .expect_err("non-special monitor workspace rejected")
                .to_string()
                .contains("non-special type")
        );
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn invalid_workspace_type_and_missing_address_are_rejected() {
        for response in [
            r#"[{"address":"0x1","type":"unknown","name":"bad"}]"#,
            r#"[{"type":"normal","name":"missing-address"}]"#,
        ] {
            assert!(
                parse_json::<Vec<WorkspaceResponse>>(
                    "list Hyprland workspaces",
                    Path::new("/fake/command.sock"),
                    response
                )
                .is_err()
            );
        }
    }

    #[test]
    fn client_workspace_may_be_unassigned_but_required_fields_must_be_valid() {
        let clients = parse_json::<Vec<ClientResponse>>(
            "list Hyprland clients",
            Path::new("/fake/command.sock"),
            r#"[{"address":"0xa","class":"kitty"},{"address":"0xb","class":"foot","workspace":null}]"#,
        )
        .expect("unassigned clients");
        assert_eq!(clients.len(), 2);
        assert!(clients.iter().all(|client| client.workspace.is_none()));

        for response in [
            r#"[{"address":"0xa","class":42}]"#,
            r#"[{"address":"0xa","workspace":{"address":"0x1"}}]"#,
            r#"[{"address":"0xa","class":"kitty","workspace":{"address":42}}]"#,
        ] {
            assert!(
                parse_json::<Vec<ClientResponse>>(
                    "list Hyprland clients",
                    Path::new("/fake/command.sock"),
                    response
                )
                .is_err()
            );
        }
    }

    #[test]
    fn workspace_switch_runs_command_in_background_and_returns_result() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let server = thread::spawn(move || {
            let mut stream = accept_bounded(&listener);
            let mut request = Vec::new();
            let mut buffer = [0; 128];
            let count = stream.read(&mut buffer).expect("request read");
            request.extend_from_slice(&buffer[..count]);
            assert_eq!(request, b"/dispatch hl.dsp.focus({ workspace = 2 })");
            stream.write_all(b"ok: dispatched").expect("response sent");
        });

        block_on(Hyprland::switch_workspace(&dir, "0002").expect("worker spawned"))
            .expect("switch succeeded");
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn named_workspace_selector_is_not_double_prefixed() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let server = thread::spawn(move || {
            let mut stream = accept_bounded(&listener);
            let mut request = Vec::new();
            let mut buffer = [0; 128];
            let count = stream.read(&mut buffer).expect("request read");
            request.extend_from_slice(&buffer[..count]);
            assert_eq!(
                request,
                b"/dispatch hl.dsp.focus({ workspace = \"name:chat\" })"
            );
            stream.write_all(b"ok").expect("response sent");
        });
        block_on(Hyprland::switch_workspace(&dir, "name:chat").expect("worker spawned"))
            .expect("switch succeeded");
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn raw_name_and_quoted_control_characters_are_lua_escaped() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let server = thread::spawn(move || {
            let mut stream = accept_bounded(&listener);
            let mut request = Vec::new();
            let mut buffer = [0; 128];
            let count = stream.read(&mut buffer).expect("request read");
            request.extend_from_slice(&buffer[..count]);
            assert_eq!(
                request,
                br#"/dispatch hl.dsp.focus({ workspace = "name:odd\"\\\t\001" })"#
            );
            stream.write_all(b"ok").expect("response sent");
        });
        block_on(Hyprland::switch_workspace(&dir, "odd\"\\\t\u{1}").expect("worker spawned"))
            .expect("switch succeeded");
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn compositor_error_response_is_preserved() {
        let dir = socket_dir();
        let socket = dir.join(".socket.sock");
        let listener = UnixListener::bind(&socket).expect("command socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let server = thread::spawn(move || {
            let mut stream = accept_bounded(&listener);
            let mut buffer = [0; 128];
            assert!(stream.read(&mut buffer).expect("request read") > 0);
            stream
                .write_all(b"no such workspace")
                .expect("error response sent");
        });
        let error =
            block_on(Hyprland::switch_workspace(&dir, "name:missing").expect("worker spawned"))
                .expect_err("compositor rejected selector");
        assert!(error.to_string().contains("no such workspace"));
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn invalid_workspace_is_rejected_before_socket_connection() {
        let dir = socket_dir();
        let listener = UnixListener::bind(dir.join(".socket.sock")).expect("command socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");

        let error = block_on(
            Hyprland::switch_workspace(&dir, "bad;dispatch exit").expect("worker spawned"),
        )
        .expect_err("invalid workspace rejected");
        assert!(error.to_string().contains("invalid characters"));
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
        fs::remove_dir_all(dir).expect("test directory removed");
    }

    #[test]
    fn malformed_response_keeps_operation_context() {
        let error = parse_json::<serde_json::Value>(
            "list Hyprland workspaces",
            Path::new("/fake/command.sock"),
            "not json",
        )
        .expect_err("invalid JSON rejected");
        assert!(error.to_string().contains("list Hyprland workspaces"));
        assert!(
            error
                .source()
                .is_some_and(|source| { source.downcast_ref::<serde_json::Error>().is_some() })
        );
    }

    struct ThreadWake(thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWake(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            assert!(Instant::now() < deadline, "future timed out");
            thread::park_timeout(Duration::from_millis(10));
        }
    }
}

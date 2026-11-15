use serde::Deserialize;
use std::{
    io::{self, BufRead, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use super::{
    error::HyprlandError,
    state::{ActiveWindow, HyprlandState, Workspace},
};

const MAX_RESPONSE: u64 = 1024 * 1024;
const MAX_EVENT_LINE: usize = 64 * 1024;
pub(super) const IO_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) fn snapshot(socket: &Path) -> Result<HyprlandState, HyprlandError> {
    #[derive(Deserialize)]
    struct WorkspaceResponse {
        id: i64,
        name: String,
    }
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
    let active_workspace: WorkspaceResponse = parse_json(
        "get active Hyprland workspace",
        socket,
        &command(socket, "j/activeworkspace")?,
    )?;
    let active_window: WindowResponse = parse_json(
        "get active Hyprland window",
        socket,
        &command(socket, "j/activewindow")?,
    )?;
    Ok(HyprlandState {
        workspaces: workspaces
            .into_iter()
            .map(|workspace| Workspace {
                id: workspace.id,
                name: workspace.name,
            })
            .collect(),
        active_workspace: (active_workspace.id != 0).then_some(Workspace {
            id: active_workspace.id,
            name: active_workspace.name,
        }),
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
                r#"[{"id":1,"name":"one"}]"#,
                r#"{"id":1,"name":"one"}"#,
                r#"{"address":"0x1","title":"Terminal","class":"kitty"}"#,
            ],
        );

        let state = snapshot(&socket).expect("snapshot");
        assert_eq!(
            state.workspaces,
            [Workspace {
                id: 1,
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
        server.join().expect("server joined");
        fs::remove_dir_all(dir).expect("test directory removed");
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
            assert_eq!(request, b"dispatch workspace 2");
            stream.write_all(b"ok").expect("response sent");
        });

        block_on(Hyprland::switch_workspace(&dir, "2").expect("worker spawned"))
            .expect("switch succeeded");
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

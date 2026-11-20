use bkss_core::{ResourceScope, SourceState};
use bkss_integration::{
    ActiveWorkspace, Clock, Hyprland, HyprlandError, HyprlandState, Source, SpecialWorkspace,
    Workspace,
};
use gpui::TestAppContext;
use std::{
    error::Error,
    fs,
    io::{self, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

#[test]
fn clock_and_hyprland_guards_remain_distinct_types() {
    assert_ne!(
        std::any::TypeId::of::<<Clock as Source>::Guard>(),
        std::any::TypeId::of::<<Hyprland as Source>::Guard>()
    );
}

fn socket_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bkss-hyprland-lifecycle-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("test directory");
    dir
}

fn accept_before(listener: &UnixListener, deadline: Instant) -> UnixStream {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .expect("set read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .expect("set write timeout");
                return stream;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "socket accept timed out");
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("socket accept failed: {error}"),
        }
    }
}

fn command_server(
    listener: UnixListener,
    responses: Vec<&'static str>,
    initial_done: Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for (index, response) in responses.into_iter().enumerate() {
            let mut stream = accept_before(&listener, Instant::now() + Duration::from_secs(4));
            let mut request = [0; 64];
            assert!(stream.read(&mut request).expect("read request") > 0);
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            if index == 4 {
                initial_done.send(()).expect("signal initial snapshot");
            }
        }
    })
}

fn event_server(
    listener: UnixListener,
    connected: Sender<()>,
) -> (
    Sender<Vec<u8>>,
    Receiver<io::Result<()>>,
    thread::JoinHandle<()>,
) {
    let (chunks_tx, chunks_rx) = mpsc::channel::<Vec<u8>>();
    let (write_tx, write_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut stream = accept_before(&listener, Instant::now() + Duration::from_secs(4));
        connected.send(()).expect("signal event connection");
        while let Ok(chunk) = chunks_rx.recv_timeout(Duration::from_secs(4)) {
            let result = stream.write_all(&chunk);
            let closed = result.is_err();
            write_tx.send(result).expect("report event write");
            if closed {
                break;
            }
        }
    });
    (chunks_tx, write_rx, worker)
}

fn wait_ready(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<HyprlandState>>,
    expected: &HyprlandState,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        cx.run_until_parked();
        if state.get(cx) == SourceState::Ready(expected.clone()) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(state.get(cx), SourceState::Ready(expected.clone()));
}

#[test]
fn activation_is_lazy_shared_event_driven_and_stops_on_last_release() {
    let cx = TestAppContext::single();
    cx.dispatcher.allow_parking();
    let dir = socket_dir();
    let commands = UnixListener::bind(dir.join(".socket.sock")).expect("command socket");
    let events = UnixListener::bind(dir.join(".socket2.sock")).expect("event socket");
    commands
        .set_nonblocking(true)
        .expect("nonblocking command listener");
    events
        .set_nonblocking(true)
        .expect("nonblocking event listener");
    let scope = cx.update(ResourceScope::new);
    let descriptor = Hyprland::new(dir.clone());
    let alias = descriptor.clone();
    assert!(matches!(
        commands.accept(),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));
    assert!(matches!(
        events.accept(),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock
    ));

    let (initial_tx, initial_rx) = mpsc::channel();
    let command_worker = command_server(
        commands,
        vec![
            r#"[{"address":"0x1","type":"normal","id":1,"name":"one"}]"#,
            r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
            "{}",
            r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x1","type":"normal","name":"one"}},{"address":"0xb","class":"firefox","workspace":{"address":"0x2","type":"normal","name":"two"}}]"#,
            "[]",
            r#"[{"address":"0x2","type":"normal","id":2,"name":"two"}]"#,
            r#"{"address":"0x2","type":"normal","id":2,"name":"two"}"#,
            "{}",
            r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x2","type":"normal","name":"two"}}]"#,
            "[]",
            r#"[{"address":"0x1","type":"normal","id":1,"name":"one"},{"address":"0x3","type":"special","name":"special"}]"#,
            r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
            "{}",
            r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x1","type":"normal","name":"one"}}]"#,
            r#"[{"focused":true,"specialWorkspace":{"address":"0x3","type":"special","name":"special"}}]"#,
            r#"[{"address":"0x1","type":"normal","id":1,"name":"one"}]"#,
            r#"{"address":"0x1","type":"normal","id":1,"name":"one"}"#,
            "{}",
            r#"[{"address":"0xa","class":"kitty","workspace":{"address":"0x1","type":"normal","name":"one"}}]"#,
            "[]",
        ],
        initial_tx,
    );
    let (event_connected_tx, event_connected_rx) = mpsc::channel();
    let (event_tx, event_write_rx, event_worker) = event_server(events, event_connected_tx);

    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("first acquire"));
    let shared = cx.update(|cx| scope.acquire(&alias, cx).expect("shared acquire"));
    let reactive = lease.reactive();
    assert_eq!(reactive.get(&cx), SourceState::Loading);
    event_connected_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worker connects event socket");
    initial_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("initial snapshot");

    let first = HyprlandState {
        workspaces: vec![Workspace {
            address: "0x1".into(),
            id: Some(1),
            name: "one".into(),
        }],
        special_workspaces: vec![],
        clients: vec![
            bkss_integration::HyprlandClient {
                address: "0xa".into(),
                class: "kitty".into(),
                workspace_address: Some("0x1".into()),
            },
            bkss_integration::HyprlandClient {
                address: "0xb".into(),
                class: "firefox".into(),
                workspace_address: Some("0x2".into()),
            },
        ],
        active_workspace: Some(ActiveWorkspace::Normal(Workspace {
            address: "0x1".into(),
            id: Some(1),
            name: "one".into(),
        })),
        active_window: None,
    };
    wait_ready(&cx, &reactive, &first);

    event_tx
        .send(b"workspace>>".to_vec())
        .expect("partial event");
    event_write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("partial write")
        .expect("partial write succeeds");
    thread::sleep(Duration::from_millis(300));
    event_tx.send(b"2\n".to_vec()).expect("event suffix");
    event_write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("suffix write")
        .expect("suffix write succeeds");

    let updated = HyprlandState {
        workspaces: vec![Workspace {
            address: "0x2".into(),
            id: Some(2),
            name: "two".into(),
        }],
        special_workspaces: vec![],
        clients: vec![bkss_integration::HyprlandClient {
            address: "0xa".into(),
            class: "kitty".into(),
            workspace_address: Some("0x2".into()),
        }],
        active_workspace: Some(ActiveWorkspace::Normal(Workspace {
            address: "0x2".into(),
            id: Some(2),
            name: "two".into(),
        })),
        active_window: None,
    };
    wait_ready(&cx, &reactive, &updated);
    event_tx
        .send(b"workspace>>3\n".to_vec())
        .expect("special workspace event");
    event_write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("special event write")
        .expect("special event write succeeds");
    let special = HyprlandState {
        workspaces: vec![Workspace {
            address: "0x1".into(),
            id: Some(1),
            name: "one".into(),
        }],
        special_workspaces: vec![SpecialWorkspace {
            address: "0x3".into(),
            name: "special".into(),
        }],
        clients: vec![bkss_integration::HyprlandClient {
            address: "0xa".into(),
            class: "kitty".into(),
            workspace_address: Some("0x1".into()),
        }],
        active_workspace: Some(ActiveWorkspace::Special(
            bkss_integration::SpecialWorkspace {
                address: "0x3".into(),
                name: "special".into(),
            },
        )),
        active_window: None,
    };
    wait_ready(&cx, &reactive, &special);
    event_tx
        .send(b"workspace>>1\n".to_vec())
        .expect("normal workspace event");
    event_write_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("normal event write")
        .expect("normal event write succeeds");
    let returned_to_normal = HyprlandState {
        workspaces: vec![Workspace {
            address: "0x1".into(),
            id: Some(1),
            name: "one".into(),
        }],
        special_workspaces: vec![],
        clients: vec![bkss_integration::HyprlandClient {
            address: "0xa".into(),
            class: "kitty".into(),
            workspace_address: Some("0x1".into()),
        }],
        active_workspace: Some(ActiveWorkspace::Normal(Workspace {
            address: "0x1".into(),
            id: Some(1),
            name: "one".into(),
        })),
        active_window: None,
    };
    wait_ready(&cx, &reactive, &returned_to_normal);
    command_worker.join().expect("command server completed");

    drop(lease);
    assert_eq!(
        reactive.get(&cx),
        SourceState::Ready(returned_to_normal.clone())
    );
    drop(shared);
    thread::sleep(Duration::from_millis(350));
    event_tx
        .send(b"workspace>>1\n".to_vec())
        .expect("late event");
    assert!(
        event_write_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("closed event socket reported")
            .is_err()
    );
    assert!(
        scope.failure().is_none(),
        "cancellation is not a source failure"
    );
    assert_eq!(reactive.get(&cx), SourceState::Ready(returned_to_normal));
    event_worker.join().expect("event server completed");
    fs::remove_dir_all(dir).expect("remove test directory");
}

#[test]
fn worker_connection_and_protocol_errors_reach_scope_with_original_cause() {
    let cx = TestAppContext::single();
    cx.dispatcher.allow_parking();
    let dir = socket_dir();
    let commands = UnixListener::bind(dir.join(".socket.sock")).expect("command socket");
    let events = UnixListener::bind(dir.join(".socket2.sock")).expect("event socket");
    commands
        .set_nonblocking(true)
        .expect("nonblocking command listener");
    events
        .set_nonblocking(true)
        .expect("nonblocking event listener");
    let (initial_tx, _initial_rx) = mpsc::channel();
    let command_worker = command_server(commands, vec!["not json"], initial_tx);
    let (event_connected_tx, event_connected_rx) = mpsc::channel();
    let (event_tx, _event_write_rx, event_worker) = event_server(events, event_connected_tx);
    let scope = cx.update(ResourceScope::new);
    let descriptor = Hyprland::new(dir.clone());
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("acquire"));
    event_connected_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("event connection made in background");

    let deadline = Instant::now() + Duration::from_secs(3);
    let failure = loop {
        cx.run_until_parked();
        if let Some(failure) = scope.failure() {
            break failure;
        }
        assert!(Instant::now() < deadline, "source failure timed out");
        thread::sleep(Duration::from_millis(5));
    };
    let cause = failure.source().expect("failure cause retained");
    assert!(cause.downcast_ref::<HyprlandError>().is_some());
    assert!(cause.to_string().contains(".socket.sock"));
    assert!(
        cause
            .source()
            .is_some_and(|source| source.downcast_ref::<serde_json::Error>().is_some())
    );
    drop(lease);
    command_worker.join().expect("command server completed");
    drop(event_tx);
    event_worker.join().expect("event server completed");
    fs::remove_dir_all(dir).expect("remove test directory");
}

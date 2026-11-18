use bkss_core::{Reactive, ResourceScope, SourceState};
use bkss_integration::{CommandIpc, CommandIpcClient};
use gpui::TestAppContext;
use std::{
    error::Error,
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

fn test_context() -> TestAppContext {
    let cx = TestAppContext::single();
    cx.dispatcher.allow_parking();
    cx
}

fn private_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bkss-command-ipc-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("test runtime directory");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .expect("private test runtime directory");
    dir
}

fn wait_ready(
    cx: &TestAppContext,
    scope: &ResourceScope,
    reactive: &Reactive<SourceState<bkss_integration::CommandIpcState>>,
) -> bkss_integration::CommandIpcState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if let SourceState::Ready(state) = reactive.get(cx) {
            return state;
        }
        if let Some(failure) = scope.failure() {
            panic!("command source failed before becoming ready: {failure}");
        }
        assert!(
            Instant::now() < deadline,
            "command source did not become ready"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_failure(cx: &TestAppContext, scope: &ResourceScope) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while scope.failure().is_none() && Instant::now() < deadline {
        cx.run_until_parked();
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        scope.failure().is_some(),
        "command source failure was not published"
    );
}

fn wait_server_stopped(socket: &Path) {
    let lock_path = PathBuf::from(format!("{}.lock", socket.display()));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !socket.exists()
            && let Ok(file) = fs::OpenOptions::new().read(true).open(&lock_path)
            && file.try_lock().is_ok()
        {
            drop(file);
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !socket.exists(),
        "source cleanup removes its endpoint before releasing the lock"
    );
}

#[test]
fn activation_publishes_requests_and_explicit_response_then_cleans_its_socket() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor =
        CommandIpc::new(&dir, "commands.sock", ["open", "shutdown"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let reactive = lease.reactive();
    let state = wait_ready(&cx, &scope, &reactive);
    assert_eq!(
        fs::metadata(&socket)
            .expect("socket exists")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let client_socket = socket.clone();
    let client = thread::spawn(move || CommandIpcClient::send(client_socket, "open", b"from-test"));
    let request = futures_lite::future::block_on(state.next_request()).expect("request delivered");
    assert_eq!(request.command(), "open");
    assert_eq!(request.payload(), b"from-test");
    request.respond(b"accepted".to_vec()).expect("respond");
    assert_eq!(
        client
            .join()
            .expect("client thread")
            .expect("client response"),
        b"accepted"
    );

    drop((state, lease));
    wait_server_stopped(&socket);
    assert!(!socket.exists(), "source cleanup removes its own socket");
    assert!(CommandIpcClient::send(&socket, "open", b"").is_err());
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn activation_recovers_a_crash_residue_without_manual_socket_removal() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let abandoned = UnixListener::bind(&socket).expect("create abandoned endpoint");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make abandoned endpoint private");
    drop(abandoned);
    assert!(socket.exists());

    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("restart source"));
    let state = wait_ready(&cx, &scope, &lease.reactive());
    let client_socket = socket.clone();
    let client = thread::spawn(move || CommandIpcClient::send(client_socket, "open", b"restart"));
    let request = futures_lite::future::block_on(state.next_request()).expect("request delivered");
    assert_eq!(request.payload(), b"restart");
    request.respond(b"ok".to_vec()).expect("respond");
    assert_eq!(
        client.join().expect("client thread").expect("response"),
        b"ok"
    );

    drop((state, lease));
    wait_server_stopped(&socket);
    assert!(dir.join("commands.sock.lock").exists());
    fs::remove_dir_all(dir).expect("remove private test runtime directory");
}

#[test]
fn unknown_and_malformed_requests_are_rejected_without_dispatch() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let _state = wait_ready(&cx, &scope, &lease.reactive());

    let unknown = CommandIpcClient::send(&socket, "not-registered", b"");
    assert!(unknown.is_err());

    let mut malformed = UnixStream::connect(&socket).expect("connect");
    malformed
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    malformed
        .write_all(&[0, 0, 0, 0, 0, 0])
        .expect("write malformed request");
    let mut response = [0_u8; 5];
    malformed.read_exact(&mut response).expect("read rejection");
    assert_eq!(response[0], 0);
    let response_len = u32::from_be_bytes(response[1..].try_into().expect("response length"));
    assert_eq!(response_len as usize, b"invalid request size".len());
    assert!(CommandIpcClient::send(&socket, "open", &vec![0; 65 * 1024]).is_err());

    let mut oversize = UnixStream::connect(&socket).expect("connect for oversized header");
    oversize
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout");
    oversize
        .write_all(&[0, 1, 0, 1, 0, 1, b'x'])
        .expect("write oversized payload declaration");
    oversize
        .read_exact(&mut response)
        .expect("read size rejection");
    assert_eq!(response[0], 0);

    drop(lease);
    wait_server_stopped(&socket);
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn burst_requests_are_delivered_once_instead_of_collapsing_to_a_state_update() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["activate"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let reactive = lease.reactive();
    let state = wait_ready(&cx, &scope, &reactive);

    let clients: Vec<_> = (0_u32..8)
        .map(|index| {
            let socket = socket.clone();
            thread::spawn(move || {
                let payload = index.to_be_bytes();
                CommandIpcClient::send(socket, "activate", &payload)
            })
        })
        .collect();
    let mut delivered = Vec::new();
    for _ in 0..clients.len() {
        let request = futures_lite::future::block_on(state.next_request())
            .expect("every accepted request is delivered");
        delivered.push(u32::from_be_bytes(
            request.payload().try_into().expect("four-byte payload"),
        ));
        let response = request.payload().to_vec();
        request.respond(response).expect("respond to request");
    }
    delivered.sort_unstable();
    assert_eq!(delivered, (0_u32..8).collect::<Vec<_>>());
    for client in clients {
        assert!(client.join().expect("client thread").is_ok());
    }

    drop((state, lease));
    wait_server_stopped(&socket);
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn startup_failures_are_published_from_the_worker_and_existing_paths_are_preserved() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let listener = UnixListener::bind(&socket).expect("first listener");
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .expect("make live endpoint private");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("async activation"));
    wait_failure(&cx, &scope);
    let failure = scope.failure().expect("duplicate endpoint failure");
    assert!(
        failure
            .source()
            .and_then(|error| error.downcast_ref::<std::io::Error>())
            .is_some_and(|error| error.kind() == std::io::ErrorKind::AddrInUse)
    );
    assert!(
        std::fs::symlink_metadata(&socket)
            .expect("existing listener remains")
            .file_type()
            .is_socket()
    );
    drop(lease);
    drop(listener);

    let file = dir.join("regular");
    fs::write(&file, b"keep").expect("create regular file");
    let descriptor = CommandIpc::new(&dir, "regular", ["open"]).expect("file descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("file activation"));
    wait_failure(&cx, &scope);
    assert_eq!(fs::read(&file).expect("read file"), b"keep");
    drop(lease);

    let insecure_dir = std::env::temp_dir().join(format!(
        "bkss-command-ipc-insecure-{}-{}",
        std::process::id(),
        NEXT_PATH.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&insecure_dir).expect("create insecure runtime directory");
    fs::set_permissions(&insecure_dir, fs::Permissions::from_mode(0o755))
        .expect("set non-private permissions");
    let descriptor = CommandIpc::new(&insecure_dir, "commands.sock", ["open"])
        .expect("declaration remains inert");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("insecure activation"));
    wait_failure(&cx, &scope);
    let failure = scope.failure().expect("private-directory failure");
    assert!(
        failure
            .source()
            .and_then(|error| error.downcast_ref::<std::io::Error>())
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    );
    drop(lease);
    fs::remove_dir_all(insecure_dir).expect("remove insecure directory");
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn cancellation_interrupts_a_partial_request_and_removes_the_endpoint() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let state = wait_ready(&cx, &scope, &lease.reactive());
    let mut stalled = UnixStream::connect(&socket).expect("connect stalled client");
    stalled.write_all(&[0]).expect("send partial header");

    drop((state, lease));
    wait_server_stopped(&socket);
    assert!(!socket.exists(), "worker cancellation releases the socket");
    drop(stalled);
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn disconnected_client_does_not_stop_later_commands() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let reactive = lease.reactive();
    let state = wait_ready(&cx, &scope, &reactive);
    let mut disconnected = UnixStream::connect(&socket).expect("connect");
    disconnected
        .write_all(&[0, 4, 0, 0, 0, 0, b'o', b'p', b'e', b'n'])
        .expect("send request");
    drop(disconnected);
    let request = futures_lite::future::block_on(state.next_request()).expect("request delivered");
    request
        .respond(Vec::new())
        .expect("complete disconnected request");

    let client_socket = socket.clone();
    let client = thread::spawn(move || CommandIpcClient::send(client_socket, "open", b"next"));
    let request =
        futures_lite::future::block_on(state.next_request()).expect("next request delivered");
    request.respond(b"ok".to_vec()).expect("respond");
    assert_eq!(
        client.join().expect("client thread").expect("response"),
        b"ok"
    );

    drop((state, lease));
    wait_server_stopped(&socket);
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

#[test]
fn unacknowledged_request_times_out_and_cannot_be_replied_to_later() {
    let cx = test_context();
    let dir = private_dir();
    let socket = dir.join("commands.sock");
    let descriptor = CommandIpc::new(&dir, "commands.sock", ["open"]).expect("descriptor");
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let reactive = lease.reactive();
    let state = wait_ready(&cx, &scope, &reactive);
    let client_socket = socket.clone();
    let client = thread::spawn(move || CommandIpcClient::send(client_socket, "open", b""));
    let request = futures_lite::future::block_on(state.next_request()).expect("request delivered");
    assert!(client.join().expect("client thread").is_err());
    assert!(request.respond(Vec::new()).is_err());

    drop((state, lease));
    wait_server_stopped(&socket);
    fs::remove_dir_all(dir).expect("remove test runtime directory");
}

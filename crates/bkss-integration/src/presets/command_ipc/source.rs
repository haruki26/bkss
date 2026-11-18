use crate::{Source, SourceDescriptor, SourcePublisher, base::worker::spawn_worker};
use gpui::App;
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{
        unix::fs::FileTypeExt,
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const MAX_COMMAND_BYTES: usize = 128;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const REQUEST_QUEUE: usize = 16;
const IO_POLL: Duration = Duration::from_millis(100);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
const SOCKET_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;

/// Inert declaration of a private Unix-socket command source.
pub struct CommandIpc {
    socket: PathBuf,
    commands: HashSet<String>,
}

impl CommandIpc {
    /// Declares a server at `runtime_dir/socket_name` for a finite set of command names.
    ///
    /// The directory and socket are checked and created only when this descriptor is mounted.
    pub fn new(
        runtime_dir: impl AsRef<Path>,
        socket_name: impl AsRef<Path>,
        commands: impl IntoIterator<Item = impl Into<String>>,
    ) -> io::Result<SourceDescriptor<Self>> {
        let runtime_dir = runtime_dir.as_ref();
        let socket_name = socket_name.as_ref();
        if !runtime_dir.is_absolute()
            || socket_name.components().count() != 1
            || socket_name.file_name() != Some(socket_name.as_os_str())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command IPC requires an absolute runtime directory and a filename",
            ));
        }
        let commands: HashSet<String> = commands.into_iter().map(Into::into).collect();
        if commands.is_empty()
            || commands.iter().any(|name| {
                name.is_empty()
                    || name.len() > MAX_COMMAND_BYTES
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "command names must be nonempty ASCII identifiers of at most 128 bytes",
            ));
        }
        Ok(SourceDescriptor::new(Self {
            socket: runtime_dir.join(socket_name),
            commands,
        }))
    }
}

impl Source for CommandIpc {
    type Key = (PathBuf, Vec<String>);
    type Value = CommandIpcState;
    type Error = io::Error;
    type Guard = crate::WorkerGuard;

    fn key(&self) -> Self::Key {
        let mut commands: Vec<_> = self.commands.iter().cloned().collect();
        commands.sort();
        (self.socket.clone(), commands)
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let (events, receiver) = async_channel::bounded(REQUEST_QUEUE);
        let socket = self.socket.clone();
        let commands = self.commands.clone();
        let guard = spawn_worker(
            "bkss-command-ipc",
            bridge,
            sender,
            move |publisher, cancellation| {
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                let (listener, cleanup, lock) = setup_server(&socket, &cancellation)?;
                let _lock = lock;
                let _cleanup = cleanup;
                if publisher
                    .publish(CommandIpcState { requests: receiver })
                    .is_err()
                {
                    return Ok(());
                }
                serve(listener, commands, events, &cancellation)
            },
        )?;
        Ok(guard)
    }
}

fn setup_server(
    socket: &Path,
    cancellation: &crate::base::worker::Cancellation,
) -> io::Result<(UnixListener, SocketCleanup, EndpointLock)> {
    let dir = socket.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path has no parent directory",
        )
    })?;
    check_private_directory(dir)?;
    let lock = acquire_endpoint_lock(&endpoint_lock_path(socket))?;
    if cancellation.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "source stopped during endpoint setup",
        ));
    }
    recover_stale_socket(socket, cancellation)?;
    if cancellation.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "source stopped during endpoint setup",
        ));
    }
    let listener = UnixListener::bind(socket)?;
    let cleanup = SocketCleanup::new(socket)?;
    if let Err(error) = listener.set_nonblocking(true) {
        drop(cleanup);
        return Err(error);
    }
    let permissions = std::fs::Permissions::from_mode(SOCKET_MODE);
    if let Err(error) = std::fs::set_permissions(socket, permissions) {
        drop(cleanup);
        return Err(error);
    }
    check_owned_private_socket(socket)?;
    Ok((listener, cleanup, lock))
}

fn endpoint_lock_path(socket: &Path) -> PathBuf {
    let mut path = socket.as_os_str().to_os_string();
    path.push(".lock");
    PathBuf::from(path)
}

#[derive(Debug)]
struct EndpointLock(File);

impl Drop for EndpointLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn acquire_endpoint_lock(path: &Path) -> io::Result<EndpointLock> {
    loop {
        let file = match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                validate_lock_metadata(&metadata)?;
                OpenOptions::new().read(true).open(path)?
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(path)
                {
                    Ok(file) => {
                        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
                        file
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };

        let opened = file.metadata()?;
        validate_lock_metadata(&opened)?;
        let path_metadata = std::fs::symlink_metadata(path)?;
        validate_lock_metadata(&path_metadata)?;
        if !same_file(&opened, &path_metadata) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC lock path changed while opening",
            ));
        }

        file.try_lock()?;
        let lock = EndpointLock(file);
        let locked_path = std::fs::symlink_metadata(path)?;
        validate_lock_metadata(&locked_path)?;
        if !same_file(&opened, &locked_path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC lock path changed while locking",
            ));
        }
        return Ok(lock);
    }
}

fn validate_lock_metadata(metadata: &std::fs::Metadata) -> io::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != current_uid()?
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC lock must be an empty, private, user-owned regular file",
        ));
    }
    Ok(())
}

fn recover_stale_socket(
    socket: &Path,
    cancellation: &crate::base::worker::Cancellation,
) -> io::Result<()> {
    let initial = match std::fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    validate_socket_metadata(&initial)?;

    match futures_lite::future::block_on(probe_socket(socket, cancellation)) {
        SocketProbe::Connected => {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "IPC endpoint has a live listener",
            ));
        }
        SocketProbe::Refused => {}
        SocketProbe::Missing => return Ok(()),
        SocketProbe::TimedOut => {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "IPC endpoint probe timed out; existing socket was preserved",
            ));
        }
        SocketProbe::Cancelled => {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "source stopped during IPC endpoint probe",
            ));
        }
        SocketProbe::Error(error) => return Err(error),
    }

    remove_inspected_socket(socket, &initial).map(|_| ())
}

enum SocketProbe {
    Connected,
    Refused,
    Missing,
    TimedOut,
    Cancelled,
    Error(io::Error),
}

async fn probe_connection<F>(
    connection: F,
    cancellation: &crate::base::worker::Cancellation,
) -> SocketProbe
where
    F: std::future::Future<Output = io::Result<async_io::Async<UnixStream>>>,
{
    let connect = async {
        match connection.await {
            Ok(_stream) => SocketProbe::Connected,
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => SocketProbe::Refused,
            Err(error) if error.kind() == io::ErrorKind::NotFound => SocketProbe::Missing,
            Err(error) => SocketProbe::Error(error),
        }
    };
    let timeout = async {
        async_io::Timer::after(SOCKET_PROBE_TIMEOUT).await;
        SocketProbe::TimedOut
    };
    let cancelled = async {
        cancellation.cancelled().await;
        SocketProbe::Cancelled
    };
    futures_lite::future::race(futures_lite::future::race(connect, timeout), cancelled).await
}

async fn probe_socket(
    socket: &Path,
    cancellation: &crate::base::worker::Cancellation,
) -> SocketProbe {
    probe_connection(async_io::Async::<UnixStream>::connect(socket), cancellation).await
}

fn same_file(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn remove_inspected_socket(socket: &Path, inspected: &std::fs::Metadata) -> io::Result<bool> {
    let current = match std::fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    validate_socket_metadata(&current)?;
    if !same_file(inspected, &current) {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "IPC endpoint changed during stale-socket probe; replacement was preserved",
        ));
    }
    match std::fs::remove_file(socket) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Requests delivered by an active command source.
#[derive(Clone)]
pub struct CommandIpcState {
    requests: async_channel::Receiver<CommandIpcRequest>,
}

impl CommandIpcState {
    /// Waits for the next request. Dropping the source activation closes the stream.
    pub async fn next_request(&self) -> Option<CommandIpcRequest> {
        self.requests.recv().await.ok()
    }
}

/// One registered external request and its single response path.
pub struct CommandIpcRequest {
    command: String,
    payload: Vec<u8>,
    reply: mpsc::SyncSender<Vec<u8>>,
}

impl CommandIpcRequest {
    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Completes the client request. The response must fit in the 64 KiB protocol limit.
    pub fn respond(self, response: Vec<u8>) -> io::Result<()> {
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "IPC response is too large",
            ));
        }
        self.reply.try_send(response).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "IPC client disconnected or timed out",
            )
        })
    }
}

/// Synchronous client for the active command source; call it from a worker, not the UI thread.
pub struct CommandIpcClient;

impl CommandIpcClient {
    /// Sends one registered command and waits for its explicit response.
    pub fn send(socket: impl AsRef<Path>, command: &str, payload: &[u8]) -> io::Result<Vec<u8>> {
        if command.is_empty()
            || command.len() > MAX_COMMAND_BYTES
            || !command
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || payload.len() > MAX_REQUEST_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid IPC request",
            ));
        }
        let socket = socket.as_ref();
        check_owned_private_socket(socket)?;
        let mut stream = UnixStream::connect(socket)?;
        set_timeouts(&stream)?;
        let command_len = u16::try_from(command.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "command name is too long"))?;
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "payload is too large"))?;
        stream.write_all(&command_len.to_be_bytes())?;
        stream.write_all(&payload_len.to_be_bytes())?;
        stream.write_all(command.as_bytes())?;
        stream.write_all(payload)?;

        let mut status = [0_u8; 1];
        stream.read_exact(&mut status)?;
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length > MAX_RESPONSE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC response is too large",
            ));
        }
        let mut response = vec![0; length];
        stream.read_exact(&mut response)?;
        if status[0] == 1 {
            Ok(response)
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&response).into_owned(),
            ))
        }
    }
}

fn serve(
    listener: UnixListener,
    commands: HashSet<String>,
    events: async_channel::Sender<CommandIpcRequest>,
    cancellation: &crate::base::worker::Cancellation,
) -> io::Result<()> {
    while !cancellation.is_cancelled() {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = handle_client(stream, &commands, &events, cancellation);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(IO_POLL);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn handle_client(
    mut stream: UnixStream,
    commands: &HashSet<String>,
    events: &async_channel::Sender<CommandIpcRequest>,
    cancellation: &crate::base::worker::Cancellation,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_POLL))?;
    stream.set_write_timeout(Some(IO_POLL))?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut header = [0_u8; 6];
    read_request_bytes(&mut stream, &mut header, cancellation, deadline)?;
    let name_len = u16::from_be_bytes([header[0], header[1]]) as usize;
    let payload_len = u32::from_be_bytes([header[2], header[3], header[4], header[5]]) as usize;
    if name_len == 0 || name_len > MAX_COMMAND_BYTES || payload_len > MAX_REQUEST_BYTES {
        return write_response(&mut stream, false, b"invalid request size");
    }
    let mut name = vec![0; name_len];
    let mut payload = vec![0; payload_len];
    read_request_bytes(&mut stream, &mut name, cancellation, deadline)?;
    read_request_bytes(&mut stream, &mut payload, cancellation, deadline)?;
    let command = match String::from_utf8(name) {
        Ok(command) if commands.contains(&command) => command,
        _ => return write_response(&mut stream, false, b"unknown command"),
    };
    let (reply, receiver) = mpsc::sync_channel(1);
    let request = CommandIpcRequest {
        command,
        payload,
        reply,
    };
    match events.try_send(request) {
        Ok(()) => {}
        Err(async_channel::TrySendError::Full(_)) => {
            return write_response(&mut stream, false, b"busy");
        }
        Err(async_channel::TrySendError::Closed(_)) => {
            return write_response(&mut stream, false, b"inactive");
        }
    }
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return write_response(&mut stream, false, b"request timed out");
        }
        match receiver.recv_timeout(remaining.min(IO_POLL)) {
            Ok(response) => return write_response(&mut stream, true, &response),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return write_response(&mut stream, false, b"request expired");
            }
        }
    }
}

fn read_request_bytes(
    stream: &mut UnixStream,
    bytes: &mut [u8],
    cancellation: &crate::base::worker::Cancellation,
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if cancellation.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "source stopped"));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "IPC request timed out",
            ));
        }
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(read) => offset += read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn write_response(stream: &mut UnixStream, success: bool, response: &[u8]) -> io::Result<()> {
    stream.write_all(&[u8::from(success)])?;
    stream.write_all(&(response.len() as u32).to_be_bytes())?;
    stream.write_all(response)
}

fn set_timeouts(stream: &UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(CLIENT_TIMEOUT))?;
    stream.set_write_timeout(Some(CLIENT_TIMEOUT))
}

fn check_private_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime directory must be absolute",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != current_uid()?
        || metadata.mode() & 0o7777 != DIRECTORY_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory must be owned by the current user with mode 0700",
        ));
    }
    Ok(())
}

fn check_owned_private_socket(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    validate_socket_metadata(&metadata)
}

fn validate_socket_metadata(metadata: &std::fs::Metadata) -> io::Result<()> {
    if !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()?
        || metadata.mode() & 0o7777 != SOCKET_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "IPC endpoint must be a private Unix socket owned by the current user",
        ));
    }
    Ok(())
}

fn current_uid() -> io::Result<u32> {
    Ok(std::fs::metadata("/proc/self")?.uid())
}

#[derive(Clone, Debug)]
struct SocketCleanup {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketCleanup {
    fn new(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && current_uid().is_ok_and(|uid| metadata.uid() == uid)
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, sync::atomic::AtomicU64};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn private_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bkss-command-ipc-{label}-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn cancellation() -> (async_channel::Sender<()>, crate::base::worker::Cancellation) {
        crate::base::worker::cancellation_channel()
    }

    fn make_socket_private(path: &Path) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE)).unwrap();
    }

    #[test]
    fn names_reject_unsafe_forms_and_allow_only_registered_commands() {
        assert!(CommandIpc::new("/tmp", "../bad", ["ok"]).is_err());
        assert!(CommandIpc::new("/tmp", "ipc.sock", [""]).is_err());
        assert!(CommandIpc::new("/tmp", "ipc.sock", ["open-menu"]).is_ok());
        let commands = HashSet::from(["open-menu".to_owned()]);
        assert!(commands.contains("open-menu"));
        assert!(!commands.contains("execute"));
    }

    #[test]
    fn stale_owned_socket_is_recovered_and_lock_file_survives_restart() {
        const ABANDONED_SOCKET_ENV: &str = "BKSS_COMMAND_IPC_ABANDONED_SOCKET";
        if let Some(path) = std::env::var_os(ABANDONED_SOCKET_ENV) {
            let listener = UnixListener::bind(&path).unwrap();
            make_socket_private(Path::new(&path));
            drop(listener);
            return;
        }

        let dir = private_dir("stale-recovery");
        let socket = dir.join("endpoint.sock");
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "presets::command_ipc::source::tests::stale_owned_socket_is_recovered_and_lock_file_survives_restart",
            ])
            .env(ABANDONED_SOCKET_ENV, &socket)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "abandoned-socket helper process succeeded"
        );
        assert!(socket.exists());
        let stale = std::fs::symlink_metadata(&socket).unwrap();
        let (cancel_sender, cancel) = cancellation();
        let (listener, cleanup, lock) = setup_server(&socket, &cancel).unwrap();
        assert!(stale.file_type().is_socket());
        assert!(
            std::fs::symlink_metadata(&socket)
                .unwrap()
                .file_type()
                .is_socket()
        );
        let client = UnixStream::connect(&socket).unwrap();
        drop(client);
        drop(listener);
        drop(cleanup);
        drop(lock);
        assert!(!socket.exists());
        let lock_path = endpoint_lock_path(&socket);
        assert!(lock_path.exists());
        assert_eq!(
            std::fs::symlink_metadata(&lock_path).unwrap().mode() & 0o777,
            0o600
        );

        let (listener, cleanup, lock) = setup_server(&socket, &cancel).unwrap();
        drop(listener);
        drop(cleanup);
        drop(lock);
        drop(cancel_sender);
        assert!(lock_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn non_socket_and_symlink_endpoints_are_preserved() {
        let dir = private_dir("non-socket");
        let socket = dir.join("endpoint.sock");
        std::fs::write(&socket, b"preserve").unwrap();
        let (cancel_sender, cancel) = cancellation();
        assert_eq!(
            setup_server(&socket, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(&socket).unwrap(), b"preserve");

        let target = dir.join("target");
        std::fs::write(&target, b"target").unwrap();
        let symlink = dir.join("symlink.sock");
        std::os::unix::fs::symlink(&target, &symlink).unwrap();
        assert_eq!(
            setup_server(&symlink, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            std::fs::symlink_metadata(&symlink)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let directory_symlink = dir.join("runtime-link");
        std::os::unix::fs::symlink(&dir, &directory_symlink).unwrap();
        assert_eq!(
            check_private_directory(&directory_symlink)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(cancel_sender);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn socket_replacement_after_probe_is_not_removed() {
        let dir = private_dir("replacement");
        let path = dir.join("endpoint.sock");
        let stale_listener = UnixListener::bind(&path).unwrap();
        make_socket_private(&path);
        let inspected = std::fs::symlink_metadata(&path).unwrap();
        drop(stale_listener);
        std::fs::remove_file(&path).unwrap();
        let replacement_path = dir.join("replacement.sock");
        let replacement = UnixListener::bind(&replacement_path).unwrap();
        make_socket_private(&replacement_path);
        let replacement_metadata = std::fs::symlink_metadata(&replacement_path).unwrap();
        std::fs::rename(&replacement_path, &path).unwrap();
        let result = remove_inspected_socket(&path, &inspected);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::AddrInUse);
        assert!(same_file(
            &replacement_metadata,
            &std::fs::symlink_metadata(&path).unwrap()
        ));
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(replacement);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_socket_and_unsafe_mode_are_never_recovered() {
        let dir = private_dir("live-socket");
        let path = dir.join("endpoint.sock");
        let listener = UnixListener::bind(&path).unwrap();
        make_socket_private(&path);
        let (cancel_sender, cancel) = cancellation();
        assert_eq!(
            setup_server(&path, &cancel).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(listener);
        let unsafe_path = dir.join("unsafe.sock");
        let unsafe_listener = UnixListener::bind(&unsafe_path).unwrap();
        std::fs::set_permissions(&unsafe_path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(
            setup_server(&unsafe_path, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(unsafe_path.exists());
        drop(unsafe_listener);
        drop(cancel_sender);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_or_locked_lock_paths_fail_without_touching_endpoint() {
        let dir = private_dir("lock-safety");
        let socket = dir.join("endpoint.sock");
        let lock_path = endpoint_lock_path(&socket);
        std::fs::write(&lock_path, b"bad").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let (cancel_sender, cancel) = cancellation();
        assert_eq!(
            setup_server(&socket, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(std::fs::read(&lock_path).unwrap(), b"bad");
        assert!(!socket.exists());

        std::fs::remove_file(&lock_path).unwrap();
        std::fs::write(&lock_path, b"").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            setup_server(&socket, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!socket.exists());
        std::fs::remove_file(&lock_path).unwrap();
        let lock_target = dir.join("lock-target");
        std::fs::write(&lock_target, b"").unwrap();
        std::fs::set_permissions(&lock_target, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&lock_target, &lock_path).unwrap();
        assert_eq!(
            setup_server(&socket, &cancel).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            std::fs::symlink_metadata(&lock_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!socket.exists());

        std::fs::remove_file(&lock_path).unwrap();
        let lock = acquire_endpoint_lock(&lock_path).unwrap();
        assert_eq!(
            setup_server(&socket, &cancel).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!socket.exists());
        drop(lock);

        let lock = acquire_endpoint_lock(&lock_path).unwrap();
        let duplicate = lock.0.try_clone().unwrap();
        drop(lock);
        let reacquired = acquire_endpoint_lock(&lock_path)
            .expect("guard drop explicitly unlocks despite a duplicated descriptor");
        drop(reacquired);
        drop(duplicate);

        drop(cancel_sender);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cancellation_interrupts_a_pending_probe() {
        let (cancel_sender, cancel) = cancellation();
        let mut probe = Box::pin(probe_connection(futures_lite::future::pending(), &cancel));
        assert!(
            futures_lite::future::block_on(futures_lite::future::poll_once(probe.as_mut()))
                .is_none()
        );
        drop(cancel_sender);
        assert!(matches!(
            futures_lite::future::block_on(probe),
            SocketProbe::Cancelled
        ));
    }

    #[test]
    fn pending_probe_times_out() {
        let (cancel_sender, cancel) = cancellation();
        assert!(matches!(
            futures_lite::future::block_on(probe_connection(
                futures_lite::future::pending(),
                &cancel
            )),
            SocketProbe::TimedOut
        ));
        drop(cancel_sender);
    }

    #[test]
    fn concurrent_server_startup_is_excluded_across_processes() {
        const CHILD_ENV: &str = "BKSS_COMMAND_IPC_LOCK_CHILD";
        if let Some(path) = std::env::var_os(CHILD_ENV) {
            let (cancel_sender, cancellation) = cancellation();
            let error = setup_server(Path::new(&path), &cancellation).err().unwrap();
            drop(cancel_sender);
            assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
            return;
        }

        let dir = private_dir("concurrent-startup");
        let socket = dir.join("endpoint.sock");
        let lock_path = endpoint_lock_path(&socket);
        let lock = acquire_endpoint_lock(&lock_path).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "presets::command_ipc::source::tests::concurrent_server_startup_is_excluded_across_processes",
            ])
            .env(CHILD_ENV, &socket)
            .status()
            .unwrap();
        assert!(status.success());
        drop(lock);
        let (cancel_sender, cancellation) = cancellation();
        let (listener, cleanup, lock) = setup_server(&socket, &cancellation).unwrap();
        drop(listener);
        drop(cleanup);
        drop(lock);
        drop(cancel_sender);
        assert!(lock_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

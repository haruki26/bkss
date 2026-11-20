use async_channel::{Receiver, Sender};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const REQUEST: &[u8] = b"quit\n";
const ACK: &[u8] = b"ok\n";
const SOCKET_MODE: u32 = 0o600;

pub(super) struct ControlReservation {
    path: PathBuf,
    _lock: File,
    _runtime_dir: File,
    uid: u32,
    sender: Option<Sender<()>>,
    receiver: Option<Receiver<()>>,
    state: Arc<Mutex<ServerState>>,
}

pub(super) struct ControlServer {
    _endpoint: BoundEndpoint,
    state: Arc<Mutex<ServerState>>,
    worker: Option<thread::JoinHandle<()>>,
    _reservation: ControlReservation,
}

#[derive(Default)]
struct ServerState {
    stopping: bool,
    accepted: bool,
    dispatcher_open: bool,
}

struct BoundEndpoint {
    path: PathBuf,
    socket_id: (u64, u64),
    uid: u32,
}

pub(super) struct DispatchReceiver {
    receiver: Receiver<()>,
    state: Arc<Mutex<ServerState>>,
}

impl DispatchReceiver {
    pub(super) async fn recv(&self) -> Result<(), async_channel::RecvError> {
        self.receiver.recv().await
    }

    #[cfg(test)]
    fn recv_blocking(&self) -> Result<(), async_channel::RecvError> {
        self.receiver.recv_blocking()
    }

    #[cfg(test)]
    fn try_recv(&self) -> Result<(), async_channel::TryRecvError> {
        self.receiver.try_recv()
    }
}

impl Drop for DispatchReceiver {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.dispatcher_open = false;
        }
    }
}

impl Drop for BoundEndpoint {
    fn drop(&mut self) {
        unlink_if_same_socket(&self.path, self.socket_id, self.uid);
    }
}

impl BoundEndpoint {
    fn capture(path: &Path, uid: u32) -> std::io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        validate_owned_socket(&metadata, uid)?;
        Ok(Self {
            path: path.to_path_buf(),
            socket_id: file_id(&metadata),
            uid,
        })
    }
}

fn bind_endpoint(path: &Path, uid: u32) -> std::io::Result<(UnixListener, BoundEndpoint)> {
    let listener = UnixListener::bind(path)?;
    let endpoint = BoundEndpoint::capture(path, uid)?;
    Ok((listener, endpoint))
}

impl ControlReservation {
    /// Claims this executable's endpoint before GPUI or user setup starts.
    pub(super) fn reserve(executable: &Path) -> std::io::Result<Option<Self>> {
        let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
            return Ok(None);
        };
        Self::reserve_at(executable, Path::new(&runtime_dir)).map(Some)
    }

    fn reserve_at(executable: &Path, runtime_dir: &Path) -> std::io::Result<Self> {
        let (executable, uid, runtime_dir_file) = validated_paths(runtime_dir, executable)?;
        let path = endpoint_path(&runtime_dir_file, &executable)?;
        let lock_path = path.with_extension("lock");
        let lock = acquire_lock(&lock_path, uid)?;
        cleanup_stale_socket(&path, uid)?;
        let (sender, receiver) = async_channel::bounded(1);
        Ok(Self {
            path,
            _lock: lock,
            _runtime_dir: runtime_dir_file,
            uid,
            sender: Some(sender),
            receiver: Some(receiver),
            state: Arc::new(Mutex::new(ServerState::default())),
        })
    }

    pub(super) fn take_dispatcher(&mut self) -> std::io::Result<DispatchReceiver> {
        self.state
            .lock()
            .map_err(|_| std::io::Error::other("lifecycle dispatcher state is unavailable"))?
            .dispatcher_open = true;
        Ok(DispatchReceiver {
            receiver: self
                .receiver
                .take()
                .ok_or_else(|| std::io::Error::other("lifecycle dispatcher was already taken"))?,
            state: Arc::clone(&self.state),
        })
    }

    pub(super) fn activate(mut self) -> std::io::Result<ControlServer> {
        let (listener, endpoint) = bind_endpoint(&self.path, self.uid)?;
        listener.set_nonblocking(true)?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(SOCKET_MODE))?;
        let metadata = fs::symlink_metadata(&self.path)?;
        validate_socket_metadata(&metadata, self.uid)?;
        let worker_state = Arc::clone(&self.state);
        let sender = self
            .sender
            .take()
            .ok_or_else(|| std::io::Error::other("lifecycle server was already activated"))?;
        let worker = thread::Builder::new()
            .name("bkss-shell-control".into())
            .spawn(move || {
                loop {
                    if should_stop(&worker_state) {
                        break;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => handle_request(stream, &sender, &worker_state),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(ControlServer {
            _endpoint: endpoint,
            state: Arc::clone(&self.state),
            worker: Some(worker),
            _reservation: self,
        })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.stopping = true;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_request(mut stream: UnixStream, sender: &Sender<()>, state: &Mutex<ServerState>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let mut request = [0; REQUEST.len()];
    let mut trailing = [0];
    if stream.read_exact(&mut request).is_err()
        || request != REQUEST
        || !stream.read(&mut trailing).is_ok_and(|read| read == 0)
    {
        return;
    }

    let Ok(mut state) = state.lock() else {
        return;
    };
    if state.stopping || state.accepted || !state.dispatcher_open || sender.is_closed() {
        return;
    }
    if stream.write_all(ACK).is_err() {
        return;
    }
    state.accepted = true;
    let _ = sender.try_send(());
}

fn should_stop(state: &Mutex<ServerState>) -> bool {
    state
        .lock()
        .map(|state| state.stopping || state.accepted)
        .unwrap_or(true)
}

pub(super) fn request_quit(executable: &Path) -> std::io::Result<()> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set")
    })?;
    request_quit_at(executable, Path::new(&runtime_dir))
}

fn request_quit_at(executable: &Path, runtime_dir: &Path) -> std::io::Result<()> {
    let (executable, uid, runtime_dir_file) = validated_paths(runtime_dir, executable)?;
    let path = endpoint_path(&runtime_dir_file, &executable)?;
    let endpoint = fs::symlink_metadata(&path)?;
    validate_socket_metadata(&endpoint, uid)?;
    let mut stream = UnixStream::connect(&path)?;
    stream.set_read_timeout(Some(Duration::from_millis(750)))?;
    stream.set_write_timeout(Some(Duration::from_millis(750)))?;
    stream.write_all(REQUEST)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut ack = [0; ACK.len()];
    stream.read_exact(&mut ack)?;
    if ack != ACK {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid lifecycle acknowledgement",
        ));
    }
    Ok(())
}

fn validated_paths(runtime_dir: &Path, executable: &Path) -> std::io::Result<(PathBuf, u32, File)> {
    let runtime_dir = fs::canonicalize(runtime_dir)?;
    let metadata = fs::metadata(&runtime_dir)?;
    let uid = effective_uid()?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be owned by the effective user and private",
        ));
    }
    let runtime_dir_file = File::open(&runtime_dir)?;
    let opened = runtime_dir_file.metadata()?;
    let current = fs::symlink_metadata(&runtime_dir)?;
    if !opened.is_dir()
        || opened.uid() != uid
        || opened.mode() & 0o077 != 0
        || file_id(&opened) != file_id(&current)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR changed while validating",
        ));
    }
    Ok((fs::canonicalize(executable)?, uid, runtime_dir_file))
}

fn acquire_lock(path: &Path, uid: u32) -> std::io::Result<File> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let before = fs::symlink_metadata(path)?;
            validate_lock_metadata(&before, uid)?;
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            let opened = file.metadata()?;
            validate_lock_metadata(&opened, uid)?;
            let after = fs::symlink_metadata(path)?;
            validate_lock_metadata(&after, uid)?;
            if file_id(&opened) != file_id(&after) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "lifecycle lock path changed while opening",
                ));
            }
            file
        }
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    validate_lock_metadata(&metadata, uid)?;
    let path_metadata = fs::symlink_metadata(path)?;
    validate_lock_metadata(&path_metadata, uid)?;
    if file_id(&metadata) != file_id(&path_metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle lock path changed while opening",
        ));
    }
    file.try_lock()?;
    let locked_path = fs::symlink_metadata(path)?;
    validate_lock_metadata(&locked_path, uid)?;
    if file_id(&metadata) != file_id(&locked_path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle lock path changed while locking",
        ));
    }
    Ok(file)
}

fn validate_lock_metadata(metadata: &fs::Metadata, uid: u32) -> std::io::Result<()> {
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle lock must be an empty, private, user-owned regular file",
        ));
    }
    Ok(())
}

fn cleanup_stale_socket(path: &Path, uid: u32) -> std::io::Result<()> {
    let initial = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    validate_owned_socket(&initial, uid)?;
    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "lifecycle endpoint has a live listener",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(error),
    }
    let current = fs::symlink_metadata(path)?;
    validate_owned_socket(&current, uid)?;
    if file_id(&initial) != file_id(&current) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "lifecycle endpoint changed during stale-socket check",
        ));
    }
    fs::remove_file(path)?;
    Ok(())
}

fn unlink_if_same_socket(path: &Path, expected_id: (u64, u64), uid: u32) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_socket()
        && metadata.uid() == uid
        && file_id(&metadata) == expected_id
    {
        let _ = fs::remove_file(path);
    }
}

fn validate_owned_socket(metadata: &fs::Metadata, uid: u32) -> std::io::Result<()> {
    if !metadata.file_type().is_socket() || metadata.uid() != uid || metadata.nlink() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle endpoint must be a user-owned socket",
        ));
    }
    Ok(())
}

fn validate_socket_metadata(metadata: &fs::Metadata, uid: u32) -> std::io::Result<()> {
    validate_owned_socket(metadata, uid)?;
    if metadata.mode() & 0o777 != SOCKET_MODE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lifecycle endpoint must be a private, user-owned socket",
        ));
    }
    Ok(())
}

fn file_id(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

#[cfg(test)]
fn socket_path(runtime_dir: &Path, executable: &Path) -> PathBuf {
    runtime_dir.join(socket_filename(executable))
}

fn socket_filename(executable: &Path) -> String {
    let bytes = executable.as_os_str().as_bytes();
    let mut hash = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d_u128;
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013bu128);
    }
    format!("bkss-{hash:032x}.sock")
}

fn endpoint_path(runtime_dir: &File, executable: &Path) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    Ok(Path::new("/proc/self/fd")
        .join(runtime_dir.as_raw_fd().to_string())
        .join(socket_filename(executable)))
}

fn effective_uid() -> std::io::Result<u32> {
    let status = fs::read_to_string("/proc/self/status")?;
    let uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|uid| uid.parse().ok())
        .ok_or_else(|| std::io::Error::other("could not determine effective uid"))?;
    Ok(uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, sync::Barrier, time::Instant};

    fn private_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bkss-control-{name}-{}-{}",
            std::process::id(),
            Instant::now().elapsed().as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn long_private_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bkss-control-{}-{}",
            std::process::id(),
            "runtime-directory-component-".repeat(3)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn executable(dir: &Path, relative: &str) -> PathBuf {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        path
    }

    #[test]
    fn lifecycle_request_acks_only_valid_quit() {
        let dir = private_dir("ack");
        let executable = fs::canonicalize(executable(&dir, "example")).unwrap();
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let path = reservation.path.clone();
        let canonical_path = socket_path(&dir, &executable);
        let (server, receiver) = start_for_test(reservation);

        let mut malformed = UnixStream::connect(&path).unwrap();
        malformed.write_all(b"stop\n").unwrap();
        let mut byte = [0];
        assert_eq!(malformed.read(&mut byte).unwrap(), 0);
        assert!(receiver.try_recv().is_err());

        let started = Instant::now();
        request_on_path(&path);
        assert!(receiver.recv_blocking().is_ok());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(server);
        assert!(!canonical_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dispatcher_gone_before_request_never_receives_an_ack() {
        let dir = private_dir("closed-dispatcher");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let path = reservation.path.clone();
        let (server, dispatcher) = start_for_test(reservation);
        drop(dispatcher);
        let mut stream = UnixStream::connect(&path).unwrap();
        stream.write_all(REQUEST).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut ack = [0; ACK.len()];
        assert_eq!(stream.read(&mut ack).unwrap(), 0);
        drop(server);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bound_endpoint_guard_removes_socket_after_partial_activation() {
        let dir = private_dir("partial-activation");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let path = reservation.path.clone();
        let (listener, endpoint) = bind_endpoint(&path, reservation.uid).unwrap();
        drop(endpoint);
        drop(listener);
        assert!(!path.exists());
        drop(reservation);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn same_executable_startup_is_exclusive_across_threads() {
        let dir = private_dir("concurrent");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let results = (0..2)
            .map(|_| {
                let dir = dir.clone();
                let executable = executable.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    ControlReservation::reserve_at(&executable, &dir)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = results
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn endpoint_identity_is_canonical_path_not_basename_and_hash_is_stable() {
        let dir = private_dir("identity");
        let first = fs::canonicalize(executable(&dir, "one/app")).unwrap();
        let second = fs::canonicalize(executable(&dir, "two/app")).unwrap();
        let first_path = socket_path(&dir, &first);
        assert_eq!(socket_path(&dir, &first), first_path);
        assert_ne!(first_path, socket_path(&dir, &second));
        assert_eq!(
            socket_path(Path::new("/run/user/1000"), Path::new("/usr/bin/app")),
            Path::new("/run/user/1000/bkss-2544a6d8089769e63654b42a10ed3faa.sock")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clients_cannot_queue_quit_before_activation() {
        let dir = private_dir("not-ready");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let path = reservation.path.clone();
        assert_eq!(
            UnixStream::connect(&path).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        let (server, receiver) = start_for_test(reservation);
        assert!(receiver.try_recv().is_err());
        drop(server);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn long_runtime_path_uses_dirfd_for_a_real_client_request() {
        let dir = long_private_dir();
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let path = socket_path(&dir, &executable);
        assert!(path.as_os_str().as_bytes().len() > 107);
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let (server, receiver) = start_for_test(reservation);
        request_quit_at(&executable, &dir).unwrap();
        assert!(receiver.recv_blocking().is_ok());
        drop(server);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_rename_keeps_endpoint_and_lock_pinned_to_open_directory() {
        let original = private_dir("renamed-runtime");
        let moved = original.with_file_name(format!(
            "{}-moved",
            original.file_name().unwrap().to_string_lossy()
        ));
        let executable = fs::canonicalize(executable(&original, "app")).unwrap();
        let canonical_socket = socket_path(&original, &executable);
        let canonical_lock = canonical_socket.with_extension("lock");
        let reservation = ControlReservation::reserve_at(&executable, &original).unwrap();

        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).unwrap();
        let (server, _) = start_for_test(reservation);

        assert!(moved.join(canonical_socket.file_name().unwrap()).exists());
        assert!(moved.join(canonical_lock.file_name().unwrap()).exists());
        assert!(!canonical_socket.exists());
        assert!(!canonical_lock.exists());
        drop(server);
        assert!(!moved.join(canonical_socket.file_name().unwrap()).exists());
        assert!(!canonical_socket.exists());
        fs::remove_dir_all(moved).unwrap();
        fs::remove_dir_all(original).unwrap();
    }

    #[test]
    fn refuses_foreign_path_and_drop_preserves_replaced_endpoint() {
        let dir = private_dir("replacement");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        let path = reservation.path.clone();
        let canonical_path = socket_path(&dir, &executable);
        let (server, _) = start_for_test(reservation);
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"foreign").unwrap();
        drop(server);
        assert_eq!(fs::read(&canonical_path).unwrap(), b"foreign");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_socket_is_replaced_but_non_socket_is_preserved() {
        let dir = private_dir("stale");
        let executable = fs::canonicalize(executable(&dir, "app")).unwrap();
        let path = socket_path(&dir, &executable);
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        drop(listener);
        let reservation = ControlReservation::reserve_at(&executable, &dir).unwrap();
        assert!(!path.exists());
        drop(reservation);
        fs::write(&path, b"not a socket").unwrap();
        assert!(ControlReservation::reserve_at(&executable, &dir).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"not a socket");
        fs::remove_dir_all(dir).unwrap();
    }

    fn start_for_test(mut reservation: ControlReservation) -> (ControlServer, DispatchReceiver) {
        let receiver = reservation.take_dispatcher().unwrap();
        let server = reservation.activate().unwrap();
        (server, receiver)
    }

    fn request_on_path(path: &Path) {
        let mut stream = UnixStream::connect(path).unwrap();
        stream.write_all(REQUEST).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut ack = [0; ACK.len()];
        stream.read_exact(&mut ack).unwrap();
        assert_eq!(&ack, ACK);
    }
}

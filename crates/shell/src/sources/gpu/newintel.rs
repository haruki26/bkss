use async_io::Async;
use bkss_integration::base::Cancellation;
use futures_lite::{future, io::AsyncReadExt};
use serde_json::Value;
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    time::Duration,
};

use super::state::{GpuMetrics, GpuState, GpuSupport};

const READ_TIMEOUT: Duration = Duration::from_millis(150);
const DRAIN_TIMEOUT: Duration = Duration::from_millis(10);
const MAX_FRAME_SIZE: usize = 64 * 1024;
const READ_SIZE: usize = 4096;
const MAX_DRAIN_FRAMES: usize = 8;

pub(super) struct IntelSamplers {
    children: HashMap<String, IntelChild>,
    failed: HashMap<String, PathBuf>,
    program: PathBuf,
}

impl Default for IntelSamplers {
    fn default() -> Self {
        Self {
            children: HashMap::new(),
            failed: HashMap::new(),
            program: PathBuf::from("intel_gpu_top"),
        }
    }
}

impl IntelSamplers {
    pub(super) async fn sample(
        &mut self,
        root: &Path,
        state: &mut GpuState,
        cancellation: &Cancellation,
    ) {
        self.children.retain(|name, _| {
            state
                .devices
                .iter()
                .any(|gpu| gpu.name == *name && gpu.support == GpuSupport::Intel)
        });
        self.failed.retain(|name, _| {
            state
                .devices
                .iter()
                .any(|gpu| gpu.name == *name && gpu.support == GpuSupport::Intel)
        });
        for gpu in &mut state.devices {
            if gpu.support != GpuSupport::Intel || cancellation.is_cancelled() {
                continue;
            }
            gpu.metrics = GpuMetrics {
                utilization_percent: None,
                memory_used_bytes: None,
                memory_total_bytes: None,
            };
            let Some(device_path) = drm_device_path(root, &gpu.name) else {
                self.children.remove(&gpu.name);
                self.failed.remove(&gpu.name);
                continue;
            };
            if self
                .children
                .get(&gpu.name)
                .is_some_and(|child| child.device_path != device_path)
            {
                self.children.remove(&gpu.name);
            }
            if self
                .failed
                .get(&gpu.name)
                .is_some_and(|failed_path| failed_path != &device_path)
            {
                self.failed.remove(&gpu.name);
            }
            if !self.children.contains_key(&gpu.name) && !self.failed.contains_key(&gpu.name) {
                match IntelChild::start(&device_path, &self.program) {
                    Ok(child) => {
                        self.children.insert(gpu.name.clone(), child);
                    }
                    Err(_) => {
                        self.failed.insert(gpu.name.clone(), device_path.clone());
                    }
                }
            }
            let Some(child) = self.children.get_mut(&gpu.name) else {
                continue;
            };
            if let Some(utilization) = child.next_sample(cancellation).await {
                gpu.metrics.utilization_percent = Some(utilization);
            } else if child.exited {
                self.children.remove(&gpu.name);
                self.failed.insert(gpu.name.clone(), device_path);
            }
        }
    }
}

fn drm_device_path(root: &Path, card: &str) -> Option<PathBuf> {
    std::fs::canonicalize(root.join(card).join("device")).ok()
}

struct IntelChild {
    child: Child,
    stdout: Async<ChildStdout>,
    frame: FrameReader,
    device_path: PathBuf,
    exited: bool,
}

impl IntelChild {
    fn start(device: &Path, program: &Path) -> io::Result<Self> {
        let filter = format!("sys:{}", device.display());
        let child = Command::new(program)
            .args(["-J", "-s", "500", "-d", &filter])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        Self::from_child(device, child)
    }

    fn from_child(device: &Path, mut child: Child) -> io::Result<Self> {
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("intel_gpu_top stdout unavailable"));
        };
        let stdout = match Async::new(stdout) {
            Ok(stdout) => stdout,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            stdout,
            frame: FrameReader::default(),
            device_path: device.to_path_buf(),
            exited: false,
        })
    }

    async fn next_sample(&mut self, cancellation: &Cancellation) -> Option<f32> {
        let mut latest = None;
        for frame_index in 0..=MAX_DRAIN_FRAMES {
            let timeout = if frame_index == 0 {
                READ_TIMEOUT
            } else {
                DRAIN_TIMEOUT
            };
            let result = future::race(
                async {
                    Some(
                        future::race(
                            async { self.frame.next(&mut self.stdout).await.map(Some) },
                            async {
                                async_io::Timer::after(timeout).await;
                                Ok(None)
                            },
                        )
                        .await,
                    )
                },
                async {
                    cancellation.cancelled().await;
                    None
                },
            )
            .await;
            match result {
                Some(Ok(Some(frame))) => {
                    if let Some(utilization) = parse_busy_percent(&frame) {
                        latest = Some(utilization);
                    }
                }
                Some(Ok(None)) => break,
                Some(Err(_)) => {
                    self.exited = true;
                    return None;
                }
                None => return None,
            }
        }
        latest
    }
}

impl Drop for IntelChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct FrameReader {
    bytes: Vec<u8>,
}

impl FrameReader {
    async fn next(&mut self, stdout: &mut Async<ChildStdout>) -> io::Result<Vec<u8>> {
        loop {
            if let Some(frame) = take_object(&mut self.bytes) {
                return Ok(frame);
            }
            if self.bytes.len() >= MAX_FRAME_SIZE {
                self.bytes.clear();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "GPU JSON frame too large",
                ));
            }
            let mut chunk = [0; READ_SIZE];
            let read = stdout.read(&mut chunk).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "GPU stream ended",
                ));
            }
            self.push(&chunk[..read])?;
        }
    }

    fn push(&mut self, chunk: &[u8]) -> io::Result<()> {
        if self.bytes.len().saturating_add(chunk.len()) > MAX_FRAME_SIZE {
            self.bytes.clear();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "GPU JSON frame too large",
            ));
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }
}

fn take_object(bytes: &mut Vec<u8>) -> Option<Vec<u8>> {
    let start = bytes.iter().position(|byte| *byte == b'{')?;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start) {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    let frame = bytes[start..=index].to_vec();
                    bytes.drain(..=index);
                    return Some(frame);
                }
            }
            _ => {}
        }
    }
    if start > 0 {
        bytes.drain(..start);
    }
    None
}

fn parse_busy_percent(frame: &[u8]) -> Option<f32> {
    let root: Value = serde_json::from_slice(frame).ok()?;
    let engines = root.get("engines")?.as_object()?;
    engines
        .values()
        .filter_map(|engine| {
            if engine.get("unit")?.as_str()? != "%" {
                return None;
            }
            let busy = engine.get("busy")?.as_f64()?;
            (busy.is_finite() && (0.0..=100.0).contains(&busy)).then_some(busy as f32)
        })
        .reduce(f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bkss_core::{ResourceLease, ResourceScope};
    use bkss_integration::{
        Source, SourceDescriptor, SourcePublisher,
        base::{Cancellation, WorkerGuard, spawn_worker},
    };
    use gpui::{App, TestAppContext};
    use std::sync::mpsc::{self, SyncSender};

    const FIXTURE: &str = include_str!("fixtures/intel-gpu-sample.json");

    struct CancellationSource {
        sender: SyncSender<Cancellation>,
    }

    impl Source for CancellationSource {
        type Key = ();
        type Value = ();
        type Error = io::Error;
        type Guard = WorkerGuard;

        fn key(&self) {}

        fn start(&self, publisher: SourcePublisher<Self>, cx: &mut App) -> io::Result<Self::Guard> {
            let (sender, bridge) = publisher.thread_bridge(|value| value, |error| error, cx);
            let cancellation_sender = self.sender.clone();
            spawn_worker(
                "bkss-gpu-cancellation-test",
                bridge,
                sender,
                move |_, cancellation| {
                    cancellation_sender
                        .send(cancellation.clone())
                        .map_err(|_| io::Error::other("cancellation receiver dropped"))?;
                    futures_lite::future::block_on(cancellation.cancelled());
                    Ok(())
                },
            )
        }
    }

    fn worker_cancellation() -> (
        TestAppContext,
        ResourceScope,
        ResourceLease<()>,
        Cancellation,
    ) {
        let cx = TestAppContext::single();
        cx.dispatcher.allow_parking();
        let scope = cx.update(ResourceScope::new);
        let (sender, receiver) = mpsc::sync_channel(1);
        let descriptor = SourceDescriptor::new(CancellationSource { sender });
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
        let cancellation = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        (cx, scope, lease, cancellation)
    }

    #[test]
    fn parses_busiest_engine_without_summing_and_rejects_invalid_values() {
        let value: Value = serde_json::from_str(FIXTURE).unwrap();
        let frame = serde_json::to_vec(&value[0]).unwrap();
        assert_eq!(parse_busy_percent(&frame), Some(11.943417));
        for frame in [
            br#"{"engines":{"x":{"busy":101,"unit":"%"}}}"#.as_slice(),
            br#"{"engines":{"x":{"busy":4,"unit":"ms"}}}"#.as_slice(),
            br#"{"engines":{"x":{"busy":"-","unit":"%"}}}"#.as_slice(),
            br#"{"engines":{"x":{"busy":null,"unit":"%"}}}"#.as_slice(),
            br#"{"engines":{"x":{"busy":NaN,"unit":"%"}}}"#.as_slice(),
        ] {
            assert_eq!(parse_busy_percent(frame), None);
        }
    }

    #[test]
    fn frames_pretty_printed_objects_and_bounds_incomplete_input() {
        let mut bytes = b"[\n{\"engines\": {\"a\": {\"text\": \"}\\\\\\\"{\"}}},\n".to_vec();
        assert!(take_object(&mut bytes).is_some());
        let mut reader = FrameReader::default();
        assert!(reader.push(&vec![b' '; MAX_FRAME_SIZE]).is_ok());
        assert!(reader.push(b" ").is_err());
        assert!(parse_busy_percent(br#"{"engines":{"x":{"busy":4"#).is_none());
    }

    #[test]
    fn missing_intel_gpu_top_is_a_recoverable_startup_failure() {
        let error = IntelChild::start(
            Path::new("/sys/devices/pci0000:00/0000:00:02.0"),
            Path::new("bkss-no-such-intel-gpu-top"),
        );
        assert!(error.is_err());
    }

    #[test]
    fn exited_child_is_detected_and_cancelled_child_is_owned() {
        let device = Path::new("/sys/devices/pci0000:00/0000:00:02.0");
        let exited = Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut exited = IntelChild::from_child(device, exited).unwrap();
        let (_cx, _scope, lease, cancellation) = worker_cancellation();
        assert_eq!(
            futures_lite::future::block_on(exited.next_sample(&cancellation)),
            None
        );
        assert!(exited.exited);
        drop(lease);

        let sleeping = Command::new("sleep")
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut sleeping = IntelChild::from_child(device, sleeping).unwrap();
        let (_cx, _scope, lease, cancellation) = worker_cancellation();
        drop(lease);
        assert_eq!(
            futures_lite::future::block_on(sleeping.next_sample(&cancellation)),
            None
        );
        assert!(!sleeping.exited);
    }

    #[test]
    fn drains_burst_to_latest_and_silent_child_does_not_replay_old_metrics() {
        let root = test_root("burst");
        let device = root.join("card0/device");
        std::fs::create_dir_all(&device).unwrap();
        let args = root.join("args");
        let program = write_program(
            &root.join("fake-intel-gpu-top"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"engines\":{{\"R\":{{\"busy\":10,\"unit\":\"%\"}}}}}}' '{{\"engines\":{{\"R\":{{\"busy\":20,\"unit\":\"%\"}}}}}}' '{{\"engines\":{{\"R\":{{\"busy\":30,\"unit\":\"%\"}}}}}}'\nexec tail -f /dev/null\n",
                args.display()
            ),
        );
        let mut samplers = IntelSamplers {
            program,
            ..IntelSamplers::default()
        };
        let (_cx, _scope, lease, cancellation) = worker_cancellation();
        let mut state = intel_state();
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert_eq!(state.devices[0].metrics.utilization_percent, Some(30.0));
        assert_eq!(
            std::fs::read_to_string(args)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            [
                "-J",
                "-s",
                "500",
                "-d",
                &format!("sys:{}", device.canonicalize().unwrap().display()),
            ]
        );

        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert_eq!(state.devices[0].metrics.utilization_percent, None);
        assert_eq!(samplers.children.len(), 1);
        drop(lease);
        drop(samplers);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_failure_is_remembered_until_device_disappears() {
        let root = test_root("failed");
        std::fs::create_dir_all(root.join("card0/device")).unwrap();
        let mut samplers = IntelSamplers {
            program: root.join("missing-tool"),
            ..IntelSamplers::default()
        };
        let (_cx, _scope, lease, cancellation) = worker_cancellation();
        let mut state = intel_state();
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert_eq!(samplers.failed.len(), 1);
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert_eq!(samplers.failed.len(), 1);
        assert!(samplers.children.is_empty());
        state.devices.clear();
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert!(samplers.failed.is_empty());
        drop(lease);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exited_child_is_not_restarted_each_sample() {
        let root = test_root("exited");
        std::fs::create_dir_all(root.join("card0/device")).unwrap();
        let starts = root.join("starts");
        let program = write_program(
            &root.join("fake-intel-gpu-top"),
            &format!(
                "#!/bin/sh\necho started >> '{}'\nexit 0\n",
                starts.display()
            ),
        );
        let mut samplers = IntelSamplers {
            program,
            ..IntelSamplers::default()
        };
        let (_cx, _scope, lease, cancellation) = worker_cancellation();
        let mut state = intel_state();
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        futures_lite::future::block_on(samplers.sample(&root, &mut state, &cancellation));
        assert_eq!(std::fs::read_to_string(starts).unwrap().lines().count(), 1);
        assert!(samplers.children.is_empty());
        assert_eq!(samplers.failed.len(), 1);
        drop(lease);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn intel_state() -> GpuState {
        GpuState {
            devices: vec![super::super::state::Gpu {
                name: "card0".to_owned(),
                vendor_id: Some("8086".to_owned()),
                device_id: Some("a7a8".to_owned()),
                support: GpuSupport::Intel,
                metrics: GpuMetrics {
                    utilization_percent: None,
                    memory_used_bytes: None,
                    memory_total_bytes: None,
                },
            }],
        }
    }

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bkss-gpu-intel-{name}-{}", std::process::id()))
    }

    fn write_program(path: &Path, source: &str) -> PathBuf {
        std::fs::write(path, source).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).unwrap();
        path.to_path_buf()
    }
}

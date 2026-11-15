use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::{
        command::{self, CommandRequest},
        worker::{self, Cancellation},
    },
};
use gpui::App;
use std::{
    error::Error,
    fmt, fs, io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

use super::state::{Backlight, BacklightState};

#[derive(Debug)]
pub enum BacklightError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    InvalidBrightness {
        requested: u64,
        maximum: u64,
    },
    DeviceUnavailable(String),
}

impl fmt::Display for BacklightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::InvalidBrightness { requested, maximum } => {
                write!(f, "brightness {requested} exceeds maximum {maximum}")
            }
            Self::DeviceUnavailable(name) => write!(f, "backlight device {name:?} is unavailable"),
        }
    }
}

impl Error for BacklightError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub struct BacklightSource {
    root: PathBuf,
}

impl BacklightSource {
    /// Declares a sysfs backlight source rooted at `/sys/class/backlight`.
    ///
    /// Reads each device's `brightness` and `max_brightness` attributes and
    /// listens for udev `backlight` subsystem events.
    pub fn new() -> SourceDescriptor<Self> {
        Self::with_root("/sys/class/backlight")
    }

    /// Declares a sysfs backlight source with a configurable root.
    pub fn with_root(root: impl Into<PathBuf>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { root: root.into() })
    }
}

impl Source for BacklightSource {
    type Key = PathBuf;
    type Value = BacklightState;
    type Error = BacklightError;
    type Guard = worker::WorkerGuard;

    fn key(&self) -> Self::Key {
        self.root.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let root = self.root.clone();
        worker::spawn_worker(
            "bkss-backlight",
            bridge,
            sender,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(root, publisher, cancellation))
            },
        )
        .map_err(|source| BacklightError::Io {
            operation: "start backlight worker",
            source,
        })
    }
}

pub(super) struct SetBrightness {
    pub(super) device: String,
    pub(super) brightness: u64,
}

async fn run(
    root: PathBuf,
    publisher: crate::ThreadPublisher<BacklightState, BacklightError>,
    cancellation: Cancellation,
) -> Result<(), BacklightError> {
    const CAPACITY: NonZeroUsize = match NonZeroUsize::new(16) {
        Some(capacity) => capacity,
        None => NonZeroUsize::MIN,
    };
    let (commands, receiver) =
        command::channel::<SetBrightness, (), BacklightError>(CAPACITY, cancellation.clone());
    if cancellation.is_cancelled() {
        return Ok(());
    }
    let monitor = udev::MonitorBuilder::new()
        .and_then(|monitor| monitor.match_subsystem("backlight"))
        .and_then(udev::MonitorBuilder::listen)
        .and_then(async_io::Async::new)
        .map_err(|source| BacklightError::Io {
            operation: "subscribe to backlight device events",
            source,
        })?;
    if !publish_state(&root, &commands, &publisher)? {
        return Ok(());
    }
    loop {
        let next = futures_lite::future::race(
            async {
                match monitor
                    .read_with(|socket| {
                        socket
                            .iter()
                            .next()
                            .ok_or_else(|| io::Error::from(io::ErrorKind::WouldBlock))
                    })
                    .await
                {
                    Ok(_) => Event::Device,
                    Err(source) => Event::MonitorError(source),
                }
            },
            async {
                match receiver.recv().await {
                    Some(request) => Event::Command(request),
                    None => Event::Closed,
                }
            },
        );
        let event = futures_lite::future::race(next, async {
            cancellation.cancelled().await;
            Event::Cancelled
        })
        .await;
        match event {
            Event::Device => {
                if !publish_state(&root, &commands, &publisher)? {
                    return Ok(());
                }
            }
            Event::MonitorError(source) => {
                return Err(BacklightError::Io {
                    operation: "wait for backlight device event",
                    source,
                });
            }
            Event::Closed | Event::Cancelled => return Ok(()),
            Event::Command(request) => {
                let (request, reply) = request.into_parts();
                let result = set_brightness(&root, request);
                if !publish_state(&root, &commands, &publisher)? {
                    return Ok(());
                }
                reply(result);
            }
        }
    }
}

fn publish_state(
    root: &Path,
    commands: &crate::base::command::CommandHandle<SetBrightness, (), BacklightError>,
    publisher: &crate::ThreadPublisher<BacklightState, BacklightError>,
) -> Result<bool, BacklightError> {
    let devices = read_devices(root)?;
    Ok(publisher
        .publish(BacklightState {
            devices,
            commands: commands.clone(),
        })
        .is_ok())
}

enum Event {
    Device,
    MonitorError(io::Error),
    Command(CommandRequest<SetBrightness, (), BacklightError>),
    Closed,
    Cancelled,
}

fn read_devices(root: &Path) -> Result<Vec<Backlight>, BacklightError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(BacklightError::Io {
                operation: "enumerate backlight devices",
                source,
            });
        }
    };
    let mut devices = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| BacklightError::Io {
            operation: "enumerate backlight devices",
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let brightness = match read_number(&path.join("brightness"), "read brightness") {
            Err(BacklightError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            result => result?,
        };
        let maximum = match read_number(&path.join("max_brightness"), "read maximum brightness") {
            Err(BacklightError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            result => result?,
        };
        let device = Backlight {
            name,
            brightness,
            maximum,
        };
        devices.push(device);
    }
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(devices)
}

fn read_number(path: &Path, operation: &'static str) -> Result<u64, BacklightError> {
    let text =
        fs::read_to_string(path).map_err(|source| BacklightError::Io { operation, source })?;
    text.trim().parse().map_err(|_| BacklightError::Io {
        operation,
        source: io::Error::new(
            io::ErrorKind::InvalidData,
            "sysfs value is not an unsigned integer",
        ),
    })
}

fn set_brightness(root: &Path, request: SetBrightness) -> Result<(), BacklightError> {
    let name = Path::new(&request.device);
    if name.components().count() != 1
        || !matches!(
            name.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(BacklightError::DeviceUnavailable(request.device));
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(BacklightError::DeviceUnavailable(request.device));
        }
        Err(source) => {
            return Err(BacklightError::Io {
                operation: "enumerate backlight devices",
                source,
            });
        }
    };
    let mut found = false;
    for entry in entries {
        let entry = entry.map_err(|source| BacklightError::Io {
            operation: "enumerate backlight devices",
            source,
        })?;
        if entry.file_name().to_string_lossy() == request.device {
            found = true;
            break;
        }
    }
    if !found {
        return Err(BacklightError::DeviceUnavailable(request.device));
    }
    let path = root.join(name);
    if !path.is_dir() {
        return Err(BacklightError::DeviceUnavailable(request.device));
    }
    let maximum = match read_number(&path.join("max_brightness"), "read maximum brightness") {
        Err(BacklightError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Err(BacklightError::DeviceUnavailable(request.device));
        }
        result => result?,
    };
    if request.brightness > maximum {
        return Err(BacklightError::InvalidBrightness {
            requested: request.brightness,
            maximum,
        });
    }
    fs::write(path.join("brightness"), request.brightness.to_string()).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            BacklightError::DeviceUnavailable(request.device)
        } else {
            BacklightError::Io {
                operation: "write brightness",
                source,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        sync::mpsc,
        thread,
        time::Duration,
    };

    #[test]
    fn validates_range_and_confines_operation_to_enumerated_name() {
        let root = std::env::temp_dir().join(format!("bkss-backlight-{}", std::process::id()));
        let device = root.join("panel");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("max_brightness"), "8\n").unwrap();
        fs::write(device.join("brightness"), "3\n").unwrap();
        assert_eq!(
            read_devices(&root).unwrap(),
            vec![Backlight {
                name: "panel".into(),
                brightness: 3,
                maximum: 8,
            }]
        );

        assert!(matches!(
            set_brightness(
                &root,
                SetBrightness {
                    device: "panel".into(),
                    brightness: 9
                }
            ),
            Err(BacklightError::InvalidBrightness { .. })
        ));
        assert_eq!(
            fs::read_to_string(device.join("brightness")).unwrap(),
            "3\n"
        );
        assert!(matches!(
            set_brightness(
                &root,
                SetBrightness {
                    device: "../outside".into(),
                    brightness: 1
                }
            ),
            Err(BacklightError::DeviceUnavailable(_))
        ));
        set_brightness(
            &root,
            SetBrightness {
                device: "panel".into(),
                brightness: 5,
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(device.join("brightness")).unwrap(), "5");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn device_removal_and_missing_root_are_normal_empty_device_sets() {
        let root =
            std::env::temp_dir().join(format!("bkss-backlight-absent-{}", std::process::id()));
        let device = root.join("panel");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("brightness"), "2\n").unwrap();
        fs::write(device.join("max_brightness"), "8\n").unwrap();
        assert_eq!(read_devices(&root).unwrap().len(), 1);
        fs::remove_dir_all(&device).unwrap();
        assert!(read_devices(&root).unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
        assert!(read_devices(&root).unwrap().is_empty());
    }

    #[test]
    fn async_read_with_waits_after_would_block_then_consumes_event() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let reader = async_io::Async::new(reader).unwrap();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(0);
        let writer = thread::spawn(move || {
            ready_receiver.recv().unwrap();
            writer.write_all(&[7]).unwrap();
        });
        let mut first_read = true;
        let read = reader.read_with(|stream| {
            let mut stream = stream;
            let mut byte = [0];
            match stream.read(&mut byte) {
                Ok(1) => Ok(byte[0]),
                Ok(_) => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
                Err(error) if first_read && error.kind() == io::ErrorKind::WouldBlock => {
                    first_read = false;
                    ready_sender
                        .send(())
                        .map_err(|_| io::Error::other("test writer stopped"))?;
                    Err(error)
                }
                Err(error) => Err(error),
            }
        });
        let timeout = async {
            async_io::Timer::after(Duration::from_secs(1)).await;
            Err(io::Error::from(io::ErrorKind::TimedOut))
        };
        assert_eq!(
            futures_lite::future::block_on(futures_lite::future::race(read, timeout)).unwrap(),
            7
        );
        writer.join().unwrap();
    }
}

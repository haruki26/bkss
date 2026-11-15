use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::worker::{self, Cancellation},
};
use gpui::App;
use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use super::state::{Gpu, GpuMetrics, GpuState, GpuSupport};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub enum GpuError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
        }
    }
}

impl Error for GpuError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

pub struct GpuSource {
    root: PathBuf,
}

impl GpuSource {
    /// Declares a safe sysfs GPU sampler rooted at `/sys/class/drm`.
    ///
    /// AMD (`0x1002`) devices use `cardN/device/gpu_busy_percent`,
    /// `mem_info_vram_used`, and `mem_info_vram_total`. Other vendors are
    /// unsupported; unavailable metric files remain `None`.
    pub fn new() -> SourceDescriptor<Self> {
        Self::with_root("/sys/class/drm")
    }

    /// Declares a safe sysfs GPU sampler with a configurable root.
    pub fn with_root(root: impl Into<PathBuf>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { root: root.into() })
    }
}

impl Source for GpuSource {
    type Key = PathBuf;
    type Value = GpuState;
    type Error = GpuError;
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
            "bkss-gpu",
            bridge,
            sender,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(root, publisher, cancellation))
            },
        )
        .map_err(|source| GpuError::Io {
            operation: "start GPU worker",
            source,
        })
    }
}

async fn run(
    root: PathBuf,
    publisher: crate::ThreadPublisher<GpuState, GpuError>,
    cancellation: Cancellation,
) -> Result<(), GpuError> {
    loop {
        if publisher.publish(snapshot(&root)?).is_err() {
            return Ok(());
        }
        futures_lite::future::race(
            async {
                async_io::Timer::after(SAMPLE_INTERVAL).await;
            },
            cancellation.cancelled(),
        )
        .await;
        if cancellation.is_cancelled() {
            return Ok(());
        }
    }
}

fn snapshot(root: &Path) -> Result<GpuState, GpuError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(GpuState::default());
        }
        Err(source) => {
            return Err(GpuError::Io {
                operation: "enumerate DRM devices",
                source,
            });
        }
    };
    let mut devices = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| GpuError::Io {
            operation: "enumerate DRM devices",
            source,
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_card(&name) {
            continue;
        }
        let device = entry.path().join("device");
        let vendor_id = read_hex(&device.join("vendor"));
        let device_id = read_hex(&device.join("device"));
        let support = if vendor_id.as_deref() == Some("1002") {
            GpuSupport::Amd
        } else {
            GpuSupport::Unsupported
        };
        let metrics = if support == GpuSupport::Amd {
            amd_metrics(&device)
        } else {
            GpuMetrics {
                utilization_percent: None,
                memory_used_bytes: None,
                memory_total_bytes: None,
            }
        };
        devices.push(Gpu {
            name,
            vendor_id,
            device_id,
            support,
            metrics,
        });
    }
    devices.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(GpuState { devices })
}

fn is_card(name: &str) -> bool {
    name.strip_prefix("card").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn read_hex(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().trim_start_matches("0x").to_ascii_lowercase())
}

fn amd_metrics(device: &Path) -> GpuMetrics {
    let utilization_percent =
        read_u64(&device.join("gpu_busy_percent")).map(|value| value.min(100) as f32);
    let memory_used_bytes = read_u64(&device.join("mem_info_vram_used"));
    let memory_total_bytes = read_u64(&device.join("mem_info_vram_total"));
    GpuMetrics {
        utilization_percent,
        memory_used_bytes,
        memory_total_bytes,
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_supported_amd_metrics_and_keeps_missing_values_absent() {
        let root = std::env::temp_dir().join(format!("bkss-gpu-{}", std::process::id()));
        let device = root.join("card0/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("vendor"), "0x1002\n").unwrap();
        fs::write(device.join("device"), "0x744c\n").unwrap();
        fs::write(device.join("gpu_busy_percent"), "142\n").unwrap();
        fs::write(device.join("mem_info_vram_used"), "2048\n").unwrap();

        let state = snapshot(&root).unwrap();
        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.devices[0].support, GpuSupport::Amd);
        assert_eq!(state.devices[0].metrics.utilization_percent, Some(100.0));
        assert_eq!(state.devices[0].metrics.memory_used_bytes, Some(2048));
        assert_eq!(state.devices[0].metrics.memory_total_bytes, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unsupported_vendor_and_absent_drm_root_are_not_fabricated_metrics() {
        let root =
            std::env::temp_dir().join(format!("bkss-gpu-unsupported-{}", std::process::id()));
        let device = root.join("card2/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("vendor"), "0x10de\n").unwrap();
        let state = snapshot(&root).unwrap();
        assert_eq!(state.devices[0].support, GpuSupport::Unsupported);
        assert_eq!(state.devices[0].metrics.utilization_percent, None);
        fs::remove_dir_all(&root).unwrap();
        assert!(snapshot(&root).unwrap().devices.is_empty());
    }
}

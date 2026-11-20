use bkss_integration::{
    Source, SourceDescriptor, SourcePublisher, ThreadPublisher,
    base::{Cancellation, WorkerGuard, spawn_worker},
};
use gpui::App;
use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use super::newintel::IntelSamplers;
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
    /// AMD (`0x1002`) devices use sysfs metrics. Intel (`0x8086`) i915 devices
    /// use `intel_gpu_top` for busiest-engine utilization. Other drivers and
    /// vendors are unsupported; unavailable metrics remain `None`.
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
    type Guard = WorkerGuard;

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
        spawn_worker(
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
    publisher: ThreadPublisher<GpuState, GpuError>,
    cancellation: Cancellation,
) -> Result<(), GpuError> {
    let mut intel = IntelSamplers::default();
    loop {
        let mut state = snapshot(&root)?;
        intel.sample(&root, &mut state, &cancellation).await;
        if publisher.publish(state).is_err() {
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
        let support = match read_hex(&device.join("vendor")).as_deref() {
            Some("1002") => GpuSupport::Amd,
            Some("8086") if intel_i915(&device) => GpuSupport::Intel,
            _ => GpuSupport::Unsupported,
        };
        let metrics = if support == GpuSupport::Amd {
            amd_metrics(&device)
        } else {
            GpuMetrics {
                utilization_percent: None,
            }
        };
        devices.push(Gpu {
            name,
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

fn intel_i915(device: &Path) -> bool {
    fs::canonicalize(device.join("driver"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name == "i915"))
        .unwrap_or(false)
}

fn amd_metrics(device: &Path) -> GpuMetrics {
    let utilization_percent =
        read_u64(&device.join("gpu_busy_percent")).map(|value| value.min(100) as f32);
    GpuMetrics {
        utilization_percent,
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
        fs::write(device.join("gpu_busy_percent"), "142\n").unwrap();

        let state = snapshot(&root).unwrap();
        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.devices[0].support, GpuSupport::Amd);
        assert_eq!(state.devices[0].metrics.utilization_percent, Some(100.0));
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

    #[test]
    fn supports_intel_i915_but_not_other_intel_drivers() {
        let root = std::env::temp_dir().join(format!("bkss-gpu-intel-{}", std::process::id()));
        let device = root.join("card0/device");
        let driver = root.join("drivers/i915");
        fs::create_dir_all(&device).unwrap();
        fs::create_dir_all(&driver).unwrap();
        fs::write(device.join("vendor"), "0x8086\n").unwrap();
        std::os::unix::fs::symlink(&driver, device.join("driver")).unwrap();
        let state = snapshot(&root).unwrap();
        assert_eq!(state.devices[0].support, GpuSupport::Intel);
        assert_eq!(state.devices[0].metrics.utilization_percent, None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn source_descriptor_accepts_a_configured_drm_root() {
        fn accepts_descriptor<S: Source>(_: &SourceDescriptor<S>) {}

        let root = PathBuf::from("/tmp/drm-fixture");
        let source = GpuSource::with_root(&root);
        accepts_descriptor(&source);
    }
}

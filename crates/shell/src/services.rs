use std::{
    io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use crate::sources::{GpuSource, WeatherSource};
use bkss_core::SourceDescriptor;
use bkss_integration::{
    Audio, BacklightSource, BatterySource, Bluetooth, Clock, ClockPrecision, CommandIpc, Hyprland,
    Notifications, NotificationsEndpoint, Tray, UsageKind, UsageSource, Wifi,
};

/// Inert source declarations used by the UI; mounting starts their integrations.
pub struct Services {
    pub hyprland_socket_dir: PathBuf,
    pub powermenu_socket: PathBuf,
    pub clock: SourceDescriptor<Clock>,
    pub hyprland: SourceDescriptor<Hyprland>,
    pub cpu: SourceDescriptor<UsageSource>,
    pub gpu: SourceDescriptor<GpuSource>,
    pub memory: SourceDescriptor<UsageSource>,
    pub storage: SourceDescriptor<UsageSource>,
    pub battery: SourceDescriptor<BatterySource>,
    pub wifi: SourceDescriptor<Wifi>,
    pub bluetooth: SourceDescriptor<Bluetooth>,
    pub backlight: SourceDescriptor<BacklightSource>,
    pub audio: SourceDescriptor<Audio>,
    pub tray: SourceDescriptor<Tray>,
    pub notifications: SourceDescriptor<Notifications>,
    pub power_menu_requests: SourceDescriptor<CommandIpc>,
    pub weather: SourceDescriptor<WeatherSource>,
}

impl Services {
    pub fn new() -> io::Result<Self> {
        let runtime_dir = runtime_dir()?;
        let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "HYPRLAND_INSTANCE_SIGNATURE is unset",
                )
            })?;
        let signature = Path::new(&signature);
        if !matches!(
            signature.components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HYPRLAND_INSTANCE_SIGNATURE must be a single path component",
            ));
        }
        let socket_dir = runtime_dir.join("hypr").join(signature);
        let powermenu_socket = runtime_dir.join("yshell.sock");

        Ok(Self {
            hyprland_socket_dir: socket_dir.clone(),
            powermenu_socket: powermenu_socket.clone(),
            clock: SourceDescriptor::new(Clock::new(ClockPrecision::Second)),
            hyprland: Hyprland::new(socket_dir),
            cpu: UsageSource::new(UsageKind::Cpu),
            gpu: GpuSource::new(),
            memory: UsageSource::new(UsageKind::Memory),
            storage: UsageSource::new(UsageKind::Storage),
            battery: BatterySource::new(),
            wifi: Wifi::new(),
            bluetooth: Bluetooth::new(),
            backlight: BacklightSource::new(),
            audio: Audio::new(),
            tray: Tray::new(),
            notifications: Notifications::with_default_timeout(
                NotificationsEndpoint::Session,
                Duration::ZERO,
            ),
            power_menu_requests: CommandIpc::new(&runtime_dir, "yshell.sock", ["toggle"])?,
            weather: WeatherSource::new(),
        })
    }
}

pub(crate) fn runtime_dir() -> io::Result<PathBuf> {
    let value = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unset"))?;
    let path = PathBuf::from(value);
    validate_runtime_dir(&path)
}

fn validate_runtime_dir(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XDG_RUNTIME_DIR must be an absolute path",
        ));
    }
    let path = std::fs::canonicalize(path)?;
    let metadata = std::fs::metadata(&path)?;
    let uid = std::fs::metadata("/proc/self")?.uid();
    if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "XDG_RUNTIME_DIR must be owned by the current user with mode 0700",
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("yshell-runtime-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn validates_absolute_private_runtime_directories() {
        assert_eq!(
            validate_runtime_dir(Path::new("relative"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let dir = temp_dir();
        std::fs::create_dir(&dir).expect("create runtime fixture");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("set private mode");
        assert_eq!(validate_runtime_dir(&dir).expect("valid runtime dir"), dir);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("set public mode");
        assert_eq!(
            validate_runtime_dir(&dir).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(dir).expect("remove runtime fixture");
    }
}

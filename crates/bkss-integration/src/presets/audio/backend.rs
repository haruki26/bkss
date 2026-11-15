use super::state::{AudioDevice, AudioDeviceState};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Snapshot {
    pub default_sink: Option<AudioDeviceState>,
    pub default_source: Option<AudioDeviceState>,
    pub sinks: Vec<AudioDevice>,
    pub sources: Vec<AudioDevice>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct RawDevice {
    pub name: String,
    pub description: String,
    pub muted: bool,
    pub volume: Vec<u32>,
}

pub(super) fn snapshot(
    default_sink: Option<String>,
    default_source: Option<String>,
    sinks: Vec<RawDevice>,
    sources: Vec<RawDevice>,
) -> Snapshot {
    let selected = |name: Option<String>, devices: &[RawDevice]| {
        devices
            .iter()
            .find(|device| Some(&device.name) == name.as_ref())
            .and_then(|device| {
                self::device(
                    Some(&device.name),
                    Some(&device.description),
                    &device.volume,
                    libpulse_binding::volume::Volume::NORMAL.0,
                    device.muted,
                )
            })
    };
    Snapshot {
        default_sink: selected(default_sink, &sinks),
        default_source: selected(default_source, &sources),
        sinks: sinks
            .iter()
            .filter_map(|device| endpoint(Some(&device.name), Some(&device.description)))
            .collect(),
        sources: sources
            .iter()
            .filter_map(|device| endpoint(Some(&device.name), Some(&device.description)))
            .collect(),
    }
}

pub(super) fn device(
    name: Option<&str>,
    description: Option<&str>,
    volume: &[u32],
    normal_volume: u32,
    muted: bool,
) -> Option<AudioDeviceState> {
    let name = name?.to_owned();
    Some(AudioDeviceState {
        name,
        description: description.unwrap_or_default().to_owned(),
        volume: volume
            .iter()
            .map(|value| *value as f32 / normal_volume as f32)
            .collect(),
        muted,
    })
}

pub(super) fn endpoint(name: Option<&str>, description: Option<&str>) -> Option<AudioDevice> {
    Some(AudioDevice {
        name: name?.to_owned(),
        description: description.unwrap_or_default().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_server_has_no_default_endpoints() {
        let snapshot = Snapshot::default();
        assert!(snapshot.default_sink.is_none());
        assert!(snapshot.default_source.is_none());
        assert!(snapshot.sinks.is_empty());
        assert!(snapshot.sources.is_empty());
        assert!(device(None, None, &[], 100, false).is_none());
        assert!(endpoint(None, None).is_none());
    }

    #[test]
    fn preserves_per_channel_volume_mute_and_endpoint_description() {
        let result =
            device(Some("sink"), Some("Speakers"), &[50, 150], 100, true).expect("named device");
        assert_eq!(result.name, "sink");
        assert_eq!(result.description, "Speakers");
        assert_eq!(result.volume, [0.5, 1.5]);
        assert!(result.muted);
    }

    #[test]
    fn event_snapshot_updates_selected_default_and_available_endpoints() {
        let state = snapshot(
            Some("new".into()),
            None,
            vec![RawDevice {
                name: "new".into(),
                description: "USB".into(),
                muted: false,
                volume: vec![libpulse_binding::volume::Volume::NORMAL.0 * 65 / 100],
            }],
            Vec::new(),
        );
        assert_eq!(
            state
                .default_sink
                .as_ref()
                .map(|device| device.name.as_str()),
            Some("new")
        );
        assert!(
            state
                .default_sink
                .as_ref()
                .is_some_and(|device| (device.volume[0] - 0.65).abs() < 0.001)
        );
        assert_eq!(state.sinks[0].description, "USB");
        assert!(state.default_source.is_none());
        assert!(state.sources.is_empty());
    }
}

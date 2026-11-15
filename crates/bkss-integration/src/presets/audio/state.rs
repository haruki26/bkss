use super::source::AudioControl;

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDeviceState {
    pub name: String,
    pub description: String,
    pub volume: Vec<f32>,
    pub muted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
}

#[derive(Clone)]
pub struct AudioState {
    pub default_sink: Option<AudioDeviceState>,
    pub default_source: Option<AudioDeviceState>,
    pub available_sinks: Vec<AudioDevice>,
    pub available_sources: Vec<AudioDevice>,
    pub control: AudioControl,
}

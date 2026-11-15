use crate::{DispatchError, Operation};

use super::source::{BacklightError, SetBrightness};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Backlight {
    pub name: String,
    pub brightness: u64,
    pub maximum: u64,
}

#[derive(Clone)]
pub struct BacklightState {
    pub devices: Vec<Backlight>,
    pub(super) commands: crate::base::command::CommandHandle<SetBrightness, (), BacklightError>,
}

impl BacklightState {
    /// Sets a device's raw sysfs brightness asynchronously.
    ///
    /// The operation is admitted only while its source activation is alive.
    pub fn set_brightness(
        &self,
        device: impl Into<String>,
        brightness: u64,
    ) -> Result<Operation<(), BacklightError>, DispatchError> {
        self.commands.dispatch(SetBrightness {
            device: device.into(),
            brightness,
        })
    }
}

use super::error::WifiError;
use crate::base::{Responder, Secret, command::CommandHandle};
use std::{fmt, sync::Arc};

/// Immutable Wi-Fi state reported by NetworkManager.
#[derive(Clone)]
pub struct WifiState {
    pub networking_enabled: bool,
    pub radio_enabled: bool,
    pub devices: Vec<WifiDevice>,
    pub saved_connections: Vec<WifiSavedConnection>,
    pub active_connection: Option<String>,
    pub operations: WifiOperations,
    pub(crate) auth_requests: AuthRequests,
}

impl fmt::Debug for WifiState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiState")
            .field("networking_enabled", &self.networking_enabled)
            .field("radio_enabled", &self.radio_enabled)
            .field("devices", &self.devices)
            .field("saved_connections", &self.saved_connections)
            .field("active_connection", &self.active_connection)
            .field("operations", &self.operations)
            .finish_non_exhaustive()
    }
}

impl PartialEq for WifiState {
    fn eq(&self, other: &Self) -> bool {
        self.networking_enabled == other.networking_enabled
            && self.radio_enabled == other.radio_enabled
            && self.devices == other.devices
            && self.saved_connections == other.saved_connections
            && self.active_connection == other.active_connection
    }
}

impl Eq for WifiState {}

impl WifiState {
    /// Waits for the next credential request from NetworkManager.
    pub async fn next_auth_request(&self) -> Result<WifiAuthRequest, async_channel::RecvError> {
        self.auth_requests.recv().await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiDevice {
    pub path: String,
    pub interface: String,
    pub state: WifiDeviceState,
    pub access_points: Vec<WifiAccessPoint>,
    pub active_access_point: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiSavedConnection {
    pub path: String,
    pub id: String,
    pub ssid: Option<Arc<[u8]>>,
    pub display_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiDeviceState {
    Unknown,
    Unmanaged,
    Unavailable,
    Disconnected,
    Preparing,
    Configuring,
    NeedAuthentication,
    IpConfig,
    IpCheck,
    Secondaries,
    Activated,
    Deactivating,
    Failed,
    Other(u32),
}

impl From<u32> for WifiDeviceState {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::Unknown,
            10 => Self::Unmanaged,
            20 => Self::Unavailable,
            30 => Self::Disconnected,
            40 => Self::Preparing,
            50 => Self::Configuring,
            60 => Self::NeedAuthentication,
            70 => Self::IpConfig,
            80 => Self::IpCheck,
            90 => Self::Secondaries,
            100 => Self::Activated,
            110 => Self::Deactivating,
            120 => Self::Failed,
            other => Self::Other(other),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiAccessPoint {
    pub path: String,
    /// SSID octets are retained verbatim; callers must not assume UTF-8.
    pub ssid: Arc<[u8]>,
    /// A safe, lossy presentation of `ssid`.
    pub display_name: String,
    pub strength: u8,
    pub frequency_mhz: u32,
    pub security: WifiSecurity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSecurity {
    Open,
    Wep,
    Wpa,
    Wpa2,
    Wpa3,
    Unknown,
}

pub(crate) fn ssid_display(bytes: &[u8]) -> String {
    let mut display = String::new();
    for character in String::from_utf8_lossy(bytes).chars() {
        if character.is_control() {
            display.extend(character.escape_default());
        } else {
            display.push(character);
        }
    }
    display
}

/// Cloneable, bounded operations available only while a source is active.
#[derive(Clone)]
pub struct WifiOperations {
    commands: CommandHandle<WifiCommand, (), WifiError>,
}

impl WifiOperations {
    pub(crate) fn new(commands: CommandHandle<WifiCommand, (), WifiError>) -> Self {
        Self { commands }
    }

    pub fn enable(
        &self,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.dispatch(WifiCommand::Radio(true))
    }

    pub fn disable(
        &self,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.dispatch(WifiCommand::Radio(false))
    }

    /// Connects using a nonpersistent profile for the selected access-point path.
    pub fn connect(
        &self,
        access_point: impl Into<String>,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.dispatch(WifiCommand::Connect(access_point.into()))
    }

    /// Activates a saved NetworkManager connection object path.
    pub fn connect_saved(
        &self,
        connection: impl Into<String>,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.dispatch(WifiCommand::ConnectSaved(connection.into()))
    }

    /// Disconnects the specified NetworkManager Wi-Fi device object path.
    pub fn disconnect(
        &self,
        device: impl Into<String>,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.dispatch(WifiCommand::Disconnect(device.into()))
    }

    fn dispatch(
        &self,
        command: WifiCommand,
    ) -> Result<crate::base::Operation<(), WifiError>, crate::base::DispatchError> {
        self.commands.dispatch(command)
    }
}

impl fmt::Debug for WifiOperations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WifiOperations")
    }
}

/// Sanitized information useful for choosing credentials; contains no settings map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WifiAuthContext {
    pub ssid: Vec<u8>,
    pub display_name: String,
    pub security: WifiSecurity,
    pub required_fields: Vec<WifiCredentialField>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiCredentialField {
    Password,
    Identity,
    Username,
}

/// A one-shot request exposed by an active source when NetworkManager needs credentials.
pub struct WifiAuthRequest {
    pub context: WifiAuthContext,
    pub responder: Responder<WifiCredentials>,
}

impl WifiAuthRequest {
    /// Cancels this pending credential prompt; repeated calls are harmless.
    ///
    /// This cannot revoke credentials already delivered to NetworkManager.
    pub fn cancel(&self) {
        self.responder.invalidate();
    }
}

impl fmt::Debug for WifiAuthRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiAuthRequest")
            .field("context", &self.context)
            .field("responder", &"[one-shot]")
            .finish()
    }
}

/// Move-only credentials. Debug formatting is redacted by `Secret`.
pub struct WifiCredentials {
    pub password: Option<Secret<String>>,
    pub identity: Option<Secret<String>>,
    pub username: Option<Secret<String>>,
}

impl fmt::Debug for WifiCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiCredentials")
            .field("password", &self.password)
            .field("identity", &self.identity)
            .field("username", &self.username)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WifiCommand {
    Radio(bool),
    Connect(String),
    ConnectSaved(String),
    Disconnect(String),
}

#[derive(Clone)]
pub(crate) struct AuthRequests(async_channel::Receiver<WifiAuthRequest>);

impl AuthRequests {
    pub(crate) fn new(receiver: async_channel::Receiver<WifiAuthRequest>) -> Self {
        Self(receiver)
    }

    pub(crate) async fn recv(&self) -> Result<WifiAuthRequest, async_channel::RecvError> {
        self.0.recv().await
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&self) -> Result<WifiAuthRequest, async_channel::TryRecvError> {
        self.0.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::{WifiCommand, WifiDeviceState, WifiOperations};
    use crate::base::{DispatchError, command, worker};
    use std::num::NonZeroUsize;

    #[test]
    fn networkmanager_device_state_values_and_unknown_ranges_are_typed() {
        assert_eq!(WifiDeviceState::from(30), WifiDeviceState::Disconnected);
        assert_eq!(WifiDeviceState::from(100), WifiDeviceState::Activated);
        assert_eq!(WifiDeviceState::from(119), WifiDeviceState::Other(119));
        assert_eq!(
            WifiDeviceState::from(u32::MAX),
            WifiDeviceState::Other(u32::MAX)
        );
    }

    #[test]
    fn dropped_source_deactivates_cloned_operations_immediately() {
        let (cancel_sender, cancellation) = worker::cancellation_channel();
        let (commands, _receiver) = command::channel::<
            WifiCommand,
            (),
            super::super::error::WifiError,
        >(NonZeroUsize::MIN, cancellation);
        let operations = WifiOperations::new(commands);

        drop(cancel_sender);
        assert_eq!(operations.enable().err(), Some(DispatchError::Inactive));
    }
}

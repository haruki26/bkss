use std::{collections::HashMap, fmt, num::NonZeroUsize};

use crate::{
    Operation,
    base::{command, command::CommandHandle},
};
use zbus::{
    Connection,
    names::OwnedInterfaceName,
    zvariant::{OwnedObjectPath, OwnedValue},
};

use super::agent::{AgentStore, BluetoothAgentDisplay, BluetoothAgentRequest};
use super::{BluetoothError, backend};

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothAdapter {
    pub path: String,
    pub address: Option<String>,
    pub name: Option<String>,
    pub powered: bool,
    pub discoverable: bool,
    pub discovering: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothDevice {
    pub path: String,
    pub adapter: Option<String>,
    pub address: Option<String>,
    pub name: Option<String>,
    pub paired: bool,
    pub connected: bool,
    pub trusted: bool,
}

#[derive(Clone, Default)]
pub struct BluetoothState {
    pub adapters: Vec<BluetoothAdapter>,
    pub devices: Vec<BluetoothDevice>,
    pub authentication_requests: Vec<BluetoothAgentRequest>,
    pub agent_displays: Vec<BluetoothAgentDisplay>,
    pub(crate) commands: Option<BluetoothCommandClient>,
}

impl fmt::Debug for BluetoothState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BluetoothState")
            .field("adapters", &self.adapters)
            .field("devices", &self.devices)
            .finish()
    }
}

impl PartialEq for BluetoothState {
    fn eq(&self, other: &Self) -> bool {
        self.adapters == other.adapters
            && self.devices == other.devices
            && self
                .authentication_requests
                .iter()
                .map(BluetoothAgentRequest::id)
                .eq(other
                    .authentication_requests
                    .iter()
                    .map(BluetoothAgentRequest::id))
            && self
                .agent_displays
                .iter()
                .map(BluetoothAgentDisplay::id)
                .eq(other.agent_displays.iter().map(BluetoothAgentDisplay::id))
    }
}

impl Eq for BluetoothState {}

impl BluetoothState {
    pub fn set_powered(
        &self,
        adapter: impl Into<String>,
        powered: bool,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::SetPowered(adapter.into(), powered))
    }

    pub fn set_discoverable(
        &self,
        adapter: impl Into<String>,
        discoverable: bool,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::SetDiscoverable(
            adapter.into(),
            discoverable,
        ))
    }

    pub fn start_discovery(
        &self,
        adapter: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Discovery(adapter.into(), true))
    }

    pub fn stop_discovery(
        &self,
        adapter: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Discovery(adapter.into(), false))
    }

    pub fn connect(
        &self,
        device: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Device(
            device.into(),
            DeviceAction::Connect,
        ))
    }

    pub fn disconnect(
        &self,
        device: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Device(
            device.into(),
            DeviceAction::Disconnect,
        ))
    }

    pub fn pair(
        &self,
        device: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Device(device.into(), DeviceAction::Pair))
    }

    pub fn cancel_pairing(
        &self,
        device: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::Device(
            device.into(),
            DeviceAction::CancelPairing,
        ))
    }

    pub fn remove_device(
        &self,
        adapter: impl Into<String>,
        device: impl Into<String>,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.dispatch(BluetoothCommand::RemoveDevice(
            adapter.into(),
            device.into(),
        ))
    }

    fn dispatch(
        &self,
        request: BluetoothCommand,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        match &self.commands {
            Some(commands) => commands.dispatch(request),
            None => Err(crate::DispatchError::Inactive),
        }
    }
}

#[derive(Debug)]
pub(crate) enum BluetoothCommand {
    SetPowered(String, bool),
    SetDiscoverable(String, bool),
    Discovery(String, bool),
    Device(String, DeviceAction),
    RemoveDevice(String, String),
}

#[derive(Clone)]
pub(crate) struct BluetoothCommandClient {
    commands: CommandHandle<BluetoothCommand, (), BluetoothError>,
}

impl BluetoothCommandClient {
    fn dispatch(
        &self,
        command: BluetoothCommand,
    ) -> Result<Operation<(), BluetoothError>, crate::DispatchError> {
        self.commands.dispatch(command)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DeviceAction {
    Connect,
    Disconnect,
    Pair,
    CancelPairing,
}

pub(super) fn map_objects(
    objects: HashMap<OwnedObjectPath, HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>>,
    commands: Option<BluetoothCommandClient>,
) -> Result<BluetoothState, BluetoothError> {
    let mut state = BluetoothState {
        commands,
        ..BluetoothState::default()
    };
    for (object_path, interfaces) in objects {
        let path = object_path.to_string();
        if !valid_path(&path) {
            return Err(BluetoothError::InvalidObjectPath(path));
        }
        if let Some(properties) = interface_properties(&interfaces, ADAPTER) {
            state.adapters.push(BluetoothAdapter {
                path: path.clone(),
                address: optional_string(properties, ADAPTER, "Address")?,
                name: optional_string(properties, ADAPTER, "Name")?,
                powered: optional_bool(properties, ADAPTER, "Powered")?.unwrap_or(false),
                discoverable: optional_bool(properties, ADAPTER, "Discoverable")?.unwrap_or(false),
                discovering: optional_bool(properties, ADAPTER, "Discovering")?.unwrap_or(false),
            });
        }
        if let Some(properties) = interface_properties(&interfaces, DEVICE) {
            let adapter = optional_path(properties, DEVICE, "Adapter")?;
            state.devices.push(BluetoothDevice {
                path: path.clone(),
                adapter,
                address: optional_string(properties, DEVICE, "Address")?,
                name: optional_string(properties, DEVICE, "Name")?,
                paired: optional_bool(properties, DEVICE, "Paired")?.unwrap_or(false),
                connected: optional_bool(properties, DEVICE, "Connected")?.unwrap_or(false),
                trusted: optional_bool(properties, DEVICE, "Trusted")?.unwrap_or(false),
            });
        }
    }
    state.adapters.sort_by(|a, b| a.path.cmp(&b.path));
    state.devices.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(state)
}

pub(super) fn attach_agent_state(state: &mut BluetoothState, agent: &AgentStore) {
    state.authentication_requests = agent.requests();
    state.agent_displays = agent.displays();
}

fn interface_properties<'a>(
    interfaces: &'a HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
    wanted: &str,
) -> Option<&'a HashMap<String, OwnedValue>> {
    interfaces
        .iter()
        .find(|(name, _)| name.as_str() == wanted)
        .map(|(_, properties)| properties)
}

fn valid_path(path: &str) -> bool {
    zbus::zvariant::ObjectPath::try_from(path).is_ok()
}

fn optional_string(
    properties: &HashMap<String, OwnedValue>,
    interface: &'static str,
    property: &'static str,
) -> Result<Option<String>, BluetoothError> {
    properties
        .get(property)
        .map(|value| {
            String::try_from(value.clone()).map_err(|_| BluetoothError::InvalidProperty {
                interface,
                property,
            })
        })
        .transpose()
}

fn optional_bool(
    properties: &HashMap<String, OwnedValue>,
    interface: &'static str,
    property: &'static str,
) -> Result<Option<bool>, BluetoothError> {
    properties
        .get(property)
        .map(|value| {
            bool::try_from(value).map_err(|_| BluetoothError::InvalidProperty {
                interface,
                property,
            })
        })
        .transpose()
}

fn optional_path(
    properties: &HashMap<String, OwnedValue>,
    interface: &'static str,
    property: &'static str,
) -> Result<Option<String>, BluetoothError> {
    properties
        .get(property)
        .map(|value| {
            OwnedObjectPath::try_from(value.clone())
                .map(|path| path.to_string())
                .map_err(|_| BluetoothError::InvalidProperty {
                    interface,
                    property,
                })
        })
        .transpose()
}

pub(super) async fn execute(
    connection: &Connection,
    command: BluetoothCommand,
) -> Result<(), BluetoothError> {
    match command {
        BluetoothCommand::SetPowered(path, value) => {
            backend::set_property(connection, &path, ADAPTER, "Powered", value).await
        }
        BluetoothCommand::SetDiscoverable(path, value) => {
            backend::set_property(connection, &path, ADAPTER, "Discoverable", value).await
        }
        BluetoothCommand::Discovery(path, start) => {
            backend::call(
                connection,
                &path,
                ADAPTER,
                if start {
                    "StartDiscovery"
                } else {
                    "StopDiscovery"
                },
            )
            .await
        }
        BluetoothCommand::Device(path, action) => {
            let method = match action {
                DeviceAction::Connect => "Connect",
                DeviceAction::Disconnect => "Disconnect",
                DeviceAction::Pair => "Pair",
                DeviceAction::CancelPairing => "CancelPairing",
            };
            backend::call(connection, &path, DEVICE, method).await
        }
        BluetoothCommand::RemoveDevice(adapter, device) => {
            backend::call_with_path(connection, &adapter, ADAPTER, "RemoveDevice", device).await
        }
    }
}

pub(super) fn command_channel(
    cancellation: crate::base::worker::Cancellation,
) -> (
    BluetoothCommandClient,
    command::CommandReceiver<BluetoothCommand, (), BluetoothError>,
) {
    let (commands, receiver) = command::channel(NonZeroUsize::MIN, cancellation);
    (BluetoothCommandClient { commands }, receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(
        path: &str,
        interfaces: HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
    ) -> (
        OwnedObjectPath,
        HashMap<OwnedInterfaceName, HashMap<String, OwnedValue>>,
    ) {
        (OwnedObjectPath::try_from(path).unwrap(), interfaces)
    }

    #[test]
    fn absent_optional_properties_map_to_defaults_and_unknown_interfaces_are_ignored() {
        let objects = HashMap::from([
            object(
                "/org/bluez/hci0",
                HashMap::from([(interface(ADAPTER), HashMap::new())]),
            ),
            object(
                "/org/bluez/hci0/dev_AA",
                HashMap::from([(interface(DEVICE), HashMap::new())]),
            ),
            object(
                "/ignored",
                HashMap::from([(interface("org.example.Other"), HashMap::new())]),
            ),
        ]);
        let state = map_objects(objects, None).expect("valid BlueZ objects");
        assert_eq!(state.adapters.len(), 1);
        assert!(!state.adapters[0].powered);
        assert_eq!(state.devices.len(), 1);
        assert!(!state.devices[0].connected);
    }

    #[test]
    fn snapshots_include_hotplugged_devices_and_reflect_removal() {
        let adapter = object(
            "/org/bluez/hci0",
            HashMap::from([(interface(ADAPTER), HashMap::new())]),
        );
        let device = object(
            "/org/bluez/hci0/dev_AA",
            HashMap::from([(interface(DEVICE), HashMap::new())]),
        );
        let initial =
            map_objects(HashMap::from([adapter.clone()]), None).expect("initial snapshot");
        assert!(initial.devices.is_empty());

        let added =
            map_objects(HashMap::from([adapter.clone(), device]), None).expect("added snapshot");
        assert_eq!(added.devices.len(), 1);
        assert_eq!(added.devices[0].path, "/org/bluez/hci0/dev_AA");

        let removed = map_objects(HashMap::from([adapter]), None).expect("removed snapshot");
        assert!(removed.devices.is_empty());
    }

    #[test]
    fn adapter_boolean_properties_are_mapped_and_wrong_types_fail() {
        let properties = HashMap::from([
            ("Powered".into(), OwnedValue::from(true)),
            ("Discovering".into(), OwnedValue::from(true)),
        ]);
        let state = map_objects(
            HashMap::from([object(
                "/org/bluez/hci0",
                HashMap::from([(interface(ADAPTER), properties)]),
            )]),
            None,
        )
        .expect("valid adapter properties");
        assert!(state.adapters[0].powered);
        assert!(state.adapters[0].discovering);
        assert!(!state.adapters[0].discoverable);

        let invalid = HashMap::from([("Powered".into(), OwnedValue::from(7_u8))]);
        assert!(matches!(
            map_objects(
                HashMap::from([object(
                    "/org/bluez/hci0",
                    HashMap::from([(interface(ADAPTER), invalid)]),
                )]),
                None,
            ),
            Err(BluetoothError::InvalidProperty {
                property: "Powered",
                ..
            })
        ));
    }

    #[test]
    fn invalid_object_paths_are_rejected() {
        assert!(!valid_path("/org/bluez/hci0/"));
        assert!(!valid_path("org/bluez/hci0"));
        assert!(valid_path("/org/bluez/hci0"));
    }

    fn interface(name: &str) -> OwnedInterfaceName {
        OwnedInterfaceName::try_from(name).expect("valid interface name")
    }
}

use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::worker::{self, Cancellation},
};
use futures_lite::StreamExt;
use gpui::App;
use std::{error::Error, fmt, future::Future};
use zbus::{
    Connection, MatchRule, Message, MessageStream, Proxy, message::Type, zvariant::OwnedObjectPath,
};

use super::state::{Battery, BatteryState, ChargeState};

const SERVICE: &str = "org.freedesktop.UPower";
const ROOT: &str = "/org/freedesktop/UPower";
const ROOT_INTERFACE: &str = "org.freedesktop.UPower";
const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";

#[derive(Debug)]
pub enum BatteryError {
    Dbus(Box<zbus::Error>),
    Worker(std::io::Error),
    SignalClosed(&'static str),
    ServiceLost,
}

impl fmt::Display for BatteryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dbus(source) => write!(f, "UPower D-Bus operation failed: {source}"),
            Self::Worker(source) => write!(f, "start UPower worker: {source}"),
            Self::SignalClosed(signal) => {
                write!(f, "UPower {signal} signal stream closed unexpectedly")
            }
            Self::ServiceLost => f.write_str("UPower service lost its D-Bus name"),
        }
    }
}

impl Error for BatteryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Dbus(source) => Some(source.as_ref()),
            Self::Worker(source) => Some(source),
            Self::SignalClosed(_) => None,
            Self::ServiceLost => None,
        }
    }
}

impl From<zbus::Error> for BatteryError {
    fn from(source: zbus::Error) -> Self {
        Self::Dbus(Box::new(source))
    }
}

/// Selects the UPower bus endpoint; `None` uses the system bus.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct BatteryEndpoint(Option<String>);

impl BatteryEndpoint {
    pub fn system() -> Self {
        Self(None)
    }

    /// Connects to a D-Bus address, primarily for isolated fake-service tests.
    pub fn address(address: impl Into<String>) -> Self {
        Self(Some(address.into()))
    }
}

pub struct BatterySource {
    endpoint: BatteryEndpoint,
}

impl BatterySource {
    /// Declares a UPower source on the system bus.
    pub fn new() -> SourceDescriptor<Self> {
        Self::with_endpoint(BatteryEndpoint::system())
    }

    /// Declares a UPower source on the selected system bus or private bus address.
    pub fn with_endpoint(endpoint: BatteryEndpoint) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { endpoint })
    }
}

impl Source for BatterySource {
    type Key = BatteryEndpoint;
    type Value = BatteryState;
    type Error = BatteryError;
    type Guard = worker::WorkerGuard;

    fn key(&self) -> Self::Key {
        self.endpoint.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let endpoint = self.endpoint.clone();
        worker::spawn_worker(
            "bkss-battery",
            bridge,
            sender,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(endpoint, publisher, cancellation))
            },
        )
        .map_err(BatteryError::Worker)
    }
}

async fn connect(endpoint: &BatteryEndpoint) -> Result<Connection, BatteryError> {
    match &endpoint.0 {
        Some(address) => zbus::connection::Builder::address(address.as_str())
            .map_err(BatteryError::from)?
            .build()
            .await
            .map_err(BatteryError::from),
        None => Connection::system().await.map_err(BatteryError::from),
    }
}

async fn run(
    endpoint: BatteryEndpoint,
    publisher: crate::ThreadPublisher<BatteryState, BatteryError>,
    cancellation: Cancellation,
) -> Result<(), BatteryError> {
    let connection = match futures_lite::future::race(
        async { Init::Connected(connect(&endpoint).await) },
        async {
            cancellation.cancelled().await;
            Init::Cancelled
        },
    )
    .await
    {
        Init::Connected(connection) => connection?,
        Init::Cancelled => return Ok(()),
    };
    let proxy = match cancellable(
        &cancellation,
        Proxy::new(&connection, SERVICE, ROOT, ROOT_INTERFACE),
    )
    .await
    {
        Some(result) => result.map_err(BatteryError::from)?,
        None => return Ok(()),
    };

    // Install device lifecycle subscriptions before the initial enumeration.
    let mut added = match cancellable(&cancellation, proxy.receive_signal("DeviceAdded")).await {
        Some(result) => result.map_err(BatteryError::from)?,
        None => return Ok(()),
    };
    let mut removed = match cancellable(&cancellation, proxy.receive_signal("DeviceRemoved")).await
    {
        Some(result) => result.map_err(BatteryError::from)?,
        None => return Ok(()),
    };
    let properties_rule = properties_changed_rule()?;
    let mut properties = match cancellable(
        &cancellation,
        MessageStream::for_match_rule(properties_rule, &connection, Some(32)),
    )
    .await
    {
        Some(result) => result.map_err(BatteryError::from)?,
        None => return Ok(()),
    };
    let owner_rule = owner_changed_rule()?;
    let mut owner = match cancellable(
        &cancellation,
        MessageStream::for_match_rule(owner_rule, &connection, Some(4)),
    )
    .await
    {
        Some(result) => result.map_err(BatteryError::from)?,
        None => return Ok(()),
    };
    loop {
        let state = match cancellable(&cancellation, snapshot(&connection, &proxy)).await {
            Some(result) => result?,
            None => return Ok(()),
        };
        if publisher.publish(state).is_err() {
            return Ok(());
        }
        let event = futures_lite::future::race(
            async {
                if added.next().await.is_some() {
                    Event::Refresh
                } else {
                    Event::Closed("DeviceAdded")
                }
            },
            futures_lite::future::race(
                async {
                    if removed.next().await.is_some() {
                        Event::Refresh
                    } else {
                        Event::Closed("DeviceRemoved")
                    }
                },
                futures_lite::future::race(
                    async {
                        match properties.next().await {
                            Some(Ok(_)) => Event::Refresh,
                            Some(Err(error)) => Event::Failure(BatteryError::from(error)),
                            None => Event::Closed("PropertiesChanged"),
                        }
                    },
                    async {
                        match owner.next().await {
                            Some(Ok(message)) => owner_change(message),
                            Some(Err(error)) => Event::Failure(BatteryError::from(error)),
                            None => Event::Closed("NameOwnerChanged"),
                        }
                    },
                ),
            ),
        );
        match futures_lite::future::race(event, async {
            cancellation.cancelled().await;
            Event::Cancelled
        })
        .await
        {
            Event::Closed(signal) => return Err(BatteryError::SignalClosed(signal)),
            Event::Cancelled => return Ok(()),
            Event::Refresh => {}
            Event::Failure(error) => return Err(error),
            Event::ServiceLost => return Err(BatteryError::ServiceLost),
        }
    }
}

async fn cancellable<T>(cancellation: &Cancellation, future: impl Future<Output = T>) -> Option<T> {
    futures_lite::future::race(async { Some(future.await) }, async {
        cancellation.cancelled().await;
        None
    })
    .await
}

enum Event {
    Refresh,
    Closed(&'static str),
    Cancelled,
    Failure(BatteryError),
    ServiceLost,
}

fn properties_changed_rule() -> Result<zbus::OwnedMatchRule, BatteryError> {
    MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(SERVICE)
        .map_err(BatteryError::from)?
        .interface("org.freedesktop.DBus.Properties")
        .map_err(BatteryError::from)?
        .member("PropertiesChanged")
        .map_err(BatteryError::from)?
        .path_namespace(ROOT)
        .map_err(BatteryError::from)
        .map(|rule| rule.build().to_owned().into())
}

fn owner_changed_rule() -> Result<zbus::OwnedMatchRule, BatteryError> {
    MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")
        .map_err(BatteryError::from)?
        .path("/org/freedesktop/DBus")
        .map_err(BatteryError::from)?
        .interface("org.freedesktop.DBus")
        .map_err(BatteryError::from)?
        .member("NameOwnerChanged")
        .map_err(BatteryError::from)?
        .add_arg(SERVICE)
        .map_err(BatteryError::from)
        .map(|rule| rule.build().to_owned().into())
}

fn owner_change(message: Message) -> Event {
    match message.body().deserialize::<(String, String, String)>() {
        Ok((_, _, new_owner)) if new_owner.is_empty() => Event::ServiceLost,
        Ok(_) => Event::Refresh,
        Err(error) => Event::Failure(BatteryError::from(error)),
    }
}

enum Init {
    Connected(Result<Connection, BatteryError>),
    Cancelled,
}

async fn snapshot(connection: &Connection, root: &Proxy<'_>) -> Result<BatteryState, BatteryError> {
    let paths: Vec<OwnedObjectPath> = root
        .call("EnumerateDevices", &())
        .await
        .map_err(BatteryError::from)?;
    let mut state = BatteryState {
        batteries: Vec::new(),
        on_battery: Some(
            root.get_property("OnBattery")
                .await
                .map_err(BatteryError::from)?,
        ),
    };
    for path in paths {
        match read_battery(connection, &path).await {
            Ok(Some(battery)) => state.batteries.push(battery),
            Ok(None) => {}
            Err(error) if unknown_object(&error) => {}
            Err(error) => return Err(BatteryError::from(error)),
        }
    }
    state
        .batteries
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(state)
}

async fn read_battery(
    connection: &Connection,
    path: &OwnedObjectPath,
) -> Result<Option<Battery>, zbus::Error> {
    let device = Proxy::new(connection, SERVICE, path.as_str(), DEVICE_INTERFACE).await?;
    let kind: u32 = device.get_property("Type").await?;
    let is_power_supply: bool = device.get_property("PowerSupply").await?;
    let present: bool = device.get_property("IsPresent").await?;
    if kind != 2 || !is_power_supply || !present {
        return Ok(None);
    }
    let percentage: f64 = device.get_property("Percentage").await?;
    let raw_state: u32 = device.get_property("State").await?;
    let model: String = device.get_property("Model").await?;
    let time_to_empty: i64 = device.get_property("TimeToEmpty").await?;
    let time_to_full: i64 = device.get_property("TimeToFull").await?;
    Ok(Some(battery_from_properties(
        path.to_string(),
        model,
        percentage,
        raw_state,
        time_to_empty,
        time_to_full,
    )))
}

fn battery_from_properties(
    path: String,
    model: String,
    percentage: f64,
    raw_state: u32,
    time_to_empty: i64,
    time_to_full: i64,
) -> Battery {
    Battery {
        path,
        model: (!model.is_empty()).then_some(model),
        percentage: percentage.clamp(0.0, 100.0),
        state: charge_state(raw_state),
        time_to_empty_seconds: (time_to_empty > 0).then_some(time_to_empty as u64),
        time_to_full_seconds: (time_to_full > 0).then_some(time_to_full as u64),
    }
}

fn unknown_object(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownObject"
    )
}

fn charge_state(value: u32) -> ChargeState {
    match value {
        1 => ChargeState::Charging,
        2 => ChargeState::Discharging,
        3 => ChargeState::Empty,
        4 => ChargeState::FullyCharged,
        5 => ChargeState::PendingCharge,
        6 => ChargeState::PendingDischarge,
        other => ChargeState::Unknown(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_identity_and_charge_state_mapping_are_typed() {
        assert_ne!(
            BatteryEndpoint::system(),
            BatteryEndpoint::address("unix:path=/tmp/fake")
        );
        assert_eq!(charge_state(1), ChargeState::Charging);
        assert_eq!(charge_state(4), ChargeState::FullyCharged);
        assert_eq!(charge_state(99), ChargeState::Unknown(99));
    }

    #[test]
    fn empty_battery_collection_is_a_valid_state() {
        let state = BatteryState::default();
        assert!(state.batteries.is_empty());
        assert_eq!(state.on_battery, None);
    }

    #[test]
    fn subscriptions_match_upower_property_and_owner_signals() {
        let properties = properties_changed_rule().unwrap();
        assert_eq!(properties.sender().unwrap().as_str(), SERVICE);
        assert_eq!(
            properties.interface().unwrap().as_str(),
            "org.freedesktop.DBus.Properties"
        );
        assert_eq!(properties.member().unwrap().as_str(), "PropertiesChanged");
        assert!(properties.path_spec().is_some());

        let owner = owner_changed_rule().unwrap();
        assert_eq!(owner.sender().unwrap().as_str(), "org.freedesktop.DBus");
        assert_eq!(owner.member().unwrap().as_str(), "NameOwnerChanged");
        assert_eq!(owner.args(), &[(0, SERVICE.into())]);
    }

    #[test]
    fn property_only_signal_matches_without_a_root_changed_signal() {
        let properties = properties_changed_rule().unwrap();
        let changed = Message::signal(
            "/org/freedesktop/UPower/devices/battery_BAT0",
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
        )
        .unwrap()
        .sender(":1.42")
        .unwrap()
        .build(&(
            DEVICE_INTERFACE.to_owned(),
            std::collections::HashMap::<String, zbus::zvariant::OwnedValue>::new(),
            vec!["Percentage".to_owned()],
        ))
        .unwrap();
        assert!(properties.matches(&changed).unwrap());

        let root_changed = Message::signal(ROOT, ROOT_INTERFACE, "Changed")
            .unwrap()
            .sender(":1.42")
            .unwrap()
            .build(&())
            .unwrap();
        assert!(!properties.matches(&root_changed).unwrap());
    }

    #[test]
    fn owner_loss_signal_is_a_typed_source_failure() {
        let lost = Message::signal(
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
        )
        .unwrap()
        .sender(":1.1")
        .unwrap()
        .build(&(SERVICE.to_owned(), ":1.42".to_owned(), String::new()))
        .unwrap();
        assert!(matches!(owner_change(lost), Event::ServiceLost));
    }

    #[test]
    fn property_snapshot_converts_percentage_state_and_unknown_times() {
        let battery = battery_from_properties("/battery".into(), "Pack".into(), 112.0, 2, -1, 300);
        assert_eq!(battery.model.as_deref(), Some("Pack"));
        assert_eq!(battery.percentage, 100.0);
        assert_eq!(battery.state, ChargeState::Discharging);
        assert_eq!(battery.time_to_empty_seconds, None);
        assert_eq!(battery.time_to_full_seconds, Some(300));
    }
}

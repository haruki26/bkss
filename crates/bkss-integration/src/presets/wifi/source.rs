use super::{
    agent::{OBJECT_PATH, SecretAgent},
    backend::{self, NM},
    error::WifiError,
    state::{WifiCommand, WifiOperations},
};
use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::{
        command::{self, CommandRequest},
        worker::{self, Cancellation, WorkerGuard},
    },
};
use futures_lite::StreamExt;
use gpui::App;
use std::num::NonZeroUsize;
use zbus::{Connection, MatchRule, MessageStream, connection::Builder, message::Type};

enum Incoming {
    Command(Option<CommandRequest<WifiCommand, (), WifiError>>),
    NetworkManager(Option<zbus::Result<zbus::Message>>),
    Owner(Option<zbus::Result<zbus::Message>>),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Bus {
    System,
    Address(String),
}

/// Lazy NetworkManager-backed Wi-Fi source.
pub struct Wifi {
    bus: Bus,
}

impl Wifi {
    /// Creates a source connected to the system message bus.
    pub fn new() -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { bus: Bus::System })
    }

    /// Creates a source connected to an explicit D-Bus address (also useful for private bus tests).
    pub fn with_address(address: impl Into<String>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            bus: Bus::Address(address.into()),
        })
    }
}

impl Source for Wifi {
    type Key = String;
    type Value = super::state::WifiState;
    type Error = WifiError;
    type Guard = WorkerGuard;

    fn key(&self) -> Self::Key {
        match &self.bus {
            Bus::System => "system".to_owned(),
            Bus::Address(address) => format!("address:{address}"),
        }
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|value| value, |error| error, cx);
        let bus = self.bus.clone();
        worker::spawn_worker(
            "bkss-wifi",
            bridge,
            sender,
            move |publisher, cancellation| {
                let (command_handle, commands) = command::channel::<WifiCommand, (), WifiError>(
                    NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN),
                    cancellation.clone(),
                );
                let operations = WifiOperations::new(command_handle);
                futures_lite::future::block_on(async move {
                    futures_lite::future::race(
                        run(bus, operations, commands, publisher, cancellation.clone()),
                        async {
                            cancellation.cancelled().await;
                            Ok(())
                        },
                    )
                    .await
                })
            },
        )
        .map_err(WifiError::Worker)
    }
}

async fn run(
    bus: Bus,
    operations: WifiOperations,
    commands: command::CommandReceiver<WifiCommand, (), WifiError>,
    publisher: crate::ThreadPublisher<super::state::WifiState, WifiError>,
    cancellation: Cancellation,
) -> Result<(), WifiError> {
    let builder = match bus {
        Bus::System => Builder::system()?,
        Bus::Address(address) => Builder::address(address.as_str())?,
    };
    let connection = builder.build().await?;
    let owner_proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let owner: String = owner_proxy.call("GetNameOwner", &NM).await?;
    let (agent, auth_requests) = SecretAgent::new(owner.clone());
    connection
        .object_server()
        .at(OBJECT_PATH, agent.clone())
        .await
        .map_err(|source| WifiError::Bus(Box::new(source)))?;

    let agent_manager = zbus::Proxy::new(
        &connection,
        NM,
        "/org/freedesktop/NetworkManager/AgentManager",
        "org.freedesktop.NetworkManager.AgentManager",
    )
    .await?;
    agent_manager
        .call::<_, _, ()>("RegisterWithCapabilities", &("bkss-wifi", 0u32))
        .await?;

    // Install the backend and name-owner subscriptions before reading the first snapshot.
    let nm_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(NM)?
        .build();
    let mut nm_events = MessageStream::for_match_rule(nm_rule, &connection, Some(128)).await?;
    let owner_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .arg(0, NM)?
        .build();
    let mut owner_events = MessageStream::for_match_rule(owner_rule, &connection, Some(8)).await?;

    let state = backend::snapshot(&connection, operations.clone(), auth_requests.clone()).await?;
    if publisher.publish(state).is_err() {
        return Ok(());
    }

    loop {
        let next_command = commands.recv();
        let next_nm_event = nm_events.next();
        let next_owner_event = owner_events.next();
        let incoming = futures_lite::future::race(
            futures_lite::future::race(async { Incoming::Command(next_command.await) }, async {
                Incoming::NetworkManager(next_nm_event.await)
            }),
            async { Incoming::Owner(next_owner_event.await) },
        )
        .await;
        match incoming {
            Incoming::Command(Some(request)) => {
                spawn_operation(&connection, request, cancellation.clone());
            }
            Incoming::Command(None) => {
                return Ok(());
            }
            Incoming::NetworkManager(Some(Ok(_))) => {
                let state =
                    backend::snapshot(&connection, operations.clone(), auth_requests.clone())
                        .await?;
                if publisher.publish(state).is_err() {
                    return Ok(());
                }
            }
            Incoming::NetworkManager(Some(Err(error))) => {
                return Err(WifiError::Bus(Box::new(error)));
            }
            Incoming::NetworkManager(None) | Incoming::Owner(None) => {
                return Err(WifiError::Protocol {
                    operation: "monitor NetworkManager",
                    message: "signal subscription ended".into(),
                });
            }
            Incoming::Owner(Some(Ok(message))) => {
                let changed = message
                    .body()
                    .deserialize::<(String, String, String)>()
                    .ok();
                if changed.is_some_and(|(_, old_owner, new_owner)| {
                    old_owner == owner && new_owner.is_empty()
                }) {
                    return Err(WifiError::Protocol {
                        operation: "monitor NetworkManager",
                        message: "NetworkManager service owner disappeared".into(),
                    });
                }
            }
            Incoming::Owner(Some(Err(error))) => {
                return Err(WifiError::Bus(Box::new(error)));
            }
        }
    }
}

fn spawn_operation(
    connection: &Connection,
    request: CommandRequest<WifiCommand, (), WifiError>,
    cancellation: Cancellation,
) {
    let (command, reply) = request.into_parts();
    let executor = connection.executor().clone();
    let connection = connection.clone();
    let operation = async move {
        let result = futures_lite::future::race(
            async {
                match command {
                    WifiCommand::Radio(enabled) => backend::set_radio(&connection, enabled).await,
                    WifiCommand::Connect(path) => backend::connect_new(&connection, &path).await,
                    WifiCommand::ConnectSaved(path) => {
                        backend::connect_saved(&connection, &path).await
                    }
                    WifiCommand::Disconnect(path) => backend::disconnect(&connection, &path).await,
                }
            },
            async {
                cancellation.cancelled().await;
                Err(WifiError::Cancelled)
            },
        )
        .await;
        reply(result);
    };
    executor.spawn(operation, "bkss-wifi-operation").detach();
}

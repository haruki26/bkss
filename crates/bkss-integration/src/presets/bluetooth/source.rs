use std::{future::Future, pin::Pin, sync::Arc};

use futures_lite::{Stream, StreamExt};
use gpui::App;
use zbus::{Connection, MatchRule, MessageStream, message::Type};

use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::{
        command::CommandRequest,
        worker::{self, Cancellation, WorkerGuard},
    },
};

use super::{
    BluetoothError,
    agent::{AgentSession, AgentStore},
    backend,
    error::bus,
    state::{self, BluetoothCommand, BluetoothCommandClient, BluetoothState, command_channel},
};

type BluetoothOperation<'a> = Pin<Box<dyn Future<Output = Result<(), BluetoothError>> + Send + 'a>>;
type BluetoothReply = Box<dyn FnOnce(Result<(), BluetoothError>) + Send>;

/// Normalized bus configuration. An address is useful for isolated private
/// buses in tests; `None` selects the system bus.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BluetoothConfig {
    address: Option<String>,
}

impl BluetoothConfig {
    pub fn system_bus() -> Self {
        Self { address: None }
    }

    pub fn private_bus(address: impl Into<String>) -> Self {
        Self {
            address: Some(address.into()),
        }
    }
}

pub struct Bluetooth {
    config: BluetoothConfig,
}

impl Bluetooth {
    pub fn new() -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            config: BluetoothConfig::system_bus(),
        })
    }

    pub fn with_config(config: BluetoothConfig) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { config })
    }
}

impl Source for Bluetooth {
    type Key = BluetoothConfig;
    type Value = BluetoothState;
    type Error = BluetoothError;
    type Guard = BluetoothGuard;

    fn key(&self) -> Self::Key {
        self.config.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (thread_publisher, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let config = self.config.clone();
        let (agent_store, agent_updates) = AgentStore::new();
        let worker_store = Arc::clone(&agent_store);
        let worker = worker::spawn_worker(
            "bkss-bluetooth",
            bridge,
            thread_publisher,
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(
                    config,
                    worker_store,
                    agent_updates,
                    publisher,
                    cancellation,
                ))
            },
        )
        .map_err(BluetoothError::Worker)?;
        Ok(BluetoothGuard {
            agent_store,
            _worker: worker,
        })
    }
}

pub struct BluetoothGuard {
    agent_store: Arc<AgentStore>,
    _worker: WorkerGuard,
}

impl Drop for BluetoothGuard {
    fn drop(&mut self) {
        self.agent_store.release();
    }
}

async fn run(
    config: BluetoothConfig,
    agent_store: Arc<AgentStore>,
    agent_updates: async_channel::Receiver<()>,
    publisher: crate::ThreadPublisher<BluetoothState, BluetoothError>,
    cancellation: Cancellation,
) -> Result<(), BluetoothError> {
    let connection = cancellable(connect(&config), &cancellation).await?;
    let mut messages = cancellable(subscribe(&connection), &cancellation).await?;
    let mut agent = AgentSession::export(&connection, agent_store).await?;
    let result = run_connected(
        &connection,
        &mut agent,
        agent_updates,
        &mut messages,
        publisher,
        cancellation,
    )
    .await;
    agent.close().await;
    result
}

async fn run_connected(
    connection: &Connection,
    agent: &mut AgentSession,
    agent_updates: async_channel::Receiver<()>,
    messages: &mut (impl Stream<Item = Result<zbus::Message, zbus::Error>> + Unpin),
    publisher: crate::ThreadPublisher<BluetoothState, BluetoothError>,
    cancellation: Cancellation,
) -> Result<(), BluetoothError> {
    let (commands, receiver) = command_channel(cancellation.clone());
    let mut initial = cancellable(snapshot(connection, &commands), &cancellation).await?;
    state::attach_agent_state(&mut initial, agent.store());
    if publisher.publish(initial).is_err() {
        return Ok(());
    }

    let mut operation: Option<BluetoothOperation<'_>> = None;
    let mut reply: Option<BluetoothReply> = None;
    loop {
        let event = if let Some(operation) = operation.as_mut() {
            futures_lite::future::or(
                async { WorkerEvent::Operation(operation.as_mut().await) },
                wait_event(&receiver, messages, &agent_updates, &cancellation, false),
            )
            .await
        } else {
            wait_event(&receiver, messages, &agent_updates, &cancellation, true).await
        };
        match event {
            WorkerEvent::Cancelled => return Ok(()),
            WorkerEvent::Command(request) => {
                let Some(request) = request else {
                    return Ok(());
                };
                let (command, command_reply) = request.into_parts();
                reply = Some(Box::new(command_reply));
                operation = Some(Box::pin(cancellable(
                    state::execute(connection, command),
                    &cancellation,
                )));
            }
            WorkerEvent::Message(message) => {
                let Some(message) = message else {
                    return Ok(());
                };
                let message = message.map_err(|source| bus("read BlueZ D-Bus event", source))?;
                if let Some(owner) = owner_change(&message)? {
                    agent.owner_changed(owner.as_deref()).await?;
                    if !publish_state(
                        connection,
                        &commands,
                        agent.store(),
                        &publisher,
                        &cancellation,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                } else if is_relevant_signal(&message)
                    && !publish_state(
                        connection,
                        &commands,
                        agent.store(),
                        &publisher,
                        &cancellation,
                    )
                    .await?
                {
                    return Ok(());
                }
            }
            WorkerEvent::AgentUpdate => {
                if !publish_state(
                    connection,
                    &commands,
                    agent.store(),
                    &publisher,
                    &cancellation,
                )
                .await?
                {
                    return Ok(());
                }
            }
            WorkerEvent::Operation(result) => {
                if let Some(reply) = reply.take() {
                    reply(result);
                }
                operation = None;
            }
        }
    }
}

enum WorkerEvent {
    Command(Option<CommandRequest<BluetoothCommand, (), BluetoothError>>),
    Message(Option<Result<zbus::Message, zbus::Error>>),
    AgentUpdate,
    Operation(Result<(), BluetoothError>),
    Cancelled,
}

async fn wait_event(
    receiver: &crate::base::command::CommandReceiver<BluetoothCommand, (), BluetoothError>,
    messages: &mut (impl Stream<Item = Result<zbus::Message, zbus::Error>> + Unpin),
    agent_updates: &async_channel::Receiver<()>,
    cancellation: &Cancellation,
    allow_commands: bool,
) -> WorkerEvent {
    let command = async {
        if allow_commands {
            WorkerEvent::Command(receiver.recv().await)
        } else {
            std::future::pending().await
        }
    };
    futures_lite::future::or(
        futures_lite::future::or(command, async {
            WorkerEvent::Message(messages.next().await)
        }),
        futures_lite::future::or(
            async {
                if agent_updates.recv().await.is_ok() {
                    WorkerEvent::AgentUpdate
                } else {
                    std::future::pending().await
                }
            },
            async {
                cancellation.cancelled().await;
                WorkerEvent::Cancelled
            },
        ),
    )
    .await
}

async fn cancellable<T>(
    future: impl Future<Output = Result<T, BluetoothError>>,
    cancellation: &Cancellation,
) -> Result<T, BluetoothError> {
    futures_lite::future::or(future, async {
        cancellation.cancelled().await;
        Err(BluetoothError::Inactive)
    })
    .await
}

async fn publish_state(
    connection: &Connection,
    commands: &BluetoothCommandClient,
    agent: &AgentStore,
    publisher: &crate::ThreadPublisher<BluetoothState, BluetoothError>,
    cancellation: &Cancellation,
) -> Result<bool, BluetoothError> {
    let mut state = cancellable(snapshot(connection, commands), cancellation).await?;
    state::attach_agent_state(&mut state, agent);
    Ok(publisher.publish(state).is_ok())
}

fn owner_change(message: &zbus::Message) -> Result<Option<Option<String>>, BluetoothError> {
    if message.message_type() != Type::Signal
        || message
            .header()
            .interface()
            .is_none_or(|value| value.as_str() != "org.freedesktop.DBus")
        || message
            .header()
            .member()
            .is_none_or(|value| value.as_str() != "NameOwnerChanged")
    {
        return Ok(None);
    }
    let (name, _old_owner, new_owner): (String, String, String) = message
        .body()
        .deserialize()
        .map_err(|source| bus("decode D-Bus owner change", source))?;
    Ok((name == "org.bluez").then(|| (!new_owner.is_empty()).then_some(new_owner)))
}

async fn connect(config: &BluetoothConfig) -> Result<Connection, BluetoothError> {
    match &config.address {
        Some(address) => zbus::connection::Builder::address(address.as_str())
            .map_err(|source| bus("configure private D-Bus connection", source))?
            .build()
            .await
            .map_err(|source| bus("connect to private D-Bus", source)),
        None => Connection::system()
            .await
            .map_err(|source| bus("connect to system D-Bus", source)),
    }
}

async fn subscribe(
    connection: &Connection,
) -> Result<impl Stream<Item = Result<zbus::Message, zbus::Error>>, BluetoothError> {
    let added = signal_stream(
        connection,
        "org.bluez",
        "org.freedesktop.DBus.ObjectManager",
        "InterfacesAdded",
    )
    .await?;
    let removed = signal_stream(
        connection,
        "org.bluez",
        "org.freedesktop.DBus.ObjectManager",
        "InterfacesRemoved",
    )
    .await?;
    let properties = signal_stream(
        connection,
        "org.bluez",
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
    )
    .await?;
    let owner = signal_stream(
        connection,
        "org.freedesktop.DBus",
        "org.freedesktop.DBus",
        "NameOwnerChanged",
    )
    .await?;
    Ok(futures_lite::stream::race(
        futures_lite::stream::race(added, removed),
        futures_lite::stream::race(properties, owner),
    ))
}

async fn signal_stream(
    connection: &Connection,
    sender: &'static str,
    interface: &'static str,
    member: &'static str,
) -> Result<MessageStream, BluetoothError> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(interface)
        .map_err(|source| bus("build BlueZ signal match", source))?
        .member(member)
        .map_err(|source| bus("build BlueZ signal match", source))?
        .sender(sender)
        .map_err(|source| bus("build BlueZ signal match", source))?
        .build();
    MessageStream::for_match_rule(rule, connection, Some(64))
        .await
        .map_err(|source| bus("subscribe to BlueZ signals", source))
}

async fn snapshot(
    connection: &Connection,
    commands: &BluetoothCommandClient,
) -> Result<BluetoothState, BluetoothError> {
    state::map_objects(backend::snapshot(connection).await?, Some(commands.clone()))
}

fn is_relevant_signal(message: &zbus::Message) -> bool {
    message.message_type() == Type::Signal
        && message.header().interface().is_some_and(|interface| {
            matches!(
                interface.as_str(),
                "org.freedesktop.DBus.ObjectManager"
                    | "org.freedesktop.DBus.Properties"
                    | "org.freedesktop.DBus"
            )
        })
}

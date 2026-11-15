use std::time::{Duration, Instant};

use crate::{
    Source, SourceDescriptor, SourcePublisher, ThreadPublisher,
    base::worker::{Cancellation, WorkerGuard, spawn_worker},
};
use gpui::App;
use std::fmt;

use super::{
    NotificationsError,
    server::{self, BUS_NAME, NotificationInterface, OBJECT_PATH, expired_signal, process_command},
    state::{NotificationCommand, NotificationStore, NotificationsState, command_queue},
};

#[derive(Clone, Eq, Hash, PartialEq)]
pub enum NotificationsEndpoint {
    Session,
    Address(String),
}

impl fmt::Debug for NotificationsEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Session => f.write_str("Session"),
            Self::Address(_) => f.write_str("Address(<redacted>)"),
        }
    }
}

pub struct Notifications {
    endpoint: NotificationsEndpoint,
}

impl Notifications {
    /// Declares a notifications server on the session bus; no connection starts until mounted.
    pub fn new() -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            endpoint: NotificationsEndpoint::Session,
        })
    }

    /// Declares a notifications server on an explicit D-Bus address.
    ///
    /// The address participates in the source key, allowing isolated private-bus
    /// instances without changing the process session-bus environment.
    pub fn with_address(address: impl Into<String>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            endpoint: NotificationsEndpoint::Address(address.into()),
        })
    }
}

impl Source for Notifications {
    type Key = NotificationsEndpoint;
    type Value = NotificationsState;
    type Error = NotificationsError;
    type Guard = WorkerGuard;

    fn key(&self) -> Self::Key {
        self.endpoint.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (thread_publisher, bridge) = publisher.thread_bridge(|value| value, |error| error, cx);
        let endpoint = self.endpoint.clone();
        spawn_worker(
            "bkss-notifications-server",
            bridge,
            thread_publisher.clone(),
            move |publisher, cancellation| {
                futures_lite::future::block_on(run(endpoint, publisher, cancellation))
            },
        )
        .map_err(NotificationsError::from)
    }
}

async fn run(
    endpoint: NotificationsEndpoint,
    publisher: ThreadPublisher<NotificationsState, NotificationsError>,
    cancellation: Cancellation,
) -> Result<(), NotificationsError> {
    let (store, changed) = NotificationStore::new();
    let (commands, receiver) = command_queue(cancellation.clone());
    let startup = futures_lite::future::race(
        async {
            cancellation.cancelled().await;
            Ok::<_, NotificationsError>(None)
        },
        async {
            let connection = match endpoint {
                NotificationsEndpoint::Session => zbus::Connection::session()
                    .await
                    .map_err(|source| super::error::bus("connect to session bus", source))?,
                NotificationsEndpoint::Address(address) => server::connection(&address)
                    .map_err(|source| {
                        super::error::bus("configure notifications bus connection", source)
                    })?
                    .build()
                    .await
                    .map_err(|source| super::error::bus("connect to notifications bus", source))?,
            };
            let interface = NotificationInterface::new(store.clone());
            connection
                .object_server()
                .at(OBJECT_PATH, interface)
                .await
                .map_err(|source| super::error::bus("export notifications interface", source))?;
            let owner = connection
                .request_name_with_flags(BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
                .await
                .map_err(|source| super::error::bus("request notifications bus name", source))?;
            if !matches!(
                owner,
                zbus::fdo::RequestNameReply::PrimaryOwner
                    | zbus::fdo::RequestNameReply::AlreadyOwner
            ) {
                return Err(NotificationsError::NameConflict {
                    source: Box::new(zbus::Error::NameTaken),
                });
            }
            publisher
                .publish(store.snapshot(commands.clone()))
                .map_err(|_| NotificationsError::Inactive)?;
            Ok::<_, NotificationsError>(Some(connection))
        },
    )
    .await;
    let Some(connection) = startup? else {
        return Ok(());
    };

    let result = futures_lite::future::race(
        async {
            cancellation.cancelled().await;
            Ok(())
        },
        async {
            loop {
                let event = futures_lite::future::race(
                    futures_lite::future::race(
                        futures_lite::future::race(
                            async { receiver.recv().await.map(Event::Command) },
                            async { changed.recv().await.ok().map(|()| Event::Changed) },
                        ),
                        async {
                            match store.expiration_delay(Instant::now()) {
                                Some(delay) => {
                                    async_io::Timer::after(delay).await;
                                    Some(Event::Expiration)
                                }
                                None => std::future::pending().await,
                            }
                        },
                    ),
                    async {
                        connection.closed().await;
                        Some(Event::Disconnected)
                    },
                )
                .await
                .ok_or(NotificationsError::Inactive)?;
                match event {
                    Event::Command(request) => {
                        let (command, reply) = request.into_parts();
                        let result = process_command(&connection, &store, command).await;
                        match result {
                            Ok(()) => reply(Ok(())),
                            Err(
                                error @ (NotificationsError::UnknownNotification(_)
                                | NotificationsError::UnknownAction { .. }),
                            ) => reply(Err(error)),
                            Err(error) => return Err(error),
                        }
                    }
                    Event::Changed => {
                        publisher
                            .publish(store.snapshot(commands.clone()))
                            .map_err(|_| NotificationsError::Inactive)?;
                    }
                    Event::Expiration => {
                        let expired = server::expire(&store);
                        for id in &expired {
                            expired_signal(&connection, *id).await?;
                        }
                        if !expired.is_empty() {
                            publisher
                                .publish(store.snapshot(commands.clone()))
                                .map_err(|_| NotificationsError::Inactive)?;
                        }
                    }
                    Event::Disconnected => return Err(NotificationsError::BusClosed),
                }
            }
        },
    )
    .await;

    let _ = futures_lite::future::race(
        async {
            let _ = connection.release_name(BUS_NAME).await;
        },
        async {
            async_io::Timer::after(Duration::from_millis(250)).await;
        },
    )
    .await;
    result
}

enum Event {
    Command(crate::base::command::CommandRequest<NotificationCommand, (), NotificationsError>),
    Changed,
    Expiration,
    Disconnected,
}

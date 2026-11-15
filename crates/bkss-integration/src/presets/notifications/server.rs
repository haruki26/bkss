use std::{collections::HashMap, time::Instant};

use zbus::{Connection, fdo, interface, object_server::SignalEmitter, zvariant::OwnedValue};

use super::{
    NotificationsError,
    error::bus,
    state::{Notification, NotificationCommand, NotificationStore},
};

pub(super) const BUS_NAME: &str = "org.freedesktop.Notifications";
pub(super) const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const MAX_TEXT: usize = 16 * 1024;
const MAX_ACTIONS: usize = 16;
const MAX_HINTS: usize = 64;

pub(super) struct NotificationInterface {
    store: NotificationStore,
}

impl NotificationInterface {
    pub(super) fn new(store: NotificationStore) -> Self {
        Self { store }
    }
}

#[interface(name = "org.freedesktop.Notifications")]
impl NotificationInterface {
    async fn get_capabilities(&self) -> Vec<String> {
        vec!["body".into(), "actions".into()]
    }

    async fn get_server_information(&self) -> (String, String, String, String) {
        (
            "BKSS".into(),
            "BKSS".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "The freedesktop Notifications Notify method has a fixed positional D-Bus protocol signature."
    )]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> fdo::Result<u32> {
        if expire_timeout < -1 {
            return Err(fdo::Error::InvalidArgs("invalid expiration timeout".into()));
        }
        if [app_name.len(), app_icon.len(), summary.len(), body.len()]
            .into_iter()
            .any(|len| len > MAX_TEXT)
            || [
                app_name.as_str(),
                app_icon.as_str(),
                summary.as_str(),
                body.as_str(),
            ]
            .into_iter()
            .any(|text| text.contains('\0'))
            || !actions.len().is_multiple_of(2)
            || actions.len() / 2 > MAX_ACTIONS
            || actions
                .iter()
                .any(|value| value.len() > 256 || value.contains('\0'))
            || hints.len() > MAX_HINTS
        {
            return Err(fdo::Error::InvalidArgs(
                "notification exceeds supported limits".into(),
            ));
        }
        // Hints are not exposed or retained. This prevents unbounded image and
        // variant payloads from entering long-lived source state. Two standard
        // scalar hints are copied after type validation.
        let urgency = hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok())
            .filter(|urgency| *urgency <= 2);
        let resident = hints
            .get("resident")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false);
        let actions = actions
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| (pair[0].clone(), pair[1].clone()))
            .collect();
        let notification = Notification {
            id: 0,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            urgency,
            resident,
        };
        let (id, _) = self
            .store
            .insert(notification, replaces_id, expire_timeout)
            .map_err(|message| fdo::Error::LimitsExceeded(message.into()))?;
        Ok(id)
    }

    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        if self.store.remove(id) {
            Self::notification_closed(&ctxt, id, 3).await?;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn notification_closed(
        ctxt: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        ctxt: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

pub(super) fn connection(
    endpoint: &str,
) -> Result<zbus::connection::Builder<'static>, zbus::Error> {
    zbus::connection::Builder::address(endpoint)
}

pub(super) fn expire(store: &NotificationStore) -> Vec<u32> {
    store.expire(Instant::now())
}

pub(super) async fn process_command(
    connection: &Connection,
    store: &NotificationStore,
    command: NotificationCommand,
) -> Result<(), NotificationsError> {
    let ctxt = SignalEmitter::new(connection, OBJECT_PATH)
        .map_err(|source| bus("create notifications signal context", source))?;
    match command {
        NotificationCommand::Dismiss(id) => {
            if store.remove(id) {
                NotificationInterface::notification_closed(&ctxt, id, 2)
                    .await
                    .map_err(|source| bus("emit NotificationClosed", source))?;
            } else {
                return Err(NotificationsError::UnknownNotification(id));
            }
        }
        NotificationCommand::Invoke(id, key) => {
            let close = store.invoke(id, &key)?;
            NotificationInterface::action_invoked(&ctxt, id, &key)
                .await
                .map_err(|source| bus("emit ActionInvoked", source))?;
            if close {
                NotificationInterface::notification_closed(&ctxt, id, 2)
                    .await
                    .map_err(|source| bus("emit NotificationClosed", source))?;
            }
        }
    }
    Ok(())
}

pub(super) async fn expired_signal(
    connection: &Connection,
    id: u32,
) -> Result<(), NotificationsError> {
    let ctxt = SignalEmitter::new(connection, OBJECT_PATH)
        .map_err(|source| bus("create notifications signal context", source))?;
    NotificationInterface::notification_closed(&ctxt, id, 1)
        .await
        .map_err(|source| bus("emit NotificationClosed", source))
}

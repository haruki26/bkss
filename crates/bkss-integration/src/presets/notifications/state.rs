use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::base::command::{self, CommandHandle, CommandReceiver, Operation};
use crate::base::worker::Cancellation;

use super::NotificationsError;

pub const MAX_ACTIVE: usize = 128;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub urgency: Option<u8>,
    pub resident: bool,
}

#[derive(Clone)]
pub struct NotificationsState {
    pub notifications: Vec<Notification>,
    commands: CommandHandle<NotificationCommand, (), NotificationsError>,
}

impl NotificationsState {
    pub fn dismiss(
        &self,
        id: u32,
    ) -> Result<Operation<(), NotificationsError>, crate::DispatchError> {
        self.commands.dispatch(NotificationCommand::Dismiss(id))
    }

    pub fn invoke_action(
        &self,
        id: u32,
        key: impl Into<String>,
    ) -> Result<Operation<(), NotificationsError>, crate::DispatchError> {
        self.commands
            .dispatch(NotificationCommand::Invoke(id, key.into()))
    }
}

#[derive(Clone)]
pub(super) enum NotificationCommand {
    Dismiss(u32),
    Invoke(u32, String),
}

pub(super) type CommandQueue = (
    CommandHandle<NotificationCommand, (), NotificationsError>,
    CommandReceiver<NotificationCommand, (), NotificationsError>,
);

pub(super) fn command_queue(cancellation: Cancellation) -> CommandQueue {
    command::channel(
        NonZeroUsize::new(64).unwrap_or(NonZeroUsize::MIN),
        cancellation,
    )
}

#[derive(Clone)]
pub(super) struct NotificationStore {
    store: Arc<Mutex<Store>>,
    changed: async_channel::Sender<()>,
}

pub(super) struct Store {
    notifications: BTreeMap<u32, StoredNotification>,
    next_id: u32,
    default_timeout: Duration,
}

struct StoredNotification {
    notification: Notification,
    expires: Option<Instant>,
}

impl NotificationStore {
    pub(super) fn new(default_timeout: Duration) -> (Self, async_channel::Receiver<()>) {
        let (changed, receiver) = async_channel::bounded(1);
        (
            Self {
                store: Arc::new(Mutex::new(Store {
                    notifications: BTreeMap::new(),
                    next_id: 1,
                    default_timeout,
                })),
                changed,
            },
            receiver,
        )
    }

    pub(super) fn snapshot(
        &self,
        commands: CommandHandle<NotificationCommand, (), NotificationsError>,
    ) -> NotificationsState {
        let notifications = self
            .store
            .lock()
            .map(|store| {
                store
                    .notifications
                    .values()
                    .map(|stored| stored.notification.clone())
                    .collect()
            })
            .unwrap_or_else(|poisoned| {
                poisoned
                    .into_inner()
                    .notifications
                    .values()
                    .map(|stored| stored.notification.clone())
                    .collect()
            });
        NotificationsState {
            notifications,
            commands,
        }
    }

    pub(super) fn insert(
        &self,
        notification: Notification,
        replaces_id: u32,
        expire_timeout: i32,
    ) -> Result<(u32, Option<u32>), &'static str> {
        let result = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(notification, replaces_id, expire_timeout);
        if result.is_ok() {
            let _ = self.changed.try_send(());
        }
        result
    }

    pub(super) fn expiration_delay(&self, now: Instant) -> Option<std::time::Duration> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expiration_delay(now)
    }

    pub(super) fn invoke(&self, id: u32, key: &str) -> Result<bool, NotificationsError> {
        let mut store = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stored = store
            .notifications
            .get(&id)
            .ok_or(NotificationsError::UnknownNotification(id))?;
        if !stored
            .notification
            .actions
            .iter()
            .any(|(action, _)| action == key)
        {
            return Err(NotificationsError::UnknownAction {
                id,
                key: key.to_owned(),
            });
        }
        let close = !stored.notification.resident;
        if close {
            store.notifications.remove(&id);
        }
        drop(store);
        if close {
            let _ = self.changed.try_send(());
        }
        Ok(close)
    }

    pub(super) fn remove(&self, id: u32) -> bool {
        let removed = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if removed {
            let _ = self.changed.try_send(());
        }
        removed
    }

    pub(super) fn expire(&self, now: Instant) -> Vec<u32> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expire(now)
    }
}

impl Store {
    pub(super) fn insert(
        &mut self,
        mut notification: Notification,
        replaces_id: u32,
        expire_timeout: i32,
    ) -> Result<(u32, Option<u32>), &'static str> {
        if self.notifications.len() >= MAX_ACTIVE
            && (replaces_id == 0 || !self.notifications.contains_key(&replaces_id))
        {
            return Err("active notification limit reached");
        }
        let replaced = (replaces_id != 0 && self.notifications.contains_key(&replaces_id))
            .then_some(replaces_id);
        let id = replaced.unwrap_or_else(|| self.allocate_id());
        notification.id = id;
        let timeout = if expire_timeout == -1 {
            (self.default_timeout != Duration::ZERO).then_some(self.default_timeout)
        } else {
            (expire_timeout > 0).then(|| Duration::from_millis(expire_timeout as u64))
        };
        let expires = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        self.notifications.insert(
            id,
            StoredNotification {
                notification,
                expires,
            },
        );
        Ok((id, replaced))
    }

    fn allocate_id(&mut self) -> u32 {
        loop {
            let id = self.next_id.max(1);
            self.next_id = id.wrapping_add(1).max(1);
            if !self.notifications.contains_key(&id) {
                return id;
            }
        }
    }

    pub(super) fn remove(&mut self, id: u32) -> bool {
        self.notifications.remove(&id).is_some()
    }

    pub(super) fn expiration_delay(&self, now: Instant) -> Option<std::time::Duration> {
        self.notifications
            .values()
            .filter_map(|stored| stored.expires)
            .min()
            .map(|deadline| deadline.saturating_duration_since(now))
    }

    pub(super) fn expire(&mut self, now: Instant) -> Vec<u32> {
        let expired = self
            .notifications
            .iter()
            .filter_map(|(&id, stored)| {
                stored
                    .expires
                    .is_some_and(|expires| expires <= now)
                    .then_some(id)
            })
            .collect::<Vec<_>>();
        for id in &expired {
            self.notifications.remove(id);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(resident: bool) -> Notification {
        Notification {
            id: 0,
            app_name: String::new(),
            app_icon: String::new(),
            summary: String::new(),
            body: String::new(),
            actions: vec![("open".into(), "Open".into())],
            urgency: None,
            resident,
        }
    }

    #[test]
    fn timeout_policy_respects_default_explicit_and_replacement_values() {
        let mut store = Store {
            notifications: BTreeMap::new(),
            next_id: 1,
            default_timeout: Duration::from_secs(10),
        };
        let (default_id, _) = store.insert(notification(false), 0, -1).unwrap();
        let (replacement_id, _) = store.insert(notification(false), 0, -1).unwrap();
        store
            .insert(notification(false), replacement_id, 0)
            .unwrap();
        let (finite_id, _) = store.insert(notification(false), 0, 30).unwrap();
        let (zero_id, _) = store.insert(notification(true), 0, 0).unwrap();

        let after_deadlines = Instant::now() + Duration::from_secs(11);
        assert_eq!(store.expire(after_deadlines), [default_id, finite_id]);
        assert!(store.notifications.contains_key(&replacement_id));
        assert!(store.notifications.contains_key(&zero_id));
    }

    #[test]
    fn zero_default_keeps_defaulted_notification_and_its_action_available() {
        let (store, _) = NotificationStore::new(Duration::ZERO);
        let (default_id, _) = store.insert(notification(true), 0, -1).unwrap();
        let (finite_id, _) = store.insert(notification(false), 0, 30).unwrap();
        let (zero_id, _) = store.insert(notification(true), 0, 0).unwrap();

        assert_eq!(
            store.expire(Instant::now() + Duration::from_secs(11)),
            [finite_id]
        );
        assert!(!store.invoke(default_id, "open").unwrap());
        assert!(!store.invoke(default_id, "open").unwrap());
        assert!(!store.invoke(zero_id, "open").unwrap());
    }
}

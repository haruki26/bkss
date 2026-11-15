use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use zbus::{fdo, message::Header, object_server::SignalEmitter, zvariant::OwnedObjectPath};

const MAX_REGISTERED_ITEMS: usize = 512;

#[derive(Clone, Default)]
pub(super) struct Watcher {
    state: Arc<Mutex<WatcherState>>,
}

#[derive(Default)]
struct WatcherState {
    items: std::collections::BTreeMap<String, String>,
    hosts: BTreeSet<String>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|state| state.items.keys().cloned().collect())
            .unwrap_or_default()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        self.state.lock().is_ok_and(|state| !state.hosts.is_empty())
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    async fn register_status_notifier_item(
        &self,
        service: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        if service.len() > 512 {
            return Err(fdo::Error::Failed("item registration is too long".into()));
        }
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("missing sender".into()))?
            .to_string();
        let (service_name, path) = if service.starts_with('/') {
            (sender.clone(), service)
        } else if let Some((name, path)) = service.split_once('/') {
            (name.to_owned(), format!("/{path}"))
        } else {
            (service.clone(), "/StatusNotifierItem".to_owned())
        };
        OwnedObjectPath::try_from(path.clone())
            .map_err(|_| fdo::Error::Failed("item object path is invalid".into()))?;
        let bus_name = zbus::names::BusName::try_from(service_name.as_str())
            .map_err(|_| fdo::Error::Failed("item service name is invalid".into()))?;
        let owner = fdo::DBusProxy::new(emitter.connection())
            .await
            .map_err(|_| fdo::Error::Failed("could not authenticate item owner".into()))?
            .get_name_owner(bus_name)
            .await
            .map_err(|_| fdo::Error::AccessDenied("item service has no owner".into()))?;
        if owner.to_string() != sender {
            return Err(fdo::Error::AccessDenied(
                "item registration sender does not own the service".into(),
            ));
        }
        let registered = format!("{service_name}{path}");
        let inserted = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| fdo::Error::Failed("watcher state unavailable".into()))?;
            if !state.items.contains_key(&registered) && state.items.len() >= MAX_REGISTERED_ITEMS {
                return Err(fdo::Error::Failed("watcher item limit reached".into()));
            }
            state
                .items
                .insert(registered.clone(), owner.to_string())
                .is_none()
        };
        if inserted {
            Self::status_notifier_item_registered(&emitter, &registered).await?;
        }
        Ok(())
    }

    async fn register_status_notifier_host(
        &self,
        service: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("missing sender".into()))?
            .to_string();
        let bus_name = zbus::names::BusName::try_from(service.as_str())
            .map_err(|_| fdo::Error::Failed("host service name is invalid".into()))?;
        let owner = fdo::DBusProxy::new(emitter.connection())
            .await
            .map_err(|_| fdo::Error::Failed("could not authenticate host owner".into()))?
            .get_name_owner(bus_name)
            .await
            .map_err(|_| fdo::Error::AccessDenied("host service has no owner".into()))?;
        if owner.to_string() != sender {
            return Err(fdo::Error::AccessDenied(
                "host registration sender does not own the service".into(),
            ));
        }
        let inserted = self
            .state
            .lock()
            .map_err(|_| fdo::Error::Failed("watcher state unavailable".into()))?
            .hosts
            .insert(owner.to_string());
        if inserted {
            Self::status_notifier_host_registered(&emitter).await?;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_unregistered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

impl Watcher {
    pub(super) async fn remove_owner(
        &self,
        owner: &str,
        emitter: &SignalEmitter<'_>,
    ) -> zbus::Result<()> {
        let (removed_items, removed_host): (Vec<String>, bool) = {
            let mut state = self.state.lock().map_err(|_| {
                zbus::Error::Failure("StatusNotifierWatcher state unavailable".into())
            })?;
            let matching: Vec<_> = state
                .items
                .iter()
                .filter(|(_, item_owner)| item_owner.as_str() == owner)
                .map(|(item, _)| item.clone())
                .collect();
            for item in &matching {
                state.items.remove(item);
            }
            let had_host = state.hosts.remove(owner);
            (matching, had_host)
        };
        for item in removed_items {
            Self::status_notifier_item_unregistered(emitter, &item).await?;
        }
        if removed_host {
            Self::status_notifier_host_unregistered(emitter).await?;
        }
        Ok(())
    }
}

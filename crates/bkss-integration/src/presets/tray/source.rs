use std::{collections::HashMap, future::poll_fn, num::NonZeroUsize};

use crate::{
    Source, SourceDescriptor, SourcePublisher,
    base::{
        command,
        worker::{self, Cancellation},
    },
};
use futures_lite::StreamExt;
use gpui::App;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::{Connection, Proxy, connection::Builder};

use super::{
    TrayError,
    item::{ItemNotification, ItemSignalKind, ItemSignals, normalize_id, read_item},
    state::{TrayActions, TrayCommand, TrayItem, TrayItemId, TrayReply, TrayState},
};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const MAX_ITEMS: usize = 512;

pub struct Tray {
    address: Option<String>,
}

impl Tray {
    pub fn new() -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self { address: None })
    }

    /// Connects to an explicit D-Bus address instead of the session bus.
    ///
    /// The address is part of the source identity, allowing isolated private-bus
    /// sources without changing process-global bus configuration.
    pub fn with_address(address: impl Into<String>) -> SourceDescriptor<Self> {
        SourceDescriptor::new(Self {
            address: Some(address.into()),
        })
    }
}

impl Source for Tray {
    type Key = Option<String>;
    type Value = TrayState;
    type Error = TrayError;
    type Guard = crate::WorkerGuard;

    fn key(&self) -> Self::Key {
        self.address.clone()
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let (sender, bridge) = publisher.thread_bridge(|state| state, |error| error, cx);
        let address = self.address.clone();
        let guard = worker::spawn_worker(
            "bkss-tray",
            bridge,
            sender,
            move |publisher, cancellation| {
                let cancelled = cancellation.clone();
                async_io::block_on(futures_lite::future::race(
                    run(address, publisher, cancellation),
                    async move {
                        cancelled.cancelled().await;
                        Ok(())
                    },
                ))
            },
        )?;
        Ok(guard)
    }
}

async fn run(
    address: Option<String>,
    publisher: crate::ThreadPublisher<TrayState, TrayError>,
    cancellation: Cancellation,
) -> Result<(), TrayError> {
    let (handle, commands) = command::channel(
        NonZeroUsize::new(32).unwrap_or(NonZeroUsize::MIN),
        cancellation.clone(),
    );
    let actions = TrayActions::new(handle);
    let setup = async move {
        match address {
            Some(address) => Ok(Builder::address(address.as_str())?.build().await?),
            None => Connection::session().await,
        }
    };
    let connection = futures_lite::future::race(setup, async {
        cancellation.cancelled().await;
        Err(zbus::Error::Failure("tray activation cancelled".into()))
    })
    .await?;
    let (watcher, owns_watcher, watcher_instance) = ensure_watcher(&connection).await?;
    // Subscribe before taking the initial snapshot to avoid registration races.
    let mut registered = watcher
        .receive_signal("StatusNotifierItemRegistered")
        .await?;
    let mut unregistered = watcher
        .receive_signal("StatusNotifierItemUnregistered")
        .await?;
    let dbus_signals = Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let mut owner_changed = dbus_signals.receive_signal("NameOwnerChanged").await?;
    watcher
        .call::<_, _, ()>(
            "RegisterStatusNotifierHost",
            &(connection
                .unique_name()
                .map(|n| n.to_string())
                .unwrap_or_default()),
        )
        .await?;

    let names: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .await?;
    if names.len() > MAX_ITEMS {
        return Err(TrayError::Limit("registered item count"));
    }
    let mut entries = TrayEntries::default();
    for name in names {
        let Some((item, signals)) = load_registered_item(&connection, &name, &actions).await?
        else {
            continue;
        };
        let id = item.id.clone();
        entries.aliases.insert(name, id.clone());
        entries.signals.insert(id.clone(), signals);
        if !entries.items.contains_key(&id) && entries.items.len() >= MAX_ITEMS {
            return Err(TrayError::Limit("registered item count"));
        }
        entries.items.insert(item.id.clone(), item);
    }
    publish(&publisher, &entries.items)?;

    enum Event {
        Registered(String),
        Unregistered(String),
        OwnerChanged(String, String, String),
        ItemChanged(ItemNotification),
        Command(crate::base::command::CommandRequest<TrayCommand, TrayReply, TrayError>),
        Cancelled,
    }
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let registration = async {
            match registered.next().await {
                Some(message) => {
                    Ok::<_, TrayError>(Event::Registered(message.body().deserialize()?))
                }
                None => Err(TrayError::Protocol(
                    "watcher registration subscription ended",
                )),
            }
        };
        let removal = async {
            match unregistered.next().await {
                Some(message) => {
                    Ok::<_, TrayError>(Event::Unregistered(message.body().deserialize()?))
                }
                None => Err(TrayError::Protocol("watcher removal subscription ended")),
            }
        };
        let owner_event = async {
            match owner_changed.next().await {
                Some(message) => {
                    let (name, old_owner, new_owner): (String, String, String) =
                        message.body().deserialize()?;
                    Ok::<_, TrayError>(Event::OwnerChanged(name, old_owner, new_owner))
                }
                None => Err(TrayError::Protocol("D-Bus owner subscription ended")),
            }
        };
        let bus_event = futures_lite::future::race(
            futures_lite::future::race(registration, removal),
            owner_event,
        );
        let item_event = poll_fn(|cx| {
            for signals in entries.signals.values_mut() {
                if let std::task::Poll::Ready(notification) = signals.poll_next(cx) {
                    return std::task::Poll::Ready(Ok(Event::ItemChanged(notification)));
                }
            }
            std::task::Poll::Pending
        });
        let next_command = async {
            match commands.recv().await {
                Some(command) => Ok::<_, TrayError>(Event::Command(command)),
                None => Err(TrayError::Inactive),
            }
        };
        let cancelled = async {
            cancellation.cancelled().await;
            Ok::<_, TrayError>(Event::Cancelled)
        };
        let event = futures_lite::future::race(
            futures_lite::future::race(
                futures_lite::future::race(bus_event, item_event),
                next_command,
            ),
            cancelled,
        )
        .await?;
        match event {
            Event::Registered(name) => {
                let Some((item, signals)) =
                    load_registered_item(&connection, &name, &actions).await?
                else {
                    continue;
                };
                let id = item.id.clone();
                if !entries.items.contains_key(&id) && entries.items.len() >= MAX_ITEMS {
                    return Err(TrayError::Limit("registered item count"));
                }
                entries.aliases.insert(name, id.clone());
                entries.signals.insert(id.clone(), signals);
                entries.items.insert(item.id.clone(), item);
                publish(&publisher, &entries.items)?;
            }
            Event::Unregistered(name) => {
                let id = match entries.aliases.remove(&name) {
                    Some(id) => id,
                    None => {
                        let Some(id) = normalize_id(&connection, &name).await? else {
                            continue;
                        };
                        id
                    }
                };
                if entries.items.remove(&id).is_some() {
                    entries.signals.remove(&id);
                    entries.aliases.retain(|_, item_id| item_id != &id);
                    publish(&publisher, &entries.items)?;
                }
            }
            Event::OwnerChanged(name, old_owner, new_owner) => {
                if apply_owner_changed(
                    &connection,
                    &name,
                    &old_owner,
                    &new_owner,
                    owns_watcher,
                    &watcher_instance,
                    &mut entries,
                )
                .await?
                {
                    publish(&publisher, &entries.items)?;
                }
            }
            Event::ItemChanged(notification) => {
                if matches!(notification.kind, ItemSignalKind::Ended) {
                    entries.items.remove(&notification.id);
                    entries.signals.remove(&notification.id);
                    entries.aliases.retain(|_, id| id != &notification.id);
                    publish(&publisher, &entries.items)?;
                    continue;
                }
                match load_item(&connection, notification.id.clone(), &actions).await {
                    Ok((item, signals)) => {
                        entries.signals.insert(notification.id.clone(), signals);
                        entries.items.insert(item.id.clone(), item);
                        publish(&publisher, &entries.items)?;
                    }
                    Err(error) if is_owner_gone(&error) => {
                        entries.items.remove(&notification.id);
                        entries.signals.remove(&notification.id);
                        entries.aliases.retain(|_, id| id != &notification.id);
                        publish(&publisher, &entries.items)?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Event::Command(command) => {
                futures_lite::future::race(
                    execute_command(&connection, command, &mut entries, &actions, |items| {
                        publish(&publisher, items)
                    }),
                    async {
                        cancellation.cancelled().await;
                        Ok::<(), TrayError>(())
                    },
                )
                .await?;
            }
            Event::Cancelled => return Ok(()),
        }
    }
}

async fn load_registered_item(
    connection: &Connection,
    registration: &str,
    actions: &TrayActions,
) -> Result<Option<(TrayItem, ItemSignals)>, TrayError> {
    let Some(id) = normalize_id(connection, registration).await? else {
        return Ok(None);
    };
    match load_item(connection, id, actions).await {
        Ok(item) => Ok(Some(item)),
        Err(error) if is_owner_gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn load_item(
    connection: &Connection,
    id: TrayItemId,
    actions: &TrayActions,
) -> Result<(TrayItem, ItemSignals), TrayError> {
    let mut signals = ItemSignals::subscribe(connection, id.clone()).await?;
    let item = read_item(connection, id, actions, &mut signals).await?;
    Ok((item, signals))
}

fn is_owner_gone(error: &TrayError) -> bool {
    matches!(
        error,
        TrayError::Bus(error)
            if matches!(error.as_ref(), zbus::Error::FDO(error)
                if matches!(error.as_ref(), zbus::fdo::Error::NameHasNoOwner(_)))
    )
}

#[derive(Default)]
struct TrayEntries {
    items: HashMap<TrayItemId, TrayItem>,
    aliases: HashMap<String, TrayItemId>,
    signals: HashMap<TrayItemId, ItemSignals>,
}

async fn apply_owner_changed(
    connection: &Connection,
    name: &str,
    old_owner: &str,
    new_owner: &str,
    owns_watcher: bool,
    watcher: &super::watcher::Watcher,
    entries: &mut TrayEntries,
) -> Result<bool, TrayError> {
    if name == WATCHER && !old_owner.is_empty() && old_owner != new_owner {
        return Err(TrayError::Protocol("StatusNotifierWatcher disappeared"));
    }
    if old_owner.is_empty() || old_owner == new_owner {
        return Ok(false);
    }
    let before = entries.items.len();
    entries.items.retain(|id, _| id.owner != old_owner);
    entries.aliases.retain(|_, id| id.owner != old_owner);
    entries.signals.retain(|id, _| id.owner != old_owner);
    let changed = entries.items.len() != before;
    if owns_watcher {
        watcher
            .remove_owner(old_owner, &SignalEmitter::new(connection, WATCHER_PATH)?)
            .await?;
    }
    Ok(changed)
}

async fn ensure_watcher(
    connection: &Connection,
) -> Result<(Proxy<'_>, bool, super::watcher::Watcher), TrayError> {
    let watcher_instance = super::watcher::Watcher::default();
    connection
        .object_server()
        .at(WATCHER_PATH, watcher_instance.clone())
        .await?;
    let owns_watcher = match connection
        .request_name_with_flags(WATCHER, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => true,
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            connection
                .object_server()
                .remove::<super::watcher::Watcher, _>(WATCHER_PATH)
                .await?;
            false
        }
        Err(error) => return Err(error.into()),
    };
    let proxy = Proxy::new(connection, WATCHER, WATCHER_PATH, WATCHER_INTERFACE).await?;
    Ok((proxy, owns_watcher, watcher_instance))
}

fn publish(
    publisher: &crate::ThreadPublisher<TrayState, TrayError>,
    items: &HashMap<TrayItemId, TrayItem>,
) -> Result<(), TrayError> {
    let mut items: Vec<_> = items.values().cloned().collect();
    items.sort_by(|a, b| a.id.owner.cmp(&b.id.owner).then(a.id.path.cmp(&b.id.path)));
    publisher
        .publish(TrayState { items })
        .map_err(|_| TrayError::Inactive)
}

async fn execute_command<F>(
    connection: &Connection,
    request: crate::base::command::CommandRequest<TrayCommand, TrayReply, TrayError>,
    entries: &mut TrayEntries,
    actions: &TrayActions,
    mut publish_snapshot: F,
) -> Result<(), TrayError>
where
    F: FnMut(&HashMap<TrayItemId, TrayItem>) -> Result<(), TrayError>,
{
    let (command, reply) = request.into_parts();
    let mut refresh_item = None;
    let result = async {
        let (item, method, args) = match command {
            TrayCommand::Activate { item, x, y } => (item, "Activate", (x, y)),
            TrayCommand::SecondaryActivate { item, x, y } => (item, "SecondaryActivate", (x, y)),
            TrayCommand::ContextMenu { item, x, y } => (item, "ContextMenu", (x, y)),
            TrayCommand::Scroll {
                item,
                delta,
                orientation,
            } => {
                let proxy = item_proxy(connection, &item).await?;
                proxy
                    .call::<_, _, ()>("Scroll", &(delta, orientation))
                    .await?;
                return Ok(TrayReply::Done);
            }
            TrayCommand::MenuEvent {
                item,
                id,
                event,
                timestamp,
            } => {
                let menu = menu_proxy(connection, &item).await?;
                menu.call::<_, _, ()>(
                    "Event",
                    &(id, event, zbus::zvariant::Value::from(""), timestamp),
                )
                .await?;
                return Ok(TrayReply::Done);
            }
            TrayCommand::AboutToShow { item, id } => {
                let menu = menu_proxy(connection, &item).await?;
                let update = menu.call::<_, _, bool>("AboutToShow", &id).await?;
                if update {
                    refresh_item = Some(item);
                }
                return Ok(TrayReply::NeedsUpdate(update));
            }
        };
        let proxy = item_proxy(connection, &item).await?;
        proxy.call::<_, _, ()>(method, &args).await?;
        Ok(TrayReply::Done)
    }
    .await;
    let result = match result {
        Ok(TrayReply::NeedsUpdate(true)) => match refresh_item {
            Some(id) => match load_item(connection, id.clone(), actions).await {
                Ok((item, signals)) => {
                    entries.signals.insert(id.clone(), signals);
                    entries.items.insert(id, item);
                    publish_snapshot(&entries.items).map(|()| TrayReply::NeedsUpdate(true))
                }
                Err(error) => Err(error),
            },
            None => Err(TrayError::Protocol(
                "AboutToShow requested refresh without an item",
            )),
        },
        result => result,
    };
    reply(result);
    Ok(())
}

async fn item_proxy<'a>(
    connection: &'a Connection,
    item: &'a TrayItemId,
) -> Result<Proxy<'a>, TrayError> {
    Ok(Proxy::new(
        connection,
        item.owner.as_str(),
        item.path.as_str(),
        super::item::ITEM_INTERFACE,
    )
    .await?)
}

async fn menu_proxy<'a>(
    connection: &'a Connection,
    item: &'a TrayItemId,
) -> Result<Proxy<'a>, TrayError> {
    let proxy = item_proxy(connection, item).await?;
    let path: zbus::zvariant::OwnedObjectPath = proxy.get_property("Menu").await?;
    Ok(Proxy::new_owned(
        connection.clone(),
        item.owner.clone(),
        path,
        "com.canonical.dbusmenu",
    )
    .await?)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        future::{Future, poll_fn},
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        },
        time::Duration,
    };

    use futures_lite::StreamExt;
    use serde::Serialize;
    use zbus::{
        Connection, Proxy,
        fdo::{DBusProxy, RequestNameFlags, RequestNameReply},
        object_server::SignalEmitter,
        zvariant::{OwnedObjectPath, OwnedValue, Type},
    };

    use super::*;

    const ITEM_NAME: &str = "org.example.BkssTrayFixture";
    const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
    type TrayIconPixmap = (i32, i32, Vec<u8>);
    type TrayIconPixmaps = Vec<TrayIconPixmap>;
    type TrayToolTipProperty = (String, TrayIconPixmaps, String, String);

    struct PrivateBus {
        address: String,
        child: Child,
    }

    impl PrivateBus {
        fn new() -> Self {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("start private test bus");
            let mut address = String::new();
            BufReader::new(child.stdout.take().expect("test bus stdout"))
                .read_line(&mut address)
                .expect("read private bus address");
            Self {
                address: address.trim().to_owned(),
                child,
            }
        }

        async fn connect(&self) -> Connection {
            zbus::connection::Builder::address(self.address.as_str())
                .expect("valid private bus address")
                .build()
                .await
                .expect("connect to private bus")
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn bounded<F: Future>(future: F) -> F::Output {
        futures_lite::future::block_on(futures_lite::future::race(future, async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            panic!("private D-Bus test timed out");
        }))
    }

    #[derive(Clone)]
    struct FakeItem {
        title: Arc<Mutex<String>>,
        icon_name: Arc<Mutex<String>>,
        status: Arc<Mutex<String>>,
        attention_icon_pixmap: Arc<Mutex<TrayIconPixmaps>>,
        overlay_icon_pixmap: Arc<Mutex<TrayIconPixmaps>>,
        menu: OwnedObjectPath,
    }

    impl Default for FakeItem {
        fn default() -> Self {
            Self {
                title: Arc::new(Mutex::new("fixture".into())),
                icon_name: Arc::new(Mutex::new("fixture-icon".into())),
                status: Arc::new(Mutex::new("Active".into())),
                attention_icon_pixmap: Arc::new(Mutex::new(vec![(1, 1, vec![255, 1, 2, 3])])),
                overlay_icon_pixmap: Arc::new(Mutex::new(vec![(1, 1, vec![128, 4, 5, 6])])),
                menu: OwnedObjectPath::try_from("/Menu").expect("valid menu path"),
            }
        }
    }

    #[zbus::interface(name = "org.kde.StatusNotifierItem")]
    impl FakeItem {
        #[zbus(property)]
        fn title(&self) -> String {
            self.title
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default()
        }

        #[zbus(property)]
        fn set_title(&self, value: String) {
            if let Ok(mut title) = self.title.lock() {
                *title = value;
            }
        }

        #[zbus(property)]
        fn icon_name(&self) -> String {
            self.icon_name
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default()
        }

        #[zbus(property)]
        fn set_icon_name(&self, value: String) {
            if let Ok(mut icon_name) = self.icon_name.lock() {
                *icon_name = value;
            }
        }

        #[zbus(property)]
        fn status(&self) -> String {
            self.status
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default()
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            self.menu.clone()
        }

        #[zbus(property)]
        fn attention_icon_pixmap(&self) -> TrayIconPixmaps {
            self.attention_icon_pixmap
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default()
        }

        #[zbus(property)]
        fn overlay_icon_pixmap(&self) -> TrayIconPixmaps {
            self.overlay_icon_pixmap
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default()
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> TrayIconPixmaps {
            vec![(1, 1, vec![255, 20, 30, 40])]
        }

        #[zbus(property)]
        fn tool_tip(&self) -> TrayToolTipProperty {
            (
                "fixture-icon".into(),
                Vec::new(),
                "tip".into(),
                "description".into(),
            )
        }

        async fn emit_item_signals(
            &self,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<()> {
            Self::new_icon(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            Self::new_attention_icon(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            Self::new_overlay_icon(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            let status = self
                .status
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            Self::new_status(&emitter, &status)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            Self::new_title(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            Self::new_tool_tip(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))?;
            Self::new_menu(&emitter)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))
        }

        #[zbus(signal)]
        async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_overlay_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn new_menu(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
    }

    #[derive(Serialize, Type)]
    struct FakeLayout(i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

    #[derive(Clone, Default)]
    struct FakeMenu {
        revision: Arc<AtomicU32>,
        events: Arc<Mutex<Vec<(i32, String, u32)>>>,
    }

    #[zbus::interface(name = "com.canonical.dbusmenu")]
    impl FakeMenu {
        fn get_layout(
            &self,
            _parent_id: i32,
            _depth: i32,
            _property_names: Vec<String>,
        ) -> (u32, FakeLayout) {
            (
                self.revision.load(Ordering::Acquire),
                FakeLayout(0, HashMap::new(), Vec::new()),
            )
        }

        fn event(&self, id: i32, event: String, _data: OwnedValue, timestamp: u32) {
            if let Ok(mut events) = self.events.lock() {
                events.push((id, event, timestamp));
            }
        }

        fn about_to_show(&self, _id: i32) -> bool {
            self.revision.fetch_add(1, Ordering::AcqRel);
            true
        }

        async fn advance_layout(
            &self,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<()> {
            let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
            Self::layout_updated(&emitter, revision, 0)
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))
        }

        async fn update_properties(
            &self,
            #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        ) -> zbus::fdo::Result<()> {
            self.revision.fetch_add(1, Ordering::AcqRel);
            Self::items_properties_updated(&emitter, Vec::new(), Vec::new())
                .await
                .map_err(|_| zbus::fdo::Error::Failed("could not emit test signal".into()))
        }

        #[zbus(signal)]
        async fn layout_updated(
            emitter: &SignalEmitter<'_>,
            revision: u32,
            parent: i32,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn items_properties_updated(
            emitter: &SignalEmitter<'_>,
            updated: Vec<(i32, HashMap<String, OwnedValue>)>,
            removed: Vec<(i32, Vec<String>)>,
        ) -> zbus::Result<()>;
    }

    async fn export_item(connection: &Connection) -> (FakeItem, FakeMenu) {
        let item = FakeItem::default();
        let menu = FakeMenu::default();
        connection
            .object_server()
            .at("/StatusNotifierItem", item.clone())
            .await
            .expect("export fake item");
        connection
            .object_server()
            .at("/Menu", menu.clone())
            .await
            .expect("export fake menu");
        assert_eq!(
            connection
                .request_name_with_flags(ITEM_NAME, RequestNameFlags::DoNotQueue.into(),)
                .await
                .expect("claim fake item name"),
            RequestNameReply::PrimaryOwner
        );
        (item, menu)
    }

    #[test]
    fn owned_watcher_exports_service_and_path_registration_and_item_owner_cleanup() {
        bounded(async {
            let bus = PrivateBus::new();
            let host = bus.connect().await;
            let item_connection = bus.connect().await;
            let (_item, _) = export_item(&item_connection).await;
            let (watcher, owns, watcher_state) =
                ensure_watcher(&host).await.expect("claim watcher");
            assert!(owns);
            watcher
                .call::<_, _, ()>(
                    "RegisterStatusNotifierHost",
                    &host.unique_name().expect("host unique name").to_string(),
                )
                .await
                .expect("register local host");
            assert!(
                watcher
                    .get_property::<bool>("IsStatusNotifierHostRegistered")
                    .await
                    .expect("host registration property")
            );

            let mut owner_changes = Proxy::new(
                &host,
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
            )
            .await
            .expect("DBus proxy")
            .receive_signal("NameOwnerChanged")
            .await
            .expect("owner-change subscription");
            let item = Proxy::new(&item_connection, WATCHER, WATCHER_PATH, WATCHER_INTERFACE)
                .await
                .expect("watcher proxy");
            item.call::<_, _, ()>(
                "RegisterStatusNotifierItem",
                &format!("{ITEM_NAME}/CustomItem"),
            )
            .await
            .expect("register service/path form");
            item.call::<_, _, ()>("RegisterStatusNotifierItem", &ITEM_NAME)
                .await
                .expect("register service-only form");
            let snapshot = Proxy::new(&host, WATCHER, WATCHER_PATH, WATCHER_INTERFACE)
                .await
                .expect("fresh watcher proxy");
            let items: Vec<String> = snapshot
                .get_property("RegisteredStatusNotifierItems")
                .await
                .expect("registered items");
            assert_eq!(
                items,
                [
                    format!("{ITEM_NAME}/CustomItem"),
                    format!("{ITEM_NAME}/StatusNotifierItem")
                ]
            );

            let old_owner = item_connection
                .unique_name()
                .expect("unique name")
                .to_string();
            let id = TrayItemId {
                owner: old_owner.clone(),
                path: "/StatusNotifierItem".into(),
            };
            let signals = ItemSignals::subscribe(&host, id.clone())
                .await
                .expect("item subscriptions");
            let (_cancel_sender, cancellation) = crate::base::worker::cancellation_channel();
            let (commands, _receiver) = crate::base::command::channel(
                NonZeroUsize::new(1).expect("positive test queue"),
                cancellation,
            );
            let mut entries = TrayEntries::default();
            entries.items.insert(
                id.clone(),
                TrayItem::new(id.clone(), TrayActions::new(commands)),
            );
            entries
                .aliases
                .insert(format!("{ITEM_NAME}/CustomItem"), id.clone());
            entries
                .aliases
                .insert(format!("{ITEM_NAME}/StatusNotifierItem"), id.clone());
            entries.signals.insert(id, signals);
            drop(item);
            drop(item_connection);
            let mut gone = false;
            while let Some(message) = owner_changes.next().await {
                let (name, old, new): (String, String, String) =
                    message.body().deserialize().expect("owner signal body");
                if name == ITEM_NAME && old == old_owner && new.is_empty() {
                    let changed = apply_owner_changed(
                        &host,
                        &name,
                        &old,
                        &new,
                        true,
                        &watcher_state,
                        &mut entries,
                    )
                    .await
                    .expect("handle vanished owner");
                    assert!(changed);
                    gone = true;
                    break;
                }
            }
            assert!(gone, "registered item owner disappearance was observed");
            assert!(entries.items.is_empty());
            assert!(entries.aliases.is_empty());
            assert!(entries.signals.is_empty());
            let items: Vec<String> = watcher
                .get_property("RegisteredStatusNotifierItems")
                .await
                .expect("updated registered items");
            assert!(items.is_empty());
        });
    }

    #[test]
    fn external_replaceable_watcher_is_not_stolen() {
        bounded(async {
            let bus = PrivateBus::new();
            let external = bus.connect().await;
            external
                .object_server()
                .at(WATCHER_PATH, super::super::watcher::Watcher::default())
                .await
                .expect("export external watcher");
            assert_eq!(
                external
                    .request_name_with_flags(WATCHER, RequestNameFlags::AllowReplacement.into(),)
                    .await
                    .expect("own external watcher name"),
                RequestNameReply::PrimaryOwner
            );
            let external_owner = external
                .unique_name()
                .expect("external unique name")
                .to_string();

            let tray = bus.connect().await;
            let (_, owns, _) = ensure_watcher(&tray).await.expect("use external watcher");
            assert!(!owns);
            let current_owner = DBusProxy::new(&tray)
                .await
                .expect("DBus proxy")
                .get_name_owner(WATCHER.try_into().expect("watcher bus name"))
                .await
                .expect("watcher owner")
                .to_string();
            assert_eq!(current_owner, external_owner);
        });
    }

    #[test]
    fn item_properties_and_menu_layout_signals_refresh_typed_snapshot_and_actions() {
        bounded(async {
            let bus = PrivateBus::new();
            let host = bus.connect().await;
            let item_connection = bus.connect().await;
            let (fake_item, fake_menu) = export_item(&item_connection).await;
            let owner = item_connection
                .unique_name()
                .expect("unique name")
                .to_string();
            let id = TrayItemId {
                owner,
                path: "/StatusNotifierItem".into(),
            };
            let (_cancel_sender, cancellation) = crate::base::worker::cancellation_channel();
            let (commands, receiver) = crate::base::command::channel(
                NonZeroUsize::new(4).expect("positive test queue"),
                cancellation,
            );
            let actions = TrayActions::new(commands);
            let mut signals = ItemSignals::subscribe(&host, id.clone())
                .await
                .expect("item signal subscriptions");
            let initial = read_item(&host, id.clone(), &actions, &mut signals)
                .await
                .expect("initial item snapshot");
            assert_eq!(initial.title, "fixture");
            assert_eq!(initial.icon.unwrap().rgba, [20, 30, 40, 255]);
            assert_eq!(initial.attention_icon.unwrap().rgba, [1, 2, 3, 255]);
            assert_eq!(initial.overlay_icon.unwrap().rgba, [4, 5, 6, 128]);
            assert_eq!(initial.menu.unwrap().revision, 0);

            *fake_item.status.lock().expect("fake item status") = "NeedsAttention".into();
            *fake_item
                .attention_icon_pixmap
                .lock()
                .expect("fake attention pixmap") = vec![(1, 1, vec![200, 7, 8, 9])];
            *fake_item
                .overlay_icon_pixmap
                .lock()
                .expect("fake overlay pixmap") = vec![(1, 1, vec![100, 10, 11, 12])];
            let item_proxy = Proxy::new(
                &item_connection,
                ITEM_NAME,
                "/StatusNotifierItem",
                super::super::item::ITEM_INTERFACE,
            )
            .await
            .expect("item proxy");
            item_proxy
                .set_property("Title", "updated")
                .await
                .expect("change item title");
            let item_signal = poll_fn(|cx| signals.poll_next(cx)).await;
            assert!(matches!(item_signal.kind, ItemSignalKind::Item));
            let updated = read_item(&host, id.clone(), &actions, &mut signals)
                .await
                .expect("updated item snapshot");
            assert_eq!(updated.title, "updated");
            let item_proxy = Proxy::new(
                &item_connection,
                ITEM_NAME,
                "/StatusNotifierItem",
                super::super::item::ITEM_INTERFACE,
            )
            .await
            .expect("item proxy");
            item_proxy
                .call::<_, _, ()>("EmitItemSignals", &())
                .await
                .expect("emit all item change signals");
            for _ in 0..7 {
                assert!(matches!(
                    poll_fn(|cx| signals.poll_next(cx)).await.kind,
                    ItemSignalKind::Item
                ));
            }
            let updated = read_item(&host, id.clone(), &actions, &mut signals)
                .await
                .expect("refresh after NewIcon");
            assert_eq!(updated.icon_name, "fixture-icon");
            assert_eq!(updated.status, crate::TrayStatus::NeedsAttention);
            assert_eq!(updated.attention_icon.unwrap().rgba, [7, 8, 9, 200]);
            assert_eq!(updated.overlay_icon.unwrap().rgba, [10, 11, 12, 100]);

            let menu_proxy = Proxy::new(&item_connection, ITEM_NAME, "/Menu", MENU_INTERFACE)
                .await
                .expect("menu proxy");
            menu_proxy
                .call::<_, _, ()>("AdvanceLayout", &())
                .await
                .expect("emit layout update");
            let menu_signal = poll_fn(|cx| signals.poll_next(cx)).await;
            assert!(matches!(menu_signal.kind, ItemSignalKind::Menu));
            let updated = read_item(&host, id.clone(), &actions, &mut signals)
                .await
                .expect("refreshed menu snapshot");
            assert_eq!(updated.menu.unwrap().revision, 1);
            menu_proxy
                .call::<_, _, ()>("UpdateProperties", &())
                .await
                .expect("emit menu property invalidation");
            assert!(matches!(
                poll_fn(|cx| signals.poll_next(cx)).await.kind,
                ItemSignalKind::Menu
            ));
            let updated = read_item(&host, id.clone(), &actions, &mut signals)
                .await
                .expect("refresh after ItemsPropertiesUpdated");
            assert_eq!(updated.menu.unwrap().revision, 2);

            let item = TrayItem::new(id.clone(), actions.clone());
            let mut entries = TrayEntries::default();
            let operation = item.menu_event(9, "clicked", 23).expect("dispatch click");
            let request = receiver.recv().await.expect("queued click");
            execute_command(&host, request, &mut entries, &actions, |_| Ok(()))
                .await
                .expect("execute menu event");
            assert!(matches!(operation.await, Ok(())));
            assert_eq!(
                fake_menu.events.lock().expect("event log").as_slice(),
                &[(9, "clicked".into(), 23)]
            );
            entries.items.insert(id.clone(), item.clone());
            let about_to_show = item.about_to_show(0).expect("dispatch AboutToShow");
            let request = receiver.recv().await.expect("queued AboutToShow");
            let mut published_revision = None;
            execute_command(&host, request, &mut entries, &actions, |items| {
                published_revision = items
                    .get(&id)
                    .and_then(|item| item.menu.as_ref().map(|menu| menu.revision));
                Ok(())
            })
            .await
            .expect("execute AboutToShow");
            assert!(about_to_show.await.expect("operation result"));
            assert_eq!(published_revision, Some(3));
            assert_eq!(
                entries
                    .items
                    .get(&id)
                    .and_then(|item| item.menu.as_ref().map(|menu| menu.revision)),
                Some(3)
            );
        });
    }

    #[test]
    fn operation_handles_become_inactive_when_the_worker_is_cancelled() {
        bounded(async {
            let (cancel_sender, cancellation) = crate::base::worker::cancellation_channel();
            let (commands, _receiver) = crate::base::command::channel(
                NonZeroUsize::new(1).expect("positive test queue"),
                cancellation,
            );
            let actions = TrayActions::new(commands);
            let item = TrayItem::new(
                TrayItemId {
                    owner: ":1.100".into(),
                    path: "/StatusNotifierItem".into(),
                },
                actions,
            );
            let operation = item.activate(0, 0).expect("admit operation");
            drop(cancel_sender);
            assert_eq!(
                item.activate(0, 0).err(),
                Some(crate::DispatchError::Inactive)
            );
            assert!(matches!(
                operation.await,
                Err(crate::OperationError::Inactive)
            ));
        });
    }
}

use std::{
    collections::HashMap,
    future::Future,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use bkss_core::{ResourceScope, SourceState};
use bkss_integration::{DispatchError, Tray, TrayState, TrayStatus};
use gpui::TestAppContext;
use serde::Serialize;
use zbus::{
    Connection, Proxy,
    fdo::{DBusProxy, RequestNameFlags, RequestNameReply},
    message::Header,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Type},
};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_NAME: &str = "org.example.BkssTrayProtocolItem";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/Menu";

type TrayIconPixmap = (i32, i32, Vec<u8>);
type TrayIconPixmaps = Vec<TrayIconPixmap>;
type TrayToolTipProperty = (String, TrayIconPixmaps, String, String);

struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> std::io::Result<Self> {
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut address = String::new();
        BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("dbus-daemon has no stdout"))?,
        )
        .read_line(&mut address)?;
        if address.trim().is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(
                "dbus-daemon did not print a bus address",
            ));
        }
        Ok(Self {
            child,
            address: address.trim().to_owned(),
        })
    }

    async fn connect(&self) -> zbus::Result<Connection> {
        zbus::connection::Builder::address(self.address.as_str())?
            .build()
            .await
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_for(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<TrayState>>,
    mut condition: impl FnMut(&TrayState) -> bool,
) -> TrayState {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        cx.run_until_parked();
        if let SourceState::Ready(value) = state.get(cx)
            && condition(&value)
        {
            return value;
        }
        assert!(Instant::now() < deadline, "tray source state timed out");
        thread::sleep(Duration::from_millis(3));
    }
}

fn bounded<F: Future>(future: F) -> F::Output {
    futures_lite::future::block_on(futures_lite::future::race(future, async {
        async_io::Timer::after(Duration::from_secs(4)).await;
        panic!("tray protocol test timed out");
    }))
}

fn one_item(state: &TrayState) -> &bkss_integration::TrayItem {
    assert_eq!(state.items.len(), 1);
    &state.items[0]
}

#[derive(Clone)]
struct FakeItem {
    status: Arc<Mutex<String>>,
    icon: Arc<Mutex<TrayIconPixmaps>>,
    attention: Arc<Mutex<TrayIconPixmaps>>,
    overlay: Arc<Mutex<TrayIconPixmaps>>,
    menu: OwnedObjectPath,
}

impl Default for FakeItem {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new("Active".into())),
            icon: Arc::new(Mutex::new(vec![(1, 1, vec![255, 10, 20, 30])])),
            attention: Arc::new(Mutex::new(vec![(1, 1, vec![255, 1, 2, 3])])),
            overlay: Arc::new(Mutex::new(vec![(1, 1, vec![128, 4, 5, 6])])),
            menu: OwnedObjectPath::try_from(MENU_PATH).expect("valid menu path"),
        }
    }
}

fn clone_locked<T: Clone>(value: &Mutex<T>) -> T {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    #[zbus(property)]
    fn title(&self) -> String {
        "protocol fixture".into()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        clone_locked(&self.status)
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        "fixture-icon-name".into()
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> String {
        "attention-icon-name-not-a-path".into()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> String {
        "overlay-icon-name-not-a-path".into()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> TrayIconPixmaps {
        clone_locked(&self.icon)
    }

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> TrayIconPixmaps {
        clone_locked(&self.attention)
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> TrayIconPixmaps {
        clone_locked(&self.overlay)
    }

    #[zbus(property)]
    fn tool_tip(&self) -> TrayToolTipProperty {
        (
            "fixture-icon-name".into(),
            Vec::new(),
            "tip".into(),
            "description".into(),
        )
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        self.menu.clone()
    }

    async fn update_icons(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = "NeedsAttention".into();
        *self
            .icon
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            vec![(1, 1, vec![200, 40, 50, 60])];
        *self
            .attention
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            vec![(1, 1, vec![190, 70, 80, 90])];
        *self
            .overlay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            vec![(1, 1, vec![180, 100, 110, 120])];
        Self::new_icon(&emitter).await.map_err(signal_error)?;
        Self::new_attention_icon(&emitter)
            .await
            .map_err(signal_error)?;
        Self::new_overlay_icon(&emitter)
            .await
            .map_err(signal_error)?;
        Self::new_status(&emitter, "NeedsAttention")
            .await
            .map_err(signal_error)
    }

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_attention_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_overlay_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;
}

fn signal_error(_: zbus::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed("could not emit fixture signal".into())
}

#[derive(Serialize, Type)]
struct LayoutNode(i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

#[derive(Clone, Default)]
struct FakeMenu {
    revision: Arc<AtomicU32>,
    events: Arc<Mutex<Vec<(i32, String, u32)>>>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl FakeMenu {
    fn get_layout(&self, _parent: i32, _depth: i32, _properties: Vec<String>) -> (u32, LayoutNode) {
        (
            self.revision.load(Ordering::Acquire),
            LayoutNode(0, HashMap::new(), Vec::new()),
        )
    }

    fn about_to_show(&self, _id: i32) -> bool {
        self.revision.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn event(&self, id: i32, name: String, _data: OwnedValue, timestamp: u32) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((id, name, timestamp));
    }
}

async fn export_item(connection: &Connection) -> (FakeItem, FakeMenu) {
    let item = FakeItem::default();
    let menu = FakeMenu::default();
    connection
        .object_server()
        .at(ITEM_PATH, item.clone())
        .await
        .expect("export fixture item");
    connection
        .object_server()
        .at(MENU_PATH, menu.clone())
        .await
        .expect("export fixture menu");
    assert_eq!(
        connection
            .request_name_with_flags(ITEM_NAME, RequestNameFlags::DoNotQueue.into())
            .await
            .expect("claim fixture item name"),
        RequestNameReply::PrimaryOwner
    );
    (item, menu)
}

#[derive(Clone, Default)]
struct ExternalWatcher {
    items: Arc<Mutex<Vec<String>>>,
    hosts: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl ExternalWatcher {
    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        clone_locked(&self.items)
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        !clone_locked(&self.hosts).is_empty()
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    async fn register_status_notifier_host(
        &self,
        service: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let inserted = {
            let mut hosts = self
                .hosts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if hosts.contains(&service) {
                false
            } else {
                hosts.push(service);
                true
            }
        };
        if inserted {
            Self::status_notifier_host_registered(&emitter)
                .await
                .map_err(signal_error)?;
        }
        Ok(())
    }

    async fn register_status_notifier_item(
        &self,
        registration: String,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::AccessDenied("missing sender".into()))?
            .to_string();
        let registered = if registration.starts_with('/') {
            format!("{sender}{registration}")
        } else if let Some((service, path)) = registration.split_once('/') {
            format!("{service}/{path}")
        } else {
            format!("{registration}/StatusNotifierItem")
        };
        let inserted = {
            let mut items = self
                .items
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if items.contains(&registered) {
                false
            } else {
                items.push(registered.clone());
                true
            }
        };
        if inserted {
            Self::status_notifier_item_registered(&emitter, &registered)
                .await
                .map_err(signal_error)?;
        }
        Ok(())
    }

    async fn unregister_fixture_item(
        &self,
        registration: String,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let removed = {
            let mut items = self
                .items
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let before = items.len();
            items.retain(|item| item != &registration);
            before != items.len()
        };
        if removed {
            Self::status_notifier_item_unregistered(&emitter, &registration)
                .await
                .map_err(signal_error)?;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &SignalEmitter<'_>,
        registration: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &SignalEmitter<'_>,
        registration: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[test]
fn owned_watcher_public_source_tracks_item_menu_and_owner_lifecycle() {
    let bus = PrivateBus::start().expect("start private dbus-daemon");
    bounded(async {
        let cx = TestAppContext::single();
        let scope = cx.update(ResourceScope::new);
        let lease = cx.update(|cx| {
            scope
                .acquire(&Tray::with_address(bus.address.clone()), cx)
                .expect("mount tray source")
        });
        let reactive = lease.reactive();
        wait_for(&cx, &reactive, |state| state.items.is_empty());

        let item_connection = bus.connect().await.expect("connect fixture item");
        let (_fake_item, fake_menu) = export_item(&item_connection).await;
        let watcher = Proxy::new(&item_connection, WATCHER, WATCHER_PATH, WATCHER_IFACE)
            .await
            .expect("local watcher proxy");
        watcher
            .call::<_, _, ()>("RegisterStatusNotifierItem", &ITEM_PATH)
            .await
            .expect("register fixture by object path");

        let initial = wait_for(&cx, &reactive, |state| state.items.len() == 1);
        let first = one_item(&initial);
        assert_eq!(first.status, TrayStatus::Active);
        assert_eq!(
            first.icon.as_ref().expect("main pixmap").rgba,
            [10, 20, 30, 255]
        );
        assert_eq!(
            first
                .attention_icon
                .as_ref()
                .expect("attention pixmap")
                .rgba,
            [1, 2, 3, 255]
        );
        assert_eq!(
            first.overlay_icon.as_ref().expect("overlay pixmap").rgba,
            [4, 5, 6, 128]
        );
        assert_eq!(
            first.icon_name, "fixture-icon-name",
            "icon names are metadata, not decoded filesystem paths"
        );

        let item_proxy = Proxy::new(
            &item_connection,
            ITEM_NAME,
            ITEM_PATH,
            "org.kde.StatusNotifierItem",
        )
        .await
        .expect("item proxy");
        item_proxy
            .call::<_, _, ()>("UpdateIcons", &())
            .await
            .expect("emit changed pixmaps");
        let updated = wait_for(&cx, &reactive, |state| {
            state.items.first().is_some_and(|item| {
                item.status == TrayStatus::NeedsAttention
                    && item
                        .icon
                        .as_ref()
                        .is_some_and(|icon| icon.rgba == [40, 50, 60, 200])
                    && item
                        .attention_icon
                        .as_ref()
                        .is_some_and(|icon| icon.rgba == [70, 80, 90, 190])
                    && item
                        .overlay_icon
                        .as_ref()
                        .is_some_and(|icon| icon.rgba == [100, 110, 120, 180])
            })
        });

        let menu_click = one_item(&updated)
            .menu_event(7, "clicked", 17)
            .expect("dispatch public menu event");
        menu_click.await.expect("menu event result");
        assert_eq!(
            clone_locked(&fake_menu.events).as_slice(),
            &[(7, "clicked".to_owned(), 17)]
        );

        let about_to_show = one_item(&updated)
            .about_to_show(0)
            .expect("dispatch public AboutToShow");
        assert!(about_to_show.await.expect("AboutToShow result"));
        let refreshed = wait_for(&cx, &reactive, |state| {
            state
                .items
                .first()
                .and_then(|item| item.menu.as_ref())
                .is_some_and(|menu| menu.revision == 1)
        });
        assert_eq!(one_item(&refreshed).menu.as_ref().unwrap().revision, 1);

        let stale_item = one_item(&refreshed).clone();
        drop(item_proxy);
        drop(watcher);
        drop(item_connection);
        let removed = wait_for(&cx, &reactive, |state| state.items.is_empty());
        assert!(removed.items.is_empty());

        drop(lease);
        cx.run_until_parked();
        assert_eq!(
            stale_item.activate(0, 0).err(),
            Some(DispatchError::Inactive)
        );
    });
}

#[test]
fn existing_replaceable_watcher_keeps_name_and_routes_registration_events() {
    let bus = PrivateBus::start().expect("start private dbus-daemon");
    bounded(async {
        let (watcher_connection, fake_watcher) = {
            let connection = bus.connect().await.expect("connect fake watcher");
            let watcher = ExternalWatcher::default();
            connection
                .object_server()
                .at(WATCHER_PATH, watcher.clone())
                .await
                .expect("export external watcher");
            assert_eq!(
                connection
                    .request_name_with_flags(WATCHER, RequestNameFlags::AllowReplacement.into())
                    .await
                    .expect("claim replaceable watcher name"),
                RequestNameReply::PrimaryOwner
            );
            (connection, watcher)
        };
        let external_owner = watcher_connection
            .unique_name()
            .expect("external watcher unique name")
            .to_string();
        let cx = TestAppContext::single();
        let scope = cx.update(ResourceScope::new);
        let lease = cx.update(|cx| {
            scope
                .acquire(&Tray::with_address(bus.address.clone()), cx)
                .expect("mount tray source")
        });
        let reactive = lease.reactive();
        wait_for(&cx, &reactive, |state| state.items.is_empty());

        let item_connection = bus.connect().await.expect("connect fixture client");
        let (_item, _menu) = export_item(&item_connection).await;
        let registration = format!("{ITEM_NAME}{ITEM_PATH}");
        let watcher_proxy = Proxy::new(&item_connection, WATCHER, WATCHER_PATH, WATCHER_IFACE)
            .await
            .expect("external watcher proxy");
        watcher_proxy
            .call::<_, _, ()>(
                "RegisterStatusNotifierItem",
                &format!("{ITEM_NAME}{ITEM_PATH}"),
            )
            .await
            .expect("register item with external watcher");
        wait_for(&cx, &reactive, |state| state.items.len() == 1);
        assert!(!clone_locked(&fake_watcher.hosts).is_empty());

        let dbus = DBusProxy::new(&item_connection).await.expect("DBus proxy");
        let watcher_name = zbus::names::BusName::try_from(WATCHER).expect("watcher bus name");
        let owner = dbus
            .get_name_owner(watcher_name.clone())
            .await
            .expect("watcher name owner")
            .to_string();
        assert_eq!(owner, external_owner);

        watcher_proxy
            .call::<_, _, ()>("UnregisterFixtureItem", &registration)
            .await
            .expect("emit item removal");
        let removed = wait_for(&cx, &reactive, |state| state.items.is_empty());
        assert!(removed.items.is_empty());

        drop(lease);
        cx.run_until_parked();
        let owner_after_drop = dbus
            .get_name_owner(watcher_name)
            .await
            .expect("external watcher remains named")
            .to_string();
        assert_eq!(owner_after_drop, external_owner);
    });
}

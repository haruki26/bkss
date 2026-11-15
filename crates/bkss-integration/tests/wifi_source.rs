use bkss_core::{ResourceScope, SourceState};
use bkss_integration::{
    DispatchError, Wifi, WifiAuthRequest, WifiCredentials, WifiDeviceState, WifiSecurity,
};
use gpui::TestAppContext;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use zbus::{
    Connection, DBusError,
    connection::Builder,
    message::Header,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const ETHERNET_PATH: &str = "/org/freedesktop/NetworkManager/Devices/0";
const DEVICE_PATH: &str = "/org/freedesktop/NetworkManager/Devices/1";
const LOOPBACK_PATH: &str = "/org/freedesktop/NetworkManager/Devices/2";
const AP_PATH: &str = "/org/freedesktop/NetworkManager/AccessPoint/1";
const SAVED_PATH: &str = "/org/freedesktop/NetworkManager/Settings/1";
const ACTIVE_PATH: &str = "/org/freedesktop/NetworkManager/ActiveConnection/1";
const TEMP_PATH: &str = "/org/freedesktop/NetworkManager/Settings/2";
const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";

fn object_path(path: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(path).expect("valid test object path")
}

fn owned(value: impl Into<Value<'static>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).expect("owned D-Bus test value")
}

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.Test")]
enum FakeError {
    Failed(String),
}

#[derive(Clone)]
struct FakeState {
    wireless_enabled: Arc<AtomicBool>,
    device_state: Arc<AtomicUsize>,
    ap_strength: Arc<AtomicUsize>,
    ap_ssid: Arc<Mutex<Vec<u8>>>,
    active_connection: Arc<Mutex<String>>,
    has_device: bool,
    registrations: Arc<AtomicUsize>,
    add_calls: Arc<AtomicUsize>,
    activate_calls: Arc<AtomicUsize>,
    disconnect_calls: Arc<AtomicUsize>,
    memory_persistence: Arc<AtomicBool>,
    requested_psk: Arc<Mutex<Option<String>>>,
    agent_owner: Arc<Mutex<Option<String>>>,
}

impl FakeState {
    fn new(has_device: bool, wireless_enabled: bool) -> Self {
        Self {
            wireless_enabled: Arc::new(AtomicBool::new(wireless_enabled)),
            device_state: Arc::new(AtomicUsize::new(30)),
            ap_strength: Arc::new(AtomicUsize::new(77)),
            ap_ssid: Arc::new(Mutex::new(vec![0xff, b'n'])),
            active_connection: Arc::new(Mutex::new("/".to_owned())),
            has_device,
            registrations: Arc::new(AtomicUsize::new(0)),
            add_calls: Arc::new(AtomicUsize::new(0)),
            activate_calls: Arc::new(AtomicUsize::new(0)),
            disconnect_calls: Arc::new(AtomicUsize::new(0)),
            memory_persistence: Arc::new(AtomicBool::new(false)),
            requested_psk: Arc::new(Mutex::new(None)),
            agent_owner: Arc::new(Mutex::new(None)),
        }
    }
}

struct FakeBus {
    process: Child,
    address: String,
    service: Connection,
    state: FakeState,
}

impl FakeBus {
    fn start(has_device: bool, wireless_enabled: bool) -> Self {
        let mut process = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start private D-Bus");
        let mut address = String::new();
        BufReader::new(process.stdout.take().expect("D-Bus stdout"))
            .read_line(&mut address)
            .expect("read private D-Bus address");
        let address = address.trim().to_owned();
        let state = FakeState::new(has_device, wireless_enabled);
        let service = futures_lite::future::block_on(async {
            Builder::address(address.as_str())
                .expect("private bus address")
                .name(NM)
                .expect("NetworkManager service name")
                .serve_at(NM_PATH, FakeNetworkManager(state.clone()))
                .expect("NetworkManager interface")
                .serve_at(
                    "/org/freedesktop/NetworkManager/AgentManager",
                    FakeAgentManager(state.clone()),
                )
                .expect("AgentManager interface")
                .serve_at(SETTINGS_PATH, FakeSettings)
                .expect("Settings interface")
                .serve_at(SAVED_PATH, FakeSavedConnection)
                .expect("saved connection interface")
                .serve_at(ETHERNET_PATH, FakeDevice::new(state.clone(), 1, "eth-test"))
                .expect("ethernet device interface")
                .serve_at(LOOPBACK_PATH, FakeDevice::new(state.clone(), 32, "lo"))
                .expect("loopback device interface")
                .serve_at(DEVICE_PATH, FakeDevice::new(state.clone(), 2, "wlan-test"))
                .expect("device interface")
                .serve_at(DEVICE_PATH, FakeWireless)
                .expect("wireless interface")
                .serve_at(AP_PATH, FakeAccessPoint(state.clone()))
                .expect("access point interface")
                .build()
                .await
                .expect("private NetworkManager service")
        });
        Self {
            process,
            address,
            service,
            state,
        }
    }
}

impl Drop for FakeBus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = &self.service;
    }
}

struct FakeNetworkManager(FakeState);

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl FakeNetworkManager {
    #[zbus(property)]
    fn networking_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.0.wireless_enabled.load(Ordering::Acquire)
    }

    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        let mut paths = vec![object_path(ETHERNET_PATH), object_path(LOOPBACK_PATH)];
        if self.0.has_device {
            paths.push(object_path(DEVICE_PATH));
        }
        paths
    }

    #[zbus(property)]
    async fn set_wireless_enabled(
        &self,
        enabled: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        self.0.wireless_enabled.store(enabled, Ordering::Release);
        if enabled {
            self.0.device_state.store(40, Ordering::Release);
            self.0.ap_strength.store(99, Ordering::Release);
            *self
                .0
                .ap_ssid
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = vec![0xfe, b'n'];
            let device_emitter =
                SignalEmitter::new(emitter.connection(), DEVICE_PATH).map_err(fdo_error)?;
            device_emitter
                .emit(
                    PROPERTIES_IFACE,
                    "PropertiesChanged",
                    &(
                        DEVICE_IFACE,
                        std::collections::HashMap::from([("State", owned(40u32))]),
                        Vec::<String>::new(),
                    ),
                )
                .await
                .map_err(fdo_error)?;
            let ap_emitter =
                SignalEmitter::new(emitter.connection(), AP_PATH).map_err(fdo_error)?;
            ap_emitter
                .emit(
                    PROPERTIES_IFACE,
                    "PropertiesChanged",
                    &(
                        AP_IFACE,
                        std::collections::HashMap::from([
                            ("Strength", owned(99u8)),
                            ("Ssid", owned(vec![0xfe_u8, b'n'])),
                        ]),
                        Vec::<String>::new(),
                    ),
                )
                .await
                .map_err(fdo_error)?;
        }
        Ok(())
    }

    async fn activate_connection(
        &self,
        _connection: OwnedObjectPath,
        _device: OwnedObjectPath,
        _specific_object: OwnedObjectPath,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0.activate_calls.fetch_add(1, Ordering::Relaxed);
        *self
            .0
            .active_connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ACTIVE_PATH.to_owned();
        emitter.device_added().await.map_err(fdo_error)?;
        Ok(object_path(ACTIVE_PATH))
    }

    async fn add_and_activate_connection2(
        &self,
        settings: std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>>,
        _device: OwnedObjectPath,
        _specific_object: OwnedObjectPath,
        options: std::collections::HashMap<String, OwnedValue>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<
        (
            OwnedObjectPath,
            OwnedObjectPath,
            std::collections::HashMap<String, OwnedValue>,
        ),
        FakeError,
    > {
        self.0.add_calls.fetch_add(1, Ordering::Relaxed);
        let persistence = options
            .get("persist")
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok());
        if persistence.as_deref() != Some("memory") {
            return Err(FakeError::Failed(
                "fresh Wi-Fi profile was not memory-only".into(),
            ));
        }
        self.0.memory_persistence.store(true, Ordering::Release);

        let agent_owner = self
            .0
            .agent_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| FakeError::Failed("source agent was not registered".into()))?;
        let proxy = zbus::Proxy::new(
            emitter.connection(),
            agent_owner,
            AGENT_PATH,
            "org.freedesktop.NetworkManager.SecretAgent",
        )
        .await
        .map_err(|error| FakeError::Failed(error.to_string()))?;
        let secrets: std::collections::HashMap<
            String,
            std::collections::HashMap<String, OwnedValue>,
        > = proxy
            .call(
                "GetSecrets",
                &(
                    settings,
                    object_path(TEMP_PATH),
                    "802-11-wireless-security",
                    Vec::<String>::new(),
                    1u32,
                ),
            )
            .await
            .map_err(|error| FakeError::Failed(error.to_string()))?;
        let psk = secrets
            .get("802-11-wireless-security")
            .and_then(|values| values.get("psk"))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .ok_or_else(|| FakeError::Failed("SecretAgent returned no PSK".into()))?;
        *self
            .0
            .requested_psk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(psk);
        *self
            .0
            .active_connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ACTIVE_PATH.to_owned();
        emitter
            .device_added()
            .await
            .map_err(|error| FakeError::Failed(error.to_string()))?;
        Ok((
            object_path(TEMP_PATH),
            object_path(ACTIVE_PATH),
            std::collections::HashMap::new(),
        ))
    }

    #[zbus(signal)]
    async fn device_added(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct FakeAgentManager(FakeState);

#[zbus::interface(name = "org.freedesktop.NetworkManager.AgentManager")]
impl FakeAgentManager {
    async fn register_with_capabilities(
        &self,
        _identifier: String,
        _capabilities: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let owner = header
            .sender()
            .map(|sender| sender.as_str().to_owned())
            .ok_or_else(|| zbus::fdo::Error::Failed("missing caller name".into()))?;
        *self
            .0
            .agent_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(owner);
        self.0.registrations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct FakeSettings;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl FakeSettings {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        vec![object_path(SAVED_PATH)]
    }
}

struct FakeSavedConnection;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl FakeSavedConnection {
    fn get_settings(
        &self,
    ) -> std::collections::HashMap<String, std::collections::HashMap<String, OwnedValue>> {
        let connection = std::collections::HashMap::from([
            ("id".to_owned(), owned("Saved Wi-Fi")),
            ("type".to_owned(), owned("802-11-wireless")),
        ]);
        let wireless =
            std::collections::HashMap::from([("ssid".to_owned(), owned(b"saved-net".to_vec()))]);
        std::collections::HashMap::from([
            ("connection".to_owned(), connection),
            ("802-11-wireless".to_owned(), wireless),
        ])
    }
}

struct FakeDevice {
    state: FakeState,
    device_type: u32,
    interface: &'static str,
}

impl FakeDevice {
    fn new(state: FakeState, device_type: u32, interface: &'static str) -> Self {
        Self {
            state,
            device_type,
            interface,
        }
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl FakeDevice {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.device_type
    }

    #[zbus(property)]
    fn interface(&self) -> String {
        self.interface.to_owned()
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        self.state.device_state.load(Ordering::Acquire) as u32
    }

    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        let path = self
            .state
            .active_connection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        object_path(&path)
    }

    fn disconnect(&self) -> zbus::fdo::Result<()> {
        self.state.disconnect_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct FakeWireless;

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl FakeWireless {
    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        object_path("/")
    }

    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        vec![object_path(AP_PATH)]
    }
}

struct FakeAccessPoint(FakeState);

#[zbus::interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl FakeAccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.0
            .ap_ssid
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.0.ap_strength.load(Ordering::Acquire) as u8
    }

    #[zbus(property)]
    fn frequency(&self) -> u32 {
        5180
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        0
    }

    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        0x100
    }
}

fn fdo_error(error: zbus::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}

fn wait_ready(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::WifiState>>,
) -> bkss_integration::WifiState {
    wait_for(cx, state, |_| true)
}

fn wait_for(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::WifiState>>,
    predicate: impl Fn(&bkss_integration::WifiState) -> bool,
) -> bkss_integration::WifiState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if let SourceState::Ready(value) = state.get(cx)
            && predicate(&value)
        {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "Wi-Fi source did not become ready"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn answer_before(
    requests: impl std::future::Future<Output = Result<WifiAuthRequest, async_channel::RecvError>>,
) -> WifiAuthRequest {
    match futures_lite::future::block_on(futures_lite::future::race(
        async { Some(requests.await.ok()) },
        async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            None
        },
    )) {
        Some(Some(request)) => request,
        _ => panic!("Wi-Fi credential prompt timed out"),
    }
}

#[test]
fn source_snapshot_events_and_new_saved_disconnect_controls_share_live_worker() {
    let bus = FakeBus::start(true, false);
    let cx = TestAppContext::single();
    let scope = cx.update(ResourceScope::new);
    let descriptor = Wifi::with_address(bus.address.clone());
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let reactive = lease.reactive();

    let initial = wait_ready(&cx, &reactive);
    assert!(!initial.radio_enabled);
    assert_eq!(initial.devices.len(), 1);
    assert_eq!(initial.devices[0].state, WifiDeviceState::Disconnected);
    assert_eq!(
        initial.devices[0].access_points[0].ssid.as_ref(),
        &[0xff, b'n']
    );
    assert_eq!(initial.devices[0].access_points[0].display_name, "�n");
    assert_eq!(
        initial.devices[0].access_points[0].security,
        WifiSecurity::Wpa2
    );
    assert_eq!(initial.saved_connections[0].path, SAVED_PATH);
    assert_eq!(
        initial.saved_connections[0].display_name.as_deref(),
        Some("saved-net")
    );
    assert_eq!(bus.state.registrations.load(Ordering::Acquire), 1);

    let enable_result =
        futures_lite::future::block_on(initial.operations.enable().expect("dispatch enable"));
    assert!(
        enable_result.is_ok(),
        "enable failed: {enable_result:?}; source failure: {:?}",
        scope.failure()
    );
    let enabled = wait_for(&cx, &reactive, |state| {
        state.radio_enabled
            && state.devices[0].state == WifiDeviceState::Preparing
            && state.devices[0].access_points[0].strength == 99
    });
    assert!(
        enabled.radio_enabled,
        "event after subscription was reflected"
    );
    assert_eq!(
        enabled.devices[0].access_points[0].ssid.as_ref(),
        &[0xfe, b'n']
    );
    assert_eq!(enabled.devices[0].access_points[0].display_name, "�n");
    assert!(bus.state.wireless_enabled.load(Ordering::Acquire));

    futures_lite::future::block_on(enabled.operations.disable().expect("dispatch disable"))
        .expect("disable radio");
    let disabled = wait_for(&cx, &reactive, |state| !state.radio_enabled);
    assert!(!bus.state.wireless_enabled.load(Ordering::Acquire));

    futures_lite::future::block_on(
        disabled
            .operations
            .connect_saved(SAVED_PATH)
            .expect("dispatch saved activation"),
    )
    .expect("activate saved connection");
    assert_eq!(bus.state.activate_calls.load(Ordering::Acquire), 1);

    let connect = disabled
        .operations
        .connect(AP_PATH)
        .expect("dispatch temporary activation");
    let result = thread::spawn(move || futures_lite::future::block_on(connect));
    let auth_request = answer_before(disabled.next_auth_request());
    assert_eq!(auth_request.context.display_name, "�n");
    auth_request
        .responder
        .respond(WifiCredentials {
            password: Some(bkss_integration::Secret::new("do-not-persist".to_owned())),
            identity: None,
            username: None,
        })
        .expect("answer request");
    result
        .join()
        .expect("operation worker")
        .expect("temporary activation and SecretAgent callback");
    let connected = wait_for(&cx, &reactive, |state| {
        state.active_connection.as_deref() == Some(ACTIVE_PATH)
    });
    assert_eq!(connected.active_connection.as_deref(), Some(ACTIVE_PATH));
    assert_eq!(bus.state.add_calls.load(Ordering::Acquire), 1);
    assert!(bus.state.memory_persistence.load(Ordering::Acquire));
    assert_eq!(
        bus.state
            .requested_psk
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref(),
        Some("do-not-persist")
    );

    futures_lite::future::block_on(
        disabled
            .operations
            .disconnect(DEVICE_PATH)
            .expect("dispatch disconnect"),
    )
    .expect("disconnect device");
    assert_eq!(bus.state.disconnect_calls.load(Ordering::Acquire), 1);

    drop(lease);
    assert_eq!(
        disabled.operations.enable().err(),
        Some(DispatchError::Inactive)
    );
}

#[test]
fn missing_adapter_and_disabled_radio_are_normal_ready_states() {
    let bus = FakeBus::start(false, false);
    let cx = TestAppContext::single();
    let scope = cx.update(ResourceScope::new);
    let descriptor = Wifi::with_address(bus.address.clone());
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate source"));
    let state = wait_ready(&cx, &lease.reactive());

    assert!(!state.radio_enabled);
    assert!(state.devices.is_empty());
    assert!(state.saved_connections.len() == 1);
    assert!(scope.failure().is_none());
}

use bkss_core::{ResourceScope, SourceState};
use bkss_integration::{
    Bluetooth, BluetoothAgentRequest, BluetoothAuthInputError, BluetoothConfig, PinCodeAnswer,
    RespondError,
};
use gpui::TestAppContext;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use zbus::{
    Connection, Proxy, fdo, interface,
    message::Header,
    names::OwnedInterfaceName,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const ADAPTER_PATH: &str = "/org/bluez/hci0";
const DEVICE_PATH: &str = "/org/bluez/hci0/dev_AA";
const AGENT_INTERFACE: &str = "org.bluez.Agent1";

type Properties = HashMap<String, OwnedValue>;
type Interfaces = HashMap<OwnedInterfaceName, Properties>;
type ManagedObjects = HashMap<OwnedObjectPath, Interfaces>;

#[test]
fn private_bluez_agent_pair_cancel_and_release_lifecycle() {
    let (mut daemon, address) = private_bus();
    let (fake, stop_fake, control) = fake_bluez(&address);
    let cx = TestAppContext::single();
    let scope = cx.update(ResourceScope::new);
    let descriptor = Bluetooth::with_config(BluetoothConfig::private_bus(address.clone()));
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("activate Bluetooth"));
    let reactive = lease.reactive();

    let initial = wait_ready(&cx, &scope, &reactive, |state| {
        state.adapters.len() == 1 && state.devices.len() == 1
    });
    assert!(initial.adapters[0].powered);
    let registration = control
        .shared
        .registration
        .lock()
        .unwrap()
        .clone()
        .expect("agent registered before initial publication");
    assert_eq!(registration.path, "/org/bkss/BluetoothAgent");
    assert_eq!(registration.capability, "KeyboardDisplay");
    assert_eq!(control.shared.register_count.load(Ordering::Relaxed), 1);
    // The fake AgentManager1 intentionally has no RequestDefaultAgent method.

    call_display_pin_code(&control.connection, &registration);
    let state = wait_ready(&cx, &scope, &reactive, |state| {
        state.agent_displays.len() == 1
    });
    match &state.agent_displays[0] {
        bkss_integration::BluetoothAgentDisplay::PinCode { device, pin, .. } => {
            assert_eq!(device, DEVICE_PATH);
            assert_eq!(pin, "5931");
        }
        other => panic!("expected PIN display update, got {other:?}"),
    }

    call_display_passkey(&control.connection, &registration);
    let state = wait_ready(&cx, &scope, &reactive, |state| {
        state.agent_displays.len() == 2
    });
    match &state.agent_displays[1] {
        bkss_integration::BluetoothAgentDisplay::Passkey {
            device,
            passkey,
            entered,
            ..
        } => {
            assert_eq!(device, DEVICE_PATH);
            assert_eq!(*passkey, 123_456);
            assert_eq!(*entered, 4);
        }
        other => panic!("expected passkey display update, got {other:?}"),
    }

    let update = current_state(&cx, &reactive)
        .set_powered(ADAPTER_PATH, false)
        .expect("power control admitted");
    let update_result = spawn_operation(update);
    control.set_powered(false);
    assert!(
        !wait_ready(&cx, &scope, &reactive, |state| {
            state
                .adapters
                .first()
                .is_some_and(|adapter| !adapter.powered)
        })
        .adapters[0]
            .powered
    );
    assert!(
        update_result
            .recv_timeout(Duration::from_secs(3))
            .expect("power result")
            .is_ok()
    );
    control.add_hotplugged_device();
    assert_eq!(
        wait_ready(&cx, &scope, &reactive, |state| state.devices.len() == 2)
            .devices
            .len(),
        2
    );
    control.remove_hotplugged_device();
    assert_eq!(
        wait_ready(&cx, &scope, &reactive, |state| state.devices.len() == 1)
            .devices
            .len(),
        1
    );

    let pair = initial.pair(DEVICE_PATH).expect("pair command admitted");
    let pair_result = spawn_operation(pair);
    let pending = wait_for_auth(&cx, &reactive);
    let responder = match &pending {
        BluetoothAgentRequest::PinCode { responder, .. } => responder.clone(),
        other => panic!("expected PIN request, got {other:?}"),
    };

    let attacker = futures_lite::future::block_on(
        zbus::connection::Builder::address(address.as_str())
            .expect("attacker connection address")
            .build(),
    )
    .expect("attacker bus connection");
    let wrong_sender = futures_lite::future::block_on(async {
        let proxy = Proxy::new(
            &attacker,
            registration.owner.as_str(),
            registration.path.as_str(),
            AGENT_INTERFACE,
        )
        .await
        .expect("agent proxy");
        let device = OwnedObjectPath::try_from(DEVICE_PATH).expect("device path");
        proxy
            .call::<_, _, String>("RequestPinCode", &(device,))
            .await
    });
    assert!(wrong_sender.is_err(), "non-BlueZ sender must be rejected");

    assert!(matches!(
        PinCodeAnswer::provide("bad\npin"),
        Err(BluetoothAuthInputError::InvalidPinCode)
    ));
    responder
        .respond(PinCodeAnswer::provide("7314").expect("valid PIN"))
        .expect("answer pending PIN request");
    assert!(
        pair_result
            .recv_timeout(Duration::from_secs(3))
            .expect("pair result")
            .is_ok()
    );
    assert_eq!(
        *control.shared.last_pin.lock().unwrap(),
        Some("7314".into())
    );
    wait_for_no_auth(&cx, &reactive);

    let pair = current_state(&cx, &reactive)
        .pair(DEVICE_PATH)
        .expect("second pair");
    let pair_result = spawn_operation(pair);
    let pending = wait_for_auth(&cx, &reactive);
    let responder = match pending {
        BluetoothAgentRequest::PinCode { responder, .. } => responder,
        other => panic!("expected PIN request, got {other:?}"),
    };
    call_agent(&control.connection, &registration, "Cancel");
    assert_eq!(
        responder.respond(PinCodeAnswer::provide("1234").unwrap()),
        Err(RespondError::Cancelled)
    );
    assert!(
        pair_result
            .recv_timeout(Duration::from_secs(3))
            .expect("cancelled pair result")
            .is_err()
    );
    wait_for_no_auth(&cx, &reactive);

    let pair = current_state(&cx, &reactive)
        .pair(DEVICE_PATH)
        .expect("third pair");
    let pair_result = spawn_operation(pair);
    let pending = wait_for_auth(&cx, &reactive);
    let responder = match pending {
        BluetoothAgentRequest::PinCode { responder, .. } => responder,
        other => panic!("expected PIN request, got {other:?}"),
    };
    call_agent(&control.connection, &registration, "Release");
    assert_eq!(
        responder.respond(PinCodeAnswer::provide("1234").unwrap()),
        Err(RespondError::Cancelled)
    );
    assert!(
        pair_result
            .recv_timeout(Duration::from_secs(3))
            .expect("released pair result")
            .is_err()
    );

    // Release stops new requests. Reactivation uses the same descriptor key and
    // a fresh source activation, so test teardown cancellation on a pending request.
    drop(lease);
    let deadline = Instant::now() + Duration::from_secs(3);
    while control.shared.registration.lock().unwrap().is_some() {
        assert!(
            Instant::now() < deadline,
            "agent was not unregistered on release"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let lease = cx.update(|cx| {
        scope
            .acquire(&descriptor, cx)
            .expect("reactivate Bluetooth")
    });
    let reactive = lease.reactive();
    let state = wait_ready(&cx, &scope, &reactive, |state| state.devices.len() == 1);
    let pair = state.pair(DEVICE_PATH).expect("pair after reactivation");
    let pair_result = spawn_operation(pair);
    let pending = wait_for_auth(&cx, &reactive);
    let responder = match pending {
        BluetoothAgentRequest::PinCode { responder, .. } => responder,
        other => panic!("expected PIN request, got {other:?}"),
    };
    drop(lease);
    assert_eq!(
        responder.respond(PinCodeAnswer::provide("1234").unwrap()),
        Err(RespondError::Cancelled)
    );
    assert!(
        pair_result
            .recv_timeout(Duration::from_secs(3))
            .expect("unmounted pair result")
            .is_err()
    );
    drop(attacker);
    stop_fake.try_send(()).expect("stop fake BlueZ server");
    fake.join()
        .expect("fake BlueZ server stops after bus shutdown");
    daemon.kill().expect("stop private bus");
    daemon.wait().expect("reap private bus");
}

fn private_bus() -> (Child, String) {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start private dbus-daemon");
    let mut address = String::new();
    BufReader::new(child.stdout.take().expect("daemon stdout"))
        .read_line(&mut address)
        .expect("read private bus address");
    assert!(!address.trim().is_empty(), "private bus address is empty");
    (child, address.trim().to_owned())
}

#[derive(Clone)]
struct Registration {
    owner: String,
    path: String,
    capability: String,
}

struct Shared {
    objects: Mutex<ManagedObjects>,
    powered: AtomicBool,
    registration: Mutex<Option<Registration>>,
    register_count: std::sync::atomic::AtomicUsize,
    last_pin: Mutex<Option<String>>,
}

#[derive(Clone)]
struct FakeManager(Arc<Shared>);

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl FakeManager {
    async fn get_managed_objects(&self) -> ManagedObjects {
        let mut objects = self.0.objects.lock().unwrap().clone();
        if let Some((_, adapter)) = objects
            .iter_mut()
            .find(|(path, _)| path.as_str() == ADAPTER_PATH)
            && let Some(properties) = adapter.get_mut("org.bluez.Adapter1")
        {
            properties.insert(
                "Powered".into(),
                OwnedValue::from(self.0.powered.load(Ordering::Acquire)),
            );
        }
        objects
    }
}

#[derive(Clone)]
struct FakeAdapter(Arc<Shared>);

#[interface(name = "org.bluez.Adapter1")]
impl FakeAdapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.0.powered.load(Ordering::Acquire)
    }

    #[zbus(property)]
    fn set_powered(&self, value: bool) {
        self.0.powered.store(value, Ordering::Release);
    }

    async fn start_discovery(&self) {}
    async fn stop_discovery(&self) {}
    async fn remove_device(&self, _device: OwnedObjectPath) {}
}

#[derive(Clone)]
struct FakeDevice {
    connection: Connection,
    shared: Arc<Shared>,
}

#[interface(name = "org.bluez.Device1")]
impl FakeDevice {
    async fn connect(&self) {}
    async fn disconnect(&self) {}
    async fn cancel_pairing(&self) {}

    async fn pair(&self) -> fdo::Result<()> {
        let registration = self
            .shared
            .registration
            .lock()
            .unwrap()
            .as_ref()
            .map(|registration| Registration {
                owner: registration.owner.clone(),
                path: registration.path.clone(),
                capability: registration.capability.clone(),
            })
            .ok_or_else(|| fdo::Error::Failed("agent not registered".into()))?;
        let proxy = Proxy::new(
            &self.connection,
            registration.owner.as_str(),
            registration.path.as_str(),
            AGENT_INTERFACE,
        )
        .await
        .map_err(|error| fdo::Error::Failed(error.to_string()))?;
        let device = OwnedObjectPath::try_from(DEVICE_PATH).expect("test device path");
        let pin: String = proxy
            .call("RequestPinCode", &(device,))
            .await
            .map_err(|error| fdo::Error::Failed(error.to_string()))?;
        *self.shared.last_pin.lock().unwrap() = Some(pin);
        Ok(())
    }
}

#[derive(Clone)]
struct FakeAgentManager(Arc<Shared>);

#[interface(name = "org.bluez.AgentManager1")]
impl FakeAgentManager {
    async fn register_agent(
        &self,
        path: OwnedObjectPath,
        capability: String,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let owner = header
            .sender()
            .map(ToString::to_string)
            .ok_or_else(|| fdo::Error::Failed("registration caller missing".into()))?;
        *self.0.registration.lock().unwrap() = Some(Registration {
            owner,
            path: path.to_string(),
            capability,
        });
        self.0.register_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn unregister_agent(&self, _path: OwnedObjectPath) -> fdo::Result<()> {
        *self.0.registration.lock().unwrap() = None;
        Ok(())
    }
}

struct FakeControl {
    connection: Connection,
    shared: Arc<Shared>,
}

impl FakeControl {
    fn set_powered(&self, powered: bool) {
        self.shared.powered.store(powered, Ordering::Release);
        let changed = HashMap::from([("Powered", OwnedValue::from(powered))]);
        futures_lite::future::block_on(self.connection.emit_signal(
            None::<&str>,
            ADAPTER_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &("org.bluez.Adapter1", changed, Vec::<String>::new()),
        ))
        .expect("emit PropertiesChanged");
    }

    fn add_hotplugged_device(&self) {
        let path = "/org/bluez/hci0/dev_BB";
        let interfaces = HashMap::from([(
            interface("org.bluez.Device1"),
            HashMap::from([("Paired".into(), OwnedValue::from(false))]),
        )]);
        self.shared.objects.lock().unwrap().insert(
            OwnedObjectPath::try_from(path).expect("hotplug path"),
            interfaces.clone(),
        );
        futures_lite::future::block_on(self.connection.emit_signal(
            None::<&str>,
            "/",
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesAdded",
            &(
                OwnedObjectPath::try_from(path).expect("hotplug path"),
                interfaces,
            ),
        ))
        .expect("emit InterfacesAdded");
    }

    fn remove_hotplugged_device(&self) {
        let path = "/org/bluez/hci0/dev_BB";
        self.shared
            .objects
            .lock()
            .unwrap()
            .remove(&OwnedObjectPath::try_from(path).expect("hotplug path"));
        futures_lite::future::block_on(self.connection.emit_signal(
            None::<&str>,
            "/",
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesRemoved",
            &(
                OwnedObjectPath::try_from(path).expect("hotplug path"),
                vec![interface("org.bluez.Device1")],
            ),
        ))
        .expect("emit InterfacesRemoved");
    }
}

fn fake_bluez(
    address: &str,
) -> (
    thread::JoinHandle<()>,
    async_channel::Sender<()>,
    Arc<FakeControl>,
) {
    let address = address.to_owned();
    let shared = Arc::new(Shared {
        objects: Mutex::new(initial_objects()),
        powered: AtomicBool::new(true),
        registration: Mutex::new(None),
        register_count: std::sync::atomic::AtomicUsize::new(0),
        last_pin: Mutex::new(None),
    });
    let thread_shared = Arc::clone(&shared);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let (stop_tx, stop_rx) = async_channel::bounded(1);
    let worker = thread::spawn(move || {
        futures_lite::future::block_on(async move {
            let result: zbus::Result<Connection> = async {
                let connection = zbus::connection::Builder::address(address.as_str())?
                    .build()
                    .await?;
                connection.request_name("org.bluez").await?;
                connection
                    .object_server()
                    .at("/", FakeManager(Arc::clone(&thread_shared)))
                    .await?;
                connection
                    .object_server()
                    .at("/org/bluez", FakeAgentManager(Arc::clone(&thread_shared)))
                    .await?;
                connection
                    .object_server()
                    .at(ADAPTER_PATH, FakeAdapter(Arc::clone(&thread_shared)))
                    .await?;
                connection
                    .object_server()
                    .at(
                        DEVICE_PATH,
                        FakeDevice {
                            connection: connection.clone(),
                            shared: Arc::clone(&thread_shared),
                        },
                    )
                    .await?;
                Ok(connection)
            }
            .await;
            match result {
                Ok(connection) => {
                    ready_tx
                        .send(Ok(connection))
                        .expect("send fake bus readiness");
                    let _ = stop_rx.recv().await;
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                }
            }
        });
    });
    let connection = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("fake BlueZ startup")
        .expect("fake BlueZ connection");
    (
        worker,
        stop_tx,
        Arc::new(FakeControl { connection, shared }),
    )
}

fn initial_objects() -> ManagedObjects {
    let adapter = OwnedObjectPath::try_from(ADAPTER_PATH).unwrap();
    let device = OwnedObjectPath::try_from(DEVICE_PATH).unwrap();
    HashMap::from([
        (
            adapter,
            HashMap::from([(
                interface("org.bluez.Adapter1"),
                HashMap::from([
                    ("Powered".into(), OwnedValue::from(true)),
                    ("Discoverable".into(), OwnedValue::from(false)),
                    ("Discovering".into(), OwnedValue::from(false)),
                ]),
            )]),
        ),
        (
            device,
            HashMap::from([(
                interface("org.bluez.Device1"),
                HashMap::from([
                    ("Paired".into(), OwnedValue::from(false)),
                    ("Connected".into(), OwnedValue::from(false)),
                    ("Trusted".into(), OwnedValue::from(false)),
                ]),
            )]),
        ),
    ])
}

fn interface(name: &str) -> OwnedInterfaceName {
    OwnedInterfaceName::try_from(name).expect("valid interface")
}

fn wait_ready(
    cx: &TestAppContext,
    scope: &bkss_core::ResourceScope,
    reactive: &bkss_core::Reactive<SourceState<bkss_integration::BluetoothState>>,
    predicate: impl Fn(&bkss_integration::BluetoothState) -> bool,
) -> bkss_integration::BluetoothState {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let current = reactive.get(cx);
        if let SourceState::Ready(state) = current.clone()
            && predicate(&state)
        {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "Bluetooth state did not match condition: {current:?}; failure: {:?}",
            scope.failure()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn current_state(
    cx: &TestAppContext,
    reactive: &bkss_core::Reactive<SourceState<bkss_integration::BluetoothState>>,
) -> bkss_integration::BluetoothState {
    match reactive.get(cx) {
        SourceState::Ready(state) => state,
        state => panic!("Bluetooth source is not ready: {state:?}"),
    }
}

fn wait_for_auth(
    cx: &TestAppContext,
    reactive: &bkss_core::Reactive<SourceState<bkss_integration::BluetoothState>>,
) -> BluetoothAgentRequest {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        cx.run_until_parked();
        if let SourceState::Ready(state) = reactive.get(cx)
            && let Some(request) = state.authentication_requests.first()
        {
            return request.clone();
        }
        assert!(
            Instant::now() < deadline,
            "Bluetooth authentication request timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_no_auth(
    cx: &TestAppContext,
    reactive: &bkss_core::Reactive<SourceState<bkss_integration::BluetoothState>>,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        cx.run_until_parked();
        if let SourceState::Ready(state) = reactive.get(cx)
            && state.authentication_requests.is_empty()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Bluetooth authentication request did not clear"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_operation(
    operation: bkss_integration::Operation<(), bkss_integration::BluetoothError>,
) -> mpsc::Receiver<Result<(), bkss_integration::OperationError<bkss_integration::BluetoothError>>>
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(futures_lite::future::block_on(operation));
    });
    receiver
}

fn call_agent(connection: &Connection, registration: &Registration, method: &str) {
    let proxy = futures_lite::future::block_on(Proxy::new(
        connection,
        registration.owner.as_str(),
        registration.path.as_str(),
        AGENT_INTERFACE,
    ))
    .expect("BlueZ agent proxy");
    futures_lite::future::block_on(proxy.call::<_, _, ()>(method, &()))
        .expect("BlueZ Agent1 method succeeds");
}

fn call_display_pin_code(connection: &Connection, registration: &Registration) {
    let proxy = futures_lite::future::block_on(Proxy::new(
        connection,
        registration.owner.as_str(),
        registration.path.as_str(),
        AGENT_INTERFACE,
    ))
    .expect("BlueZ agent proxy");
    let device = OwnedObjectPath::try_from(DEVICE_PATH).expect("device path");
    futures_lite::future::block_on(proxy.call::<_, _, ()>("DisplayPinCode", &(device, "5931")))
        .expect("two-argument DisplayPinCode succeeds");
}

fn call_display_passkey(connection: &Connection, registration: &Registration) {
    let proxy = futures_lite::future::block_on(Proxy::new(
        connection,
        registration.owner.as_str(),
        registration.path.as_str(),
        AGENT_INTERFACE,
    ))
    .expect("BlueZ agent proxy");
    let device = OwnedObjectPath::try_from(DEVICE_PATH).expect("device path");
    futures_lite::future::block_on(
        proxy.call::<_, _, ()>("DisplayPasskey", &(device, 123_456_u32, 4_u16)),
    )
    .expect("three-argument DisplayPasskey succeeds");
}

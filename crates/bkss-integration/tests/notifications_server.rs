use std::{
    error::Error,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use bkss_core::{ResourceScope, SourceState};
use bkss_integration::{Notifications, NotificationsError, OperationError};
use futures_lite::StreamExt;
use gpui::TestAppContext;
use zbus::{Connection, Proxy, fdo::RequestNameFlags, zvariant::OwnedValue};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const IFACE: &str = "org.freedesktop.Notifications";

struct PrivateBus {
    child: Child,
    address: String,
}

impl PrivateBus {
    fn start() -> std::io::Result<Option<Self>> {
        let mut child = match Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
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
        Ok(Some(Self {
            child,
            address: address.trim().to_owned(),
        }))
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_ready(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::NotificationsState>>,
    count: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        cx.run_until_parked();
        if matches!(state.get(cx), SourceState::Ready(ref value) if value.notifications.len() == count)
        {
            return;
        }
        assert!(Instant::now() < deadline, "notification state timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_summary(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::NotificationsState>>,
    expected: &str,
) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        cx.run_until_parked();
        if matches!(
            state.get(cx),
            SourceState::Ready(ref value)
                if value.notifications.iter().any(|item| item.summary == expected)
        ) {
            return;
        }
        assert!(Instant::now() < deadline, "notification update timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_summary_within(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::NotificationsState>>,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        cx.run_until_parked();
        if matches!(
            state.get(cx),
            SourceState::Ready(ref value)
                if value.notifications.iter().any(|item| item.summary == expected)
        ) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "event-driven update missed its deadline"
        );
        thread::sleep(Duration::from_millis(2));
    }
}

fn assert_count_for(
    cx: &TestAppContext,
    state: &bkss_core::Reactive<SourceState<bkss_integration::NotificationsState>>,
    count: usize,
    duration: Duration,
) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        cx.run_until_parked();
        assert!(
            matches!(state.get(cx), SourceState::Ready(ref value) if value.notifications.len() == count),
            "notification count changed during the observation interval"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

async fn connect(address: &str) -> zbus::Result<Connection> {
    zbus::connection::Builder::address(address)?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await
}

async fn next_signal<S>(stream: &mut S) -> zbus::Message
where
    S: futures_lite::stream::Stream<Item = zbus::Message> + Unpin,
{
    futures_lite::future::race(stream.next(), async {
        async_io::Timer::after(Duration::from_secs(3)).await;
        None
    })
    .await
    .expect("timed out waiting for D-Bus signal")
}

async fn notify(
    proxy: &Proxy<'_>,
    replaces_id: u32,
    summary: &str,
    expire_timeout: i32,
) -> zbus::Result<u32> {
    notify_actions(
        proxy,
        replaces_id,
        summary,
        vec!["open".to_owned(), "Open".to_owned()],
        expire_timeout,
    )
    .await
}

async fn notify_actions(
    proxy: &Proxy<'_>,
    replaces_id: u32,
    summary: &str,
    actions: Vec<String>,
    expire_timeout: i32,
) -> zbus::Result<u32> {
    let args = (
        "test-app",
        replaces_id,
        "",
        summary,
        "plain body",
        actions,
        std::collections::HashMap::<String, OwnedValue>::new(),
        expire_timeout,
    );
    let reply = proxy.call_method("Notify", &args).await?;
    reply.body().deserialize()
}

async fn notify_resident(proxy: &Proxy<'_>, summary: &str) -> zbus::Result<u32> {
    let hints = std::collections::HashMap::from([("resident".to_owned(), OwnedValue::from(true))]);
    let args = (
        "test-app",
        0_u32,
        "",
        summary,
        "plain body",
        vec!["open".to_owned(), "Open".to_owned()],
        hints,
        0_i32,
    );
    proxy
        .call_method("Notify", &args)
        .await?
        .body()
        .deserialize()
}

#[test]
fn protocol_replacement_action_close_expiry_and_lifetime() {
    let Some(bus) = PrivateBus::start().expect("start private dbus-daemon") else {
        eprintln!("skipped: dbus-daemon is not installed");
        return;
    };
    futures_lite::future::block_on(async {
        let cx = TestAppContext::single();
        let scope = cx.update(ResourceScope::new);
        let descriptor = Notifications::with_address(bus.address.clone());
        let lease = cx.update(|cx| scope.acquire(&descriptor, cx).expect("mount source"));
        let reactive = lease.reactive();
        wait_ready(&cx, &reactive, 0);

        let client = connect(&bus.address).await.expect("private bus connection");
        let proxy = Proxy::new(&client, NAME, PATH, IFACE)
            .await
            .expect("notification proxy");
        let dbus = Proxy::new(
            &client,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .expect("DBus proxy");
        let capabilities: Vec<String> = proxy
            .call("GetCapabilities", &())
            .await
            .expect("GetCapabilities");
        assert_eq!(capabilities, ["body", "actions"]);
        let server_info: (String, String, String, String) = proxy
            .call("GetServerInformation", &())
            .await
            .expect("GetServerInformation");
        assert_eq!(server_info.0, "BKSS");

        let id = notify(&proxy, 0, "first", -1).await.expect("Notify");
        assert_ne!(id, 0);
        wait_ready(&cx, &reactive, 1);
        assert_count_for(&cx, &reactive, 1, Duration::from_millis(350));
        let replaced = notify(&proxy, id, "replacement", 0)
            .await
            .expect("replacement Notify");
        assert_eq!(replaced, id);
        wait_summary_within(&cx, &reactive, "replacement", Duration::from_millis(200));
        assert!(matches!(
            reactive.get(&cx),
            SourceState::Ready(ref value)
                if value.notifications[0].summary == "replacement"
        ));

        let mut action_signal = proxy
            .receive_signal("ActionInvoked")
            .await
            .expect("action signal");
        let mut closed_signal = proxy
            .receive_signal("NotificationClosed")
            .await
            .expect("closed signal");
        let state = match reactive.get(&cx) {
            SourceState::Ready(state) => state,
            _ => panic!("notification source is not ready"),
        };
        let invalid_action = state
            .invoke_action(id, "missing")
            .expect("dispatch unknown action");
        assert!(matches!(
            futures_lite::future::block_on(invalid_action),
            Err(OperationError::Backend(
                NotificationsError::UnknownAction { .. }
            ))
        ));
        futures_lite::future::block_on(state.invoke_action(id, "open").expect("dispatch action"))
            .expect("invoke action");
        let action = next_signal(&mut action_signal).await;
        assert_eq!(
            action
                .body()
                .deserialize::<(u32, String)>()
                .expect("signal body"),
            (id, "open".into())
        );
        let closed = next_signal(&mut closed_signal).await;
        assert_eq!(
            closed
                .body()
                .deserialize::<(u32, u32)>()
                .expect("signal body"),
            (id, 2)
        );
        wait_ready(&cx, &reactive, 0);

        let resident = notify_resident(&proxy, "resident")
            .await
            .expect("resident Notify");
        wait_summary(&cx, &reactive, "resident");
        let mut resident_action = proxy
            .receive_signal("ActionInvoked")
            .await
            .expect("resident action signal");
        let mut resident_closed = proxy
            .receive_signal("NotificationClosed")
            .await
            .expect("resident close signal");
        let state = match reactive.get(&cx) {
            SourceState::Ready(state) => state,
            _ => panic!("notification source is not ready"),
        };
        futures_lite::future::block_on(
            state
                .invoke_action(resident, "open")
                .expect("dispatch action"),
        )
        .expect("invoke resident action");
        assert_eq!(
            next_signal(&mut resident_action)
                .await
                .body()
                .deserialize::<(u32, String)>()
                .expect("resident ActionInvoked"),
            (resident, "open".into())
        );
        assert!(
            futures_lite::future::race(resident_closed.next(), async {
                async_io::Timer::after(Duration::from_millis(100)).await;
                None
            })
            .await
            .is_none(),
            "resident action must not close the notification"
        );
        assert_count_for(&cx, &reactive, 1, Duration::from_millis(350));
        proxy
            .call::<_, _, ()>("CloseNotification", &(resident,))
            .await
            .expect("close resident notification");
        assert_eq!(
            next_signal(&mut resident_closed)
                .await
                .body()
                .deserialize::<(u32, u32)>()
                .expect("resident NotificationClosed"),
            (resident, 3)
        );
        wait_ready(&cx, &reactive, 0);

        let expiration_started = Instant::now();
        let expiring = notify(&proxy, 0, "expires", 50)
            .await
            .expect("expiring Notify");
        let mut expired_signal = proxy
            .receive_signal("NotificationClosed")
            .await
            .expect("expiration signal");
        let signal = next_signal(&mut expired_signal).await;
        assert!(
            expiration_started.elapsed() < Duration::from_millis(200),
            "positive timeout expired later than its deadline allowance"
        );
        assert_eq!(
            signal
                .body()
                .deserialize::<(u32, u32)>()
                .expect("signal body"),
            (expiring, 1)
        );
        wait_ready(&cx, &reactive, 0);

        let no_actions = notify_actions(&proxy, 0, "no actions", vec![], 0)
            .await
            .expect("notification without actions");
        wait_summary(&cx, &reactive, "no actions");
        let state = match reactive.get(&cx) {
            SourceState::Ready(state) => state,
            _ => panic!("notification source is not ready"),
        };
        let operation = state
            .invoke_action(no_actions, "open")
            .expect("dispatch missing action");
        assert!(matches!(
            futures_lite::future::block_on(operation),
            Err(OperationError::Backend(
                NotificationsError::UnknownAction { .. }
            ))
        ));
        let odd_actions = proxy
            .call_method(
                "Notify",
                &(
                    "test-app",
                    0_u32,
                    "",
                    "invalid",
                    "",
                    vec!["unpaired".to_owned()],
                    std::collections::HashMap::<String, OwnedValue>::new(),
                    0_i32,
                ),
            )
            .await;
        assert!(odd_actions.is_err(), "odd action list must be rejected");
        let too_many_actions = (0..34).map(|value| value.to_string()).collect();
        assert!(
            notify_actions(&proxy, 0, "too many actions", too_many_actions, 0)
                .await
                .is_err(),
            "action limit must be enforced"
        );
        proxy
            .call::<_, _, ()>("CloseNotification", &(no_actions,))
            .await
            .expect("close no-actions notification");
        wait_ready(&cx, &reactive, 0);

        let manually_closed = notify(&proxy, 0, "manual", 0).await.expect("manual Notify");
        wait_summary(&cx, &reactive, "manual");
        assert_count_for(&cx, &reactive, 1, Duration::from_millis(350));
        let mut manually_closed_signal = proxy
            .receive_signal("NotificationClosed")
            .await
            .expect("manual close signal");
        proxy
            .call::<_, _, ()>("CloseNotification", &(manually_closed,))
            .await
            .expect("CloseNotification");
        let signal = next_signal(&mut manually_closed_signal).await;
        assert_eq!(
            signal
                .body()
                .deserialize::<(u32, u32)>()
                .expect("signal body"),
            (manually_closed, 3)
        );
        wait_ready(&cx, &reactive, 0);

        let mut ids = Vec::with_capacity(128);
        for index in 0..128 {
            ids.push(
                notify(&proxy, 0, &format!("bounded-{index}"), 0)
                    .await
                    .expect("within-limit Notify"),
            );
        }
        assert!(ids.iter().all(|id| *id != 0));
        let unique_ids = ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique_ids.len(), ids.len());
        wait_ready(&cx, &reactive, 128);
        assert!(
            notify(&proxy, 0, "over-limit", 0).await.is_err(),
            "active notification cap must be enforced"
        );
        assert_eq!(
            notify(&proxy, ids[0], "replace-at-cap", 0)
                .await
                .expect("replace at cap"),
            ids[0]
        );

        let state = match reactive.get(&cx) {
            SourceState::Ready(state) => state,
            _ => panic!("notification source is not ready"),
        };
        drop(lease);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(
                dbus.call::<_, _, bool>("NameHasOwner", &(NAME,)).await,
                Ok(false)
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "server name was not released");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            state.dismiss(ids[0]).err(),
            Some(bkss_integration::DispatchError::Inactive)
        );
    });
}

#[test]
fn name_collision_does_not_replace_owner_and_can_be_acquired_after_release() {
    let Some(bus) = PrivateBus::start().expect("start private dbus-daemon") else {
        eprintln!("skipped: dbus-daemon is not installed");
        return;
    };
    futures_lite::future::block_on(async {
        let owner = connect(&bus.address).await.expect("owner connection");
        owner
            .request_name_with_flags(
                NAME,
                RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement,
            )
            .await
            .expect("claim name");
        let dbus = Proxy::new(
            &owner,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .expect("DBus proxy");
        let original_owner: String = dbus
            .call("GetNameOwner", &(NAME,))
            .await
            .expect("record original owner");
        let cx = TestAppContext::single();
        let scope = cx.update(ResourceScope::new);
        let descriptor = Notifications::with_address(bus.address.clone());
        let lease = cx.update(|cx| {
            scope
                .acquire(&descriptor, cx)
                .expect("mount collision source")
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while scope.failure().is_none() {
            cx.run_until_parked();
            assert!(Instant::now() < deadline, "name conflict was not reported");
            thread::sleep(Duration::from_millis(5));
        }
        let failure = scope.failure().expect("source failure");
        let cause = failure.source().expect("typed cause");
        let notification_error = cause
            .downcast_ref::<NotificationsError>()
            .expect("typed notifications failure");
        assert!(matches!(
            notification_error,
            NotificationsError::NameConflict { .. }
        ));
        assert!(
            notification_error
                .source()
                .is_some_and(|source| source.downcast_ref::<zbus::Error>().is_some())
        );
        let current_owner: String = dbus
            .call("GetNameOwner", &(NAME,))
            .await
            .expect("check original owner");
        assert_eq!(current_owner, original_owner);

        drop(lease);
        owner
            .release_name(NAME)
            .await
            .expect("release original name");
        let fresh_scope = cx.update(ResourceScope::new);
        let replacement = Notifications::with_address(bus.address.clone());
        let fresh = cx.update(|cx| {
            fresh_scope
                .acquire(&replacement, cx)
                .expect("fresh activation")
        });
        let state = fresh.reactive();
        wait_ready(&cx, &state, 0);
    });
}

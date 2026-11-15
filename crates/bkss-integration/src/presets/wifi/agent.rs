use super::state::{
    AuthRequests, WifiAuthContext, WifiAuthRequest, WifiCredentialField, WifiCredentials,
    WifiSecurity, ssid_display,
};
use crate::base::{RespondError, Responder, auth};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use zbus::{
    DBusError,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue},
};

#[cfg(test)]
const IFACE: &str = "org.freedesktop.NetworkManager.SecretAgent";
pub(super) const OBJECT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const TIMEOUT: Duration = Duration::from_secs(120);
const ALLOW_INTERACTION: u32 = 1;

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
enum AgentError {
    NoSecrets(String),
    UserCanceled(String),
    PermissionDenied(String),
}

type RequestKey = (String, String, u64);
type Waiting = Arc<Mutex<HashMap<RequestKey, Vec<Responder<WifiCredentials>>>>>;

#[derive(Clone)]
pub(crate) struct SecretAgent {
    sender: async_channel::Sender<WifiAuthRequest>,
    waiting: Waiting,
    network_manager_owner: String,
    next_request: Arc<AtomicU64>,
}

impl SecretAgent {
    pub(crate) fn new(network_manager_owner: String) -> (Self, AuthRequests) {
        let (sender, receiver) = async_channel::bounded(8);
        (
            Self {
                sender,
                waiting: Arc::new(Mutex::new(HashMap::new())),
                network_manager_owner,
                next_request: Arc::new(AtomicU64::new(0)),
            },
            AuthRequests::new(receiver),
        )
    }

    pub(crate) fn invalidate_all(&self) {
        for responders in self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            for responder in responders {
                responder.invalidate();
            }
        }
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.sender.close();
    }
}

impl Drop for SecretAgent {
    fn drop(&mut self) {
        self.invalidate_all();
    }
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl SecretAgent {
    async fn get_secrets(
        &self,
        connection: HashMap<String, HashMap<String, OwnedValue>>,
        connection_path: OwnedObjectPath,
        setting_name: String,
        _hints: Vec<String>,
        flags: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<HashMap<String, HashMap<String, OwnedValue>>, AgentError> {
        self.verify_sender(&header)?;
        if flags & ALLOW_INTERACTION == 0 {
            return Err(AgentError::NoSecrets(
                "interaction was not permitted".into(),
            ));
        }
        if !matches!(setting_name.as_str(), "802-11-wireless-security" | "802-1x") {
            return Err(AgentError::NoSecrets(
                "requested setting is not supported".into(),
            ));
        }

        let context = auth_context(&connection);
        let required_fields = context.required_fields.clone();
        let (responder, pending) = auth::request(TIMEOUT)
            .map_err(|_| AgentError::NoSecrets("credential request unavailable".into()))?;
        let key = (
            connection_path.to_string(),
            setting_name.clone(),
            self.next_request.fetch_add(1, Ordering::Relaxed),
        );
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key.clone())
            .or_default()
            .push(responder.clone());

        if self
            .sender
            .try_send(WifiAuthRequest { context, responder })
            .is_err()
        {
            self.remove_waiter(&key);
            return Err(AgentError::NoSecrets(
                "credential request queue is unavailable".into(),
            ));
        }

        let answer = pending.receive().await;
        self.remove_waiter(&key);
        let answer = answer.map_err(|error| match error {
            RespondError::Expired => AgentError::NoSecrets("credential request expired".into()),
            RespondError::Cancelled => {
                AgentError::UserCanceled("credential request cancelled".into())
            }
            RespondError::AlreadyCompleted | RespondError::InvalidExpiry => {
                AgentError::NoSecrets("credential request is no longer available".into())
            }
        })?;
        if required_fields.iter().any(|field| match field {
            WifiCredentialField::Password => answer.password.is_none(),
            WifiCredentialField::Identity => answer.identity.is_none(),
            WifiCredentialField::Username => answer.username.is_none(),
        }) {
            return Err(AgentError::NoSecrets(
                "one or more required credentials were not supplied".into(),
            ));
        }

        let key_mgmt = connection
            .get("802-11-wireless-security")
            .and_then(|values| values.get("key-mgmt"))
            .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
            .unwrap_or_default();
        let setting = if key_mgmt == "wpa-eap" || key_mgmt == "ieee8021x" {
            "802-1x"
        } else {
            "802-11-wireless-security"
        };
        if setting_name != setting {
            return Err(AgentError::NoSecrets(
                "requested setting does not match the credential type".into(),
            ));
        }
        let mut result = HashMap::new();
        let mut credentials = HashMap::new();
        if let Some(password) = answer.password {
            let field = if key_mgmt == "none" {
                "wep-key0"
            } else if setting == "802-1x" {
                "password"
            } else {
                "psk"
            };
            let value = OwnedValue::try_from(zbus::zvariant::Value::from(password.into_inner()))
                .map_err(|_| AgentError::NoSecrets("credential could not be encoded".into()))?;
            credentials.insert(field.to_owned(), value);
        }
        for (key, value) in [
            ("identity", answer.identity),
            ("leap-username", answer.username),
        ] {
            if let Some(value) = value {
                let value = OwnedValue::try_from(zbus::zvariant::Value::from(value.into_inner()))
                    .map_err(|_| {
                    AgentError::NoSecrets("credential could not be encoded".into())
                })?;
                credentials.insert(key.to_owned(), value);
            }
        }
        if credentials.is_empty() {
            return Err(AgentError::NoSecrets("no credentials were supplied".into()));
        }
        result.insert(setting.to_owned(), credentials);
        Ok(result)
    }

    async fn cancel_get_secrets(
        &self,
        connection_path: OwnedObjectPath,
        setting_name: String,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.verify_sender(&header)?;
        let mut waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = waiting
            .keys()
            .filter(|(path, setting, _)| {
                path == connection_path.as_str() && setting == &setting_name
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(responders) = waiting.remove(&key) {
                for responder in responders {
                    responder.invalidate();
                }
            }
        }
        Ok(())
    }

    async fn save_secrets(
        &self,
        _connection: HashMap<String, HashMap<String, OwnedValue>>,
        _connection_path: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.verify_sender(&header)?;
        Err(AgentError::NoSecrets(
            "this agent never persists secrets".into(),
        ))
    }

    async fn delete_secrets(
        &self,
        _connection: HashMap<String, HashMap<String, OwnedValue>>,
        _connection_path: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.verify_sender(&header)
    }
}

impl SecretAgent {
    fn verify_sender(&self, header: &Header<'_>) -> Result<(), AgentError> {
        let sender = header.sender().map(|sender| sender.as_str()).unwrap_or("");
        if sender == self.network_manager_owner {
            Ok(())
        } else {
            Err(AgentError::PermissionDenied(
                "caller is not the NetworkManager service owner".into(),
            ))
        }
    }

    fn remove_waiter(&self, key: &RequestKey) {
        let mut waiting = self
            .waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        waiting.remove(key);
    }
}

fn auth_context(settings: &HashMap<String, HashMap<String, OwnedValue>>) -> WifiAuthContext {
    let wireless = settings.get("802-11-wireless");
    let ssid = wireless
        .and_then(|values| values.get("ssid"))
        .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
        .unwrap_or_default();
    let security = match settings
        .get("802-11-wireless-security")
        .and_then(|values| values.get("key-mgmt"))
        .and_then(|value| String::try_from(value.try_clone().ok()?).ok())
        .as_deref()
    {
        Some("wpa-psk") => WifiSecurity::Wpa2,
        Some("sae") => WifiSecurity::Wpa3,
        Some("wpa-eap") | Some("ieee8021x") => WifiSecurity::Wpa,
        Some("none") => WifiSecurity::Wep,
        _ => WifiSecurity::Unknown,
    };
    let display_name = ssid_display(&ssid);
    let mut required_fields = vec![WifiCredentialField::Password];
    if security == WifiSecurity::Wpa {
        required_fields = vec![WifiCredentialField::Identity, WifiCredentialField::Password];
    }
    WifiAuthContext {
        ssid,
        display_name,
        security,
        required_fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Secret;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        sync::mpsc::{self, Receiver},
        thread,
    };
    use zbus::{Connection, connection::Builder};

    type Settings = HashMap<String, HashMap<String, OwnedValue>>;
    type SecretsReply = Result<Settings, zbus::Error>;
    type SecretsResponse = Receiver<SecretsReply>;

    #[test]
    fn authentication_context_preserves_ssid_bytes_without_copying_settings() {
        let mut wireless = HashMap::new();
        wireless.insert(
            "ssid".to_owned(),
            OwnedValue::try_from(zbus::zvariant::Value::from(vec![0xff_u8, 0x00, 0x41]))
                .expect("byte array value"),
        );
        let mut security = HashMap::new();
        security.insert(
            "key-mgmt".to_owned(),
            OwnedValue::try_from(zbus::zvariant::Value::from("wpa-psk")).expect("string value"),
        );
        let settings = HashMap::from([
            ("802-11-wireless".to_owned(), wireless),
            ("802-11-wireless-security".to_owned(), security),
        ]);

        let context = auth_context(&settings);
        assert_eq!(context.ssid, [0xff, 0x00, 0x41]);
        assert_eq!(context.security, WifiSecurity::Wpa2);
        assert_eq!(context.required_fields, [WifiCredentialField::Password]);
        assert_eq!(context.display_name, "�\\u{0}A");
    }

    #[test]
    fn debug_output_redacts_all_credential_fields() {
        let credentials = WifiCredentials {
            password: Some(crate::base::Secret::new("secret-password".to_owned())),
            identity: Some(crate::base::Secret::new("private-identity".to_owned())),
            username: Some(crate::base::Secret::new("private-user".to_owned())),
        };

        let debug = format!("{credentials:?}");
        assert!(!debug.contains("secret-password"));
        assert!(!debug.contains("private-identity"));
        assert!(!debug.contains("private-user"));
    }

    struct PrivateBus {
        process: Child,
        address: String,
    }

    impl PrivateBus {
        fn start() -> Self {
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
            Self {
                process,
                address: address.trim().to_owned(),
            }
        }

        fn stop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn connections() -> (
        PrivateBus,
        Connection,
        Connection,
        String,
        SecretAgent,
        AuthRequests,
    ) {
        let bus = PrivateBus::start();
        let (manager, agent_connection) = futures_lite::future::block_on(async {
            let manager = Builder::address(bus.address.as_str())
                .expect("manager bus address")
                .name("org.freedesktop.NetworkManager")
                .expect("manager name")
                .build()
                .await
                .expect("manager connection");
            let agent_connection = Builder::address(bus.address.as_str())
                .expect("agent bus address")
                .build()
                .await
                .expect("agent connection");
            (manager, agent_connection)
        });
        let owner = manager
            .unique_name()
            .expect("manager unique name")
            .to_string();
        let destination = agent_connection
            .unique_name()
            .expect("agent unique name")
            .to_string();
        let (agent, requests) = SecretAgent::new(owner);
        futures_lite::future::block_on(
            agent_connection
                .object_server()
                .at(OBJECT_PATH, agent.clone()),
        )
        .expect("export SecretAgent");
        (bus, manager, agent_connection, destination, agent, requests)
    }

    fn client_call(
        connection: Connection,
        call: impl FnOnce(Connection) -> SecretsReply + Send + 'static,
    ) -> SecretsResponse {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(call(connection));
        });
        receiver
    }

    fn get_secrets(
        connection: Connection,
        destination: String,
        setting_name: &'static str,
        flags: u32,
    ) -> SecretsResponse {
        client_call(connection, move |connection| {
            futures_lite::future::block_on(async move {
                let proxy = zbus::Proxy::new(&connection, destination, OBJECT_PATH, IFACE).await?;
                proxy
                    .call(
                        "GetSecrets",
                        &(
                            auth_settings(),
                            OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Settings/1")
                                .expect("connection object path"),
                            setting_name,
                            Vec::<String>::new(),
                            flags,
                        ),
                    )
                    .await
            })
        })
    }

    fn auth_settings() -> Settings {
        let mut wireless = HashMap::new();
        wireless.insert(
            "ssid".to_owned(),
            OwnedValue::try_from(zbus::zvariant::Value::from(b"test-net".to_vec()))
                .expect("SSID value"),
        );
        let mut security = HashMap::new();
        security.insert(
            "key-mgmt".to_owned(),
            OwnedValue::try_from(zbus::zvariant::Value::from("wpa-psk"))
                .expect("key management value"),
        );
        HashMap::from([
            ("802-11-wireless".to_owned(), wireless),
            ("802-11-wireless-security".to_owned(), security),
        ])
    }

    fn next_request(requests: &AuthRequests) -> Option<WifiAuthRequest> {
        futures_lite::future::block_on(futures_lite::future::race(
            async { requests.recv().await.ok() },
            async {
                async_io::Timer::after(Duration::from_secs(3)).await;
                None
            },
        ))
    }

    fn result_before(receiver: &SecretsResponse) -> SecretsReply {
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("SecretAgent method response")
    }

    #[test]
    fn private_bus_secret_agent_answers_once_and_returns_only_requested_secret() {
        let (_bus, manager, _agent_connection, destination, _agent, requests) = connections();
        let response = get_secrets(
            manager,
            destination,
            "802-11-wireless-security",
            ALLOW_INTERACTION,
        );
        let request = next_request(&requests).expect("credential request");
        assert_eq!(request.context.ssid, b"test-net");
        request
            .responder
            .respond(WifiCredentials {
                password: Some(Secret::new("private-passphrase".to_owned())),
                identity: None,
                username: None,
            })
            .expect("answer once");
        assert_eq!(
            request.responder.respond(WifiCredentials {
                password: Some(Secret::new("second-password".to_owned())),
                identity: None,
                username: None,
            }),
            Err(RespondError::AlreadyCompleted)
        );

        let result = result_before(&response).expect("secret response");
        let security = result
            .get("802-11-wireless-security")
            .expect("only requested setting");
        let password = String::try_from(
            security
                .get("psk")
                .expect("PSK value")
                .try_clone()
                .expect("clone value"),
        )
        .expect("PSK string");
        assert_eq!(password, "private-passphrase");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn private_bus_caller_cancellation_returns_user_canceled_without_secrets() {
        let (_bus, manager, _agent_connection, destination, _agent, requests) = connections();
        let response = get_secrets(
            manager,
            destination,
            "802-11-wireless-security",
            ALLOW_INTERACTION,
        );
        let request = next_request(&requests).expect("credential request");
        let cloned_responder = request.responder.clone();

        request.cancel();

        assert_eq!(
            request.responder.respond(WifiCredentials {
                password: Some(Secret::new("must-not-publish".to_owned())),
                identity: None,
                username: None,
            }),
            Err(RespondError::Cancelled)
        );
        assert_eq!(
            cloned_responder.respond(WifiCredentials {
                password: Some(Secret::new("must-not-publish-either".to_owned())),
                identity: None,
                username: None,
            }),
            Err(RespondError::Cancelled)
        );
        let error = result_before(&response).expect_err("cancelled request returns no secrets");
        assert!(
            error
                .to_string()
                .contains("org.freedesktop.NetworkManager.SecretAgent.UserCanceled"),
            "unexpected SecretAgent cancellation error: {error}"
        );
    }

    #[test]
    fn private_bus_rejects_wrong_sender_and_interaction_disabled_without_prompt() {
        let (bus, manager, _agent_connection, destination, _agent, requests) = connections();
        let stranger = futures_lite::future::block_on(async {
            Builder::address(bus.address.as_str())
                .expect("stranger bus address")
                .build()
                .await
                .expect("stranger connection")
        });
        let wrong_sender = get_secrets(
            stranger,
            destination.clone(),
            "802-11-wireless-security",
            ALLOW_INTERACTION,
        );
        let error = result_before(&wrong_sender).expect_err("wrong sender rejected");
        assert!(error.to_string().contains("PermissionDenied"));

        let no_interaction = get_secrets(
            manager.clone(),
            destination.clone(),
            "802-11-wireless-security",
            0,
        );
        let error = result_before(&no_interaction).expect_err("NoSecrets error");
        assert!(error.to_string().contains("NoSecrets"));
        assert!(requests.try_recv().is_err());

        futures_lite::future::block_on(async {
            let proxy = zbus::Proxy::new(&manager, destination, OBJECT_PATH, IFACE)
                .await
                .expect("SecretAgent proxy");
            let connection_path =
                OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Settings/1")
                    .expect("connection object path");
            let save_error = proxy
                .call::<_, _, ()>("SaveSecrets", &(auth_settings(), connection_path.clone()))
                .await
                .expect_err("saving must fail rather than claim persistence");
            assert!(save_error.to_string().contains("NoSecrets"));
            proxy
                .call::<_, _, ()>("DeleteSecrets", &(auth_settings(), connection_path))
                .await
                .expect("delete is idempotent");
        });
    }

    #[test]
    fn private_bus_cancel_invalidates_every_matching_responder() {
        let (_bus, manager, _agent_connection, destination, _agent, requests) = connections();
        let response = get_secrets(
            manager.clone(),
            destination.clone(),
            "802-11-wireless-security",
            ALLOW_INTERACTION,
        );
        let second_response = get_secrets(
            manager.clone(),
            destination.clone(),
            "802-11-wireless-security",
            ALLOW_INTERACTION,
        );
        let request = next_request(&requests).expect("first credential request");
        let second_request = next_request(&requests).expect("second credential request");
        futures_lite::future::block_on(async {
            let proxy = zbus::Proxy::new(&manager, destination, OBJECT_PATH, IFACE)
                .await
                .expect("SecretAgent proxy");
            proxy
                .call::<_, _, ()>(
                    "CancelGetSecrets",
                    &(
                        OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/Settings/1")
                            .expect("connection object path"),
                        "802-11-wireless-security",
                    ),
                )
                .await
                .expect("cancel pending request");
        });
        assert_eq!(
            request.responder.respond(WifiCredentials {
                password: Some(Secret::new("late".to_owned())),
                identity: None,
                username: None,
            }),
            Err(RespondError::Cancelled)
        );
        assert_eq!(
            second_request.responder.respond(WifiCredentials {
                password: Some(Secret::new("also late".to_owned())),
                identity: None,
                username: None,
            }),
            Err(RespondError::Cancelled)
        );
        assert!(result_before(&response).is_err());
        assert!(result_before(&second_response).is_err());
    }
}

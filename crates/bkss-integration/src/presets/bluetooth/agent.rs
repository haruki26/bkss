use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    Responder, Secret,
    base::{auth, auth::Pending},
};
use async_channel::{Receiver, Sender};
use zbus::{Connection, DBusError, interface, message::Header, zvariant::OwnedObjectPath};

use super::BluetoothError;
use super::backend;

pub(super) const AGENT_PATH: &str = "/org/bkss/BluetoothAgent";
const AGENT_MANAGER_PATH: &str = "/org/bluez";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_PENDING: usize = 32;
const MAX_DISPLAYS: usize = 16;

#[derive(Debug, DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    Canceled(String),
    Failed(String),
    InvalidArguments(String),
    NotAuthorized(String),
    Rejected(String),
}

type AgentResult<T> = Result<T, AgentError>;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BluetoothAuthInputError {
    InvalidPinCode,
    PasskeyOutOfRange,
}

impl fmt::Display for BluetoothAuthInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPinCode => "PIN code must contain 1 to 16 printable characters",
            Self::PasskeyOutOfRange => "passkey must be between 0 and 999999",
        })
    }
}

impl std::error::Error for BluetoothAuthInputError {}

pub struct PinCodeAnswer(PinCodeAnswerKind);

enum PinCodeAnswerKind {
    Provide(Secret<String>),
    Reject,
}

impl PinCodeAnswer {
    pub fn provide(pin: impl Into<String>) -> Result<Self, BluetoothAuthInputError> {
        let pin = pin.into();
        if pin.is_empty() || pin.len() > 16 || pin.chars().any(char::is_control) {
            return Err(BluetoothAuthInputError::InvalidPinCode);
        }
        Ok(Self(PinCodeAnswerKind::Provide(Secret::new(pin))))
    }

    pub fn reject() -> Self {
        Self(PinCodeAnswerKind::Reject)
    }
}

impl fmt::Debug for PinCodeAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PinCodeAnswerKind::Provide(secret) => f.debug_tuple("Provide").field(secret).finish(),
            PinCodeAnswerKind::Reject => f.write_str("Reject"),
        }
    }
}

pub struct PasskeyAnswer(PasskeyAnswerKind);

enum PasskeyAnswerKind {
    Provide(u32),
    Reject,
}

impl PasskeyAnswer {
    pub fn provide(passkey: u32) -> Result<Self, BluetoothAuthInputError> {
        if passkey > 999_999 {
            return Err(BluetoothAuthInputError::PasskeyOutOfRange);
        }
        Ok(Self(PasskeyAnswerKind::Provide(passkey)))
    }

    pub fn reject() -> Self {
        Self(PasskeyAnswerKind::Reject)
    }
}

impl fmt::Debug for PasskeyAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PasskeyAnswerKind::Provide(_) => f.write_str("Provide([REDACTED])"),
            PasskeyAnswerKind::Reject => f.write_str("Reject"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationAnswer {
    Confirm,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationAnswer {
    Authorize,
    Reject,
}

pub enum BluetoothAgentRequest {
    PinCode {
        id: u64,
        device: String,
        expires_at: Instant,
        responder: Responder<PinCodeAnswer>,
    },
    Passkey {
        id: u64,
        device: String,
        expires_at: Instant,
        responder: Responder<PasskeyAnswer>,
    },
    Confirmation {
        id: u64,
        device: String,
        passkey: u32,
        expires_at: Instant,
        responder: Responder<ConfirmationAnswer>,
    },
    Authorization {
        id: u64,
        device: String,
        service: Option<String>,
        expires_at: Instant,
        responder: Responder<AuthorizationAnswer>,
    },
}

impl BluetoothAgentRequest {
    pub fn id(&self) -> u64 {
        match self {
            Self::PinCode { id, .. }
            | Self::Passkey { id, .. }
            | Self::Confirmation { id, .. }
            | Self::Authorization { id, .. } => *id,
        }
    }

    pub fn device(&self) -> &str {
        match self {
            Self::PinCode { device, .. }
            | Self::Passkey { device, .. }
            | Self::Confirmation { device, .. }
            | Self::Authorization { device, .. } => device,
        }
    }

    pub fn expires_at(&self) -> Instant {
        match self {
            Self::PinCode { expires_at, .. }
            | Self::Passkey { expires_at, .. }
            | Self::Confirmation { expires_at, .. }
            | Self::Authorization { expires_at, .. } => *expires_at,
        }
    }

    fn invalidate(&self) {
        match self {
            Self::PinCode { responder, .. } => responder.invalidate(),
            Self::Passkey { responder, .. } => responder.invalidate(),
            Self::Confirmation { responder, .. } => responder.invalidate(),
            Self::Authorization { responder, .. } => responder.invalidate(),
        }
    }
}

impl Clone for BluetoothAgentRequest {
    fn clone(&self) -> Self {
        match self {
            Self::PinCode {
                id,
                device,
                expires_at,
                responder,
            } => Self::PinCode {
                id: *id,
                device: device.clone(),
                expires_at: *expires_at,
                responder: responder.clone(),
            },
            Self::Passkey {
                id,
                device,
                expires_at,
                responder,
            } => Self::Passkey {
                id: *id,
                device: device.clone(),
                expires_at: *expires_at,
                responder: responder.clone(),
            },
            Self::Confirmation {
                id,
                device,
                passkey,
                expires_at,
                responder,
            } => Self::Confirmation {
                id: *id,
                device: device.clone(),
                passkey: *passkey,
                expires_at: *expires_at,
                responder: responder.clone(),
            },
            Self::Authorization {
                id,
                device,
                service,
                expires_at,
                responder,
            } => Self::Authorization {
                id: *id,
                device: device.clone(),
                service: service.clone(),
                expires_at: *expires_at,
                responder: responder.clone(),
            },
        }
    }
}

impl fmt::Debug for BluetoothAgentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::PinCode { .. } => "PinCode",
            Self::Passkey { .. } => "Passkey",
            Self::Confirmation { .. } => "Confirmation",
            Self::Authorization { .. } => "Authorization",
        };
        f.debug_struct("BluetoothAgentRequest")
            .field("kind", &kind)
            .field("id", &self.id())
            .field("device", &self.device())
            .field("expires_at", &self.expires_at())
            .finish()
    }
}

#[derive(Clone)]
pub enum BluetoothAgentDisplay {
    PinCode {
        id: u64,
        device: String,
        pin: String,
    },
    Passkey {
        id: u64,
        device: String,
        passkey: u32,
        entered: u16,
    },
}

impl BluetoothAgentDisplay {
    pub fn id(&self) -> u64 {
        match self {
            Self::PinCode { id, .. } | Self::Passkey { id, .. } => *id,
        }
    }
}

impl fmt::Debug for BluetoothAgentDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PinCode { id, device, .. } => f
                .debug_struct("PinCodeDisplay")
                .field("id", id)
                .field("device", device)
                .finish(),
            Self::Passkey {
                id,
                device,
                entered,
                ..
            } => f
                .debug_struct("PasskeyDisplay")
                .field("id", id)
                .field("device", device)
                .field("entered", entered)
                .finish(),
        }
    }
}

pub(super) struct AgentStore {
    requests: Mutex<Vec<BluetoothAgentRequest>>,
    displays: Mutex<VecDeque<BluetoothAgentDisplay>>,
    updates: Sender<()>,
    next_id: AtomicU64,
    released: AtomicBool,
}

pub(super) struct AgentSession {
    connection: Connection,
    store: Arc<AgentStore>,
    registered: bool,
}

impl AgentSession {
    pub(super) async fn export(
        connection: &Connection,
        store: Arc<AgentStore>,
    ) -> Result<Self, BluetoothError> {
        connection
            .object_server()
            .at(
                AGENT_PATH,
                AgentInterface::new(connection.clone(), Arc::clone(&store)),
            )
            .await
            .map_err(|source| super::error::bus("export Bluetooth Agent1", source))?;
        let mut session = Self {
            connection: connection.clone(),
            store,
            registered: false,
        };
        if backend::bluez_owner(connection).await?.is_some()
            && let Err(error) = session.register().await
        {
            session.close().await;
            return Err(error);
        }
        Ok(session)
    }

    pub(super) fn store(&self) -> &Arc<AgentStore> {
        &self.store
    }

    pub(super) async fn owner_changed(
        &mut self,
        owner: Option<&str>,
    ) -> Result<(), BluetoothError> {
        if owner.is_some() {
            self.store.reset_for_owner();
            self.register().await
        } else {
            self.store.release();
            self.registered = false;
            Ok(())
        }
    }

    async fn register(&mut self) -> Result<(), BluetoothError> {
        let manager = zbus::Proxy::new(
            &self.connection,
            "org.bluez",
            AGENT_MANAGER_PATH,
            "org.bluez.AgentManager1",
        )
        .await
        .map_err(|source| super::error::bus("create BlueZ AgentManager1 proxy", source))?;
        let agent_path = OwnedObjectPath::try_from(AGENT_PATH)
            .map_err(|_| BluetoothError::InvalidObjectPath(AGENT_PATH.into()))?;
        let _: () = manager
            .call("RegisterAgent", &(agent_path, "KeyboardDisplay"))
            .await
            .map_err(|source| super::error::bus("register Bluetooth agent", source))?;
        self.registered = true;
        Ok(())
    }

    pub(super) async fn close(&mut self) {
        self.store.release();
        if self.registered {
            let agent_path = OwnedObjectPath::try_from(AGENT_PATH);
            if let Ok(manager) = zbus::Proxy::new(
                &self.connection,
                "org.bluez",
                AGENT_MANAGER_PATH,
                "org.bluez.AgentManager1",
            )
            .await
                && let Ok(agent_path) = agent_path
            {
                let _: zbus::Result<()> = manager.call("UnregisterAgent", &(agent_path,)).await;
            }
            self.registered = false;
        }
        let _ = self
            .connection
            .object_server()
            .remove::<AgentInterface, _>(AGENT_PATH)
            .await;
    }
}

impl AgentStore {
    pub(super) fn new() -> (Arc<Self>, Receiver<()>) {
        let (updates, receiver) = async_channel::bounded(64);
        (
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                displays: Mutex::new(VecDeque::new()),
                updates,
                next_id: AtomicU64::new(1),
                released: AtomicBool::new(false),
            }),
            receiver,
        )
    }

    fn lock_requests(&self) -> MutexGuard<'_, Vec<BluetoothAgentRequest>> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn id(&self) -> AgentResult<u64> {
        self.next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| AgentError::Failed("Agent request identifiers exhausted".into()))
    }

    fn insert(&self, request: BluetoothAgentRequest) -> AgentResult<()> {
        let mut requests = self.lock_requests();
        if self.released.load(Ordering::Acquire) {
            request.invalidate();
            return Err(AgentError::Failed("Agent has been released".into()));
        }
        if requests.len() >= MAX_PENDING {
            request.invalidate();
            return Err(AgentError::Failed(
                "Too many pending Bluetooth requests".into(),
            ));
        }
        requests.push(request);
        self.notify();
        Ok(())
    }

    fn remove(&self, id: u64) {
        let mut requests = self.lock_requests();
        requests.retain(|request| request.id() != id);
        self.notify();
    }

    fn notify(&self) {
        // The queue is bounded; a queued notification already guarantees the
        // consumer will snapshot the latest shared state.
        let _ = self.updates.try_send(());
    }

    pub(super) fn requests(&self) -> Vec<BluetoothAgentRequest> {
        self.lock_requests().clone()
    }

    pub(super) fn displays(&self) -> Vec<BluetoothAgentDisplay> {
        self.displays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn clear(&self) {
        let mut requests = self.lock_requests();
        for request in requests.iter() {
            request.invalidate();
        }
        requests.clear();
        self.displays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.notify();
    }

    pub(super) fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.clear();
    }

    pub(super) fn reset_for_owner(&self) {
        self.released.store(false, Ordering::Release);
    }

    fn push_display(&self, display: BluetoothAgentDisplay) -> AgentResult<()> {
        if self.released.load(Ordering::Acquire) {
            return Err(AgentError::Failed("Agent has been released".into()));
        }
        let mut displays = self
            .displays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if displays.len() == MAX_DISPLAYS {
            displays.pop_front();
        }
        displays.push_back(display);
        self.notify();
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct AgentInterface {
    connection: Connection,
    store: Arc<AgentStore>,
}

impl AgentInterface {
    pub(super) fn new(connection: Connection, store: Arc<AgentStore>) -> Self {
        Self { connection, store }
    }

    async fn authorize_sender(&self, header: &Header<'_>) -> AgentResult<()> {
        let Some(sender) = header.sender().map(ToString::to_string) else {
            return Err(AgentError::NotAuthorized(
                "Missing D-Bus caller identity".into(),
            ));
        };
        let owner = backend::bluez_owner(&self.connection)
            .await
            .map_err(|_| AgentError::Failed("Could not verify BlueZ caller".into()))?;
        if owner.as_deref() == Some(sender.as_str()) {
            Ok(())
        } else {
            Err(AgentError::NotAuthorized(
                "Only the current BlueZ owner may use this agent".into(),
            ))
        }
    }

    async fn begin<T>(
        &self,
        build: impl FnOnce(u64, Instant, Responder<T>) -> BluetoothAgentRequest,
    ) -> AgentResult<(u64, Pending<T>)> {
        let id = self.store.id()?;
        let expires_at = Instant::now()
            .checked_add(RESPONSE_TIMEOUT)
            .ok_or_else(|| AgentError::Failed("Authentication deadline out of range".into()))?;
        let (responder, pending) = auth::request(RESPONSE_TIMEOUT)
            .map_err(|_| AgentError::Failed("Authentication deadline out of range".into()))?;
        self.store.insert(build(id, expires_at, responder))?;
        Ok((id, pending))
    }

    async fn finish<T>(&self, id: u64, pending: Pending<T>) -> AgentResult<T> {
        let answer = pending.receive().await.map_err(|_| {
            AgentError::Canceled("Bluetooth authentication was cancelled or expired".into())
        });
        self.store.remove(id);
        answer
    }
}

#[interface(name = "org.bluez.Agent1")]
impl AgentInterface {
    async fn release(&self, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        self.store.release();
        Ok(())
    }

    async fn request_pin_code(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<String, AgentError> {
        self.authorize_sender(&header).await?;
        let (id, pending) = self
            .begin(|id, expires_at, responder| BluetoothAgentRequest::PinCode {
                id,
                device: device.to_string(),
                expires_at,
                responder,
            })
            .await?;
        match self.finish(id, pending).await?.0 {
            PinCodeAnswerKind::Provide(pin) => Ok(pin.into_inner()),
            PinCodeAnswerKind::Reject => Err(AgentError::Rejected("PIN entry rejected".into())),
        }
    }

    async fn display_pin_code(
        &self,
        device: OwnedObjectPath,
        pin: String,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        if pin.len() > 16 || pin.chars().any(char::is_control) {
            return Err(AgentError::InvalidArguments("Invalid display PIN".into()));
        }
        let id = self.store.id()?;
        self.store.push_display(BluetoothAgentDisplay::PinCode {
            id,
            device: device.to_string(),
            pin,
        })
    }

    async fn request_passkey(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<u32, AgentError> {
        self.authorize_sender(&header).await?;
        let (id, pending) = self
            .begin(|id, expires_at, responder| BluetoothAgentRequest::Passkey {
                id,
                device: device.to_string(),
                expires_at,
                responder,
            })
            .await?;
        match self.finish(id, pending).await?.0 {
            PasskeyAnswerKind::Provide(passkey) => Ok(passkey),
            PasskeyAnswerKind::Reject => Err(AgentError::Rejected("Passkey entry rejected".into())),
        }
    }

    async fn display_passkey(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        entered: u16,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        if passkey > 999_999 {
            return Err(AgentError::InvalidArguments(
                "Passkey exceeds 999999".into(),
            ));
        }
        let id = self.store.id()?;
        self.store.push_display(BluetoothAgentDisplay::Passkey {
            id,
            device: device.to_string(),
            passkey,
            entered,
        })
    }

    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        if passkey > 999_999 {
            return Err(AgentError::InvalidArguments(
                "Passkey exceeds 999999".into(),
            ));
        }
        let (id, pending) = self
            .begin(
                |id, expires_at, responder| BluetoothAgentRequest::Confirmation {
                    id,
                    device: device.to_string(),
                    passkey,
                    expires_at,
                    responder,
                },
            )
            .await?;
        match self.finish(id, pending).await? {
            ConfirmationAnswer::Confirm => Ok(()),
            ConfirmationAnswer::Reject => Err(AgentError::Rejected("Confirmation rejected".into())),
        }
    }

    async fn request_authorization(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        self.authorization(device.to_string(), None).await
    }

    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        service: String,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        if service.len() > 256 || service.contains('\0') {
            return Err(AgentError::InvalidArguments(
                "Invalid service identifier".into(),
            ));
        }
        self.authorization(device.to_string(), Some(service)).await
    }

    async fn cancel(&self, #[zbus(header)] header: Header<'_>) -> Result<(), AgentError> {
        self.authorize_sender(&header).await?;
        self.store.clear();
        Ok(())
    }
}

impl AgentInterface {
    async fn authorization(&self, device: String, service: Option<String>) -> AgentResult<()> {
        let (id, pending) = self
            .begin(
                |id, expires_at, responder| BluetoothAgentRequest::Authorization {
                    id,
                    device,
                    service,
                    expires_at,
                    responder,
                },
            )
            .await?;
        match self.finish(id, pending).await? {
            AuthorizationAnswer::Authorize => Ok(()),
            AuthorizationAnswer::Reject => {
                Err(AgentError::Rejected("Authorization rejected".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_and_passkey_validation_enforces_protocol_ranges() {
        assert!(PinCodeAnswer::provide("1234").is_ok());
        assert!(PinCodeAnswer::provide("").is_err());
        assert!(PinCodeAnswer::provide("x".repeat(17)).is_err());
        assert!(PinCodeAnswer::provide("a\nb").is_err());
        assert!(PasskeyAnswer::provide(0).is_ok());
        assert!(PasskeyAnswer::provide(999_999).is_ok());
        assert!(PasskeyAnswer::provide(1_000_000).is_err());
        assert_eq!(
            format!("{:?}", PinCodeAnswer::provide("secret").unwrap()),
            "Provide(Secret([REDACTED]))"
        );
    }
}

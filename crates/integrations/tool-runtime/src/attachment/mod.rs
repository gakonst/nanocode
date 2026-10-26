mod driver;
mod host;
pub(crate) mod management;
#[cfg(not(target_family = "wasm"))]
mod pool;
#[cfg(not(target_family = "wasm"))]
pub use pool::{
    Runner, RunnerEvent, RunnerHandle, RunnerInfo, RunnerPool, RunnerProtocol, RunnerServer,
    RunnerServerConfig, RunnerSpec, RunnerStatus,
};
pub(crate) mod process_routes;
mod protocol;
pub(crate) use driver::{INITIAL_TRANSPORT_BACKOFF, MAX_TRANSPORT_BACKOFF, STABLE_CONNECTION};
pub(crate) use protocol::HEARTBEAT_INTERVAL;
pub use protocol::{
    AttachmentCall, CommandStatus, DiagnosticStage, EndpointFrame, ReceiptTiming, RunnerFrame,
};

pub(crate) fn reconnect_delay(backoff: &mut std::time::Duration) -> std::time::Duration {
    driver::Failure::Transport.delay(backoff, None)
}
pub use nanocode_tool_host::attachment::{
    AttachmentCallOutcome, AttachmentError, ProcessRouteStore, RemoteMode, RunnerRouting,
};
pub(crate) mod routing;
mod runner;
pub use runner::{RunnerConfig, RunnerEndpoint, run_runner, run_runner_with_tools};

pub use host::{RemoteRunner, RemoteTool, RemoteTools, RunnerConnection, RunnerHost};

use std::{collections::HashSet, fmt, sync::Arc};
use tokio::sync::{mpsc, watch};
use url::{Host, Url};

use crate::{
    Tools,
    prepared::{PreparedToolError, PreparedToolRuntime, PreparedTools},
};

const MAX_ATTACHMENT_ID_BYTES: usize = 123;
const MAX_MACHINE_NAME_BYTES: usize = 128;
const MAX_MACHINE_WORKSPACE_BYTES: usize = 1024;
const MAX_MACHINE_CAPABILITIES: usize = 64;
const MAX_MACHINE_CAPABILITY_BYTES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "MachineWire")]
pub struct AttachmentMachine {
    id: Box<str>,
    name: Box<str>,
    workspace: Box<str>,
    capabilities: Box<[Box<str>]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resources: Option<AttachmentResourceObservation>,
}

impl AttachmentMachine {
    pub fn new<I, S>(
        id: impl Into<String>,
        name: impl Into<String>,
        workspace: impl Into<String>,
        capabilities: I,
    ) -> Result<Self, AttachmentError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let id = id.into();
        if !valid_safe_identifier(&id, MAX_ATTACHMENT_ID_BYTES) {
            return Err(AttachmentError::Catalog(
                "attachment machine id must be a safe ASCII identifier of at most 123 bytes".into(),
            ));
        }
        let name = name.into();
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_MACHINE_NAME_BYTES {
            return Err(AttachmentError::Catalog(
                "attachment machine name must be 1-128 UTF-8 bytes".into(),
            ));
        }
        let workspace = workspace.into();
        if workspace.trim().is_empty()
            || workspace.len() > MAX_MACHINE_WORKSPACE_BYTES
            || workspace.contains('\0')
        {
            return Err(AttachmentError::Catalog(
                "attachment machine workspace must be 1-1024 UTF-8 bytes without NUL".into(),
            ));
        }
        let capabilities = capabilities
            .into_iter()
            .map(Into::into)
            .collect::<Vec<String>>();
        if capabilities.len() > MAX_MACHINE_CAPABILITIES {
            return Err(AttachmentError::Catalog(
                "attachment machines support at most 64 capabilities".into(),
            ));
        }
        let mut unique = HashSet::with_capacity(capabilities.len());
        if capabilities
            .iter()
            .any(|capability| !valid_capability(capability) || !unique.insert(capability.as_str()))
        {
            return Err(AttachmentError::Catalog(
                "attachment machine capabilities must be unique safe ASCII identifiers".into(),
            ));
        }
        Ok(Self {
            id: id.into(),
            name: name.into(),
            workspace: workspace.into(),
            resources: None,
            capabilities: capabilities
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
        })
    }

    #[must_use]
    pub const fn resources(&self) -> Option<&AttachmentResourceObservation> {
        self.resources.as_ref()
    }

    #[must_use]
    pub const fn with_resources(mut self, resources: AttachmentResourceObservation) -> Self {
        self.resources = Some(resources);
        self
    }

    #[must_use]
    pub const fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub const fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn workspace(&self) -> &str {
        &self.workspace
    }

    #[must_use]
    pub const fn capabilities(&self) -> &[Box<str>] {
        &self.capabilities
    }
}

#[derive(serde::Deserialize)]
struct MachineWire {
    id: String,
    name: String,
    workspace: String,
    capabilities: Vec<String>,
    #[serde(default)]
    resources: Option<AttachmentResourceObservation>,
}

impl TryFrom<MachineWire> for AttachmentMachine {
    type Error = AttachmentError;

    fn try_from(wire: MachineWire) -> Result<Self, Self::Error> {
        let machine = Self::new(wire.id, wire.name, wire.workspace, wire.capabilities)?;
        Ok(match wire.resources {
            Some(resources) => machine.with_resources(resources),
            None => machine,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttachmentResourceObservation {
    pub observed_at_ms: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_logical_count: Option<u32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_available_bytes: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_total_bytes: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_available_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachmentMetadata {
    attachment_id: Box<str>,
    machine: Option<AttachmentMachine>,
}

impl AttachmentMetadata {
    pub fn named(attachment_id: impl Into<String>) -> Result<Self, AttachmentError> {
        let attachment_id = attachment_id.into();
        if !valid_safe_identifier(&attachment_id, MAX_ATTACHMENT_ID_BYTES) {
            return Err(AttachmentError::Catalog(
                "attachment id must be a safe ASCII identifier of at most 123 bytes".into(),
            ));
        }
        Ok(Self {
            attachment_id: attachment_id.into(),
            machine: None,
        })
    }

    #[must_use]
    pub fn machine(machine: AttachmentMachine) -> Self {
        Self {
            attachment_id: machine.id.clone(),
            machine: Some(machine),
        }
    }

    #[must_use]
    pub const fn attachment_id(&self) -> &str {
        &self.attachment_id
    }

    #[must_use]
    pub const fn attached_machine(&self) -> Option<&AttachmentMachine> {
        self.machine.as_ref()
    }
}

fn valid_safe_identifier(value: &str, max_bytes: usize) -> bool {
    value.len() <= max_bytes
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn valid_capability(value: &str) -> bool {
    value.len() <= MAX_MACHINE_CAPABILITY_BYTES
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
}

#[derive(Clone, Eq, PartialEq)]
pub struct AttachmentTarget {
    endpoint: Url,
    bearer: Arc<str>,
}

impl AttachmentTarget {
    pub fn new(
        endpoint: impl AsRef<str>,
        bearer: impl Into<String>,
    ) -> Result<Self, AttachmentError> {
        let endpoint = Url::parse(endpoint.as_ref())
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
        if !matches!(endpoint.scheme(), "ws" | "wss")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(AttachmentError::Transport(
                "attachment target must be a final ws/wss URL without credentials or a fragment"
                    .into(),
            ));
        }
        if endpoint.scheme() == "ws" && !is_literal_loopback(&endpoint) {
            return Err(AttachmentError::Transport(
                "plaintext attachment targets require a literal loopback host; use wss otherwise"
                    .into(),
            ));
        }
        let bearer = bearer.into();
        if bearer.trim().is_empty() {
            return Err(AttachmentError::Authentication(
                "bearer credential must not be empty".into(),
            ));
        }
        Ok(Self {
            endpoint,
            bearer: bearer.into(),
        })
    }

    #[must_use]
    pub const fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    #[must_use]
    pub fn bearer(&self) -> &str {
        &self.bearer
    }
}

impl fmt::Debug for AttachmentTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentTarget")
            .field("endpoint", &self.endpoint)
            .field("bearer", &"[REDACTED]")
            .finish()
    }
}

pub struct AttachmentConnector {
    tools: Tools,
    target: AttachmentTarget,
    metadata: Option<AttachmentMetadata>,
}

impl nanocode_tool_host::AttachTarget for AttachmentTarget {
    type Connector = AttachmentConnector;

    fn connect_tools(self, tools: Tools) -> AttachmentConnector {
        AttachmentConnector {
            tools,
            target: self,
            metadata: None,
        }
    }
}

impl AttachmentConnector {
    #[must_use]
    pub fn metadata(mut self, metadata: AttachmentMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    pub fn start(self) -> Result<(Attachment, AttachmentEvents), AttachmentError> {
        install_default_rustls_crypto_provider();
        let prepared = PreparedTools::prepare(&self.tools)?;
        let (command_tx, command_rx) = mpsc::channel(8);
        let (event_tx, event_rx) = mpsc::channel(128);
        let (status_tx, status_rx) = watch::channel(AttachmentStatus::Connecting);
        let (closed_tx, closed_rx) = watch::channel(None);
        let refs = Arc::new(HandleRefs { command_tx });
        tokio::spawn(initialize_and_run(
            prepared,
            self.target,
            self.metadata,
            command_rx,
            event_tx,
            status_tx,
            closed_tx,
        ));
        Ok((
            Attachment {
                refs,
                status: status_rx,
                closed: closed_rx,
            },
            AttachmentEvents { events: event_rx },
        ))
    }

    pub fn connect(
        self,
    ) -> futures_util::future::BoxFuture<
        'static,
        Result<(Attachment, AttachmentEvents), AttachmentError>,
    > {
        Box::pin(async move {
            let (attachment, events) = self.start()?;
            attachment.wait_until_ready().await?;
            Ok((attachment, events))
        })
    }
}

async fn initialize_and_run(
    prepared: PreparedTools,
    target: AttachmentTarget,
    metadata: Option<AttachmentMetadata>,
    mut command_rx: mpsc::Receiver<driver::Command>,
    event_tx: mpsc::Sender<AttachmentEvent>,
    status_tx: watch::Sender<AttachmentStatus>,
    closed_tx: watch::Sender<Option<Result<(), AttachmentError>>>,
) {
    let prepare_started = std::time::Instant::now();
    let runtime = tokio::select! {
        biased;
        command = command_rx.recv() => {
            debug_assert!(matches!(command, Some(driver::Command::Detach) | None));
            let _ = status_tx.send(AttachmentStatus::Disconnected);
            let _ = closed_tx.send(Some(Ok(())));
            return;
        }
        initialized = PreparedToolRuntime::initialize(prepared) => match initialized {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => {
                let _ = status_tx.send(AttachmentStatus::Disconnected);
                let _ = closed_tx.send(Some(Err(error.into())));
                return;
            }
        },
    };

    let config = (|| {
        let catalog = runtime.catalog()?;
        let tools = serde_json::to_value(catalog)
            .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
        let names = tools
            .as_array()
            .and_then(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .pointer("/definition/name")
                            .and_then(serde_json::Value::as_str)
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| AttachmentError::Catalog("catalog tool name is missing".into()))?;
        crate::host::validate_public_tool_catalog_names(names)
            .map_err(|error| AttachmentError::Catalog(error.to_string().into()))?;
        Ok::<_, AttachmentError>(driver::Config {
            endpoint: target.endpoint,
            listener: None,
            authorization: format!("Bearer {}", target.bearer).into(),
            tools,
            metadata,
        })
    })();
    let config = match config {
        Ok(config) => config,
        Err(error) => {
            let _ = status_tx.send(AttachmentStatus::Disconnected);
            runtime.shutdown().await;
            let _ = closed_tx.send(Some(Err(error)));
            return;
        }
    };
    tracing::info!(target: "nanocode_code_mode::attachment",
        stage = "attachment.catalog_prepared",
        duration_ms = prepare_started.elapsed().as_secs_f64() * 1000.0,
        "attachment catalog prepared");
    driver::run(config, runtime, command_rx, event_tx, status_tx, closed_tx).await;
}

fn is_literal_loopback(endpoint: &Url) -> bool {
    match endpoint.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

use nanocode_tls::install_default_rustls_crypto_provider;

struct HandleRefs {
    command_tx: mpsc::Sender<driver::Command>,
}

impl Drop for HandleRefs {
    fn drop(&mut self) {
        let _ = self.command_tx.try_send(driver::Command::Detach);
    }
}

#[derive(Clone)]
pub struct Attachment {
    refs: Arc<HandleRefs>,
    status: watch::Receiver<AttachmentStatus>,
    closed: watch::Receiver<Option<Result<(), AttachmentError>>>,
}

impl fmt::Debug for Attachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Attachment")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Attachment {
    #[must_use]
    pub fn status(&self) -> AttachmentStatus {
        self.status.borrow().clone()
    }

    pub fn detach(self) -> futures_util::future::BoxFuture<'static, Result<(), AttachmentError>> {
        Box::pin(async move {
            let _ = self.refs.command_tx.try_send(driver::Command::Detach);
            self.closed().await
        })
    }

    pub fn closed<'call>(
        &'call self,
    ) -> futures_util::future::BoxFuture<'call, Result<(), AttachmentError>> {
        Box::pin(async move {
            let mut closed = self.closed.clone();
            loop {
                if let Some(result) = closed.borrow().clone() {
                    return result;
                }
                if closed.changed().await.is_err() {
                    return closed
                        .borrow()
                        .clone()
                        .unwrap_or(Err(AttachmentError::Closed));
                }
            }
        })
    }

    async fn wait_until_ready(&self) -> Result<(), AttachmentError> {
        let mut status = self.status.clone();
        loop {
            let current = status.borrow().clone();
            match current {
                AttachmentStatus::Ready => return Ok(()),
                AttachmentStatus::Fenced => return self.closed().await,
                _ => {}
            }
            tokio::select! {
                changed = status.changed() => if changed.is_err() { return self.closed().await },
                result = self.closed() => return result,
            }
        }
    }
}

pub struct AttachmentEvents {
    events: mpsc::Receiver<AttachmentEvent>,
}

impl fmt::Debug for AttachmentEvents {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentEvents")
            .finish_non_exhaustive()
    }
}

impl AttachmentEvents {
    pub fn recv<'call>(
        &'call mut self,
    ) -> futures_util::future::BoxFuture<'call, Option<AttachmentEvent>> {
        Box::pin(async move { self.events.recv().await })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AttachmentStatus {
    Connecting,

    Ready,

    Disconnected,

    Fenced,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AttachmentEvent {
    Connecting,

    Retrying {
        reason: Box<str>,
        delay_ms: u64,
    },

    Attached,

    CatalogPublished {
        tool_count: usize,
    },

    CallStarted {
        call_id: Box<str>,

        name: Box<str>,
    },

    CallCompleted {
        call_id: Box<str>,

        outcome: AttachmentCallOutcome,
    },

    Detached {
        reason: Box<str>,
    },

    Fenced {
        reason: Box<str>,
    },
}

impl From<PreparedToolError> for AttachmentError {
    fn from(error: PreparedToolError) -> Self {
        Self::Catalog(error.to_string().into())
    }
}

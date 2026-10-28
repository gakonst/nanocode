use super::{
    AttachmentError, AttachmentTarget, RemoteMode, RemoteRunner, RunnerConnection,
    host::DialedSocket, routing::Route,
};
use crate::{
    embedded::CodeModeHost,
    remote::{CodeModeExecutors, SocketPolicy, SocketShutdown, SocketTransport},
};
use futures_util::{SinkExt, StreamExt, future};
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nanocode_sync::{Atom, Ordered, Slot};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    task::{AbortHandle, JoinHandle, JoinSet},
};
use tokio_tungstenite::tungstenite::{
    self, Message,
    handshake::server::{ErrorResponse, Request, Response},
    protocol::{CloseFrame, frame::coding::CloseCode},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Runner {
    endpoint: String,
    token: String,
    name: String,
    named: bool,
    mode: RemoteMode,
    timeout: Option<Duration>,
    workspace: Option<PathBuf>,
}

pub struct RunnerSpec {
    pub endpoint: String,
    pub token: String,
    pub name: Option<String>,
    pub replace: bool,
    pub call_timeout: Option<Duration>,
}

impl Runner {
    pub fn spec(&self) -> RunnerSpec {
        RunnerSpec {
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            name: self.named.then(|| self.name.clone()),
            replace: self.mode == RemoteMode::Replace,
            call_timeout: self.timeout,
        }
    }
    pub fn workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }
    pub fn validate(&self) -> Result<(), AttachmentError> {
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| !workspace.is_absolute())
        {
            return Err(AttachmentError::Catalog(
                "runner workspace override must be absolute".into(),
            ));
        }
        AttachmentTarget::new(&self.endpoint, self.token.clone())?;
        let remote = RemoteRunner::new(&self.name)?;
        if let Some(timeout) = self.timeout {
            remote.with_call_timeout(timeout)?;
        }
        Ok(())
    }
    pub fn connect(url: impl Into<String>) -> Self {
        Self {
            endpoint: url.into(),
            token: String::new(),
            name: "runner".into(),
            named: false,
            mode: RemoteMode::Alongside,
            timeout: None,
            workspace: None,
        }
    }
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = token.into();
        self
    }
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self.named = true;
        self
    }
    pub const fn alongside(mut self) -> Self {
        self.mode = RemoteMode::Alongside;
        self
    }
    pub const fn replace(mut self) -> Self {
        self.mode = RemoteMode::Replace;
        self
    }
    pub const fn call_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}
impl fmt::Debug for Runner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runner")
            .field("endpoint", &self.endpoint)
            .field("name", &self.name)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct RunnerServerConfig {
    address: String,
    token: Option<String>,
    timeout: Option<Duration>,
    mode: RemoteMode,
    workspace: Option<PathBuf>,
}
impl RunnerServerConfig {
    pub fn workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }
    pub fn validate(&self) -> Result<(), AttachmentError> {
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| !workspace.is_absolute())
        {
            return Err(AttachmentError::Catalog(
                "runner workspace override must be absolute".into(),
            ));
        }
        if self.address.trim().is_empty() {
            return Err(error("runner listen address must not be empty"));
        }
        if self
            .token
            .as_ref()
            .is_some_and(|token| token.trim().is_empty())
        {
            return Err(AttachmentError::Authentication(
                "bearer credential must not be empty".into(),
            ));
        }
        if let Some(timeout) = self.timeout {
            RemoteRunner::new("validation")?.with_call_timeout(timeout)?;
        }
        Ok(())
    }
    pub fn listen(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            token: None,
            timeout: None,
            workspace: None,
            mode: RemoteMode::Alongside,
        }
    }
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
    pub const fn alongside(mut self) -> Self {
        self.mode = RemoteMode::Alongside;
        self
    }
    pub const fn replace(mut self) -> Self {
        self.mode = RemoteMode::Replace;
        self
    }
    pub const fn call_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}
impl fmt::Debug for RunnerServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunnerServerConfig")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunnerStatus {
    Connecting,
    Ready,
    Disconnected,
    Failed(String),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerProtocol {
    Tools,
    CodeMode,
}
#[derive(Clone, Debug)]
pub struct RunnerInfo {
    pub name: String,
    pub endpoint: String,
    pub mode: RemoteMode,
    pub status: RunnerStatus,
    pub workspace: Option<PathBuf>,
    pub tool_count: usize,
    pub protocol: RunnerProtocol,
}
#[derive(Clone, Debug)]
pub enum RunnerEvent {
    StatusChanged { info: RunnerInfo },
    Failed { name: String, error: String },
}

#[derive(Clone)]
pub struct RunnerPool {
    inner: Arc<PoolInner>,
    model_management: bool,
}
impl Default for RunnerPool {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            model_management: true,
        }
    }
}
struct PoolInner {
    state: Atom<PoolState>,
    events: Ordered<Vec<RunnerEvent>>,
    subscribers: Atom<Vec<mpsc::UnboundedSender<RunnerEvent>>>,
    code_mode: CodeModeExecutors,
}
impl Default for PoolInner {
    fn default() -> Self {
        Self {
            state: Atom::default(),
            events: Ordered::new(1),
            subscribers: Atom::default(),
            code_mode: CodeModeExecutors::default(),
        }
    }
}
#[derive(Clone, Default)]
struct PoolState {
    revision: u64,
    runners: BTreeMap<String, Arc<Entry>>,
    executors: BTreeMap<String, Arc<ExecutorEntry>>,
    revocations: HashSet<(String, Box<str>)>,
    servers: Vec<Arc<ServerInner>>,
    closed: bool,
}
struct ExecutorEntry {
    config: Option<Runner>,
    endpoint: String,
    mode: RemoteMode,
    status: watch::Sender<RunnerStatus>,
    workspace: Option<PathBuf>,
    tool_count: usize,
    identity: Arc<()>,
    runtime_id: Option<Box<str>>,
    shutdown: SocketShutdown,
    closed: watch::Receiver<Option<String>>,
    host: Arc<dyn CodeModeHost>,
    monitor: Slot<AbortHandle>,
}
impl Drop for ExecutorEntry {
    fn drop(&mut self) {
        self.shutdown.close();
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
    }
}
impl ExecutorEntry {
    fn info(&self, name: &str) -> RunnerInfo {
        RunnerInfo {
            name: name.into(),
            endpoint: self.endpoint.clone(),
            mode: self.mode,
            status: self.status.borrow().clone(),
            workspace: self.workspace.clone(),
            tool_count: self.tool_count,
            protocol: RunnerProtocol::CodeMode,
        }
    }
}
struct Entry {
    remote: RemoteRunner,
    endpoint: String,
    mode: RemoteMode,
    status: watch::Sender<RunnerStatus>,
    connection: Slot<RunnerConnection>,
    cancel: CancellationToken,
    monitor: Slot<AbortHandle>,
    identity: Arc<()>,
    active: AtomicBool,
    workspace: Option<PathBuf>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
    }
}
impl PoolInner {
    fn commit<R>(&self, mut change: impl FnMut(&mut PoolState) -> (R, Vec<RunnerEvent>)) -> R {
        let (output, revision, events) = self.state.update(|state| {
            state.revision += 1;
            let (output, events) = change(state);
            (output, state.revision, events)
        });
        self.publish(revision, events);
        output
    }
    fn try_commit<R, E>(
        &self,
        mut change: impl FnMut(&mut PoolState) -> Result<(R, Vec<RunnerEvent>), E>,
    ) -> Result<R, E> {
        let (output, revision, events) = self.state.modify(|state| {
            let (output, events) = change(state)?;
            state.revision += 1;
            Ok((output, state.revision, events))
        })?;
        self.publish(revision, events);
        Ok(output)
    }
    fn publish(&self, revision: u64, events: Vec<RunnerEvent>) {
        self.events.publish(revision, events, |events| {
            if events.is_empty() {
                return;
            }
            let mut stale = false;
            for subscriber in self.subscribers.get().iter() {
                for event in &events {
                    stale |= subscriber.send(event.clone()).is_err();
                }
            }
            if stale {
                self.subscribers
                    .update(|subscribers| subscribers.retain(|sender| !sender.is_closed()));
            }
        });
    }
    fn emit(&self, event: RunnerEvent) {
        self.commit(|_| ((), vec![event.clone()]));
    }
    fn update(&self, name: &str, status: RunnerStatus, identity: &Arc<()>) {
        let Some(entry) = self.state.read(|state| state.runners.get(name).cloned()) else {
            return;
        };
        if !Arc::ptr_eq(&entry.identity, identity)
            || !entry.active.load(Ordering::SeqCst)
            || *entry.status.borrow() == status
        {
            return;
        }
        entry.status.send_replace(status.clone());
        let info = entry.info(name);
        tracing::info!(target: "nanocode_code_mode::attachment", runner = name, ?status, "runner status changed");
        let mut events = vec![RunnerEvent::StatusChanged { info }];
        if let RunnerStatus::Failed(error) = status {
            events.push(RunnerEvent::Failed {
                name: name.into(),
                error,
            });
        }
        self.commit(|_| ((), events.clone()));
    }
}
impl Entry {
    fn info(&self, name: &str) -> RunnerInfo {
        let tools = self.remote.tools_snapshot();
        RunnerInfo {
            name: name.into(),
            endpoint: self.endpoint.clone(),
            mode: self.mode,
            status: self.status.borrow().clone(),
            workspace: self.workspace.clone().or_else(|| {
                tools
                    .as_ref()
                    .and_then(|tools| tools.workspace().map(PathBuf::from))
            }),
            tool_count: tools.map_or(0, |tools| tools.iter().count()),
            protocol: RunnerProtocol::Tools,
        }
    }
}
enum Refusal {
    Revoked,
    Duplicate,
    Failed(AttachmentError),
}
fn error(value: impl fmt::Display) -> AttachmentError {
    AttachmentError::Transport(value.to_string().into())
}

impl RunnerPool {
    pub fn new() -> Self {
        Self::default()
    }
    pub const fn with_model_management(mut self, enabled: bool) -> Self {
        self.model_management = enabled;
        self
    }
    pub const fn model_management_enabled(&self) -> bool {
        self.model_management
    }
    pub fn code_mode_executors(&self) -> CodeModeExecutors {
        self.inner.code_mode.clone()
    }
    pub fn report_failure(&self, name: impl Into<String>, failure: impl Into<String>) {
        self.inner.emit(RunnerEvent::Failed {
            name: name.into(),
            error: failure.into(),
        });
    }
    pub fn servers(&self) -> Vec<RunnerServer> {
        self.inner.state.read(|state| {
            state
                .servers
                .iter()
                .filter(|server| !server.cancel.is_cancelled())
                .map(|inner| RunnerServer {
                    inner: inner.clone(),
                })
                .collect()
        })
    }
    pub fn start(&self, config: Runner) -> Result<RunnerHandle, AttachmentError> {
        let name = config.name.clone();
        let result = self.start_inner(config, None);
        if let Err(error) = &result {
            self.report_failure(name, error.to_string());
        }
        result
    }
    fn start_inner(
        &self,
        config: Runner,
        initial: Option<(DialedSocket, String)>,
    ) -> Result<RunnerHandle, AttachmentError> {
        config.validate()?;
        tokio::runtime::Handle::try_current().map_err(error)?;
        super::install_default_rustls_crypto_provider();
        let target = AttachmentTarget::new(&config.endpoint, config.token)?;
        let mut remote = RemoteRunner::new(&config.name)?;
        if let Some(timeout) = config.timeout {
            remote = remote.with_call_timeout(timeout)?;
        }
        let admissible = |state: &PoolState| {
            validate_registration(state, &config.name, config.mode)?;
            if config.mode == RemoteMode::Replace
                && state.servers.iter().any(|server| {
                    server.mode == RemoteMode::Replace && !server.cancel.is_cancelled()
                })
            {
                return Err(AttachmentError::Catalog(
                    "a replacement listener is already configured".into(),
                ));
            }
            Ok(())
        };
        self.inner.state.read(admissible)?;
        let connection = remote.connect_established(target, initial)?;
        let status = watch::channel(RunnerStatus::Connecting).0;
        let identity = Arc::new(());
        let handle = RunnerHandle {
            pool: self.clone(),
            name: config.name.clone(),
            endpoint: config.endpoint.clone(),
            mode: config.mode,
            workspace: config.workspace.clone(),
            kind: HandleKind::Tools(remote.clone()),
            identity: identity.clone(),
            status: status.subscribe(),
        };
        let monitor = monitor(
            Arc::downgrade(&self.inner),
            config.name.clone(),
            remote.clone(),
            identity.clone(),
        );
        let entry = Arc::new(Entry {
            remote,
            endpoint: config.endpoint,
            mode: config.mode,
            status,
            connection: Slot::new(Some(connection)),
            cancel: CancellationToken::new(),
            monitor: Slot::new(Some(monitor)),
            identity,
            active: AtomicBool::new(true),
            workspace: config.workspace,
        });
        let info = entry.info(&config.name);
        self.inner.try_commit(|state| {
            admissible(state)?;
            state
                .runners
                .insert(config.name.clone(), Arc::clone(&entry));
            Ok::<_, AttachmentError>(((), vec![RunnerEvent::StatusChanged { info: info.clone() }]))
        })?;
        Ok(handle)
    }
    #[must_use]
    pub fn awaiting_replacement(&self) -> bool {
        self.inner.code_mode.awaiting_replacement()
    }
    pub async fn replacement_hand(
        &self,
    ) -> Option<(String, Option<nanocode_code_mode::remote::HandDescription>)> {
        self.inner
            .code_mode
            .replacement()
            .await
            .map(|(name, host)| (name.to_string(), host.describe()))
    }
    pub fn keep_connected(&self, config: Runner) -> Result<(), AttachmentError> {
        config.validate()?;
        if config.mode == RemoteMode::Replace {
            self.inner.code_mode.await_replacement(true);
        }
        tokio::runtime::Handle::try_current().map_err(error)?;
        tokio::spawn(supervise(Arc::downgrade(&self.inner), config));
        Ok(())
    }
    pub fn connect<'call>(
        &'call self,
        config: Runner,
    ) -> futures_util::future::BoxFuture<'call, Result<RunnerHandle, AttachmentError>> {
        Box::pin(async move {
            let name = config.name.clone();
            let result = self.connect_detected(config).await;
            if let Err(error) = &result {
                self.report_failure(name, error.to_string());
            }
            result
        })
    }
    async fn connect_detected(&self, config: Runner) -> Result<RunnerHandle, AttachmentError> {
        config.validate()?;
        tokio::runtime::Handle::try_current().map_err(error)?;
        super::install_default_rustls_crypto_provider();
        self.inner
            .state
            .read(|state| validate_registration(state, &config.name, config.mode))?;
        let target = AttachmentTarget::new(&config.endpoint, config.token.clone())?;
        let (socket, first) = dial(&target).await?;
        if code_mode_catalog(&first) {
            let transport =
                SocketTransport::established(socket, first, SocketPolicy::runner(), config.timeout)
                    .await
                    .map_err(|failure| AttachmentError::Catalog(failure.to_string().into()))?;
            let name = match transport.name() {
                Some(advertised) if !config.named => {
                    self.inner
                        .state
                        .read(|state| validate_registration(state, advertised, config.mode))?;
                    advertised.to_owned()
                }
                _ => config.name.clone(),
            };
            let redial = Runner {
                name: name.clone(),
                named: true,
                ..config.clone()
            };
            return self.register_executor(
                name,
                config.endpoint,
                config.mode,
                config.workspace,
                transport,
                false,
                Some(redial),
            );
        }
        let timeout = config.timeout;
        let handle = self.start_inner(config, Some((socket, first)))?;
        let wait = handle.ready();
        let result = match timeout {
            Some(timeout) => tokio::time::timeout(timeout, wait)
                .await
                .map_err(error)
                .and_then(|result| result),
            None => wait.await,
        };
        if let Err(error) = result {
            handle.disconnect().await?;
            return Err(error);
        }
        Ok(handle)
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "each executor registration supplies its own connection facts"
    )]
    fn register_executor(
        &self,
        name: String,
        endpoint: String,
        mode: RemoteMode,
        workspace: Option<PathBuf>,
        transport: SocketTransport,
        reconnect: bool,
        config: Option<Runner>,
    ) -> Result<RunnerHandle, AttachmentError> {
        RemoteRunner::new(&name)?;
        let workspace = workspace.or_else(|| transport.workspace().map(PathBuf::from));
        let tool_count = transport.definitions().len();
        let runtime_id = transport.runtime_id().map(Box::<str>::from);
        let closed = transport.closed();
        let shutdown = transport.shutdown_handle();
        let host: Arc<dyn CodeModeHost> = Arc::new(transport.into_host());
        let identity = Arc::new(());
        let status = watch::channel(RunnerStatus::Ready).0;
        let handle = RunnerHandle {
            pool: self.clone(),
            name: name.clone(),
            endpoint: endpoint.clone(),
            mode,
            workspace: workspace.clone(),
            kind: HandleKind::CodeMode { tool_count },
            identity: identity.clone(),
            status: status.subscribe(),
        };
        let monitor = executor_monitor(
            Arc::downgrade(&self.inner),
            name.clone(),
            identity.clone(),
            Arc::clone(&host),
            closed.clone(),
        );
        let entry = Arc::new(ExecutorEntry {
            config,
            endpoint,
            mode,
            status,
            workspace,
            tool_count,
            identity,
            runtime_id: runtime_id.clone(),
            shutdown: shutdown.clone(),
            closed,
            host: Arc::clone(&host),
            monitor: Slot::new(Some(monitor)),
        });
        let registered = self.inner.state.modify(|state| {
            if state.closed {
                return Err(Refusal::Failed(AttachmentError::Closed));
            }
            if reconnect {
                if let Some(runtime_id) = &runtime_id
                    && state
                        .revocations
                        .contains(&(name.clone(), runtime_id.clone()))
                {
                    return Err(Refusal::Revoked);
                }
            } else {
                state.revocations.retain(|(revoked, _)| *revoked != name);
            }
            let duplicate = state
                .runners
                .get(&name)
                .is_some_and(|entry| entry.active.load(Ordering::SeqCst))
                || state.executors.get(&name).is_some_and(|entry| {
                    *entry.status.borrow() == RunnerStatus::Ready
                        && !(reconnect && runtime_id.is_some() && entry.runtime_id == runtime_id)
                });
            if duplicate {
                return Err(Refusal::Duplicate);
            }
            if mode == RemoteMode::Replace
                && self
                    .inner
                    .code_mode
                    .default_executor()
                    .is_some_and(|current| *current != *name)
            {
                return Err(Refusal::Failed(AttachmentError::Catalog(
                    "a replacement Code Mode executor is already configured".into(),
                )));
            }
            state.revision += 1;
            state.executors.insert(name.clone(), Arc::clone(&entry));
            Ok(state.revision)
        });
        let revision = match registered {
            Ok(revision) => revision,
            Err(refusal) => {
                if let Some(monitor) = entry.monitor.take() {
                    monitor.abort();
                }
                return Err(match refusal {
                    Refusal::Revoked => {
                        shutdown.revoke();
                        AttachmentError::Fenced(super::driver::ATTACHMENT_REVOKED_REASON.into())
                    }
                    Refusal::Duplicate => {
                        if reconnect {
                            shutdown.reject(DUPLICATE_EXECUTOR);
                        }
                        AttachmentError::Catalog(
                            format!("runner `{name}` is already registered").into(),
                        )
                    }
                    Refusal::Failed(error) => error,
                });
            }
        };
        self.inner.publish(revision, Vec::new());
        self.inner
            .code_mode
            .insert(name.as_str(), Arc::clone(&host));
        if mode == RemoteMode::Replace {
            self.inner
                .code_mode
                .set_default(Some(Arc::from(name.as_str())));
        }
        let current = self.inner.state.read(|state| {
            state
                .executors
                .get(&name)
                .map(|current| Arc::clone(&current.host))
        });
        if !current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &host))
        {
            if self.inner.code_mode.remove_host(&name, &host)
                && let Some(current) = current.clone()
            {
                self.inner.code_mode.insert(name.as_str(), current);
            }
            if current.is_none() && mode == RemoteMode::Replace {
                self.inner.code_mode.clear_selections(&name);
            }
            return Ok(handle);
        }
        let info = entry.info(&name);
        tracing::info!(target: "nanocode_code_mode::attachment", runner = name, "Code Mode executor registered");
        self.inner.emit(RunnerEvent::StatusChanged { info });
        Ok(handle)
    }
    pub fn connected_hands(&self) -> Vec<RunnerSpec> {
        self.inner.state.read(|state| {
            state
                .executors
                .values()
                .filter(|entry| *entry.status.borrow() == RunnerStatus::Ready)
                .filter_map(|entry| entry.config.as_ref().map(Runner::spec))
                .collect()
        })
    }
    pub fn list(&self) -> Vec<RunnerInfo> {
        let state = self.inner.state.get();
        let mut runners = state
            .runners
            .iter()
            .map(|(name, entry)| entry.info(name))
            .chain(state.executors.iter().map(|(name, entry)| entry.info(name)))
            .collect::<Vec<_>>();
        runners.sort_by(|left, right| left.name.cmp(&right.name));
        runners
    }
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<RunnerEvent> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.inner
            .subscribers
            .update(|subscribers| subscribers.push(sender.clone()));
        receiver
    }
    pub fn disconnect<'call>(
        &'call self,
        name: &'call str,
    ) -> futures_util::future::BoxFuture<'call, Result<(), AttachmentError>> {
        Box::pin(async move { self.disconnect_entry(name, None).await })
    }
    async fn disconnect_entry(
        &self,
        name: &str,
        identity: Option<&Arc<()>>,
    ) -> Result<(), AttachmentError> {
        if let Some(closed) = self.disconnect_executor(name, identity, true)? {
            Self::executor_closed(closed).await;
            return Ok(());
        }
        Self::wait_disconnected(self.begin_disconnect(name, identity)?).await
    }
    fn disconnect_executor(
        &self,
        name: &str,
        identity: Option<&Arc<()>>,
        revoke: bool,
    ) -> Result<Option<watch::Receiver<Option<String>>>, AttachmentError> {
        let entry = self.inner.try_commit(|state| {
            let Some(entry) = state.executors.get(name) else {
                return Ok((None, Vec::new()));
            };
            if identity.is_some_and(|identity| !Arc::ptr_eq(&entry.identity, identity)) {
                return Err(error(format!(
                    "runner handle `{name}` is stale; the name belongs to a newer registration"
                )));
            }
            let Some(entry) = state.executors.remove(name) else {
                return Ok((None, Vec::new()));
            };
            let mut info = entry.info(name);
            info.status = RunnerStatus::Disconnected;
            if revoke && let Some(runtime_id) = &entry.runtime_id {
                state
                    .revocations
                    .insert((name.to_owned(), runtime_id.clone()));
            }
            Ok((Some(entry), vec![RunnerEvent::StatusChanged { info }]))
        })?;
        let Some(entry) = entry else {
            return Ok(None);
        };
        entry.status.send_replace(RunnerStatus::Disconnected);
        self.inner.code_mode.remove_host(name, &entry.host);
        self.inner.code_mode.clear_selections(name);
        if revoke {
            entry.shutdown.revoke();
        } else {
            entry.shutdown.close();
        }
        Ok(Some(entry.closed.clone()))
    }
    async fn executor_closed(mut closed: watch::Receiver<Option<String>>) {
        let _ = closed.wait_for(Option::is_some).await;
    }
    fn begin_disconnect(
        &self,
        name: &str,
        identity: Option<&Arc<()>>,
    ) -> Result<RemoteRunner, AttachmentError> {
        let entry = self.inner.try_commit(|state| {
            let entry = state
                .runners
                .get(name)
                .cloned()
                .ok_or_else(|| error(format!("runner `{name}` is not registered")))?;
            if identity.is_some_and(|identity| !Arc::ptr_eq(&entry.identity, identity)) {
                return Err(error(format!(
                    "runner handle `{name}` is stale; the name belongs to a newer registration"
                )));
            }
            entry.active.store(false, Ordering::SeqCst);
            if let Some(runtime_id) = entry.remote.runtime_id() {
                state.revocations.insert((name.to_owned(), runtime_id));
            }
            let mut info = entry.info(name);
            info.status = RunnerStatus::Disconnected;
            Ok((entry, vec![RunnerEvent::StatusChanged { info }]))
        })?;
        entry.cancel.cancel();
        if let Some(monitor) = entry.monitor.take() {
            monitor.abort();
        }
        entry.connection.take();
        entry.status.send_replace(RunnerStatus::Disconnected);
        let remote = entry.remote.clone();
        remote.revoke();
        Ok(remote)
    }
    async fn wait_disconnected(remote: RemoteRunner) -> Result<(), AttachmentError> {
        let mut connected = remote.connection_status();
        connected
            .wait_for(|connected| !connected)
            .await
            .map_err(error)?;
        Ok(())
    }
    pub fn shutdown<'call>(
        &'call self,
    ) -> futures_util::future::BoxFuture<'call, Result<(), AttachmentError>> {
        Box::pin(async move {
            let (names, executors, servers) = self.inner.commit(|state| {
                state.closed = true;
                (
                    (
                        state.runners.keys().cloned().collect::<Vec<_>>(),
                        state.executors.keys().cloned().collect::<Vec<_>>(),
                        std::mem::take(&mut state.servers),
                    ),
                    Vec::new(),
                )
            });
            for inner in &servers {
                inner.cancel.cancel();
            }
            let executors = executors
                .iter()
                .map(|name| self.disconnect_executor(name, None, false))
                .collect::<Result<Vec<_>, _>>()?;
            let remotes = names
                .into_iter()
                .map(|name| self.begin_disconnect(&name, None))
                .collect::<Result<Vec<_>, _>>()?;
            for closed in executors.into_iter().flatten() {
                Self::executor_closed(closed).await;
            }
            for remote in remotes {
                Self::wait_disconnected(remote).await?;
            }
            for inner in servers {
                RunnerServer { inner }.stop().await?;
            }
            self.inner.subscribers.set(Vec::new());
            Ok(())
        })
    }
    pub(crate) fn routes(&self) -> Vec<Route> {
        self.inner.state.read(|state| {
            state
                .runners
                .values()
                .filter(|entry| entry.active.load(Ordering::SeqCst) && entry.remote.is_connected())
                .filter_map(|entry| {
                    let tools = entry.remote.tools_snapshot()?;
                    Some(Route {
                        workspace: entry
                            .workspace
                            .clone()
                            .or_else(|| tools.workspace().map(PathBuf::from))?,
                        tools,
                        mode: entry.mode,
                    })
                })
                .collect()
        })
    }
    pub(crate) fn reserved_mounts(&self) -> Vec<String> {
        self.inner
            .state
            .read(|state| state.runners.keys().cloned().collect())
    }
    pub(crate) fn replacement(&self) -> Option<String> {
        let state = self.inner.state.get();
        state
            .runners
            .iter()
            .find(|(_, entry)| entry.mode == RemoteMode::Replace)
            .map(|(name, _)| name.clone())
            .or_else(|| {
                state
                    .servers
                    .iter()
                    .find(|server| {
                        server.mode == RemoteMode::Replace && !server.cancel.is_cancelled()
                    })
                    .map(|_| "incoming replacement runner".into())
            })
    }
    pub fn listen<'call>(
        &'call self,
        config: RunnerServerConfig,
    ) -> futures_util::future::BoxFuture<'call, Result<RunnerServer, AttachmentError>> {
        Box::pin(async move {
            super::install_default_rustls_crypto_provider();
            config.validate()?;
            let token = match config.token {
                Some(token) if !token.trim().is_empty() => token,
                Some(_) => {
                    return Err(AttachmentError::Authentication(
                        "bearer credential must not be empty".into(),
                    ));
                }
                None => {
                    let mut bytes = [0u8; 32];
                    getrandom::fill(&mut bytes).map_err(error)?;
                    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
                }
            };
            let listener = TcpListener::bind(&config.address).await.map_err(error)?;
            let address = listener.local_addr().map_err(error)?;
            if !address.ip().is_loopback() {
                return Err(error(
                    "plaintext runner listeners require a loopback address; use a TLS proxy for remote access",
                ));
            }
            let endpoint = format!("ws://{address}/");
            let cancel = CancellationToken::new();
            let weak = Arc::downgrade(&self.inner);
            let admissible = |state: &PoolState| {
                if state.closed {
                    return Err(AttachmentError::Closed);
                }
                if config.mode == RemoteMode::Replace
                    && (state
                        .runners
                        .values()
                        .any(|entry| entry.mode == RemoteMode::Replace)
                        || state.servers.iter().any(|server| {
                            server.mode == RemoteMode::Replace && !server.cancel.is_cancelled()
                        }))
                {
                    return Err(AttachmentError::Catalog(
                        "a replacement runner or listener is already configured".into(),
                    ));
                }
                Ok(())
            };
            self.inner.state.read(admissible)?;
            let task = tokio::spawn(accept(
                listener,
                weak,
                token.clone(),
                endpoint.clone(),
                config.timeout,
                config.mode,
                config.workspace.clone(),
                cancel.clone(),
            ));
            let inner = Arc::new(ServerInner {
                endpoint,
                token,
                cancel,
                task: Slot::new(Some(task)),
                pool: Arc::downgrade(&self.inner),
                mode: config.mode,
            });
            self.inner.try_commit(|state| {
                admissible(state)?;
                state.servers.push(Arc::clone(&inner));
                Ok::<_, AttachmentError>(((), Vec::new()))
            })?;
            if config.mode == RemoteMode::Replace {
                self.inner.code_mode.await_replacement(true);
            }
            Ok(RunnerServer { inner })
        })
    }
}
fn validate_registration(
    state: &PoolState,
    name: &str,
    mode: RemoteMode,
) -> Result<(), AttachmentError> {
    if state.closed {
        return Err(AttachmentError::Closed);
    }
    if state
        .runners
        .get(name)
        .is_some_and(|entry| entry.active.load(Ordering::SeqCst))
        || state
            .executors
            .get(name)
            .is_some_and(|entry| *entry.status.borrow() == RunnerStatus::Ready)
    {
        return Err(AttachmentError::Catalog(
            format!("runner `{name}` is already registered").into(),
        ));
    }
    if mode == RemoteMode::Replace
        && state
            .runners
            .iter()
            .any(|(existing, entry)| existing != name && entry.mode == RemoteMode::Replace)
    {
        return Err(AttachmentError::Catalog(
            "a replacement runner is already configured".into(),
        ));
    }
    Ok(())
}
async fn supervise(pool: Weak<PoolInner>, config: Runner) {
    let mut backoff = super::INITIAL_TRANSPORT_BACKOFF;
    let mut reported = false;
    loop {
        let Some(inner) = pool.upgrade() else {
            return;
        };
        let pool_handle = RunnerPool {
            inner,
            model_management: true,
        };
        match pool_handle.connect_detected(config.clone()).await {
            Ok(handle) => {
                backoff = super::INITIAL_TRANSPORT_BACKOFF;
                reported = false;
                let HandleKind::CodeMode { .. } = handle.kind else {
                    return;
                };
                let (name, identity, mut status) = (
                    handle.name.clone(),
                    handle.identity.clone(),
                    handle.subscribe(),
                );
                drop((handle, pool_handle));
                if status
                    .wait_for(|status| {
                        matches!(status, RunnerStatus::Disconnected | RunnerStatus::Failed(_))
                    })
                    .await
                    .is_err()
                {
                    return;
                }
                let Some(inner) = pool.upgrade() else {
                    return;
                };
                let dropped = inner.state.read(|state| {
                    state
                        .executors
                        .get(&name)
                        .is_some_and(|entry| Arc::ptr_eq(&entry.identity, &identity))
                });
                if !dropped {
                    return;
                }
            }
            Err(failure @ (AttachmentError::Transport(_) | AttachmentError::Timeout { .. })) => {
                if !reported {
                    pool_handle.report_failure(config.name.clone(), failure.to_string());
                    reported = true;
                }
                drop(pool_handle);
                tokio::time::sleep(super::reconnect_delay(&mut backoff)).await;
            }
            Err(failure) => {
                pool_handle.report_failure(config.name.clone(), failure.to_string());
                return;
            }
        }
    }
}
fn executor_monitor(
    pool: Weak<PoolInner>,
    name: String,
    identity: Arc<()>,
    host: Arc<dyn CodeModeHost>,
    mut closed: watch::Receiver<Option<String>>,
) -> AbortHandle {
    tokio::spawn(async move {
        let reason = match closed.wait_for(Option::is_some).await {
            Ok(reason) => reason.clone().unwrap_or_default(),
            Err(_) => crate::remote::LOCAL_CLOSED.to_owned(),
        };
        let Some(pool) = pool.upgrade() else {
            return;
        };
        pool.code_mode.remove_host(&name, &host);
        let Some(entry) = pool.state.read(|state| state.executors.get(&name).cloned()) else {
            return;
        };
        if !Arc::ptr_eq(&entry.identity, &identity) {
            return;
        }
        let status = if matches!(
            reason.as_str(),
            crate::remote::PEER_CLOSED | crate::remote::LOCAL_CLOSED
        ) {
            RunnerStatus::Disconnected
        } else {
            RunnerStatus::Failed(reason)
        };
        tracing::info!(target: "nanocode_code_mode::attachment", runner = name, ?status, "Code Mode executor closed");
        entry.status.send_replace(status);
        let info = entry.info(&name);
        pool.emit(RunnerEvent::StatusChanged { info });
    })
    .abort_handle()
}
async fn dial(target: &AttachmentTarget) -> Result<(DialedSocket, String), AttachmentError> {
    use tungstenite::client::IntoClientRequest;
    let mut request = target
        .endpoint()
        .as_str()
        .into_client_request()
        .map_err(error)?;
    let mut authorization =
        http::HeaderValue::from_str(&format!("Bearer {}", target.bearer())).map_err(error)?;
    authorization.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, authorization);
    let connector = super::driver::tls_connector(&request)
        .await
        .map_err(error)?;
    let (mut socket, _) = tokio::time::timeout(
        super::driver::HANDSHAKE_TIMEOUT,
        tokio_tungstenite::connect_async_tls_with_config(request, None, true, connector),
    )
    .await
    .map_err(error)?
    .map_err(|failure| match failure {
        tungstenite::Error::Http(response) if matches!(response.status().as_u16(), 401 | 403) => {
            AttachmentError::Authentication("runner rejected the bearer credential".into())
        }
        failure => error(failure),
    })?;
    let first = tokio::time::timeout(super::driver::HANDSHAKE_TIMEOUT, first_text(&mut socket))
        .await
        .map_err(error)??;
    Ok((socket, first))
}
async fn first_text<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> Result<String, AttachmentError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(message) = socket.next().await {
        match message.map_err(error)? {
            Message::Text(text) => return Ok(text.to_string()),
            Message::Binary(_) => return Err(error("runner sent a binary frame")),
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err(error("runner closed before publishing its catalog"))
}
fn code_mode_catalog(first: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(first).is_ok_and(|frame| {
        frame.get("type").and_then(serde_json::Value::as_str) == Some("catalog")
            && frame.get("definitions").is_some()
    })
}
const UNNAMED_EXECUTOR: &str = "executor";
const DUPLICATE_EXECUTOR: &str = "another runner already uses this name; pass a different --name";
async fn incoming_executor(
    socket: tokio_tungstenite::WebSocketStream<TcpStream>,
    first: String,
    pool: Weak<PoolInner>,
    endpoint: String,
    timeout: Option<Duration>,
    mode: RemoteMode,
    workspace: Option<PathBuf>,
) -> Result<(), AttachmentError> {
    let transport = SocketTransport::established(socket, first, SocketPolicy::runner(), timeout)
        .await
        .map_err(|failure| AttachmentError::Catalog(failure.to_string().into()))?;
    let name = transport.name().unwrap_or(UNNAMED_EXECUTOR).to_owned();
    let pool = RunnerPool {
        inner: pool.upgrade().ok_or(AttachmentError::Closed)?,
        model_management: true,
    };
    pool.register_executor(name, endpoint, mode, workspace, transport, true, None)?;
    Ok(())
}
fn monitor(
    pool: Weak<PoolInner>,
    name: String,
    remote: RemoteRunner,
    identity: Arc<()>,
) -> AbortHandle {
    tokio::spawn(async move {
        let mut connected = remote.connection_status();
        let mut failure = remote.failure_status();
        loop {
            let status = if let Some(error) = failure.borrow().clone() { RunnerStatus::Failed(error.to_string()) } else if *connected.borrow() { RunnerStatus::Ready } else { RunnerStatus::Disconnected };
            let Some(pool) = pool.upgrade() else { return };
            if status != RunnerStatus::Disconnected || remote.tools_snapshot().is_some() { pool.update(&name, status, &identity); }
            drop(pool);
            tokio::select! { result = connected.changed() => if result.is_err() { return }, result = failure.changed() => if result.is_err() { return } }
        }
    }).abort_handle()
}

#[derive(Clone)]
pub struct RunnerHandle {
    pool: RunnerPool,
    name: String,
    endpoint: String,
    mode: RemoteMode,
    workspace: Option<PathBuf>,
    kind: HandleKind,
    identity: Arc<()>,
    status: watch::Receiver<RunnerStatus>,
}
#[derive(Clone)]
enum HandleKind {
    Tools(RemoteRunner),
    CodeMode { tool_count: usize },
}
impl RunnerHandle {
    pub fn info(&self) -> RunnerInfo {
        match &self.kind {
            HandleKind::Tools(remote) => {
                let tools = remote.tools_snapshot();
                RunnerInfo {
                    name: self.name.clone(),
                    endpoint: self.endpoint.clone(),
                    mode: self.mode,
                    status: self.status(),
                    workspace: self.workspace.clone().or_else(|| {
                        tools
                            .as_ref()
                            .and_then(|tools| tools.workspace().map(PathBuf::from))
                    }),
                    tool_count: tools.map_or(0, |tools| tools.iter().count()),
                    protocol: RunnerProtocol::Tools,
                }
            }
            HandleKind::CodeMode { tool_count } => RunnerInfo {
                name: self.name.clone(),
                endpoint: self.endpoint.clone(),
                mode: self.mode,
                status: self.status(),
                workspace: self.workspace.clone(),
                tool_count: *tool_count,
                protocol: RunnerProtocol::CodeMode,
            },
        }
    }
    pub fn status(&self) -> RunnerStatus {
        self.status.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<RunnerStatus> {
        self.status.clone()
    }
    pub fn disconnect<'call>(
        &'call self,
    ) -> futures_util::future::BoxFuture<'call, Result<(), AttachmentError>> {
        Box::pin(async move {
            self.pool
                .disconnect_entry(&self.name, Some(&self.identity))
                .await
        })
    }
    async fn ready(&self) -> Result<(), AttachmentError> {
        let mut status = self.subscribe();
        loop {
            match status.borrow().clone() {
                RunnerStatus::Ready => return Ok(()),
                RunnerStatus::Failed(reason) => {
                    let failure = match &self.kind {
                        HandleKind::Tools(remote) => remote.failure_status().borrow().clone(),
                        HandleKind::CodeMode { .. } => None,
                    };
                    return Err(failure.unwrap_or_else(|| error(reason)));
                }
                RunnerStatus::Disconnected => return Err(AttachmentError::Closed),
                RunnerStatus::Connecting => {}
            }
            status.changed().await.map_err(error)?;
        }
    }
}
impl fmt::Debug for RunnerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.info().fmt(f)
    }
}

#[derive(Clone)]
pub struct RunnerServer {
    inner: Arc<ServerInner>,
}
struct ServerInner {
    endpoint: String,
    token: String,
    cancel: CancellationToken,
    task: Slot<JoinHandle<()>>,
    pool: Weak<PoolInner>,
    mode: RemoteMode,
}
impl Drop for ServerInner {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl RunnerServer {
    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }
    pub fn token(&self) -> &str {
        &self.inner.token
    }
    pub fn stop<'call>(
        &'call self,
    ) -> futures_util::future::BoxFuture<'call, Result<(), AttachmentError>> {
        Box::pin(async move {
            self.inner.cancel.cancel();
            if let Some(task) = self.inner.task.take() {
                task.await.map_err(error)?;
            }
            if let Some(inner) = self.inner.pool.upgrade() {
                if self.inner.mode == RemoteMode::Replace {
                    inner.code_mode.await_replacement(false);
                }
                let pool = RunnerPool {
                    inner,
                    model_management: true,
                };
                let names = pool
                    .list()
                    .into_iter()
                    .filter(|info| info.endpoint == self.endpoint())
                    .map(|info| info.name)
                    .collect::<Vec<_>>();
                for name in names {
                    pool.disconnect(&name).await?;
                }
            }
            Ok(())
        })
    }
}
impl fmt::Debug for RunnerServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunnerServer")
            .field("endpoint", &self.endpoint())
            .finish_non_exhaustive()
    }
}
#[allow(clippy::too_many_arguments)]
async fn accept(
    listener: TcpListener,
    pool: Weak<PoolInner>,
    token: String,
    endpoint: String,
    timeout: Option<Duration>,
    mode: RemoteMode,
    workspace: Option<PathBuf>,
    cancel: CancellationToken,
) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => { connections.shutdown().await; return },
            accepted = listener.accept() => match accepted {
                Ok((socket, _)) => {
                    let pool = pool.clone(); let token = token.clone(); let endpoint = endpoint.clone(); let workspace = workspace.clone();
                    connections.spawn(async move {
                        if let Err(error) = incoming(socket, pool.clone(), token, endpoint, timeout, mode, workspace).await {
                            tracing::warn!(target: "nanocode_code_mode::attachment", %error, "runner listener connection failed");
                            if let Some(pool) = pool.upgrade() { pool.emit(RunnerEvent::Failed { name: String::new(), error: error.to_string() }); }
                        }
                    });
                },
                Err(error) => { tracing::warn!(target: "nanocode_code_mode::attachment", %error, "runner listener accept failed"); return }
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}
async fn incoming(
    stream: TcpStream,
    pool: Weak<PoolInner>,
    token: String,
    endpoint: String,
    timeout: Option<Duration>,
    mode: RemoteMode,
    workspace: Option<PathBuf>,
) -> Result<(), AttachmentError> {
    let refused = std::sync::atomic::AtomicBool::new(false);
    let authenticate = |request: &Request, mut response: Response| {
        let bearer = request
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        let protocol = request
            .headers()
            .get(http::header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let mut values = value.split(',').map(str::trim);
                if values.next() == Some("bearer") {
                    values.next()
                } else {
                    None
                }
            });
        if protocol.is_some() {
            response.headers_mut().insert(
                http::header::SEC_WEBSOCKET_PROTOCOL,
                http::HeaderValue::from_static("bearer"),
            );
        }
        if bearer.or(protocol) == Some(token.as_str()) {
            Ok(response)
        } else if bearer.is_none() && protocol.is_some() {
            refused.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(response)
        } else {
            let mut response = ErrorResponse::new(Some("invalid runner credential".into()));
            *response.status_mut() = http::StatusCode::UNAUTHORIZED;
            Err(response)
        }
    };
    let mut socket = tokio::time::timeout(
        super::driver::HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async(stream, authenticate),
    )
    .await
    .map_err(error)?
    .map_err(error)?;
    if refused.load(std::sync::atomic::Ordering::Relaxed) {
        let close = tungstenite::protocol::CloseFrame {
            code: tungstenite::protocol::frame::coding::CloseCode::Policy,
            reason: "invalid runner credential".into(),
        };
        let _ =
            tokio::time::timeout(super::driver::HANDSHAKE_TIMEOUT, socket.close(Some(close))).await;
        return Err(AttachmentError::Authentication(
            "invalid runner credential".into(),
        ));
    }
    let first = tokio::time::timeout(super::driver::HANDSHAKE_TIMEOUT, first_text(&mut socket))
        .await
        .map_err(error)??;
    if code_mode_catalog(&first) {
        return incoming_executor(socket, first, pool, endpoint, timeout, mode, workspace).await;
    }
    let mut frames = socket
        .with(|text: String| future::ready(Ok::<_, tungstenite::Error>(Message::text(text))))
        .filter_map(|message| {
            future::ready(match message {
                Ok(Message::Text(text)) => Some(Ok(text.to_string())),
                Ok(Message::Binary(_)) => Some(Err(tungstenite::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "runner sent a binary frame",
                )))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
    let catalog: serde_json::Value = serde_json::from_str(&first).map_err(error)?;
    if catalog.get("type").and_then(serde_json::Value::as_str) != Some("catalog") {
        return Err(AttachmentError::Catalog(
            "runner must publish its catalog first".into(),
        ));
    }
    let name = catalog
        .pointer("/machines/0/id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AttachmentError::Catalog("incoming runner catalog requires machines[0].id".into())
        })?
        .to_owned();
    let registration = (|| {
        let pool = pool.upgrade().ok_or(AttachmentError::Closed)?;
        let admissible = |state: &PoolState| {
            if catalog
                .get("runtime_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|runtime_id| {
                    state
                        .revocations
                        .contains(&(name.clone(), runtime_id.into()))
                })
            {
                return Err(AttachmentError::Fenced(
                    super::driver::ATTACHMENT_REVOKED_REASON.into(),
                ));
            }
            validate_registration(state, &name, mode)
        };
        let existing = pool.state.read(|state| {
            admissible(state)?;
            Ok::<_, AttachmentError>(match state.runners.get(&name) {
                Some(entry) if !entry.cancel.is_cancelled() => Some(entry.remote.clone()),
                _ => None,
            })
        })?;
        let mut remote = match existing {
            Some(remote) => remote,
            None => RemoteRunner::new(&name)?,
        };
        if let Some(timeout) = timeout {
            remote = remote.with_call_timeout(timeout)?;
        }

        let status = watch::channel(RunnerStatus::Connecting).0;
        let cancel = CancellationToken::new();
        let identity = Arc::new(());
        let monitor = monitor(
            Arc::downgrade(&pool),
            name.clone(),
            remote.clone(),
            identity.clone(),
        );
        let entry = Arc::new(Entry {
            remote: remote.clone(),
            endpoint: endpoint.clone(),
            mode,
            status,
            connection: Slot::default(),
            cancel: cancel.clone(),
            monitor: Slot::new(Some(monitor)),
            identity: identity.clone(),
            active: AtomicBool::new(true),
            workspace: workspace.clone(),
        });
        let info = entry.info(&name);
        let registered = pool.try_commit(|state| {
            admissible(state)?;
            state.runners.insert(name.clone(), Arc::clone(&entry));
            Ok::<_, AttachmentError>(((), vec![RunnerEvent::StatusChanged { info: info.clone() }]))
        });
        if let Err(error) = registered {
            if let Some(monitor) = entry.monitor.take() {
                monitor.abort();
            }
            return Err(error);
        }
        Ok((remote, cancel, identity))
    })();
    let (remote, cancel, identity) = match registration {
        Ok(registration) => registration,
        Err(error) => {
            if error.is_revoked() {
                let _ = frames
                    .get_mut()
                    .get_mut()
                    .close(Some(revoked_close()))
                    .await;
            }
            return Err(error);
        }
    };
    let (result, cancelled) = tokio::select! { biased; _ = cancel.cancelled() => (Ok(()), true), result = remote.serve_catalog(first, &mut frames) => (result, false) };
    if cancelled {
        let _ = frames
            .get_mut()
            .get_mut()
            .close(Some(revoked_close()))
            .await;
    }
    if let Some(pool) = pool.upgrade() {
        pool.update(
            &name,
            match &result {
                Ok(()) => RunnerStatus::Disconnected,
                Err(error) => RunnerStatus::Failed(error.to_string()),
            },
            &identity,
        );
        if let Some(entry) = pool.state.read(|state| state.runners.get(&name).cloned())
            && Arc::ptr_eq(&entry.identity, &identity)
        {
            entry.active.store(false, Ordering::SeqCst);
        }
    }
    result
}

fn revoked_close() -> CloseFrame {
    CloseFrame {
        code: CloseCode::Policy,
        reason: super::driver::ATTACHMENT_REVOKED_REASON.into(),
    }
}

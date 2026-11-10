use std::{collections::HashMap, io, path::PathBuf};

use nanocode::{
    ExecutorSelection, ModelSelection, Runner, RunnerSpec, ServiceTier, Thinking,
    agent::input::Prompt,
};
use nanocode_tui::{
    queue::{QueueKind, QueueState, QueuedPrompt},
    theme::Theme,
    transcript::NoticeLevel,
};
use serde::{Deserialize, Serialize};

use super::{App, AppEvent};
use crate::host::Result;

#[derive(Serialize, Deserialize)]
pub(crate) struct State {
    pub session: Option<String>,
    pub model: ModelSelection,
    pub thinking: Thinking,
    pub tier: ServiceTier,
    pub moved: bool,
    draft: String,
    attachments: Vec<PathBuf>,
    prompts: Vec<SavedPrompt>,
    next_local: u64,
    hold_queue: bool,
    stashed_draft: Option<String>,
    pending_switch: Option<(ModelSelection, Thinking, ServiceTier)>,
    theme: Theme,
    side: Option<crate::side::SavedSide>,
    focus: Option<nanocode::subagents::AgentId>,
    #[serde(default)]
    pub(super) executor: SavedExecutor,
    #[serde(default)]
    pub(super) hands: Vec<SavedHand>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pending_provider: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct SavedHand {
    endpoint: String,
    token: String,
    name: Option<String>,
    replace: bool,
    call_timeout_ms: Option<u64>,
}

impl From<RunnerSpec> for SavedHand {
    fn from(spec: RunnerSpec) -> Self {
        Self {
            endpoint: spec.endpoint,
            token: spec.token,
            name: spec.name,
            replace: spec.replace,
            call_timeout_ms: spec
                .call_timeout
                .map(|timeout| u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)),
        }
    }
}

impl SavedHand {
    fn runner(self) -> Runner {
        let mut runner = Runner::connect(self.endpoint).token(self.token);
        if let Some(name) = self.name {
            runner = runner.name(name);
        }
        if let Some(timeout) = self.call_timeout_ms {
            runner = runner.call_timeout(std::time::Duration::from_millis(timeout));
        }
        if self.replace {
            runner.replace()
        } else {
            runner.alongside()
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SavedPrompt {
    id: String,
    text: String,
    prompt: Prompt,
    steer: bool,
    durable: bool,
    #[serde(default)]
    attachments: Vec<PathBuf>,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) enum SavedExecutor {
    #[default]
    Local,
    Names(Vec<String>),
    All,
}

pub(crate) async fn read(path: &std::path::Path) -> Result<State> {
    Ok(serde_json::from_slice(&tokio::fs::read(path).await?)?)
}

impl App {
    pub(super) fn reload_executor(&self) -> SavedExecutor {
        match self.agent.as_ref().map(|agent| {
            self.host
                .runtime
                .runner_pool()
                .code_mode_executors()
                .selection(agent.session_id())
        }) {
            Some(ExecutorSelection::Executor(name)) => SavedExecutor::Names(vec![name.to_string()]),
            _ => SavedExecutor::Local,
        }
    }

    pub(super) fn request_reload(&mut self, broadcast: bool) {
        if !cfg!(unix) {
            self.notice(
                NoticeLevel::Error,
                "reload is not supported on this platform",
            );
            return;
        }
        if self.host.config.store.is_none() {
            self.notice(
                NoticeLevel::Warning,
                "reload requires a saved session; restart without --ephemeral",
            );
            return;
        }
        self.reload = true;
        self.reload_all = broadcast;
        self.quit = true;
    }

    pub(super) fn reload_state(&self) -> State {
        State {
            session: self.session_id(),
            model: self.model.clone(),
            thinking: self.thinking,
            tier: self.tier,
            moved: self.moved,
            draft: self.composer.text().to_owned(),
            attachments: self.attachments.clone(),
            prompts: self
                .queue
                .items()
                .iter()
                .filter(|item| !(item.kind == QueueKind::Steer && item.state == QueueState::Sent))
                .map(|item| {
                    let (prompt, durable) =
                        self.resumes.get(&item.id).cloned().unwrap_or_else(|| {
                            (
                                crate::extras::prompt_for(
                                    &item.text,
                                    self.attachments_by_id
                                        .get(&item.id)
                                        .map_or(&[], Vec::as_slice),
                                ),
                                false,
                            )
                        });
                    SavedPrompt {
                        id: item.id.clone(),
                        text: item.text.clone(),
                        prompt,
                        steer: item.kind == QueueKind::Steer,
                        durable,
                        attachments: self
                            .attachments_by_id
                            .get(&item.id)
                            .cloned()
                            .unwrap_or_default(),
                    }
                })
                .collect(),
            next_local: self.next_local,
            hold_queue: self.hold_queue,
            stashed_draft: self.stashed_draft.clone(),
            pending_switch: self
                .pending_switch
                .clone()
                .map(|(model, thinking, tier, _)| (model, thinking, tier)),
            theme: self.theme.clone(),
            side: self.panes.saved(),
            focus: self.focus,
            executor: self.reload_executor(),
            hands: self.reload_hands(),
            provider: self.route.provider().map(str::to_owned),
            pending_provider: self
                .pending_switch
                .as_ref()
                .and_then(|(_, _, _, route)| route.provider())
                .map(str::to_owned),
        }
    }

    pub(super) fn reload_hands(&self) -> Vec<SavedHand> {
        self.host
            .runtime
            .runner_pool()
            .connected_hands()
            .into_iter()
            .map(SavedHand::from)
            .collect()
    }

    fn reconnect_hands(&self, hands: Vec<SavedHand>) {
        let registered = self
            .host
            .runtime
            .list_runners()
            .into_iter()
            .map(|runner| runner.name)
            .collect::<Vec<_>>();
        for hand in hands {
            if hand
                .name
                .as_ref()
                .is_some_and(|name| registered.contains(name))
            {
                continue;
            }
            let runtime = self.host.runtime.clone();
            let tx = self.tx.clone();
            let runner = hand.runner();
            tokio::spawn(async move {
                let (level, message) = match runtime.connect_runner(runner).await {
                    Ok(handle) => (
                        NoticeLevel::Info,
                        format!("reconnected hand {}", handle.info().name),
                    ),
                    Err(error) => (
                        NoticeLevel::Error,
                        format!("hand reconnection failed: {error}"),
                    ),
                };
                drop(tx.send(AppEvent::Notice(level, message)));
            });
        }
    }

    pub(super) fn restore_reload(&mut self, state: State) {
        if let Some(session) = &state.session {
            let selection = match state.executor {
                SavedExecutor::Names(names) => names
                    .into_iter()
                    .next()
                    .map_or(ExecutorSelection::Local, |name| {
                        ExecutorSelection::Executor(name.into())
                    }),
                SavedExecutor::Local | SavedExecutor::All => ExecutorSelection::Local,
            };
            self.host
                .runtime
                .runner_pool()
                .code_mode_executors()
                .select(session, Some(selection));
        }
        self.reconnect_hands(state.hands);
        self.composer.set_text(state.draft);
        self.attachments = state.attachments;
        self.next_local = state.next_local;
        self.hold_queue = state.hold_queue;
        self.stashed_draft = state.stashed_draft;
        let pending = crate::host::Route::of(state.pending_provider.as_deref());
        self.pending_switch = match (state.pending_switch, pending) {
            (Some((model, thinking, tier)), Ok(route)) => Some((model, thinking, tier, route)),
            (Some(_), Err(error)) => {
                self.notice(NoticeLevel::Error, error.to_string());
                None
            }
            (None, _) => None,
        };
        self.theme = state.theme;
        self.reload_side = state.side;
        self.reload_focus = state.focus;
        self.resumes = HashMap::new();
        for item in state.prompts {
            self.attachments_by_id
                .insert(item.id.clone(), item.attachments);
            self.resumes
                .insert(item.id.clone(), (item.prompt, item.durable));
            self.queue.push(QueuedPrompt {
                id: item.id,
                text: item.text,
                kind: if item.steer {
                    QueueKind::Steer
                } else {
                    QueueKind::FollowUp
                },
                state: QueueState::Pending,
            });
        }
    }
}

#[cfg(unix)]
pub(super) struct Instance {
    socket: std::sync::Arc<tokio::net::UnixDatagram>,
    path: PathBuf,
    listener: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl Instance {
    pub(super) async fn start(tx: tokio::sync::mpsc::UnboundedSender<AppEvent>) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!("nanocode-{}", nix::unistd::getuid()));
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .await?;
        let permissions = tokio::fs::metadata(&directory).await?.permissions();
        if permissions.mode() & 0o077 != 0 {
            return Err(format!("reload directory {} must be private", directory.display()).into());
        }
        let path = directory.join(format!("{}.sock", std::process::id()));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let socket = std::sync::Arc::new(tokio::net::UnixDatagram::bind(&path)?);
        let receiver = socket.clone();
        let listener = tokio::spawn(async move {
            let mut message = [0; REQUEST.len() + 1];
            loop {
                match receiver.recv(&mut message).await {
                    Ok(length) if &message[..length] == REQUEST => {
                        if tx.send(AppEvent::Reload).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        drop(tx.send(AppEvent::Notice(
                            NoticeLevel::Error,
                            format!("reload listener: {error}"),
                        )));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            socket,
            path,
            listener,
        })
    }

    pub(super) async fn broadcast(&self) -> Result<()> {
        let mut entries =
            tokio::fs::read_dir(self.path.parent().ok_or("reload socket has no directory")?)
                .await?;
        let mut failures = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path == self.path || path.extension().is_none_or(|extension| extension != "sock") {
                continue;
            }
            match self.socket.send_to(REQUEST, &path).await {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    if let Err(error) = tokio::fs::remove_file(&path).await
                        && error.kind() != io::ErrorKind::NotFound
                    {
                        failures.push(format!("{}: {error}", path.display()));
                    }
                }
                Err(error) => failures.push(format!("{}: {error}", path.display())),
            }
        }
        if !failures.is_empty() {
            return Err(failures.join("\n").into());
        }
        Ok(())
    }

    pub(super) async fn save(&self, state: &State) -> Result<PathBuf> {
        use tokio::io::AsyncWriteExt;
        let path = self.path.with_extension("json");
        let bytes = serde_json::to_vec(state)?;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        Ok(path)
    }

    pub(super) async fn close(self) -> Result<()> {
        self.listener.abort();
        drop(self.listener.await);
        drop(self.socket);
        tokio::fs::remove_file(self.path).await?;
        Ok(())
    }
}

#[cfg(unix)]
const REQUEST: &[u8] = b"reload";

#[cfg(not(unix))]
pub(super) struct Instance;

#[cfg(not(unix))]
impl Instance {
    pub(super) async fn start(_: tokio::sync::mpsc::UnboundedSender<AppEvent>) -> Result<Self> {
        Ok(Self)
    }
    pub(super) async fn broadcast(&self) -> Result<()> {
        Err("reload is not supported on this platform".into())
    }
    pub(super) async fn save(&self, _: &State) -> Result<PathBuf> {
        Err("reload is not supported on this platform".into())
    }
    pub(super) async fn close(self) -> Result<()> {
        Ok(())
    }
}

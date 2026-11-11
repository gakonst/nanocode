pub use nanocode_tool_runtime::attachment::{
    AttachmentError, Runner, RunnerEvent, RunnerPool, RunnerProtocol, RunnerServerConfig,
    RunnerStatus,
};

use nanocode_sync::{Atom, Signal, Slot};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

type Startup = tokio::sync::watch::Receiver<Option<Result<(), AttachmentError>>>;

pub(crate) struct RunnerAgents {
    pool: RunnerPool,
    pending: Slot<Pending>,
    startup: Atom<Vec<Startup>>,
    activated: Signal,
    code_mode_hosts: Atom<nanocode_sync::Map<String, bool>>,
    agents: Atom<nanocode_sync::Map<String, (crate::WeakAgent, mpsc::UnboundedSender<Notice>)>>,
    folded: Atom<nanocode_sync::Map<String, (crate::WeakAgent, String)>>,
    initialized: tokio::sync::OnceCell<Result<(), AttachmentError>>,
}

const NOTICE_PREFIX: &str = "Runner ";
const NO_RUNNERS: &str = "Runner mounts and hands: none connected.";

struct Pending {
    events: mpsc::UnboundedReceiver<RunnerEvent>,
    runners: Vec<Runner>,
    servers: Vec<RunnerServerConfig>,
}

struct Notice {
    text: String,
    prefix: Option<String>,
    completed: Option<oneshot::Sender<nanocode_agent_core::Result<()>>>,
}

impl RunnerAgents {
    pub(crate) fn new(
        pool: RunnerPool,
        runners: Vec<Runner>,
        servers: Vec<RunnerServerConfig>,
    ) -> Arc<Self> {
        let events = pool.subscribe();
        Arc::new(Self {
            pool,
            initialized: tokio::sync::OnceCell::new(),
            pending: Slot::new(Some(Pending {
                events,
                runners,
                servers,
            })),
            startup: Atom::default(),
            activated: Signal::new(),
            code_mode_hosts: Atom::default(),
            agents: Atom::default(),
            folded: Atom::default(),
        })
    }

    pub(crate) fn activate(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Some(Pending {
            mut events,
            runners,
            servers,
        }) = self.pending.take()
        else {
            return;
        };
        let weak = Arc::downgrade(self);
        runtime.spawn(async move {
            while let Some(event) = events.recv().await {
                let Some(agents) = weak.upgrade() else { break };
                agents.notify(event);
            }
        });
        let mut startup = Vec::new();
        for runner in runners {
            if let Err(error) = self.pool.keep_connected(runner) {
                startup.push(tokio::sync::watch::channel(Some(Err(error))).1);
            }
        }
        for server in servers {
            let pool = self.pool.clone();
            let (receipt, result) = tokio::sync::watch::channel(None);
            runtime.spawn(async move {
                let outcome = match pool.listen(server).await {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        pool.report_failure("runner-server", error.to_string());
                        Err(error)
                    }
                };
                receipt.send_replace(Some(outcome));
            });
            startup.push(result);
        }
        self.startup.set(startup);
        self.activated.fire();
    }

    pub(crate) async fn initialize(self: &Arc<Self>) -> Result<(), AttachmentError> {
        tokio::runtime::Handle::try_current()
            .map_err(|error| AttachmentError::Transport(error.to_string().into()))?;
        self.activate();
        self.activated.wait().await;
        self.initialized
            .get_or_init(|| async {
                let mut result = Ok(());
                for mut receipt in self.startup.get().iter().cloned() {
                    let outcome = loop {
                        if let Some(outcome) = receipt.borrow().clone() {
                            break outcome;
                        }
                        if receipt.changed().await.is_err() {
                            break Err(AttachmentError::Transport(
                                "runner startup stopped before completion".into(),
                            ));
                        }
                    };
                    if result.is_ok() {
                        result = outcome;
                    }
                }
                result
            })
            .await
            .clone()
    }

    fn summary(&self) -> Option<String> {
        let runners = self.pool.list();
        let ready = |protocol| {
            runners.iter().filter(move |runner| {
                runner.status == RunnerStatus::Ready && runner.protocol == protocol
            })
        };
        let mounts = ready(RunnerProtocol::Tools)
            .map(|runner| {
                format!(
                    "/{} (mode: {:?}, tools: {})",
                    runner.name, runner.mode, runner.tool_count,
                )
            })
            .collect::<Vec<_>>();
        let executors = ready(RunnerProtocol::CodeMode)
            .map(|runner| {
                format!(
                    "{} (mode: {:?}, directory: {}, tools: {})",
                    runner.name,
                    runner.mode,
                    runner
                        .workspace
                        .as_ref()
                        .map_or_else(|| "unadvertised".into(), |path| path.display().to_string()),
                    runner.tool_count,
                )
            })
            .collect::<Vec<_>>();
        if mounts.is_empty() && executors.is_empty() {
            return None;
        }
        let mut summary = if mounts.is_empty() {
            "Runner mounts: none.".to_owned()
        } else {
            format!(
                "Runner mounts currently available: {}. Workspace tools accept an optional workdir: omit it for the default host or select an attached runner with /<runner-name>/...; paths below that root map to its workspace. Process input uses the returned logical session_id, and patches use absolute paths within one host.",
                mounts.join("; ")
            )
        };
        if !executors.is_empty() {
            summary.push_str(&format!(
                " Hands connected: {}. The runners tool with action=select runs a session's exec cells on one hand, several, or every hand at once.",
                executors.join("; ")
            ));
        }
        Some(summary)
    }

    fn notify(&self, event: RunnerEvent) {
        let notice = match event {
            RunnerEvent::StatusChanged { info } => match info.status {
                RunnerStatus::Ready | RunnerStatus::Disconnected | RunnerStatus::Failed(_) => {
                    match info.protocol {
                        RunnerProtocol::Tools => format!(
                            "Runner /{} status: {:?}. {}",
                            info.name,
                            info.status,
                            self.status_summary()
                        ),
                        RunnerProtocol::CodeMode => format!(
                            "Hand {} status: {:?}. {}",
                            info.name,
                            info.status,
                            self.status_summary()
                        ),
                    }
                }
                RunnerStatus::Connecting => return,
            },
            RunnerEvent::Failed { name, error } => {
                format!("Runner /{name} failed: {error}. {}", self.status_summary())
            }
        };
        let mut stale = false;
        for (agent, notices) in self.agents.get().values() {
            stale |= !agent.is_live()
                || notices
                    .send(Notice {
                        text: notice.clone(),
                        prefix: Some(NOTICE_PREFIX.to_owned()),
                        completed: None,
                    })
                    .is_err();
        }
        if stale {
            self.agents.update(|agents| {
                agents.retain(|_, (agent, notices)| agent.is_live() && !notices.is_closed());
            });
        }
    }

    pub(crate) fn unregister(&self, session_id: &str) {
        self.pool.code_mode_executors().forget(session_id);
        self.agents.update(|agents| {
            agents.remove(session_id);
        });
        self.folded.update(|folded| {
            folded.remove(session_id);
        });
        self.code_mode_hosts.update(|hosts| {
            hosts.remove(session_id);
        });
    }

    pub(crate) fn initial_notice(
        self: &Arc<Self>,
        session_id: &str,
        parent_session_id: Option<&str>,
    ) -> Option<String> {
        self.activate();
        let configured = self.code_mode_hosts.read(|hosts| {
            hosts
                .get(session_id)
                .copied()
                .or_else(|| parent_session_id.and_then(|parent| hosts.get(parent).copied()))
                .unwrap_or(false)
        });
        if configured { None } else { self.summary() }
    }

    fn status_summary(&self) -> String {
        self.summary().unwrap_or_else(|| NO_RUNNERS.to_owned())
    }

    pub(crate) fn initial_notice_committed(&self, agent: &crate::Agent, text: String) {
        let weak = agent.downgrade();
        self.folded.update(|folded| {
            folded.insert(agent.session_id().to_owned(), (weak.clone(), text.clone()));
        });
    }

    pub(crate) fn code_mode_host(&self, session_id: &str, configured: bool) {
        self.code_mode_hosts.update(|hosts| {
            hosts.insert(session_id.to_owned(), configured);
        });
    }

    pub(crate) async fn register(
        self: &Arc<Self>,
        agent: &crate::Agent,
        await_initial: bool,
    ) -> nanocode_agent_core::Result<()> {
        let parent = agent.session().lineage.parent_session_id.clone();
        let configured = self.code_mode_hosts.try_update(|hosts| {
            hosts
                .get(agent.session_id())
                .copied()
                .or_else(|| {
                    parent
                        .as_ref()
                        .and_then(|parent| hosts.get(parent).copied())
                })
                .unwrap_or(false)
                .then(|| {
                    hosts.insert(agent.session_id().to_owned(), true);
                })
        });
        if configured.is_some() {
            return Ok(());
        }
        self.activate();
        if !agent.capabilities().developer_messages {
            return Ok(());
        }
        let weak = agent.downgrade();
        let worker = weak.clone();
        let (notices, mut receiver) = mpsc::unbounded_channel::<Notice>();
        let (completed, result) = oneshot::channel();
        let claimed = self.agents.try_update(|agents| {
            if agents
                .get(agent.session_id())
                .is_some_and(|(agent, notices)| agent.is_live() && !notices.is_closed())
            {
                return None;
            }
            agents.insert(
                agent.session_id().to_owned(),
                (weak.clone(), notices.clone()),
            );
            if agents.len().is_power_of_two() {
                agents.retain(|_, (agent, notices)| agent.is_live() && !notices.is_closed());
            }
            Some(())
        });
        if claimed.is_none() {
            return Ok(());
        }
        let folded = self.folded.update(|folded| {
            folded
                .remove(agent.session_id())
                .filter(|(agent, _)| agent.is_live())
                .map(|(_, text)| text)
        });
        let folded_initial = folded.is_some();
        let text = match (folded, self.summary()) {
            (Some(folded), Some(summary)) if folded == summary => None,
            (Some(_), None) => Some(NO_RUNNERS.to_owned()),
            (None, summary) | (Some(_), summary) => summary,
        };
        let awaited = await_initial && !folded_initial && text.is_some();
        if let Some(text) = text {
            notices
                .send(Notice {
                    text,
                    prefix: Some(NOTICE_PREFIX.to_owned()),
                    completed: awaited.then_some(completed),
                })
                .map_err(|_| crate::NanocodeError::AgentStopped)?;
        }
        tokio::spawn(async move {
            while let Some(mut notice) = receiver.recv().await {
                while notice.completed.is_none()
                    && let Ok(next) = receiver.try_recv()
                {
                    notice = next;
                }
                let outcome = worker
                    .append_developer_notice(notice.text, notice.prefix)
                    .await
                    .unwrap_or(Err(crate::NanocodeError::AgentStopped))
                    .map(|_| ());
                let failed = outcome.is_err();
                if let Some(completed) = notice.completed {
                    let _ = completed.send(outcome);
                }
                if failed {
                    break;
                }
            }
        });
        if awaited {
            result
                .await
                .map_err(|_| crate::NanocodeError::AgentStopped)?
        } else {
            Ok(())
        }
    }
}

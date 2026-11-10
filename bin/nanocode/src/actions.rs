use nanocode::{
    AgentStatus, AgentUpdate, ModelSelection, Mutability, ScopedAgentUpdate, ServiceTier, Thinking,
    agent::events::AgentEventKind,
};
use nanocode_tui::{
    transcript::{CellKind, NoticeLevel},
    tree::{NodeStatus, TreeNode},
};

use crate::{
    app::{
        App, AppEvent, BtwUpdate, Ready, Redraw, TurnState, apply_tier, family_label, open_session,
        subagent_status,
    },
    focus::Scope,
    host::Route,
};

impl App {
    pub(crate) fn request_switch(
        &mut self,
        model: ModelSelection,
        thinking: Thinking,
        tier: ServiceTier,
        route: Route,
    ) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "request_switch", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self.session_started && (model != self.model || route != self.route) {
            self.flash(
                "the model is fixed for this thread; use /new before choosing another model",
            );
            return;
        }
        if !matches!(self.turn, TurnState::Idle) || self.starting {
            self.pending_switch = Some((model, thinking, tier, route));
            self.flash("settings will apply when the current turn finishes");
            return;
        }
        self.switch(model, thinking, tier, route);
    }

    pub(crate) fn switch(
        &mut self,
        model: ModelSelection,
        thinking: Thinking,
        tier: ServiceTier,
        route: Route,
    ) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "switch", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self.session_started && (model != self.model || route != self.route) {
            self.flash(
                "the model is fixed for this thread; use /new before choosing another model",
            );
            return;
        }
        let capabilities = model.capabilities(nanocode::ModelTransport::Native);
        if let Err(error) = capabilities
            .check_thinking(thinking)
            .and_then(|()| capabilities.check_service_tier(tier))
        {
            self.notice(NoticeLevel::Error, error.to_string());
            return;
        }
        let Some(agent) = self.agent.clone() else {
            self.model = model;
            self.route = route;
            self.thinking = thinking;
            self.tier = tier;
            return;
        };
        let tx = self.tx.clone();
        let mutability = agent.capabilities();
        let started = self.session_started;
        let mutable = |setting: Mutability| {
            matches!(setting, Mutability::Anytime | Mutability::BetweenTurns)
                || setting == Mutability::BeforeFirstPrompt && !started
        };
        let model_changed = model != self.model;
        let thinking_changed = thinking != self.thinking;
        let tier_changed = tier != self.tier;
        let in_place = route == self.route
            && model.family() == self.model.family()
            && (!model_changed || mutable(mutability.model))
            && (!thinking_changed || mutable(mutability.thinking))
            && (!tier_changed || mutable(mutability.service_tier));
        if in_place {
            self.starting = true;
            tokio::spawn(async move {
                let applied = async {
                    if model_changed {
                        agent.set_harness_model(model.clone()).await?;
                    }
                    if thinking_changed || model_changed {
                        agent.set_thinking(thinking).await?;
                    }
                    if tier_changed || model_changed {
                        apply_tier(&agent, &model, tier).await?;
                    }
                    Ok::<_, crate::host::Error>(())
                };
                let result = match applied.await {
                    Ok(()) => {
                        let notice =
                            format!("using {model} · thinking {thinking} · {}", tier.as_str());
                        Ok((model, thinking, tier, notice))
                    }
                    Err(error) => Err(error.to_string()),
                };
                drop(tx.send(AppEvent::Applied(result)));
            });
            return;
        }
        let host = self.host.clone();
        let previous = self.route.label(&self.model);
        let previous_family = self.model.family();
        self.starting = true;
        tokio::spawn(async move {
            let ready = async {
                let session_id = agent.session_id().to_owned();
                let (next, events) = if started {
                    host.runtime
                        .switch(&agent, model.clone(), Some(thinking))
                        .await?
                } else {
                    host.start(&model, thinking, route).await?
                };
                if let Err(error) = apply_tier(&next, &model, tier).await {
                    drop(next.shutdown().await);
                    return Err(error);
                }
                drop(agent.flush().await);
                drop(agent.shutdown().await);
                let carried = if started {
                    format!("; conversation carried over, previous session {session_id} kept")
                } else {
                    String::new()
                };
                Ok::<_, crate::host::Error>(Ready {
                    notice: format!(
                        "switched {} {previous} → {} {} · thinking {thinking} · {} · new session {}{carried}",
                        family_label(&previous_family),
                        family_label(&model.family()),
                        route.label(&model),
                        tier.as_str(),
                        next.session_id()
                    ),
                    agent: next,
                    events,
                    model,
                    route,
                    thinking,
                    tier,
                    history: None,
                    reset_transcript: false,
                    pending: Vec::new(),
                    prompts: Vec::new(),
                })
            };
            let event = match ready.await {
                Ok(ready) => AppEvent::AgentReady(Box::new(ready)),
                Err(error) => AppEvent::AgentFailed(format!("switch failed: {error}")),
            };
            drop(tx.send(event));
        });
    }

    pub(crate) fn btw_update(&mut self, id: u64, update: BtwUpdate) {
        let Some(&(owner, cell)) = self.btw.get(&id) else {
            return;
        };
        let finished = matches!(update, BtwUpdate::Done | BtwUpdate::Failed(_));
        let failed = matches!(update, BtwUpdate::Failed(_));
        if !self.side_owns(id, owner) {
            self.btw.remove(&id);
            return;
        }
        let Some(transcript) = self.side_transcript_mut() else {
            return;
        };
        let extended = match (&update, transcript.get(cell)) {
            (BtwUpdate::Message(text), Some(CellKind::Btw { answer, .. }))
                if text.starts_with(answer.as_str()) =>
            {
                Some(answer.len())
            }
            _ => None,
        };
        match (extended, &update) {
            (Some(start), BtwUpdate::Message(text)) => {
                if start < text.len() {
                    transcript.append(cell, &text[start..]);
                }
            }
            _ => {
                transcript.update(cell, |kind| {
                    if let CellKind::Btw { answer, done, .. } = kind {
                        match update {
                            BtwUpdate::Message(text) => *answer = text,
                            BtwUpdate::Done => *done = true,
                            BtwUpdate::Failed(error) => {
                                *done = true;
                                answer.push_str(&format!("\n\n*side question failed: {error}*"));
                            }
                        }
                    }
                });
            }
        }
        if finished {
            self.btw.remove(&id);
            self.side_answered(id, cell, failed);
        }
    }

    pub(crate) fn subagent(&mut self, scoped: ScopedAgentUpdate) -> Redraw {
        let scope = match self.scope_of(&scoped.root_session_id) {
            Some(scope) => scope,
            None if self.agent.is_none() => Scope::Main,
            None => return Redraw::None,
        };
        match scoped.update {
            AgentUpdate::Added(descriptor) => {
                self.track_subagent(scope, &descriptor);
                let id = scope.tree_id(descriptor.id);
                let mut existing = None;
                self.tree.update(&id, |node| {
                    existing = Some((node.status, node.label.clone()))
                });
                let (status, label) = existing.unwrap_or_else(|| {
                    (
                        NodeStatus::Pending,
                        format!("{}#{} {}", scope.prefix(), descriptor.id, descriptor.role),
                    )
                });
                self.tree.upsert(TreeNode {
                    id,
                    parent: descriptor.parent.map(|parent| scope.tree_id(parent)),
                    label,
                    detail: descriptor
                        .task
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                    status,
                });
                Redraw::Streaming
            }
            AgentUpdate::Status { id, status } => {
                if matches!(status, AgentStatus::Interrupted) && scope == Scope::Main {
                    self.refresh_agents();
                }
                let (status, detail) = subagent_status(&status);
                self.tree.update(&scope.tree_id(id), |node| {
                    if node.status != NodeStatus::Closed {
                        node.status = status;
                    }
                    if !detail.is_empty() {
                        node.detail = detail;
                    }
                });
                Redraw::Streaming
            }
            AgentUpdate::Completion { id, status, .. } => {
                if matches!(status, AgentStatus::Interrupted) && scope == Scope::Main {
                    self.refresh_agents();
                }
                let (node_status, detail) = subagent_status(&status);
                let key = scope.tree_id(id);
                self.tree.update(&key, |node| {
                    if node.status != NodeStatus::Closed {
                        node.status = node_status;
                    }
                    if !detail.is_empty() {
                        node.detail = detail;
                    }
                });
                Redraw::Streaming
            }
            AgentUpdate::Event { id, event, .. } => {
                let payload = matches!(
                    event.kind,
                    AgentEventKind::RunStarted
                        | AgentEventKind::ToolCall
                        | AgentEventKind::AssistantMessage
                )
                .then(|| serde_json::from_str::<serde_json::Value>(event.payload.get()).ok())
                .flatten();
                let changed = self.subagent_event(scope, id, &event, payload.as_ref());
                let mut redraw = if changed && self.focus_of(scope) == Some(id) {
                    Redraw::Streaming
                } else {
                    Redraw::None
                };
                let key = scope.tree_id(id);
                if event.kind == AgentEventKind::RunStarted
                    && let Some(model) = payload
                        .as_ref()
                        .and_then(|payload| payload["model"].as_str())
                {
                    let model = model.to_owned();
                    self.tree.update(&key, |node| {
                        let role = node
                            .label
                            .split(" · ")
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        node.label = format!("{role} · {model}");
                    });
                    redraw = Redraw::Streaming;
                }
                let detail = payload.and_then(|payload| match event.kind {
                    AgentEventKind::ToolCall => payload["tool"].as_str().map(|tool| {
                        format!(
                            "▸ {}",
                            crate::events::tool_label(tool, &payload["arguments"])
                        )
                    }),
                    AgentEventKind::AssistantMessage => payload["text"]
                        .as_str()
                        .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
                        .map(str::to_owned),
                    _ => None,
                });
                if let Some(detail) = detail {
                    self.tree.update(&key, |node| {
                        node.detail = detail;
                        if node.status == NodeStatus::Idle {
                            node.status = NodeStatus::Running;
                        }
                    });
                    redraw = Redraw::Streaming;
                }
                redraw
            }
            AgentUpdate::Message(_) => Redraw::None,
        }
    }

    pub(crate) fn open_session_picker(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_session_picker", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(store) = self.host.config.store.clone() else {
            self.flash("sessions are not saved with --ephemeral");
            return;
        };
        let workspace = self.host.config.workspace.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let sessions = store
                .list()
                .await
                .map_err(|error| error.to_string())
                .map(|sessions| {
                    sessions
                        .into_iter()
                        .filter(|session| {
                            crate::sessions::user_facing(session.record.lineage.origin)
                                && session.record.workspace.as_deref() == Some(workspace.as_path())
                        })
                        .collect()
                });
            drop(tx.send(AppEvent::Sessions(sessions)));
        });
    }

    pub(crate) fn resume(&mut self, session_id: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "resume", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if !matches!(self.turn, TurnState::Idle) {
            self.flash("interrupt the running turn (esc) before resuming another session");
            return;
        }
        if self.session_id().as_deref() == Some(session_id.as_str()) {
            self.flash("already in this session");
            return;
        }
        let previous = self.agent.take();
        let host = self.host.clone();
        let tx = self.tx.clone();
        self.starting = true;
        self.generation += 1;
        tokio::spawn(async move {
            if let Some(previous) = previous {
                drop(previous.flush().await);
                drop(previous.shutdown().await);
            }
            let event = match open_session(&host, &session_id, None, None).await {
                Ok(ready) => AppEvent::AgentReady(Box::new(ready)),
                Err(error) => AppEvent::AgentFailed(format!("resume failed: {error}")),
            };
            drop(tx.send(event));
        });
    }

    pub(crate) fn new_session(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "new_session", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if !matches!(self.turn, TurnState::Idle) {
            self.flash("interrupt the running turn (esc) before starting a new session");
            return;
        }
        let previous = self.agent.take();
        let host = self.host.clone();
        let model = self.model.clone();
        let route = self.route;
        let thinking = self.thinking;
        let tier = self.tier;
        let tx = self.tx.clone();
        self.starting = true;
        self.generation += 1;
        tokio::spawn(async move {
            if let Some(previous) = previous {
                drop(previous.flush().await);
                drop(previous.shutdown().await);
            }
            let ready = async {
                let (agent, events) = host.start(&model, thinking, route).await?;
                apply_tier(&agent, &model, tier).await?;
                Ok::<_, crate::host::Error>(Ready {
                    notice: format!("new session {}", agent.session_id()),
                    agent,
                    events,
                    model,
                    route,
                    thinking,
                    tier,
                    history: None,
                    reset_transcript: true,
                    pending: Vec::new(),
                    prompts: Vec::new(),
                })
            };
            let event = match ready.await {
                Ok(ready) => AppEvent::AgentReady(Box::new(ready)),
                Err(error) => AppEvent::AgentFailed(error.to_string()),
            };
            drop(tx.send(event));
        });
    }

    pub(crate) fn compact(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "compact", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(agent) = self.agent.clone() else {
            return;
        };
        if !matches!(self.turn, TurnState::Idle) {
            self.flash("wait for the turn to finish before compacting");
            return;
        }
        if let Some(gateway) = &self.host.config.gateway
            && self.route == Route::Native
            && !gateway.compacts(&self.model)
        {
            self.notice(
                NoticeLevel::Warning,
                format!(
                    "the {} gateway does not support compaction; use /new to start a fresh thread",
                    gateway.name()
                ),
            );
            return;
        }
        let tx = self.tx.clone();
        self.flash("compacting…");
        tokio::spawn(async move {
            let event = match agent.compact().await {
                Ok(()) => AppEvent::Notice(NoticeLevel::Info, "context compacted".into()),
                Err(error) => {
                    AppEvent::Notice(NoticeLevel::Error, format!("compaction failed: {error}"))
                }
            };
            drop(tx.send(event));
        });
    }
}

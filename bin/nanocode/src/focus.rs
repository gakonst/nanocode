use nanocode::{
    agent::events::{AgentEvent, AgentEventKind},
    durability::TranscriptItem,
    subagents::{AgentDescriptor, AgentDirectoryEntry, AgentId, MessagePriority},
};
use nanocode_tui::{
    transcript::{NoticeLevel, Transcript, TranscriptPolicy},
    tree::{NodeStatus, TreeNode},
};

use crate::{
    app::{App, AppEvent, DIFF_PREVIEW_LINES, TOOL_PREVIEW_LINES, subagent_status},
    events::{EventMapper, subagent_prompt_display},
};

const AGENT_ARGUMENT_COMMANDS: [&str; 4] = ["/stop", "/stop-agent", "/agent", "/focus"];

pub struct SubagentView {
    pub role: String,
    pub model: Option<String>,
    pub transcript: Transcript,
    mapper: EventMapper,
    loading: bool,
}

pub struct FocusReady {
    root: String,
    id: AgentId,
    role: String,
    model: Option<String>,
    history: Vec<TranscriptItem>,
}

impl SubagentView {
    fn new(role: String, model: Option<String>) -> Self {
        Self {
            role,
            model,
            transcript: Transcript::new(TranscriptPolicy {
                collapsed_preview_lines: TOOL_PREVIEW_LINES,
                collapsed_diff_lines: DIFF_PREVIEW_LINES,
            }),
            mapper: EventMapper::subagent(),
            loading: false,
        }
    }
}

impl App {
    pub(crate) fn track_subagent(&mut self, descriptor: &AgentDescriptor) {
        self.mapper
            .learn_agent(descriptor.id, descriptor.role.clone());
        self.views.entry(descriptor.id).or_insert_with(|| {
            SubagentView::new(
                descriptor.role.clone(),
                descriptor.model.as_ref().map(ToString::to_string),
            )
        });
    }

    pub(crate) fn subagent_event(
        &mut self,
        id: AgentId,
        event: &AgentEvent,
        payload: Option<&serde_json::Value>,
    ) -> bool {
        let Some(view) = self.views.get_mut(&id) else {
            return false;
        };
        if event.kind == AgentEventKind::RunStarted
            && let Some(model) = payload.and_then(|payload| payload["model"].as_str())
        {
            view.model = Some(model.to_owned());
        }
        view.mapper.apply(&mut view.transcript, event).changed
    }

    pub(crate) fn visible_transcript(&mut self) -> &mut Transcript {
        match self.focus.and_then(|id| self.views.get_mut(&id)) {
            Some(view) => &mut view.transcript,
            None => &mut self.transcript,
        }
    }

    pub(crate) fn reset_focus(&mut self) {
        self.focus = None;
        self.views.clear();
    }

    pub(crate) fn focus_command(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "focus_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let argument = argument.trim().trim_start_matches('#');
        if argument.is_empty() || argument.eq_ignore_ascii_case("main") {
            self.unfocus();
            return;
        }
        let id = argument.parse::<u64>().ok().map(AgentId::new).or_else(|| {
            self.views
                .iter()
                .filter(|(_, view)| view.role.eq_ignore_ascii_case(argument))
                .map(|(id, _)| *id)
                .min()
        });
        match id {
            Some(id) => self.focus_on(id),
            None => self.flash(format!("no subagent named {argument}; use its #id")),
        }
    }

    pub(crate) fn cycle_focus(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "cycle_focus", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let order = self
            .tree
            .ordered()
            .into_iter()
            .filter_map(|(_, _, node)| node.id.parse::<u64>().ok().map(AgentId::new))
            .collect::<Vec<_>>();
        let next = match self.focus {
            None => order.first().copied(),
            Some(current) => order
                .iter()
                .position(|id| *id == current)
                .and_then(|index| order.get(index + 1))
                .copied(),
        };
        match next {
            Some(id) => self.focus_on(id),
            None if self.focus.is_some() => self.unfocus(),
            None => self.flash("no subagents to focus"),
        }
    }

    fn unfocus(&mut self) {
        if self.focus.take().is_some() {
            self.flash("back to the main thread");
        }
    }

    fn focus_on(&mut self, id: AgentId) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "focus_on", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(root) = self
            .agent
            .as_ref()
            .map(|agent| agent.session_id().to_owned())
        else {
            self.flash("no session yet");
            return;
        };
        let Some(control) = self.host.runtime.subagents().cloned() else {
            self.flash("subagents are disabled for this session");
            return;
        };
        let known = self.views.get_mut(&id).map(|view| {
            let empty = view.transcript.is_empty() && !view.loading;
            view.loading |= empty;
            (view.role.clone(), empty)
        });
        let backfill = match known {
            Some((role, empty)) => {
                self.focus = Some(id);
                self.flash(format!(
                    "focused #{id} {role} · enter messages it · esc esc interrupts · /agent returns"
                ));
                empty
            }
            None => true,
        };
        if !backfill {
            return;
        }
        let store = self.host.config.store.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ready = async {
                let entry = control
                    .agents(&root)
                    .await
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|entry| entry.agent_id == id)
                    .ok_or_else(|| format!("no subagent #{id} in this session"))?;
                let history = match store {
                    Some(store) => {
                        let session = control
                            .session_id(&root, id)
                            .await
                            .map_err(|error| error.to_string())?;
                        store
                            .load(&session)
                            .await
                            .map_err(|error| error.to_string())?
                            .transcript
                    }
                    None => Vec::new(),
                };
                Ok::<_, String>(FocusReady {
                    root: root.clone(),
                    id,
                    role: entry.role,
                    model: entry.model.as_ref().map(ToString::to_string),
                    history,
                })
            };
            drop(tx.send(match ready.await {
                Ok(ready) => AppEvent::FocusReady(Box::new(ready)),
                Err(error) => AppEvent::Notice(NoticeLevel::Error, format!("focus #{id}: {error}")),
            }));
        });
    }

    pub(crate) fn focus_ready(&mut self, ready: FocusReady) {
        let FocusReady {
            root,
            id,
            role,
            model,
            history,
        } = ready;
        if self.session_id().as_deref() != Some(root.as_str()) {
            return;
        }
        let view = self
            .views
            .entry(id)
            .or_insert_with(|| SubagentView::new(role, model));
        view.loading = false;
        if view.transcript.is_empty() {
            crate::sessions::replay(
                &mut view.transcript,
                history
                    .into_iter()
                    .map(|item| match item {
                        TranscriptItem::User(text) => {
                            TranscriptItem::User(subagent_prompt_display(&text))
                        }
                        other => other,
                    })
                    .collect(),
            );
        }
        if self.focus != Some(id) {
            self.focus = Some(id);
            let role = view.role.clone();
            self.flash(format!(
                "focused #{id} {role} · enter messages it · esc esc interrupts · /agent returns"
            ));
        }
    }

    pub(crate) fn message_focused(&mut self, id: AgentId, text: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "message_focused", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let (Some(root), Some(control)) = (
            self.agent
                .as_ref()
                .map(|agent| agent.session_id().to_owned()),
            self.host.runtime.subagents().cloned(),
        ) else {
            self.flash("no session to message");
            return;
        };
        if let Some(view) = self.views.get_mut(&id) {
            view.transcript.scroll_to_bottom();
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(error) = control
                .send_user_message(&root, id, MessagePriority::Urgent, text)
                .await
            {
                drop(tx.send(AppEvent::Notice(
                    NoticeLevel::Error,
                    format!("message to #{id} failed: {error}"),
                )));
            }
        });
    }

    fn agent_label(&self, id: AgentId) -> String {
        match self.views.get(&id) {
            Some(view) => format!("#{id} {}", view.role),
            None => format!("#{id}"),
        }
    }

    fn listed_agents(&self) -> Vec<(AgentId, String)> {
        self.tree
            .ordered()
            .into_iter()
            .filter_map(|(_, _, node)| {
                let id = AgentId::new(node.id.parse::<u64>().ok()?);
                let role = self
                    .views
                    .get(&id)
                    .map_or_else(String::new, |view| view.role.clone());
                Some((id, role))
            })
            .collect()
    }

    pub(crate) fn agent_argument_candidates(&self, text: &str) -> Option<Vec<(String, String)>> {
        let (command, partial) = text.split_once(' ')?;
        if !AGENT_ARGUMENT_COMMANDS.contains(&command) || partial.contains(char::is_whitespace) {
            return None;
        }
        let partial = partial.trim_start_matches('#').to_ascii_lowercase();
        let candidates = self
            .listed_agents()
            .into_iter()
            .filter(|(id, role)| {
                id.to_string().starts_with(&partial)
                    || role.to_ascii_lowercase().starts_with(&partial)
            })
            .map(|(id, role)| {
                let key = id.to_string();
                let detail = self
                    .tree
                    .nodes()
                    .iter()
                    .find(|node| node.id == key)
                    .map(|node| format!("{role} · {}", node.status.label()))
                    .unwrap_or(role);
                (format!("#{id}"), detail)
            })
            .collect::<Vec<_>>();
        (!candidates.is_empty()).then_some(candidates)
    }

    pub(crate) fn complete_agent_argument(&mut self) -> bool {
        let text = self.composer.text().to_owned();
        let Some((command, partial)) = text.split_once(' ') else {
            return false;
        };
        if !AGENT_ARGUMENT_COMMANDS.contains(&command) || partial.contains(char::is_whitespace) {
            return false;
        }
        let listed = self.listed_agents();
        let current = partial.trim_start_matches('#');
        let next = match listed
            .iter()
            .position(|(id, _)| !current.is_empty() && id.to_string() == current)
        {
            Some(index) => listed.get((index + 1) % listed.len()).map(|(id, _)| *id),
            None => self
                .agent_argument_candidates(&text)
                .and_then(|candidates| candidates.into_iter().next())
                .and_then(|(key, _)| key.trim_start_matches('#').parse::<u64>().ok())
                .map(AgentId::new),
        };
        match next {
            Some(id) => {
                self.composer.set_text(format!("{command} #{id}"));
                true
            }
            None => false,
        }
    }

    pub(crate) fn agents_command(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "agents_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if argument.eq_ignore_ascii_case("all") {
            self.list_all_agents();
            return;
        }
        if !argument.is_empty() {
            self.notice(
                NoticeLevel::Warning,
                format!("unknown /agents option {argument}; use /agents or /agents all"),
            );
            return;
        }
        if self.tree.listed() == 0 {
            self.show_tree = true;
            self.refresh_agents();
            self.flash(match self.tree.finished() {
                0 => "no active subagents".to_owned(),
                count => format!("no active subagents · {count} finished in /agents all"),
            });
            return;
        }
        self.show_tree = !self.show_tree;
        if self.show_tree {
            self.refresh_agents();
        }
    }

    fn list_all_agents(&mut self) {
        let (Some(root), Some(control)) = (
            self.agent
                .as_ref()
                .map(|agent| agent.session_id().to_owned()),
            self.host.runtime.subagents().cloned(),
        ) else {
            self.flash("no subagents in this session");
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = match control.agents(&root).await {
                Ok(entries) if entries.is_empty() => Ok("no subagents in this session".to_owned()),
                Ok(entries) => {
                    let mut lines = vec![format!(
                        "{} subagent(s) · /agent #id opens a transcript · /stop #id stops an active one",
                        entries.len()
                    )];
                    lines.extend(entries.into_iter().map(|entry| {
                        let (status, _) = subagent_status(&entry.status);
                        let mut state = status.label().to_owned();
                        if matches!(entry.status, nanocode::AgentStatus::Interrupted) {
                            state = if entry.can_message {
                                "interrupted".into()
                            } else {
                                "interrupted, not recoverable".into()
                            };
                        }
                        let parent = entry
                            .parent_agent_id
                            .map_or_else(String::new, |parent| format!(" · under #{parent}"));
                        let model = entry
                            .model
                            .as_ref()
                            .map_or_else(String::new, |model| format!(" · {model}"));
                        let cost = entry.usage.cost_usd.map_or_else(String::new, |cost| {
                            let reported = entry.usage.estimated_cost_turns == 0;
                            format!(" · {}", crate::events::dollars(cost.as_f64(), reported))
                        });
                        format!(
                            "#{} {} · {state}{model}{cost}{parent}",
                            entry.agent_id, entry.role
                        )
                    }));
                    Ok(lines.join("\n"))
                }
                Err(error) => Err(error.to_string()),
            };
            drop(tx.send(AppEvent::AgentsListed { root, result }));
        });
    }

    pub(crate) fn refresh_agents(&mut self) {
        let (Some(root), Some(control)) = (
            self.agent
                .as_ref()
                .map(|agent| agent.session_id().to_owned()),
            self.host.runtime.subagents().cloned(),
        ) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = control
                .agents(&root)
                .await
                .map_err(|error| error.to_string());
            drop(tx.send(AppEvent::AgentDirectory { root, result }));
        });
    }

    pub(crate) fn agent_directory(&mut self, result: Result<Vec<AgentDirectoryEntry>, String>) {
        let entries = match result {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(target: "nanocode", %error, "subagent directory unavailable");
                return;
            }
        };
        let focus = self.reload_focus.take();
        for entry in entries {
            let id = entry.agent_id;
            let key = id.to_string();
            let unrecoverable =
                matches!(entry.status, nanocode::AgentStatus::Interrupted) && !entry.can_message;
            let known = self.tree.update(&key, |node| {
                if unrecoverable && node.status == NodeStatus::Idle {
                    node.status = NodeStatus::Unrecoverable;
                }
            });
            if known {
                if focus == Some(id) {
                    self.focus_on(id);
                }
                continue;
            }
            self.mapper.learn_agent(id, entry.role.clone());
            let model = entry.model.as_ref().map(ToString::to_string);
            self.views
                .entry(id)
                .or_insert_with(|| SubagentView::new(entry.role.clone(), model.clone()));
            if focus == Some(id) {
                self.focus_on(id);
            }
            let (status, detail) = subagent_status(&entry.status);
            let label = match &model {
                Some(model) => format!("#{id} {} · {model}", entry.role),
                None => format!("#{id} {}", entry.role),
            };
            self.tree.upsert(TreeNode {
                id: key,
                parent: entry.parent_agent_id.map(|parent| parent.to_string()),
                label,
                detail: if detail.is_empty() {
                    entry.task.lines().next().unwrap_or_default().to_owned()
                } else {
                    detail
                },
                status: if unrecoverable {
                    NodeStatus::Unrecoverable
                } else {
                    status
                },
            });
        }
    }

    pub(crate) fn stop_command(&mut self, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "stop_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let argument = argument.trim().trim_start_matches('#');
        if argument.eq_ignore_ascii_case("main") {
            self.flash("/stop stops subagents; esc esc interrupts the main turn");
            return;
        }
        let id = if argument.is_empty() {
            match self.focus {
                Some(id) => id,
                None => {
                    let listed = self.listed_agents();
                    self.flash(if listed.is_empty() {
                        "no active subagents to stop".to_owned()
                    } else {
                        let names = listed
                            .iter()
                            .map(|(id, role)| format!("#{id} {role}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("name the subagent to stop: /stop #id or role · {names}")
                    });
                    return;
                }
            }
        } else if let Ok(number) = argument.parse::<u64>() {
            AgentId::new(number)
        } else {
            let matches = self
                .listed_agents()
                .into_iter()
                .filter(|(_, role)| role.eq_ignore_ascii_case(argument))
                .map(|(id, _)| id)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [id] => *id,
                [] => {
                    self.flash(format!("no active subagent named {argument}; use its #id"));
                    return;
                }
                several => {
                    let ids = several
                        .iter()
                        .map(|id| format!("#{id}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.flash(format!(
                        "several active subagents are named {argument} ({ids}); use its #id"
                    ));
                    return;
                }
            }
        };
        let key = id.to_string();
        if self
            .tree
            .nodes()
            .iter()
            .any(|node| node.id == key && node.status == NodeStatus::Closed)
        {
            self.flash(format!("{} is already stopped", self.agent_label(id)));
            return;
        }
        self.stop_agent(id);
    }

    fn stop_agent(&mut self, id: AgentId) {
        let (Some(root), Some(control)) = (
            self.agent
                .as_ref()
                .map(|agent| agent.session_id().to_owned()),
            self.host.runtime.subagents().cloned(),
        ) else {
            self.flash("no session with subagents");
            return;
        };
        self.flash(format!("stopping {}…", self.agent_label(id)));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = control
                .close(&root, id)
                .await
                .map(|summaries| {
                    summaries
                        .into_iter()
                        .map(|summary| summary.agent_id)
                        .collect()
                })
                .map_err(|error| error.to_string());
            drop(tx.send(AppEvent::AgentStopped { root, id, result }));
        });
    }

    pub(crate) fn agent_stopped(&mut self, id: AgentId, result: Result<Vec<AgentId>, String>) {
        self.status = None;
        let label = self.agent_label(id);
        match result {
            Ok(stopped) => {
                let descendants = stopped.iter().filter(|stopped| **stopped != id).count();
                self.notice(
                    NoticeLevel::Info,
                    match descendants {
                        0 => format!("stopped {label}"),
                        count => format!("stopped {label} and {count} subagent(s) under it"),
                    },
                );
                if self.focus.is_some_and(|focus| stopped.contains(&focus)) {
                    self.focus = None;
                    self.flash(format!("stopped {label} · back to the main thread"));
                }
            }
            Err(error) => self.notice(NoticeLevel::Error, format!("stop {label} failed: {error}")),
        }
    }

    pub(crate) fn interrupt_focused(&mut self, id: AgentId) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "interrupt_focused", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let (Some(root), Some(control)) = (
            self.agent
                .as_ref()
                .map(|agent| agent.session_id().to_owned()),
            self.host.runtime.subagents().cloned(),
        ) else {
            return;
        };
        self.flash(format!("interrupting #{id}"));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(error) = control.interrupt(&root, id).await {
                drop(tx.send(AppEvent::Notice(
                    NoticeLevel::Error,
                    format!("interrupt #{id} failed: {error}"),
                )));
            }
        });
    }
}

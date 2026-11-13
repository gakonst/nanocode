use std::collections::HashMap;

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
    side::SIDE_AGENT,
};

const AGENT_ARGUMENT_COMMANDS: [(&str, Scope); 6] = [
    ("/stop", Scope::Main),
    ("/stop-agent", Scope::Main),
    ("/agent", Scope::Main),
    ("/focus", Scope::Main),
    ("/btw agent", Scope::Side),
    ("/btw stop", Scope::Side),
];

const BTW_AGENT_COMMANDS: [(&str, &str); 2] = [
    (
        "agent",
        "focus a /btw subagent in the side pane by #id or role; alone returns to the side thread",
    ),
    (
        "stop",
        "close a /btw subagent and its children by #id or role; alone closes the focused one",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Main,
    Side,
}

impl Scope {
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Main => "",
            Self::Side => "btw ",
        }
    }

    pub fn tree_id(self, id: AgentId) -> String {
        match self {
            Self::Main => id.to_string(),
            Self::Side => format!("{SIDE_AGENT}{id}"),
        }
    }

    fn agent_id(self, tree_id: &str) -> Option<AgentId> {
        let id = match self {
            Self::Main => tree_id,
            Self::Side => tree_id.strip_prefix(SIDE_AGENT)?,
        };
        id.parse::<u64>().ok().map(AgentId::new)
    }

    const fn thread(self) -> &'static str {
        match self {
            Self::Main => "main thread",
            Self::Side => "side thread",
        }
    }
}

fn agent_argument(text: &str) -> Option<(Scope, &str, &str)> {
    let (command, partial) = text.rsplit_once(' ')?;
    let (_, scope) = AGENT_ARGUMENT_COMMANDS
        .iter()
        .find(|(name, _)| *name == command)?;
    (!partial.contains(char::is_whitespace)).then_some((*scope, command, partial))
}

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
    pub(crate) fn scope_of(&self, root: &str) -> Option<Scope> {
        if self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.session_id() == root)
        {
            Some(Scope::Main)
        } else if self.panes.session() == Some(root) {
            Some(Scope::Side)
        } else {
            None
        }
    }

    fn scope_root(&self, scope: Scope) -> Option<String> {
        match scope {
            Scope::Main => self.session_id(),
            Scope::Side => self.panes.session().map(str::to_owned),
        }
    }

    fn scope_views(&self, scope: Scope) -> Option<&HashMap<AgentId, SubagentView>> {
        match scope {
            Scope::Main => Some(&self.views),
            Scope::Side => self.panes.side.as_ref().map(|side| &side.views),
        }
    }

    fn scope_views_mut(&mut self, scope: Scope) -> Option<&mut HashMap<AgentId, SubagentView>> {
        match scope {
            Scope::Main => Some(&mut self.views),
            Scope::Side => self.panes.side.as_mut().map(|side| &mut side.views),
        }
    }

    pub(crate) fn focus_of(&self, scope: Scope) -> Option<AgentId> {
        match scope {
            Scope::Main => self.focus,
            Scope::Side => self.panes.side.as_ref().and_then(|side| side.focus),
        }
    }

    fn set_focus(&mut self, scope: Scope, focus: Option<AgentId>) -> Option<AgentId> {
        match scope {
            Scope::Main => std::mem::replace(&mut self.focus, focus),
            Scope::Side => self
                .panes
                .side
                .as_mut()
                .and_then(|side| std::mem::replace(&mut side.focus, focus)),
        }
    }

    fn scope_notice(&mut self, scope: Scope, level: NoticeLevel, text: String) {
        match scope {
            Scope::Main => self.notice(level, text),
            Scope::Side => self.side_notice(level, text),
        }
    }

    pub(crate) fn track_subagent(&mut self, scope: Scope, descriptor: &AgentDescriptor) {
        match scope {
            Scope::Main => self
                .mapper
                .learn_agent(descriptor.id, descriptor.role.clone()),
            Scope::Side => self
                .panes
                .learn_agent(descriptor.id, descriptor.role.clone()),
        }
        if let Some(views) = self.scope_views_mut(scope) {
            views.entry(descriptor.id).or_insert_with(|| {
                SubagentView::new(
                    descriptor.role.clone(),
                    descriptor.model.as_ref().map(ToString::to_string),
                )
            });
        }
    }

    pub(crate) fn subagent_event(
        &mut self,
        scope: Scope,
        id: AgentId,
        event: &AgentEvent,
        payload: Option<&serde_json::Value>,
    ) -> bool {
        let Some(view) = self
            .scope_views_mut(scope)
            .and_then(|views| views.get_mut(&id))
        else {
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

    pub(crate) fn btw_agent_command(&mut self, text: &str) -> bool {
        let mut words = text.split_whitespace();
        let name = words.next().unwrap_or_default();
        let argument = words.next().unwrap_or_default();
        if !matches!(name, "agent" | "stop") || words.next().is_some() {
            return false;
        }
        if self.panes.session().is_none() {
            self.flash(format!("/btw {name} needs a forked /btw side pane"));
        } else if name == "agent" {
            self.focus_command(Scope::Side, argument);
        } else {
            self.stop_command(Scope::Side, argument);
        }
        true
    }

    pub(crate) fn focus_command(&mut self, scope: Scope, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "focus_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let argument = argument.trim().trim_start_matches('#');
        if argument.is_empty() || argument.eq_ignore_ascii_case("main") {
            self.unfocus(scope);
            return;
        }
        let id = argument.parse::<u64>().ok().map(AgentId::new).or_else(|| {
            self.scope_views(scope)?
                .iter()
                .filter(|(_, view)| view.role.eq_ignore_ascii_case(argument))
                .map(|(id, _)| *id)
                .min()
        });
        match id {
            Some(id) => self.focus_on(scope, id),
            None => self.flash(format!(
                "no {}subagent named {argument}; use its #id",
                scope.prefix()
            )),
        }
    }

    pub(crate) fn cycle_focus(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "cycle_focus", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let order = self
            .tree
            .ordered()
            .into_iter()
            .filter_map(|(_, _, node)| Scope::Main.agent_id(&node.id))
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
            Some(id) => self.focus_on(Scope::Main, id),
            None if self.focus.is_some() => self.unfocus(Scope::Main),
            None => self.flash("no subagents to focus"),
        }
    }

    fn unfocus(&mut self, scope: Scope) {
        if self.set_focus(scope, None).is_some() {
            self.flash(format!("back to the {}", scope.thread()));
        }
    }

    fn show_focus(&mut self, scope: Scope, id: AgentId, role: &str) {
        self.set_focus(scope, Some(id));
        if scope == Scope::Side {
            self.panes.active = true;
        }
        let prefix = scope.prefix();
        self.flash(format!(
            "focused {prefix}#{id} {role} · enter messages it · esc esc interrupts · /{prefix}agent returns"
        ));
    }

    fn focus_on(&mut self, scope: Scope, id: AgentId) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "focus_on", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(root) = self.scope_root(scope) else {
            self.flash("no session yet");
            return;
        };
        let Some(control) = self.host.runtime.subagents().cloned() else {
            self.flash("subagents are disabled for this session");
            return;
        };
        let known = self
            .scope_views_mut(scope)
            .and_then(|views| views.get_mut(&id))
            .map(|view| {
                let empty = view.transcript.is_empty() && !view.loading;
                view.loading |= empty;
                (view.role.clone(), empty)
            });
        let backfill = match known {
            Some((role, empty)) => {
                self.show_focus(scope, id, &role);
                empty
            }
            None => true,
        };
        if !backfill {
            return;
        }
        let store = self.host.config.store.clone();
        let tx = self.tx.clone();
        nanocode_platform::spawn_task(async move {
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
        let Some(scope) = self.scope_of(&root) else {
            return;
        };
        let Some(view) = self.scope_views_mut(scope).map(|views| {
            views
                .entry(id)
                .or_insert_with(|| SubagentView::new(role, model))
        }) else {
            return;
        };
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
        let role = view.role.clone();
        if self.focus_of(scope) != Some(id) {
            self.show_focus(scope, id, &role);
        }
    }

    pub(crate) fn message_focused(&mut self, scope: Scope, id: AgentId, text: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "message_focused", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let (Some(root), Some(control)) = (
            self.scope_root(scope),
            self.host.runtime.subagents().cloned(),
        ) else {
            self.flash("no session to message");
            return;
        };
        if let Some(view) = self
            .scope_views_mut(scope)
            .and_then(|views| views.get_mut(&id))
        {
            view.transcript.scroll_to_bottom();
        }
        let tx = self.tx.clone();
        nanocode_platform::spawn_task(async move {
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

    fn agent_label(&self, scope: Scope, id: AgentId) -> String {
        let prefix = scope.prefix();
        match self.scope_views(scope).and_then(|views| views.get(&id)) {
            Some(view) => format!("{prefix}#{id} {}", view.role),
            None => format!("{prefix}#{id}"),
        }
    }

    fn listed_agents(&self, scope: Scope) -> Vec<(AgentId, String)> {
        self.tree
            .ordered()
            .into_iter()
            .filter_map(|(_, _, node)| {
                let id = scope.agent_id(&node.id)?;
                let role = self
                    .scope_views(scope)
                    .and_then(|views| views.get(&id))
                    .map_or_else(String::new, |view| view.role.clone());
                Some((id, role))
            })
            .collect()
    }

    fn btw_subcommands(&self, text: &str) -> Vec<(&'static str, &'static str)> {
        match text.strip_prefix("/btw ") {
            Some(partial)
                if !partial.is_empty()
                    && !partial.contains(char::is_whitespace)
                    && self.panes.session().is_some() =>
            {
                BTW_AGENT_COMMANDS
                    .into_iter()
                    .filter(|(name, _)| name.starts_with(partial))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn agent_argument_candidates(&self, text: &str) -> Option<Vec<(String, String)>> {
        let subcommands = self.btw_subcommands(text);
        if !subcommands.is_empty() {
            return Some(
                subcommands
                    .into_iter()
                    .map(|(name, description)| (format!("/btw {name}"), description.to_owned()))
                    .collect(),
            );
        }
        let (scope, _, partial) = agent_argument(text)?;
        let partial = partial.trim_start_matches('#').to_ascii_lowercase();
        let candidates = self
            .listed_agents(scope)
            .into_iter()
            .filter(|(id, role)| {
                id.to_string().starts_with(&partial)
                    || role.to_ascii_lowercase().starts_with(&partial)
            })
            .map(|(id, role)| {
                let key = scope.tree_id(id);
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
        if let Some((name, _)) = self.btw_subcommands(&text).first() {
            self.composer.set_text(format!("/btw {name} "));
            return true;
        }
        let Some((scope, command, partial)) = agent_argument(&text) else {
            return false;
        };
        let listed = self.listed_agents(scope);
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
        nanocode_platform::spawn_task(async move {
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
        nanocode_platform::spawn_task(async move {
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
                    self.focus_on(Scope::Main, id);
                }
                continue;
            }
            self.mapper.learn_agent(id, entry.role.clone());
            let model = entry.model.as_ref().map(ToString::to_string);
            self.views
                .entry(id)
                .or_insert_with(|| SubagentView::new(entry.role.clone(), model.clone()));
            if focus == Some(id) {
                self.focus_on(Scope::Main, id);
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

    pub(crate) fn stop_command(&mut self, scope: Scope, argument: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "stop_command", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let argument = argument.trim().trim_start_matches('#');
        let prefix = scope.prefix();
        if scope == Scope::Main && argument.eq_ignore_ascii_case("main") {
            self.flash("/stop stops subagents; esc esc interrupts the main turn");
            return;
        }
        let id = if argument.is_empty() {
            match self.focus_of(scope) {
                Some(id) => id,
                None => {
                    let listed = self.listed_agents(scope);
                    self.flash(if listed.is_empty() {
                        format!("no active {prefix}subagents to stop")
                    } else {
                        let names = listed
                            .iter()
                            .map(|(id, role)| format!("#{id} {role}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!(
                            "name the {prefix}subagent to stop: /{prefix}stop #id or role · {names}"
                        )
                    });
                    return;
                }
            }
        } else if let Ok(number) = argument.parse::<u64>() {
            AgentId::new(number)
        } else {
            let matches = self
                .listed_agents(scope)
                .into_iter()
                .filter(|(_, role)| role.eq_ignore_ascii_case(argument))
                .map(|(id, _)| id)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [id] => *id,
                [] => {
                    self.flash(format!(
                        "no active {prefix}subagent named {argument}; use its #id"
                    ));
                    return;
                }
                several => {
                    let ids = several
                        .iter()
                        .map(|id| format!("#{id}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    self.flash(format!(
                        "several active {prefix}subagents are named {argument} ({ids}); use its #id"
                    ));
                    return;
                }
            }
        };
        let key = scope.tree_id(id);
        if self
            .tree
            .nodes()
            .iter()
            .any(|node| node.id == key && node.status == NodeStatus::Closed)
        {
            self.flash(format!(
                "{} is already stopped",
                self.agent_label(scope, id)
            ));
            return;
        }
        self.stop_agent(scope, id);
    }

    fn stop_agent(&mut self, scope: Scope, id: AgentId) {
        let (Some(root), Some(control)) = (
            self.scope_root(scope),
            self.host.runtime.subagents().cloned(),
        ) else {
            self.flash("no session with subagents");
            return;
        };
        self.flash(format!("stopping {}…", self.agent_label(scope, id)));
        let tx = self.tx.clone();
        nanocode_platform::spawn_task(async move {
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

    pub(crate) fn agent_stopped(
        &mut self,
        scope: Scope,
        id: AgentId,
        result: Result<Vec<AgentId>, String>,
    ) {
        self.status = None;
        let label = self.agent_label(scope, id);
        match result {
            Ok(stopped) => {
                let descendants = stopped.iter().filter(|stopped| **stopped != id).count();
                self.scope_notice(
                    scope,
                    NoticeLevel::Info,
                    match descendants {
                        0 => format!("stopped {label}"),
                        count => format!("stopped {label} and {count} subagent(s) under it"),
                    },
                );
                if self
                    .focus_of(scope)
                    .is_some_and(|focus| stopped.contains(&focus))
                {
                    self.set_focus(scope, None);
                    self.flash(format!("stopped {label} · back to the {}", scope.thread()));
                }
            }
            Err(error) => self.scope_notice(
                scope,
                NoticeLevel::Error,
                format!("stop {label} failed: {error}"),
            ),
        }
    }

    pub(crate) fn interrupt_focused(&mut self, scope: Scope, id: AgentId) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "interrupt_focused", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let (Some(root), Some(control)) = (
            self.scope_root(scope),
            self.host.runtime.subagents().cloned(),
        ) else {
            return;
        };
        self.flash(format!("interrupting {}#{id}", scope.prefix()));
        let tx = self.tx.clone();
        nanocode_platform::spawn_task(async move {
            if let Err(error) = control.interrupt(&root, id).await {
                drop(tx.send(AppEvent::Notice(
                    NoticeLevel::Error,
                    format!("interrupt #{id} failed: {error}"),
                )));
            }
        });
    }
}

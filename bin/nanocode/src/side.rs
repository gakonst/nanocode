use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use tokio::task::AbortHandle;

use nanocode::{
    ForkRequest, PromptRequest, TurnControl,
    agent::{
        Agent,
        events::{AgentEvent, AgentEventKind},
    },
    subagents::AgentId,
};
use nanocode_tui::{
    transcript::{CellKind, NoticeLevel, Transcript, TranscriptPolicy},
    ui::layout::{Position, Rect},
};

use crate::{
    app::{
        App, AppEvent, BtwUpdate, DIFF_PREVIEW_LINES, PendingTurn, QUIT_CONFIRMATION, Redraw,
        TOOL_PREVIEW_LINES,
    },
    events::EventMapper,
    focus::{Scope, SubagentView},
};

const BTW_BOUNDARY: &str = "You are answering an ephemeral BTW side question.
Treat inherited conversation history only as reference context. Do not resume or complete an
earlier task. Answer only the question after this boundary. Do not modify the workspace unless
that side question explicitly requests a mutation.

BTW question:
";

const COLLAPSED: &str = "Collapsed /btw";

const COLLAPSE_INSTRUCTION: &str = "The user finished a /btw side conversation forked from this conversation. It ran separately while this thread continued, so its answers may reflect earlier context and any tool activity in it is not shown. Incorporate its relevant findings into the main task; do not repeat work it already settled unless current evidence contradicts it.";

const EARLIER: &str = "Earlier questions and answers in this side conversation, for context:";

pub const SIDE_AGENT: &str = "btw:";

const SIDE_COMMANDS: [&str; 15] = [
    "btw", "collapse", "split", "close", "zoom", "copy", "id", "expand", "mouse", "help", "keys",
    "quit", "exit", "reload", "restart",
];

pub fn prompt_display(text: &str) -> &str {
    if let Some(question) = text.strip_prefix(BTW_BOUNDARY) {
        return question;
    }
    match text.split_once("\n\n") {
        Some((display, rest))
            if display.starts_with(COLLAPSED) && rest.starts_with(COLLAPSE_INSTRUCTION) =>
        {
            display
        }
        _ => text,
    }
}

pub enum SideUpdate {
    Forked(Agent),
    Restored {
        agent: Agent,
        pending: Vec<PendingTurn>,
        history: Vec<nanocode::durability::TranscriptItem>,
    },
    ForkFailed(String),
    Event(AgentEvent),
    Started(TurnControl),
    Finished(Result<(), String>),
    PromptFailed(String),
}

enum Mode {
    Forking,
    Fork(Agent),
    Questions,
}

pub struct SidePane {
    id: u64,
    owner: Option<AgentId>,
    label: String,
    transcript: Transcript,
    mapper: EventMapper,
    mode: Mode,
    control: Option<TurnControl>,
    busy: bool,
    completed: bool,
    first: bool,
    exchanges: Vec<(bool, String)>,
    waiting: Vec<String>,
    asks: HashSet<u64>,
    asking: Option<(u64, AbortHandle)>,
    since: Option<Instant>,
    current: Option<PendingTurn>,
    resumes: Vec<PendingTurn>,
    session_hint: Option<String>,
    pub(crate) focus: Option<AgentId>,
    pub(crate) views: HashMap<AgentId, SubagentView>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedSide {
    session: Option<String>,
    owner: Option<AgentId>,
    exchanges: Vec<(bool, String)>,
    waiting: Vec<String>,
    current: Option<PendingTurn>,
    first: bool,
    completed: bool,
    active: bool,
    zoomed: bool,
}

impl SidePane {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn busy(&self) -> bool {
        self.busy
    }

    pub const fn since(&self) -> Option<Instant> {
        self.since
    }

    pub const fn waiting(&self) -> usize {
        self.waiting.len()
    }

    pub const fn kind(&self) -> &'static str {
        match self.mode {
            Mode::Forking => "forking…",
            Mode::Fork(_) => "forked thread",
            Mode::Questions => "side questions",
        }
    }
}

#[derive(Default)]
pub struct Panes {
    pub side: Option<SidePane>,
    pub active: bool,
    pub zoomed: bool,
    pub main_area: Rect,
    pub side_area: Rect,
    armed: Option<Instant>,
    next: u64,
}

impl Panes {
    pub(crate) fn saved(&self) -> Option<SavedSide> {
        let side = self.side.as_ref()?;
        let mut exchanges = side.exchanges.clone();
        let mut waiting = side.waiting.clone();
        if side.busy
            && matches!(side.mode, Mode::Questions)
            && let Some((true, question)) = exchanges.pop()
        {
            waiting.insert(0, question);
        }
        Some(SavedSide {
            session: self.session().map(str::to_owned),
            owner: side.owner,
            exchanges,
            waiting,
            current: side.current.clone(),
            first: side.first,
            completed: side.completed,
            active: self.active,
            zoomed: self.zoomed,
        })
    }

    pub fn session(&self) -> Option<&str> {
        let side = self.side.as_ref()?;
        match &side.mode {
            Mode::Fork(agent) => Some(agent.session_id()),
            _ => side.session_hint.as_deref(),
        }
    }

    pub(crate) fn learn_agent(&mut self, id: AgentId, role: String) {
        if let Some(side) = self.side.as_mut() {
            side.mapper.learn_agent(id, role);
        }
    }

    pub const fn side_active(&self) -> bool {
        self.active && self.side.is_some()
    }

    pub fn busy(&self) -> bool {
        self.side.as_ref().is_some_and(|side| side.busy)
    }
}

fn conversation(exchanges: &[(bool, String)]) -> String {
    exchanges
        .iter()
        .map(|(user, text)| {
            format!(
                "{}:\n{}\n\n",
                if *user { "User" } else { "Assistant" },
                text.trim()
            )
        })
        .collect()
}

fn collapse_prompt(exchanges: &[(bool, String)]) -> String {
    let question = exchanges
        .iter()
        .find_map(|(user, text)| user.then(|| text.trim().lines().next().unwrap_or_default()))
        .unwrap_or_default();
    format!(
        "{COLLAPSED}: {question}\n\n{COLLAPSE_INSTRUCTION}\n\n<btw_conversation>\n{}</btw_conversation>",
        conversation(exchanges)
    )
}

impl App {
    pub(crate) fn pane_transcript(&mut self, side: bool) -> &mut Transcript {
        match self.panes.side.as_mut().filter(|_| side) {
            Some(pane) => match pane.focus.and_then(|id| pane.views.get_mut(&id)) {
                Some(view) => &mut view.transcript,
                None => &mut pane.transcript,
            },
            None => match self.focus.and_then(|id| self.views.get_mut(&id)) {
                Some(view) => &mut view.transcript,
                None => &mut self.transcript,
            },
        }
    }

    pub(crate) fn active_transcript(&mut self) -> &mut Transcript {
        let side = self.panes.active;
        self.pane_transcript(side)
    }

    pub(crate) fn transcript_at(&mut self, column: u16, row: u16) -> &mut Transcript {
        let position = Position::new(column, row);
        let side = if self.panes.side_area.contains(position) {
            true
        } else if self.panes.main_area.contains(position) {
            false
        } else {
            self.panes.active
        };
        self.pane_transcript(side)
    }

    pub(crate) const fn pane_at(&self, column: u16, row: u16) -> Option<bool> {
        let position = Position::new(column, row);
        if self.panes.side_area.contains(position) {
            Some(true)
        } else if self.panes.main_area.contains(position) {
            Some(false)
        } else {
            None
        }
    }

    pub(crate) const fn click_pane(&mut self, column: u16, row: u16) -> bool {
        let Some(side) = self.pane_at(column, row) else {
            return false;
        };
        let changed = self.panes.active != side;
        self.panes.active = side;
        self.panes.armed = None;
        changed
    }

    pub(crate) const fn toggle_pane(&mut self) {
        if self.panes.side.is_some() {
            self.panes.active = !self.panes.active;
            self.panes.armed = None;
        }
    }

    pub(crate) fn btw(&mut self, question: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "btw", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self.btw_agent_command(&question) {
            return;
        }
        if self.panes.side.is_none() && !self.open_side() {
            return;
        }
        self.panes.active = true;
        if !question.trim().is_empty() {
            self.side_ask(question);
        }
    }

    fn open_side(&mut self) -> bool {
        self.open_side_session(None)
    }

    pub(crate) fn restore_side(&mut self, saved: SavedSide) {
        let focused = self.focus;
        self.focus = saved.owner;
        let restore = saved.session.map(|session| (session, saved.current));
        let opened = self.open_side_session(restore);
        self.focus = focused;
        if !opened {
            return;
        }
        if let Some(side) = self.panes.side.as_mut() {
            side.exchanges = saved.exchanges;
            side.waiting = saved.waiting;
            side.first = saved.first;
            side.completed = saved.completed;
        }
        self.panes.active = saved.active;
        self.panes.zoomed = saved.zoomed;
        self.side_next();
    }

    fn open_side_session(&mut self, restore: Option<(String, Option<PendingTurn>)>) -> bool {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_side", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(agent) = self.agent.clone() else {
            self.flash("the agent is still starting");
            return false;
        };
        self.panes.next += 1;
        let id = self.panes.next;
        let owner = self.focus;
        let label = match owner {
            Some(child) => format!(
                "#{child} {}",
                self.views
                    .get(&child)
                    .map_or("subagent", |view| view.role.as_str())
            ),
            None => "main thread".to_owned(),
        };
        let mut transcript = Transcript::new(TranscriptPolicy {
            collapsed_preview_lines: TOOL_PREVIEW_LINES,
            collapsed_diff_lines: DIFF_PREVIEW_LINES,
        });
        transcript.push(CellKind::Notice(
            NoticeLevel::Info,
            match owner {
                Some(_) => format!(
                    "side questions to {label}, answered from its context without interrupting it · /collapse sends them to it · /close"
                ),
                None => "forking the main thread into a side conversation…".to_owned(),
            },
        ));
        self.panes.side = Some(SidePane {
            id,
            owner,
            label,
            transcript,
            mapper: EventMapper::default(),
            mode: if owner.is_some() && restore.is_none() {
                Mode::Questions
            } else {
                Mode::Forking
            },
            control: None,
            busy: false,
            completed: false,
            first: true,
            exchanges: Vec::new(),
            waiting: Vec::new(),
            asks: HashSet::new(),
            asking: None,
            since: None,
            current: None,
            resumes: Vec::new(),
            session_hint: restore.as_ref().map(|(session, _)| session.clone()),
            focus: None,
            views: HashMap::new(),
        });
        self.panes.zoomed = false;
        if owner.is_none() || restore.is_some() {
            let tx = self.tx.clone();
            let host = self.host.clone();
            nanocode_platform::spawn_task(async move {
                let send = |update| tx.send(AppEvent::Side { pane: id, update }).is_ok();
                if let Some((session, current)) = restore {
                    let restored = async {
                        let ready = crate::app::open_session(&host, &session, None, None).await?;
                        let mut pending = ready.pending;
                        if let Some(current) = current
                            && !pending.iter().any(|turn| turn.id == current.id)
                            && host
                                .config
                                .store
                                .as_ref()
                                .ok_or("side resume requires durability")?
                                .turn(&session, &current.id)
                                .await?
                                .is_none()
                        {
                            pending.push(current);
                        }
                        Ok::<_, crate::host::Error>((
                            ready.agent,
                            ready.events,
                            pending,
                            ready.history.unwrap_or_default(),
                        ))
                    }
                    .await;
                    match restored {
                        Ok((agent, mut events, pending, history)) => {
                            if !send(SideUpdate::Restored {
                                agent: agent.clone(),
                                pending,
                                history,
                            }) {
                                drop(agent.shutdown().await);
                                return;
                            }
                            while let Some(event) = events.recv().await {
                                if !send(SideUpdate::Event(event)) {
                                    break;
                                }
                            }
                        }
                        Err(error) => {
                            send(SideUpdate::ForkFailed(error.to_string()));
                        }
                    }
                    return;
                }
                match agent.fork(ForkRequest::latest().side_conversation()).await {
                    Ok((side, mut events)) => {
                        if !send(SideUpdate::Forked(side.clone())) {
                            drop(side.shutdown().await);
                            return;
                        }
                        while let Some(event) = events.recv().await {
                            if !send(SideUpdate::Event(event)) {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        send(SideUpdate::ForkFailed(error.to_string()));
                    }
                }
            });
        }
        true
    }

    fn side_ask(&mut self, question: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_ask", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        if side.busy || matches!(side.mode, Mode::Forking) {
            side.waiting.push(question);
            self.flash("queued for the side pane");
            return;
        }
        if matches!(side.mode, Mode::Fork(_)) {
            self.side_prompt(question);
        } else {
            self.side_question(question);
        }
    }

    fn side_next(&mut self) {
        if self.quit {
            return;
        }
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        if side.busy || matches!(side.mode, Mode::Forking) {
            return;
        }
        if !side.resumes.is_empty() {
            let next = side.resumes.remove(0);
            self.side_launch(next);
            return;
        }
        if side.waiting.is_empty() {
            return;
        }
        let next = side.waiting.remove(0);
        self.side_ask(next);
    }

    fn side_prompt(&mut self, question: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_prompt", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        if !matches!(side.mode, Mode::Fork(_)) {
            return;
        }
        let sent = if std::mem::take(&mut side.first) {
            format!("{BTW_BOUNDARY}{question}")
        } else {
            question.clone()
        };
        side.exchanges.push((true, question.clone()));
        self.side_launch(PendingTurn {
            id: nanocode::platform::uuid_v7().to_string(),
            text: question,
            prompt: sent.into(),
            shown: false,
        });
    }

    fn side_launch(&mut self, turn: PendingTurn) {
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        let Mode::Fork(agent) = &side.mode else {
            return;
        };
        let agent = agent.clone();
        side.current = Some(turn.clone());
        side.busy = true;
        side.since = Some(Instant::now());
        if turn.shown {
            side.mapper.replayed_prompt(turn.id.clone());
        }
        side.mapper.begin_turn();
        side.mapper.mark_turn_start(side.transcript.len());
        side.transcript.scroll_to_bottom();
        let id = side.id;
        let tx = self.tx.clone();
        nanocode_platform::spawn_joined(&mut self.submissions, async move {
            let send = |update| drop(tx.send(AppEvent::Side { pane: id, update }));
            match agent
                .prompt(PromptRequest::new(turn.prompt).request_id(turn.id))
                .await
            {
                Ok(turn) => {
                    send(SideUpdate::Started(turn.control()));
                    let result = turn
                        .result()
                        .await
                        .map(drop)
                        .map_err(|error| error.to_string());
                    send(SideUpdate::Finished(result));
                }
                Err(error) => send(SideUpdate::PromptFailed(error.to_string())),
            }
        });
    }

    fn side_question(&mut self, question: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_question", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let root = self.agent.clone();
        let control = self.host.runtime.subagents().cloned();
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        let prompt = if side.exchanges.is_empty() {
            question.clone()
        } else {
            format!(
                "{EARLIER}\n<btw_conversation>\n{}</btw_conversation>\n\n{question}",
                conversation(&side.exchanges)
            )
        };
        let cell = side.transcript.push(CellKind::Btw {
            question: question.clone(),
            answer: String::new(),
            done: false,
        });
        side.transcript.scroll_to_bottom();
        side.exchanges.push((true, question));
        side.busy = true;
        side.since = Some(Instant::now());
        self.next_btw += 1;
        let id = self.next_btw;
        side.asks.insert(id);
        let owner = side.owner;
        self.btw.insert(id, (owner, cell));
        let tx = self.tx.clone();
        let task = nanocode_platform::spawn_task(async move {
            let send = |update| drop(tx.send(AppEvent::Btw { id, update }));
            let answer = match (owner, root, control) {
                (None, Some(agent), _) => agent.side_question(prompt).await,
                (Some(child), Some(agent), Some(control)) => {
                    control
                        .side_question(agent.session_id(), child, prompt)
                        .await
                }
                _ => {
                    send(BtwUpdate::Failed("no session to ask".to_owned()));
                    return;
                }
            };
            match answer {
                Ok(answer) => {
                    send(BtwUpdate::Message(answer.into_text()));
                    send(BtwUpdate::Done);
                }
                Err(error) => send(BtwUpdate::Failed(error.to_string())),
            }
        });
        side.asking = Some((id, task.abort_handle()));
    }

    pub(crate) fn side_owns(&self, id: u64, owner: Option<AgentId>) -> bool {
        self.panes
            .side
            .as_ref()
            .is_some_and(|side| side.owner == owner && side.asks.contains(&id))
    }

    pub(crate) fn side_transcript_mut(&mut self) -> Option<&mut Transcript> {
        self.panes.side.as_mut().map(|side| &mut side.transcript)
    }

    pub(crate) fn side_answered(&mut self, id: u64, cell: usize, failed: bool) {
        let Some(side) = self.panes.side.as_mut() else {
            return;
        };
        if !side.asks.remove(&id) {
            return;
        }
        if side
            .asking
            .as_ref()
            .is_some_and(|(asking, _)| *asking == id)
        {
            side.asking = None;
        }
        side.busy = false;
        side.since = None;
        if failed {
            side.exchanges.pop();
        } else if let Some(CellKind::Btw { answer, .. }) = side.transcript.get(cell) {
            side.exchanges.push((false, answer.clone()));
            side.completed = true;
        }
        self.side_next();
    }

    pub(crate) fn side_update(&mut self, pane: u64, update: SideUpdate) -> Redraw {
        let native = self
            .agent
            .as_ref()
            .is_some_and(|agent| agent.capabilities().side_question);
        let shown = self.panes.active || !self.panes.zoomed;
        let Some(side) = self.panes.side.as_mut().filter(|side| side.id == pane) else {
            if let SideUpdate::Forked(agent) = update {
                nanocode_platform::spawn_task(async move { drop(agent.shutdown().await) });
            }
            return Redraw::None;
        };
        match update {
            SideUpdate::Restored {
                agent,
                pending,
                history,
            } => {
                side.transcript.clear();
                crate::sessions::replay(&mut side.transcript, history);
                side.mode = Mode::Fork(agent);
                side.resumes = pending;
                self.side_next();
            }
            SideUpdate::Forked(agent) => {
                side.transcript.push(CellKind::Notice(
                    NoticeLevel::Info,
                    format!(
                        "side conversation {} forked from the main thread; its turns never reach main · /collapse brings it back · /split opens it in a terminal pane · /close",
                        agent.session_id()
                    ),
                ));
                side.mode = Mode::Fork(agent);
                self.side_next();
            }
            SideUpdate::ForkFailed(error) if native => {
                side.transcript.push(CellKind::Notice(
                    NoticeLevel::Warning,
                    format!(
                        "could not fork the main thread ({error}); answering one-off side questions without tools instead · /split needs a fork"
                    ),
                ));
                side.mode = Mode::Questions;
                self.side_next();
            }
            SideUpdate::ForkFailed(error) => {
                side.transcript.push(CellKind::Notice(
                    NoticeLevel::Error,
                    format!("could not fork the main thread: {error}"),
                ));
                side.waiting.clear();
                side.mode = Mode::Questions;
            }
            SideUpdate::Event(event) => {
                if event.kind == AgentEventKind::AssistantMessage
                    && let Ok(payload) =
                        serde_json::from_str::<serde_json::Value>(event.payload.get())
                    && let Some(text) = payload["text"].as_str()
                {
                    side.exchanges.push((false, text.to_owned()));
                }
                if !side.mapper.apply(&mut side.transcript, &event).changed || !shown {
                    return Redraw::None;
                }
            }
            SideUpdate::Started(control) => side.control = Some(control),
            SideUpdate::Finished(result) => {
                side.current = None;
                side.busy = false;
                side.since = None;
                side.control = None;
                match result {
                    Ok(()) => side.completed = true,
                    Err(error) => {
                        side.transcript
                            .finish_running_tools(nanocode_tui::transcript::ToolStatus::Failed);
                        if !side.mapper.reported() {
                            side.transcript
                                .push(CellKind::Notice(NoticeLevel::Error, error));
                        }
                    }
                }
                self.side_next();
            }
            SideUpdate::PromptFailed(error) => {
                side.current = None;
                side.busy = false;
                side.since = None;
                side.exchanges.pop();
                side.transcript.push(CellKind::Notice(
                    NoticeLevel::Error,
                    format!("side prompt failed: {error}"),
                ));
                self.side_next();
            }
        }
        Redraw::Streaming
    }

    pub(crate) fn side_submit(&mut self, text: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_submit", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if let Some(command) = text.strip_prefix('/')
            && !command.starts_with('/')
        {
            let name = command.split_whitespace().next().unwrap_or_default();
            if name == "id"
                && let Some(Mode::Fork(agent)) = self.panes.side.as_ref().map(|side| &side.mode)
            {
                let session = agent.session_id().to_owned();
                self.notice(NoticeLevel::Info, format!("side session {session}"));
                self.pending_copy = Some((session, "copied the side session id to the clipboard"));
            } else if SIDE_COMMANDS.contains(&name) {
                self.command(command.trim());
            } else {
                self.flash(
                    "the side pane takes questions, /btw agent, /btw stop, /collapse, /split, /close, /zoom, /copy and /id · tab returns to the chat",
                );
            }
            return;
        }
        if let Some(id) = self.focus_of(Scope::Side) {
            self.message_focused(Scope::Side, id, text);
            return;
        }
        self.side_ask(text);
    }

    pub(crate) fn side_escape(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_escape", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(side) = self.panes.side.as_ref() else {
            return;
        };
        if !side.busy && side.focus.is_none() {
            return;
        }
        let focus = side.focus;
        let control = side.control.clone();
        let asking = side.asking.as_ref().map(|(id, task)| (*id, task.clone()));
        if self
            .panes
            .armed
            .is_some_and(|armed| armed.elapsed() < QUIT_CONFIRMATION)
        {
            self.panes.armed = None;
            if let Some(id) = focus {
                self.interrupt_focused(Scope::Side, id);
                return;
            }
            self.flash("cancelling the side turn");
            if let Some((id, task)) = asking {
                task.abort();
                self.btw_update(id, BtwUpdate::Failed("cancelled".to_owned()));
                return;
            }
            let Some(control) = control else {
                return;
            };
            let tx = self.tx.clone();
            nanocode_platform::spawn_task(async move {
                if let Err(error) = control.cancel().await {
                    drop(tx.send(AppEvent::Notice(
                        NoticeLevel::Error,
                        format!("side cancel failed: {error}"),
                    )));
                }
            });
        } else {
            self.panes.armed = Some(Instant::now());
            self.flash(if focus.is_some() {
                "press esc again to interrupt"
            } else {
                "press esc again to cancel the side turn"
            });
        }
    }

    pub(crate) fn side_ctrl_c(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "side_ctrl_c", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self
            .panes
            .armed
            .is_some_and(|armed| armed.elapsed() < QUIT_CONFIRMATION)
        {
            self.close_side();
        } else {
            self.panes.armed = Some(Instant::now());
            self.flash("press ctrl+c again to close the side pane");
        }
    }

    fn take_side(&mut self) -> Option<SidePane> {
        let side = self.panes.side.take()?;
        for id in &side.asks {
            self.btw.remove(id);
        }
        if let Some((_, task)) = &side.asking {
            task.abort();
        }
        self.tree.retain(|node| !node.id.starts_with(SIDE_AGENT));
        self.panes.active = false;
        self.panes.zoomed = false;
        self.panes.armed = None;
        self.panes.side_area = Rect::default();
        Some(side)
    }

    pub(crate) fn close_side(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "close_side", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(side) = self.take_side() else {
            self.flash("no /btw side pane is open");
            return;
        };
        if let Mode::Fork(agent) = side.mode {
            let control = side.control;
            nanocode_platform::spawn_task(async move {
                if let Some(control) = control {
                    drop(control.cancel().await);
                }
                drop(agent.shutdown().await);
            });
        }
        self.flash("closed the side pane");
    }

    pub(crate) fn zoom_pane(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "zoom_pane", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if self.panes.side.is_some() {
            self.panes.zoomed = !self.panes.zoomed;
        } else {
            self.flash("/zoom needs an open /btw side pane");
        }
    }

    fn settled_side(&mut self, command: &str) -> bool {
        let problem = match &self.panes.side {
            None => format!("/{command} needs an open /btw side pane"),
            Some(side) if side.busy || !side.waiting.is_empty() => {
                format!("the side pane is still answering; wait for it before /{command}")
            }
            Some(side) if !side.completed => {
                format!("/{command} needs at least one answered side question")
            }
            Some(_) => return true,
        };
        self.flash(problem);
        false
    }

    pub(crate) fn collapse_side(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "collapse_side", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if !self.settled_side("collapse") {
            return;
        }
        let Some(side) = self.take_side() else {
            return;
        };
        let text = collapse_prompt(&side.exchanges);
        if let Mode::Fork(agent) = side.mode {
            nanocode_platform::spawn_task(async move { drop(agent.shutdown().await) });
        }
        match side.owner {
            None => self.deliver_to_main(text),
            Some(child) => self.message_focused(Scope::Main, child, text),
        }
        self.flash(format!("collapsed the side pane into {}", side.label));
    }

    pub(crate) fn side_notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        if let Some(side) = self.panes.side.as_mut() {
            side.transcript.push(CellKind::Notice(level, text.into()));
            side.transcript.scroll_to_bottom();
        }
    }

    pub(crate) fn split_side(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "split_side", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        if !self.settled_side("split") {
            return;
        }
        let Some(side) = self.panes.side.as_ref() else {
            return;
        };
        let agent = match (&side.mode, side.owner) {
            (Mode::Fork(agent), _) => Ok(agent.clone()),
            (_, Some(_)) => Err(format!(
                "/split needs a forked side thread; side questions to {} are answered one-off and aren't saved; use /collapse",
                side.label
            )),
            _ => Err("/split needs a forked side thread, but this pane answers one-off side questions; use /collapse".to_owned()),
        };
        let agent = match agent {
            Ok(agent) => agent,
            Err(reason) => {
                self.side_notice(NoticeLevel::Warning, reason);
                return;
            }
        };
        let session = agent.session_id().to_owned();
        if self.host.config.store.is_none() {
            self.side_notice(
                NoticeLevel::Warning,
                "/split needs a saved session, but this side thread is ephemeral; use /collapse",
            );
            return;
        }
        let split = match crate::split::Split::detect() {
            Ok(split) => split,
            Err(error) => {
                self.side_notice(
                    NoticeLevel::Error,
                    format!("/split: {error}; run `nc resume {session}` in another terminal"),
                );
                return;
            }
        };
        drop(self.take_side());
        let home = self.home.clone();
        let workspace = self.host.config.workspace.clone();
        let tx = self.tx.clone();
        nanocode_platform::spawn_task(async move {
            let notice = |level, text| drop(tx.send(AppEvent::Notice(level, text)));
            if let Err(error) = agent.flush().await {
                drop(agent.shutdown().await);
                notice(
                    NoticeLevel::Error,
                    format!(
                        "/split stopped: could not save side thread {session}: {error}; the side pane was closed and its latest turns may be missing"
                    ),
                );
                return;
            }
            if let Err(error) = agent.shutdown().await {
                notice(
                    NoticeLevel::Error,
                    format!(
                        "could not hand off side thread {session}: {error}; run `nc resume {session}`"
                    ),
                );
                return;
            }
            let launched = {
                let session = session.clone();
                nanocode_platform::spawn_blocking(move || split.launch(&session, &home, &workspace)).await
            };
            match launched {
                Ok(Ok(destination)) => notice(
                    NoticeLevel::Info,
                    format!("side thread {session} opened in {destination}"),
                ),
                Ok(Err(error)) => notice(
                    NoticeLevel::Error,
                    format!("{error}; side thread {session} is saved, run `nc resume {session}`"),
                ),
                Err(error) => notice(NoticeLevel::Error, format!("/split failed: {error}")),
            }
        });
    }
}

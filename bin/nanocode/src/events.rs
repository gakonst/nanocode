use std::{borrow::Cow, collections::HashMap, time::Duration};

use nanocode::agent::events::{AgentEvent, AgentEventKind, TextDeltaRef};
use nanocode_observability::TUI_TARGET;
use nanocode_tui::{
    format::{humanize_tool, shorten_home},
    transcript::{CellKind, NoticeLevel, ToolBody, ToolCell, ToolStatus, Transcript},
};
use serde_json::Value;

const ROUTED_TOOLS: [&str; 6] = [
    "exec_command",
    "write_stdin",
    "Bash",
    "TaskOutput",
    "TaskStop",
    "apply_patch",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerRoute {
    pub name: String,
    pub replace: bool,
}

struct CodeLink {
    cell_id: String,
    latest: String,
}

pub const COMPACTED: &str = "context compacted";

#[derive(Default)]
pub struct EventMapper {
    assistant: HashMap<String, usize>,
    calls: HashMap<String, Option<u64>>,
    restarted: Vec<(Option<u64>, usize)>,
    reasoning: HashMap<u64, usize>,
    tools: HashMap<String, usize>,
    links: HashMap<String, CodeLink>,
    waits: HashMap<String, String>,
    agents: HashMap<String, String>,
    runners: Vec<RunnerRoute>,
    names: HashMap<String, String>,
    usage: Usage,
    last: Usage,
    session: Usage,
    turn_start: usize,
    compaction_start: usize,
    reported: bool,
    subagent: bool,
    replayed: Option<String>,
}

pub fn subagent_prompt_display(text: &str) -> String {
    let Some(rest) = text.strip_prefix("A directed message from ") else {
        return text.to_owned();
    };
    let rest = rest
        .split_once(&format!(
            "\n\n{}",
            nanocode::subagents::COMPLETION_INSTRUCTIONS_LEAD
        ))
        .map_or(rest, |(messages, _)| messages);
    let mut shown = Vec::new();
    for message in rest.split("\n\nA directed message from ") {
        let (Some((sender, _)), Some((_, body))) = (
            message.split_once(" was delivered by the sub-agent runtime."),
            message.split_once("\n\nMessage body:\n"),
        ) else {
            return text.to_owned();
        };
        shown.push(if sender == "the user" {
            body.to_owned()
        } else {
            format!("from {sender}: {body}")
        });
    }
    shown.join("\n\n")
}

#[derive(Clone, Copy, Default)]
pub struct Usage {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub tool_calls: u64,
    pub turns: u64,
}

pub fn retry_notice(payload: &Value) -> String {
    let attempt = payload["attempt"].as_u64().unwrap_or_default();
    let delay = Duration::from_nanos(payload["delay_ns"].as_u64().unwrap_or_default());
    let error = payload["error"].as_str().unwrap_or_default();
    let tries = match payload["max_attempts"].as_u64() {
        Some(max) => format!("attempt {attempt} of {max}"),
        None => format!("attempt {attempt}"),
    };
    format!(
        "model request failed ({tries}), retrying in {:.1}s: {error}",
        delay.as_secs_f64()
    )
    .trim_end_matches([' ', ':'])
    .to_owned()
}

fn completion_agent(text: &str) -> Option<&str> {
    text.rsplit_once("<subagent_completion agent_id=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(agent, _)| agent)
}

pub fn text_payload(event: &AgentEvent) -> Option<TextDeltaRef<'_>> {
    matches!(
        event.kind,
        AgentEventKind::AssistantDelta
            | AgentEventKind::AssistantMessage
            | AgentEventKind::ReasoningSummaryDelta
    )
    .then(|| serde_json::from_str(event.payload.get()).ok())
    .flatten()
}

pub enum Accepted {
    Prompt { request_id: Option<String> },
    Steer { id: Option<String>, text: String },
}

pub struct Mapped {
    pub accepted: Option<Accepted>,
    pub changed: bool,
}

impl EventMapper {
    pub fn subagent() -> Self {
        Self {
            subagent: true,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.assistant.clear();
        self.calls.clear();
        self.restarted.clear();
        self.reasoning.clear();
        self.tools.clear();
        self.names.clear();
    }

    pub fn set_runners(&mut self, runners: Vec<RunnerRoute>) {
        self.runners = runners;
    }

    pub fn learn_agent(&mut self, id: impl ToString, role: impl Into<String>) {
        self.agents.insert(id.to_string(), role.into());
    }

    pub fn replay_tool_call(
        &mut self,
        transcript: &mut Transcript,
        call_id: &str,
        name: &str,
        arguments: &str,
        parent_call_id: Option<&str>,
    ) {
        let arguments = serde_json::from_str::<Value>(arguments)
            .unwrap_or_else(|_| Value::String(arguments.to_owned()));
        self.tool_call(
            transcript,
            &serde_json::json!({
                "call_id": call_id,
                "tool": name,
                "arguments": arguments,
                "parent_call_id": parent_call_id,
            }),
        );
        self.usage.tool_calls -= 1;
    }

    pub fn replay_tool_result(
        &mut self,
        transcript: &mut Transcript,
        call_id: &str,
        output: &str,
        failed: bool,
    ) {
        let Some(name) = self.names.get(call_id).cloned() else {
            return;
        };
        self.tool_result(
            transcript,
            &serde_json::json!({
                "call_id": call_id,
                "tool": name,
                "status": if failed { "failed" } else { "completed" },
                "result": match serde_json::from_str::<Value>(output) {
                    Ok(content @ Value::Array(_)) => content,
                    _ => serde_json::json!({ "text": output }),
                },
            }),
        );
    }

    pub const fn begin_turn(&mut self) {
        self.reported = false;
    }

    pub const fn reported(&self) -> bool {
        self.reported
    }

    pub const fn session_usage(&self) -> Usage {
        self.session
    }

    pub const fn last_usage(&self) -> Usage {
        self.last
    }

    pub const fn turn_start(&self) -> usize {
        self.turn_start
    }

    pub const fn mark_turn_start(&mut self, index: usize) {
        self.turn_start = index;
    }

    pub fn replayed_prompt(&mut self, request_id: String) {
        self.replayed = Some(request_id);
    }

    pub fn apply(&mut self, transcript: &mut Transcript, event: &AgentEvent) -> Mapped {
        let _span = nanocode_trace::info_span!(target: TUI_TARGET, "nc.transcript.apply", event.kind = ?event.kind, event.bytes = event.payload.get().len(), cells = transcript.len()).entered();
        let mut mapped = Mapped {
            accepted: None,
            changed: false,
        };
        if let Some(delta) = text_payload(event) {
            mapped.changed = match event.kind {
                AgentEventKind::ReasoningSummaryDelta => {
                    self.reasoning(transcript, delta.model_call_index.into(), &delta.text)
                }
                kind => {
                    let key = delta
                        .item_id
                        .unwrap_or_else(|| Cow::Owned(format!("call-{}", delta.model_call_index)));
                    self.assistant(
                        transcript,
                        (&key, Some(u64::from(delta.model_call_index))),
                        kind == AgentEventKind::AssistantMessage,
                        &delta.text,
                    )
                }
            };
            return mapped;
        }
        let Ok(payload) = serde_json::from_str::<Value>(event.payload.get()) else {
            transcript.push(CellKind::Notice(
                NoticeLevel::Error,
                format!("undecodable {:?} event", event.kind),
            ));
            mapped.changed = true;
            return mapped;
        };
        mapped.changed = match event.kind {
            AgentEventKind::InputAccepted => {
                let text = prompt_text(&payload["input"]);
                let text = if self.subagent {
                    subagent_prompt_display(&text)
                } else {
                    text
                };
                let shown = text.as_str();
                let replayed = payload["kind"] != "steer"
                    && self
                        .replayed
                        .take_if(|id| payload["request_id"].as_str() == Some(id.as_str()))
                        .is_some();
                if !replayed {
                    match completion_agent(shown) {
                        Some(agent) => transcript.push(CellKind::Notice(
                            NoticeLevel::Info,
                            format!("subagent #{agent} completed · continuing"),
                        )),
                        None => transcript.push(CellKind::User(
                            crate::side::prompt_display(shown).to_owned(),
                        )),
                    };
                }
                self.reasoning.clear();
                mapped.accepted = Some(match payload["kind"].as_str() {
                    Some("steer") => Accepted::Steer {
                        id: payload["item_id"]
                            .as_str()
                            .and_then(|item| item.rsplit_once(":steer:"))
                            .map(|(_, id)| id.to_owned()),
                        text,
                    },
                    _ => Accepted::Prompt {
                        request_id: payload["request_id"].as_str().map(str::to_owned),
                    },
                });
                true
            }
            AgentEventKind::RunStarted => {
                self.reset();
                self.usage = Usage::default();
                self.reported = false;
                self.turn_start = transcript.len();
                false
            }
            AgentEventKind::ModelCallCompleted => {
                let usage = &payload["usage"];
                self.usage.input += usage["input_tokens"].as_u64().unwrap_or_default();
                self.usage.output += usage["output_tokens"].as_u64().unwrap_or_default();
                self.usage.cached += usage["input_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or_default();
                false
            }
            AgentEventKind::AssistantDelta | AgentEventKind::AssistantMessage => {
                let key = payload["item_id"].as_str().map_or_else(
                    || Cow::Owned(format!("call-{}", payload["model_call_index"])),
                    Cow::Borrowed,
                );
                self.assistant(
                    transcript,
                    (&key, payload["model_call_index"].as_u64()),
                    event.kind == AgentEventKind::AssistantMessage,
                    payload["text"].as_str().unwrap_or_default(),
                )
            }
            AgentEventKind::ReasoningSummaryDelta => self.reasoning(
                transcript,
                payload["model_call_index"].as_u64().unwrap_or_default(),
                payload["text"].as_str().unwrap_or_default(),
            ),
            AgentEventKind::ToolCall => {
                self.tool_call(transcript, &payload);
                true
            }
            AgentEventKind::ToolResult => {
                self.tool_result(transcript, &payload);
                true
            }
            AgentEventKind::ToolNotification => self.tool_notification(transcript, &payload),
            AgentEventKind::RunError => {
                let message = payload["message"]
                    .as_str()
                    .unwrap_or("run error")
                    .to_owned();
                let fresh =
                    !transcript.has_notice_since(self.turn_start, NoticeLevel::Error, &message);
                if fresh {
                    transcript.push(CellKind::Notice(NoticeLevel::Error, message));
                }
                fresh
            }
            AgentEventKind::ModelCallFailed => {
                transcript.push(CellKind::Notice(
                    NoticeLevel::Warning,
                    format!(
                        "model call failed: {}",
                        payload["error"].as_str().unwrap_or_default()
                    ),
                ));
                true
            }
            AgentEventKind::ModelAttemptRetrying => {
                self.restart(payload["model_call_index"].as_u64());
                transcript.push(CellKind::Notice(NoticeLevel::Warning, retry_notice(&payload)));
                true
            }
            AgentEventKind::ModelCompactionStarted => {
                self.compaction_start = transcript.push(CellKind::Notice(
                    NoticeLevel::Info,
                    "compacting context…".into(),
                ));
                true
            }
            AgentEventKind::ModelCompactionCompleted => {
                let fresh =
                    !transcript.has_notice_since(self.compaction_start, NoticeLevel::Info, COMPACTED);
                if fresh {
                    transcript.push(CellKind::Notice(NoticeLevel::Info, COMPACTED.into()));
                }
                fresh
            }
            AgentEventKind::ModelCompactionFailed => {
                transcript.push(CellKind::Notice(
                    NoticeLevel::Warning,
                    format!(
                        "compaction failed: {}",
                        payload["error"].as_str().unwrap_or_default()
                    ),
                ));
                true
            }
            AgentEventKind::RunCompleted | AgentEventKind::RunFailed => {
                let status = payload["status"].as_str().unwrap_or("failed");
                let tool_status = match status {
                    "completed" => ToolStatus::Completed,
                    "cancelled" => ToolStatus::Cancelled,
                    _ => ToolStatus::Failed,
                };
                transcript.finish_running_tools(tool_status);
                transcript.push(CellKind::Summary(summary(&payload, status, self.usage)));
                self.last = self.usage;
                self.session.input += self.usage.input;
                self.session.cached += self.usage.cached;
                self.session.output += self.usage.output;
                self.session.tool_calls += self.usage.tool_calls;
                self.session.turns += 1;
                self.reported = true;
                self.reset();
                true
            }
            _ => false,
        };
        mapped
    }

    fn restart(&mut self, call: Option<u64>) {
        let dead: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, owner)| **owner == call)
            .map(|(key, _)| key.clone())
            .collect();
        for key in dead {
            self.calls.remove(&key);
            if let Some(index) = self.assistant.remove(&key) {
                self.restarted.push((call, index));
            }
        }
    }

    fn assistant(
        &mut self,
        transcript: &mut Transcript,
        (key, call): (&str, Option<u64>),
        final_message: bool,
        text: &str,
    ) -> bool {
        if !self.assistant.contains_key(key)
            && !text.is_empty()
            && let Some(position) = self.restarted.iter().position(|(owner, _)| *owner == call)
        {
            let (_, index) = self.restarted.remove(position);
            self.assistant.insert(key.to_owned(), index);
            self.calls.insert(key.to_owned(), call);
            return transcript.update(index, |cell| {
                if let CellKind::Assistant(existing) = cell {
                    text.clone_into(existing);
                }
            });
        }
        self.calls.insert(key.to_owned(), call);
        match self.assistant.get(key).copied() {
            Some(_) if text.is_empty() && !final_message => false,
            Some(index) if !final_message => transcript.append(index, text),
            Some(index) => match transcript.get(index) {
                Some(CellKind::Assistant(existing)) if text.starts_with(existing.as_str()) => {
                    let start = existing.len();
                    start < text.len() && transcript.append(index, &text[start..])
                }
                _ => transcript.update(index, |cell| {
                    if let CellKind::Assistant(existing) = cell {
                        text.clone_into(existing);
                    }
                }),
            },
            None if !text.is_empty() => {
                let index = transcript.push(CellKind::Assistant(text.to_owned()));
                self.assistant.insert(key.to_owned(), index);
                self.reasoning.clear();
                true
            }
            None => false,
        }
    }

    fn reasoning(&mut self, transcript: &mut Transcript, call: u64, text: &str) -> bool {
        match self.reasoning.get(&call).copied() {
            Some(index) if transcript.last().is_some_and(|(last, _)| last == index) => {
                !text.is_empty() && transcript.append(index, text)
            }
            _ if !text.is_empty() => {
                let index = transcript.push(CellKind::Reasoning(text.to_owned()));
                self.reasoning.insert(call, index);
                true
            }
            _ => false,
        }
    }

    fn tool_call(&mut self, transcript: &mut Transcript, payload: &Value) {
        self.usage.tool_calls += 1;
        let call_id = payload["call_id"].as_str().unwrap_or_default().to_owned();
        let name = payload["tool"].as_str().unwrap_or("tool").to_owned();
        let parent = payload["parent_call_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                call_id
                    .rsplit_once('/')
                    .map(|(parent, _)| parent.to_owned())
            });
        let arguments = match &payload["arguments"] {
            Value::String(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
            }
            other => other.clone(),
        };
        let mut cell = ToolCell::new(call_id.clone(), name.clone());
        describe_tool(&mut cell, &name, &arguments);
        self.annotate(&mut cell, &name, &arguments);
        self.reasoning.clear();
        self.names.insert(call_id.clone(), name.clone());
        if name == "wait"
            && let Some(cell_id) = cell_id_argument(&arguments)
            && let Some((origin, link)) = self
                .links
                .iter_mut()
                .find(|(_, link)| link.cell_id == cell_id)
        {
            link.latest.clone_from(&call_id);
            self.waits.insert(call_id.clone(), origin.clone());
        }
        if let Some(parent) = parent
            && let Some(target) = self.target(&parent)
            && let Some(index) = self.index(transcript, &target)
        {
            let pushed = transcript.update(index, |kind| {
                if let CellKind::Tool(tool) = kind
                    && let Some(owner) = tool.find_mut(&target)
                {
                    owner.children.push(cell.clone());
                }
            });
            if pushed {
                self.tools.insert(call_id, index);
                return;
            }
        }
        let index = transcript.push(CellKind::Tool(cell));
        self.tools.insert(call_id, index);
    }

    fn target(&self, parent: &str) -> Option<String> {
        if let Some(link) = self.links.get(parent) {
            return Some(link.latest.clone());
        }
        self.names.contains_key(parent).then(|| parent.to_owned())
    }

    fn index(&self, transcript: &Transcript, call_id: &str) -> Option<usize> {
        self.tools
            .get(call_id)
            .copied()
            .or_else(|| transcript.find_tool(call_id))
    }

    fn annotate(&self, cell: &mut ToolCell, name: &str, arguments: &Value) {
        let agent = |id: &Value| {
            let id = match id {
                Value::String(id) => id.clone(),
                Value::Null => return None,
                other => other.to_string(),
            };
            Some(match self.agents.get(&id) {
                Some(role) => format!("#{id} {role}"),
                None => format!("#{id}"),
            })
        };
        match name {
            "send_agent_message" | "close_agent" | "interrupt_agent" => {
                if let Some(label) = agent(&arguments["agent_id"]) {
                    cell.title = label;
                }
            }
            "wait_agent" => {
                if let Some(ids) = arguments["agent_ids"].as_array() {
                    cell.title = ids.iter().filter_map(agent).collect::<Vec<_>>().join(", ");
                }
            }
            _ => {}
        }
        if let Some(runner) = self.runner(name, arguments) {
            cell.details.push(format!("on {runner}"));
        }
    }

    fn runner(&self, name: &str, arguments: &Value) -> Option<String> {
        let mut paths = ["workdir", "cwd", "path", "file_path", "notebook_path"]
            .into_iter()
            .filter_map(|key| arguments.get(key).and_then(Value::as_str))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if name == "apply_patch" {
            paths.extend(patch_files(&patch_text(arguments)));
        }
        self.runners
            .iter()
            .find(|runner| {
                let mount = format!("/{}", runner.name);
                paths.iter().any(|path| {
                    path == &mount
                        || path
                            .strip_prefix(&mount)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
            .or_else(|| {
                (!paths.is_empty() || ROUTED_TOOLS.contains(&name))
                    .then(|| self.runners.iter().find(|runner| runner.replace))
                    .flatten()
            })
            .map(|runner| runner.name.clone())
    }

    fn tool_result(&mut self, transcript: &mut Transcript, payload: &Value) {
        let call_id = payload["call_id"].as_str().unwrap_or_default();
        let name = payload["tool"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.names.get(call_id).cloned())
            .unwrap_or_else(|| "tool".to_owned());
        let Some(index) = self.index(transcript, call_id) else {
            self.tool_call(
                transcript,
                &serde_json::json!({
                    "call_id": call_id,
                    "tool": name,
                    "parent_call_id": payload["parent_call_id"],
                }),
            );
            self.usage.tool_calls -= 1;
            if self.index(transcript, call_id).is_some() {
                self.tool_result(transcript, payload);
            }
            return;
        };
        let mut outcome = outcome(&name, payload);
        self.learn_agents(&name, payload);
        let duration = payload["duration_ns"].as_u64().map(Duration::from_nanos);
        let yielded = matches!(name.as_str(), "exec" | "code_mode" | "js")
            .then(|| yielded_cell(&outcome.raw))
            .flatten();
        if let Some(cell_id) = &yielded {
            self.links.insert(
                call_id.to_owned(),
                CodeLink {
                    cell_id: cell_id.clone(),
                    latest: call_id.to_owned(),
                },
            );
            outcome.status = Some(ToolStatus::Yielded);
        }
        let status = outcome.status.unwrap_or(match payload["status"].as_str() {
            Some("completed") => ToolStatus::Completed,
            Some("cancelled") => ToolStatus::Cancelled,
            _ => ToolStatus::Failed,
        });
        transcript.update(index, |cell| {
            if let CellKind::Tool(top) = cell
                && let Some(tool) = top.find_mut(call_id)
            {
                tool.status = status;
                tool.duration = duration;
                if tool.output_body == ToolBody::Text && looks_like_diff(&outcome.text) {
                    tool.output_body = ToolBody::Diff;
                }
                if let Some(cell_id) = &yielded {
                    tool.title = format!("cell {cell_id}");
                }
                tool.exit_code = outcome.exit_code;
                tool.details.extend(outcome.details);
                tool.output = outcome.text;
                tool.raw_output = outcome.raw;
            }
        });
        if status != ToolStatus::Yielded
            && let Some(origin) = self.waits.get(call_id).cloned()
        {
            self.links.remove(&origin);
            if let Some(index) = self.index(transcript, &origin) {
                transcript.update(index, |cell| {
                    if let CellKind::Tool(top) = cell
                        && let Some(tool) = top.find_mut(&origin)
                        && tool.status == ToolStatus::Yielded
                    {
                        tool.status = status;
                        tool.details.push("finished below".to_owned());
                    }
                });
            }
        }
    }

    fn tool_notification(&self, transcript: &mut Transcript, payload: &Value) -> bool {
        let call_id = payload["call_id"].as_str().unwrap_or_default();
        let text = payload["text"].as_str().unwrap_or_default();
        let Some(target) = self.target(call_id) else {
            return false;
        };
        let Some(index) = self.index(transcript, &target) else {
            return false;
        };
        transcript.update(index, |cell| {
            if let CellKind::Tool(top) = cell
                && let Some(tool) = top.find_mut(&target)
                && tool.status == ToolStatus::Running
            {
                text.clone_into(&mut tool.output);
            }
        })
    }

    fn learn_agents(&mut self, name: &str, payload: &Value) {
        if !matches!(
            name,
            "spawn_agent" | "wait_agent" | "list_agents" | "close_agent" | "interrupt_agent"
        ) {
            return;
        }
        let mut texts = Vec::new();
        collect_text(&payload["result"], &mut texts);
        let parsed = serde_json::from_str::<Value>(texts.concat().trim()).ok();
        for value in [parsed.as_ref(), Some(&payload["structured_result"])]
            .into_iter()
            .flatten()
        {
            self.learn_agent_values(value);
        }
    }

    fn learn_agent_values(&mut self, value: &Value) {
        match value {
            Value::Object(map) => {
                if let (Some(id), Some(role)) =
                    (map.get("agent_id"), map.get("role").and_then(Value::as_str))
                {
                    let id = id.as_str().map_or_else(|| id.to_string(), str::to_owned);
                    self.agents.insert(id, role.to_owned());
                }
                map.values()
                    .for_each(|value| self.learn_agent_values(value));
            }
            Value::Array(items) => items.iter().for_each(|item| self.learn_agent_values(item)),
            _ => {}
        }
    }
}

struct Outcome {
    text: String,
    raw: String,
    exit_code: Option<i64>,
    details: Vec<String>,
    status: Option<ToolStatus>,
}

fn outcome(name: &str, payload: &Value) -> Outcome {
    let mut texts = Vec::new();
    collect_text(&payload["result"], &mut texts);
    let raw = texts.concat();
    let structured = &payload["structured_result"];
    let mut outcome = Outcome {
        text: String::new(),
        raw: raw.clone(),
        exit_code: None,
        details: Vec::new(),
        status: None,
    };
    let body = raw.as_str();
    match name {
        "exec_command" | "write_stdin" => {
            let shell = if structured.get("output").is_some() {
                structured.clone()
            } else {
                shell_text(body)
            };
            outcome.text = shell["output"].as_str().unwrap_or(body).to_owned();
            outcome.exit_code = shell["exit_code"].as_i64();
            if let Some(session) = shell["session_id"].as_i64() {
                outcome.details.push(format!("session {session} running"));
            }
            return outcome;
        }
        "exec" | "code_mode" | "js" | "wait" => {
            outcome.status = code_status(body);
            outcome.text = humanize(strip_code_header(body));
            return outcome;
        }
        _ => {}
    }
    if let Some(output) = structured.get("output").and_then(Value::as_str) {
        outcome.text = output.to_owned();
        return outcome;
    }
    let parsed = serde_json::from_str::<Value>(body.trim()).ok();
    if let Some(Value::Object(map)) = &parsed
        && (map.contains_key("stdout")
            || map.contains_key("stderr")
            || map.contains_key("background"))
    {
        let parts = ["stdout", "stderr", "output"]
            .into_iter()
            .filter_map(|key| map.get(key).and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .map(|text| text.trim_end().to_owned())
            .collect::<Vec<_>>();
        outcome.text = parts.join("\n");
        outcome.exit_code = map.get("exit_code").and_then(Value::as_i64);
        if map.get("background").and_then(Value::as_bool) == Some(true) {
            let task = map
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            outcome.details.push(format!("background task {task}"));
        }
        if map.get("interrupted").and_then(Value::as_bool) == Some(true) {
            outcome.details.push("interrupted".to_owned());
        }
        if map.get("truncated").and_then(Value::as_bool) == Some(true) {
            outcome.details.push("output truncated".to_owned());
        }
        return outcome;
    }
    outcome.text = humanize(strip_code_header(body));
    outcome
}

fn shell_text(text: &str) -> Value {
    let (header, output) = text.split_once("Output:\n").unwrap_or(("", text));
    let mut shell = serde_json::json!({ "output": output });
    for line in header.lines() {
        if let Some(code) = line
            .strip_prefix("Process exited with code ")
            .and_then(|code| code.trim().parse::<i64>().ok())
        {
            shell["exit_code"] = code.into();
        }
        if let Some(session) = line
            .strip_prefix("Process running with session ID ")
            .and_then(|session| session.trim().parse::<i64>().ok())
        {
            shell["session_id"] = session.into();
        }
    }
    shell
}

fn code_status(text: &str) -> Option<ToolStatus> {
    let header = text.lines().next()?;
    match header {
        "Script completed" => Some(ToolStatus::Completed),
        "Script failed" => Some(ToolStatus::Failed),
        "Script terminated" => Some(ToolStatus::Cancelled),
        header if header.starts_with("Script running with cell ID ") => Some(ToolStatus::Yielded),
        _ => None,
    }
}

fn yielded_cell(text: &str) -> Option<String> {
    text.lines()
        .next()?
        .strip_prefix("Script running with cell ID ")
        .map(|id| id.trim().to_owned())
}

fn cell_id_argument(arguments: &Value) -> Option<String> {
    match &arguments["cell_id"] {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

fn patch_text(arguments: &Value) -> String {
    match arguments {
        Value::String(patch) => patch.clone(),
        other => other
            .get("input")
            .or_else(|| other.get("patch"))
            .and_then(Value::as_str)
            .map_or_else(|| other.to_string(), str::to_owned),
    }
}

fn patch_files(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            [
                "*** Add File: ",
                "*** Update File: ",
                "*** Delete File: ",
                "*** Move to: ",
            ]
            .into_iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|path| path.trim().to_owned())
        .collect()
}

fn first_line(text: &str) -> (String, bool) {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or_default().to_owned();
    (first, lines.next().is_some())
}

fn visible_input(chars: &str) -> String {
    chars
        .chars()
        .map(|character| match character {
            '\n' => "⏎\n".to_owned(),
            '\u{1b}' => "⎋".to_owned(),
            character if character.is_control() && (character as u32) < 0x20 => {
                format!("^{}", char::from(b'@' + character as u8))
            }
            character => character.to_string(),
        })
        .collect()
}

fn lines_patch(header: &str, path: &str, removed: &str, added: &str) -> String {
    let mut patch = format!("*** {header}: {path}\n");
    if header == "Update File" {
        patch.push_str("@@\n");
    }
    for line in removed.lines() {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in added.lines() {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

pub fn tool_label(name: &str, arguments: &Value) -> String {
    let mut cell = ToolCell::new(String::new(), name);
    describe_tool(&mut cell, name, arguments);
    if cell.title.is_empty() {
        cell.name
    } else {
        format!("{} {}", cell.name, first_line(&cell.title).0)
    }
}

pub fn describe_tool(cell: &mut ToolCell, name: &str, arguments: &Value) {
    let field = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let command = |cell: &mut ToolCell, text: String| {
        let (first, more) = first_line(&text);
        cell.name = "run".into();
        cell.title = first;
        if more {
            cell.input = text;
            cell.input_body = ToolBody::Code;
            cell.language = "bash".into();
        }
    };
    let workdir = field("workdir");
    match name {
        "exec" | "code_mode" | "js" => {
            cell.name = "code".into();
            cell.input = match arguments {
                Value::String(code) => code.clone(),
                Value::Null => String::new(),
                other => field("code").unwrap_or_else(|| other.to_string()),
            };
            cell.input_body = ToolBody::Code;
            cell.language = "js".into();
        }
        "wait" => {
            cell.name = "code".into();
            let id = cell_id_argument(arguments).unwrap_or_default();
            cell.title = if arguments["terminate"].as_bool() == Some(true) {
                format!("cell {id} · terminate")
            } else {
                format!("cell {id} · wait")
            };
        }
        "exec_command" => {
            command(cell, field("cmd").unwrap_or_default());
            if arguments["tty"].as_bool() == Some(true) {
                cell.details.push("tty".into());
            }
        }
        "Bash" | "shell" | "run_command" => {
            command(
                cell,
                field("command")
                    .or_else(|| field("cmd"))
                    .unwrap_or_default(),
            );
            if arguments["run_in_background"].as_bool() == Some(true) {
                cell.details.push("background".into());
            }
        }
        "write_stdin" => {
            let session = arguments
                .get("session_id")
                .map(Value::to_string)
                .unwrap_or_default();
            let chars = field("chars").unwrap_or_default();
            if chars.is_empty() {
                cell.name = "poll".into();
                cell.title = format!("session {session}");
            } else {
                cell.name = "stdin".into();
                cell.title = format!("session {session}");
                cell.input = visible_input(&chars);
            }
        }
        "TaskOutput" => {
            cell.name = "task output".into();
            cell.title = field("task_id").unwrap_or_default();
        }
        "TaskStop" => {
            cell.name = "stop task".into();
            cell.title = field("task_id").unwrap_or_default();
        }
        "apply_patch" => {
            cell.name = "patch".into();
            cell.input = patch_text(arguments);
            cell.title = patch_files(&cell.input).join(", ");
            cell.input_body = ToolBody::Diff;
        }
        "Edit" => {
            let path = field("file_path").unwrap_or_default();
            cell.name = "edit".into();
            cell.input = lines_patch(
                "Update File",
                &path,
                &field("old_string").unwrap_or_default(),
                &field("new_string").unwrap_or_default(),
            );
            cell.input_body = ToolBody::Diff;
            if arguments["replace_all"].as_bool() == Some(true) {
                cell.details.push("all occurrences".into());
            }
            cell.title = path;
        }
        "MultiEdit" => {
            let path = field("file_path").unwrap_or_default();
            cell.name = "edit".into();
            cell.input = arguments["edits"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|edit| {
                    lines_patch(
                        "Update File",
                        &path,
                        edit["old_string"].as_str().unwrap_or_default(),
                        edit["new_string"].as_str().unwrap_or_default(),
                    )
                })
                .collect();
            cell.input_body = ToolBody::Diff;
            cell.title = path;
        }
        "Write" => {
            let path = field("file_path").unwrap_or_default();
            let content = field("content").unwrap_or_default();
            cell.name = "write".into();
            cell.input = lines_patch("Add File", &path, "", &content);
            cell.input_body = ToolBody::Diff;
            cell.title = path;
        }
        "Read" => {
            cell.name = "read".into();
            cell.title = field("file_path").unwrap_or_default();
            let offset = arguments["offset"].as_u64();
            let limit = arguments["limit"].as_u64();
            match (offset, limit) {
                (Some(offset), Some(limit)) => cell.details.push(format!(
                    "lines {offset}–{}",
                    offset + limit.saturating_sub(1)
                )),
                (Some(offset), None) => cell.details.push(format!("from line {offset}")),
                (None, Some(limit)) => cell.details.push(format!("first {limit} lines")),
                (None, None) => {}
            }
            if let Some(pages) = field("pages") {
                cell.details.push(format!("pages {pages}"));
            }
        }
        "view_image" => {
            cell.name = "view image".into();
            cell.title = field("path").unwrap_or_default();
        }
        "Glob" | "Grep" => {
            cell.name = name.to_lowercase();
            cell.title = field("pattern").unwrap_or_default();
            if let Some(path) = field("path") {
                cell.title = format!("{} in {path}", cell.title);
            }
            for key in ["glob", "type", "output_mode"] {
                if let Some(value) = field(key) {
                    cell.details.push(format!("{key} {value}"));
                }
            }
        }
        "WebSearch" | "web_search" => {
            cell.name = "search".into();
            cell.title = field("query").unwrap_or_default();
        }
        "WebFetch" => {
            cell.name = "fetch".into();
            cell.title = field("url").unwrap_or_default();
        }
        "spawn_agent" => {
            cell.name = "spawn agent".into();
            cell.title = field("role").unwrap_or_default();
            cell.input = field("task").unwrap_or_default();
            for key in ["model", "thinking", "harness"] {
                if let Some(value) = field(key) {
                    cell.details.push(value);
                }
            }
        }
        "send_agent_message" => {
            cell.name = "message agent".into();
            cell.title = arguments
                .get("agent_id")
                .map(|id| {
                    format!(
                        "#{}",
                        id.as_str().map_or_else(|| id.to_string(), str::to_owned)
                    )
                })
                .unwrap_or_default();
            cell.input = field("message").unwrap_or_default();
            for key in ["purpose", "priority"] {
                if let Some(value) = field(key) {
                    cell.details.push(value);
                }
            }
        }
        "wait_agent" => cell.name = "wait for agents".into(),
        "list_agents" => cell.name = "list agents".into(),
        "close_agent" => cell.name = "close agent".into(),
        "interrupt_agent" => cell.name = "interrupt agent".into(),
        "submit_result" => {
            cell.name = "submit result".into();
            cell.input = match &arguments["output"] {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
            };
        }
        "runners" => {
            cell.name = "runner".into();
            cell.title = ["action", "name", "url", "address"]
                .into_iter()
                .filter_map(field)
                .collect::<Vec<_>>()
                .join(" ");
        }
        "update_plan" => {
            cell.name = "plan".into();
            cell.title = field("explanation").unwrap_or_default();
            cell.input = steps(&arguments["plan"], "step");
        }
        "TodoWrite" => {
            cell.name = "todos".into();
            cell.input = steps(&arguments["todos"], "content");
        }
        name if name.starts_with("mcp__") => {
            cell.name = humanize_tool(name);
            if let Some(code) = field("code") {
                cell.title = field("title").unwrap_or_default();
                cell.input = code;
                cell.input_body = ToolBody::Code;
                cell.language = "js".into();
            } else {
                cell.title = compact(arguments);
            }
        }
        _ => cell.title = compact(arguments),
    }
    if let Some(workdir) = workdir {
        cell.details.push(format!(
            "in {}",
            shorten_home(std::path::Path::new(&workdir))
        ));
    }
}

fn compact(arguments: &Value) -> String {
    match arguments {
        Value::Object(map) if map.is_empty() => String::new(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn steps(items: &Value, key: &str) -> String {
    items
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let marker = match item["status"].as_str() {
                Some("completed") => "✓",
                Some("in_progress") => "▸",
                _ => "○",
            };
            format!("{marker} {}", item[key].as_str().unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn runner_routes(runtime: &nanocode::Nanocode) -> Vec<RunnerRoute> {
    runtime
        .list_runners()
        .into_iter()
        .filter(|runner| runner.protocol == nanocode::RunnerProtocol::Tools)
        .map(|runner| RunnerRoute {
            replace: runner.mode == nanocode::code_mode::attachment::RemoteMode::Replace,
            name: runner.name,
        })
        .collect()
}

fn prompt_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        other => {
            let mut texts = Vec::new();
            collect_text(other, &mut texts);
            if texts.is_empty() {
                other.to_string()
            } else {
                texts.join("\n")
            }
        }
    }
}

fn collect_text(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) => texts.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_text(item, texts)),
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("text") {
                texts.push(text.clone());
            } else if map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.contains("image"))
            {
                let source = ["path", "image_url", "file_id"]
                    .into_iter()
                    .find_map(|key| map.get(key).and_then(Value::as_str))
                    .unwrap_or("attached");
                let name = source.rsplit('/').next().unwrap_or(source);
                texts.push(format!("[image: {name}]"));
            } else {
                map.values().for_each(|item| collect_text(item, texts));
            }
        }
        _ => {}
    }
}

pub fn humanize(text: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return text.to_owned();
    };
    match &value {
        Value::Object(map) if map.contains_key("stdout") || map.contains_key("stderr") => {
            let mut parts = Vec::new();
            for key in ["stdout", "stderr", "output"] {
                if let Some(text) = map
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    parts.push(text.trim_end().to_owned());
                }
            }
            if let Some(code) = map
                .get("exit_code")
                .and_then(Value::as_i64)
                .filter(|code| *code != 0)
            {
                parts.push(format!("exit code {code}"));
            }
            parts.join("\n")
        }
        Value::Object(map)
            if map.contains_key("agents")
                || map.contains_key("agent_id")
                || map.contains_key("spawned") =>
        {
            let mut lines = Vec::new();
            agent_lines(&value, &mut lines);
            if lines.is_empty() {
                text.to_owned()
            } else {
                lines.join("\n")
            }
        }
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => text.to_owned(),
    }
}

fn agent_lines(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Object(map) if map.contains_key("agent_id") => {
            let id = map
                .get("agent_id")
                .map(Value::to_string)
                .unwrap_or_default();
            let role = map.get("role").and_then(Value::as_str).unwrap_or("agent");
            let status = &value["status"];
            let state = status["state"].as_str().unwrap_or("unknown");
            let mut line = format!("#{id} {role} · {state}");
            match &status["output"] {
                Value::Null => {}
                Value::String(output) => {
                    line.push_str(&format!(" · {}", output.lines().next().unwrap_or_default()));
                }
                other => line.push_str(&format!(" · {other}")),
            }
            if let Some(error) = status["error"].as_str() {
                line.push_str(&format!(" · {error}"));
            }
            let usage = &value["usage"];
            if let Some(cost) = usage["cost_usd"]
                .as_str()
                .and_then(|cost| cost.parse::<f64>().ok())
            {
                let reported = usage["estimated_cost_turns"].as_u64().is_none();
                line.push_str(&format!(" · {}", dollars(cost, reported)));
            }
            lines.push(line);
        }
        Value::Object(map) => map.values().for_each(|value| agent_lines(value, lines)),
        Value::Array(items) => items.iter().for_each(|item| agent_lines(item, lines)),
        _ => {}
    }
}

fn strip_code_header(text: &str) -> &str {
    let Some(rest) = text
        .split_once('\n')
        .filter(|(status, rest)| status.starts_with("Script ") && rest.starts_with("Wall time"))
        .map(|(_, rest)| rest)
    else {
        return text;
    };
    let rest = rest
        .strip_prefix("Wall time")
        .and_then(|rest| rest.split_once('\n'))
        .map_or(rest, |(_, rest)| rest);
    rest.strip_prefix("Output:\n").unwrap_or(rest)
}

pub fn looks_like_diff(text: &str) -> bool {
    text.starts_with("diff --git") || text.starts_with("--- ") && text.contains("\n+++ ")
}

fn summary(payload: &Value, status: &str, observed: Usage) -> String {
    let mut parts = Vec::new();
    if status != "completed" {
        parts.push(status.to_owned());
    }
    if let Some(duration) = payload["duration_ns"].as_u64() {
        parts.push(nanocode_tui::format::format_duration(duration));
    }
    let tools = payload["tool_calls"]
        .as_u64()
        .unwrap_or(observed.tool_calls);
    if tools > 0 {
        parts.push(format!(
            "{tools} tool call{}",
            if tools == 1 { "" } else { "s" }
        ));
    }
    let usage = &payload["usage"];
    let (input, output, cached) = match (
        usage["input_tokens"].as_u64(),
        usage["output_tokens"].as_u64(),
    ) {
        (Some(input), Some(output)) => (
            input,
            output,
            usage["cached_input_tokens"].as_u64().unwrap_or_default(),
        ),
        _ => (observed.input, observed.output, observed.cached),
    };
    if input > 0 || output > 0 {
        let mut usage_text = format!("{} in · {} out", tokens(input), tokens(output));
        if cached > 0 {
            usage_text.push_str(&format!(" · {} cached", tokens(cached)));
        }
        parts.push(usage_text);
    }
    if let Some(cost) = payload["cost_usd"].as_f64() {
        parts.push(dollars(
            cost,
            payload["cost_status"] == "reported_by_provider",
        ));
    }
    parts.join(" · ")
}

pub(crate) fn dollars(cost: f64, reported: bool) -> String {
    match reported {
        true => format!("{}{cost:.6}", '$'),
        false => format!("~{}{cost:.4}", '$'),
    }
}

fn tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

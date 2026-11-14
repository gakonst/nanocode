use std::{collections::BTreeSet, sync::Arc};

use serde_json::{Value, json};

use crate::{
    httpd::WireMutation,
    rng::{FNV_OFFSET, Rng, fnv},
    world::World,
};

pub const ROOT_MARK: &str = "SIMROOT";
pub const CHILD_MARK: &str = "SIMCHILD";
pub const EFFECT_TOOL: &str = "sim_effect";
const RUNNING_MARK: &str = "Script running with cell ID ";
pub const EFFECTS_MAX: u64 = 2;
pub const CHILDREN_MAX: u64 = 3;
pub const WAIT_YIELD_MS: u64 = 30_000;
pub const WAIT_AGENT_TIMEOUT_MS: u64 = 300_000;
const SPAWN_PPM: u64 = 600_000;
const EFFECT_PPM: u64 = 700_000;
const MESSAGE_PPM: u64 = 400_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Action {
    Effects(Vec<String>),
    Spawn(Vec<String>),
    Message,
    Submit,
}

#[derive(Clone, Debug)]
enum Agent {
    Root { label: String, turn: u64 },
    Child { label: String, depth: u64 },
}

impl Agent {
    fn label(&self) -> &str {
        match self {
            Self::Root { label, .. } | Self::Child { label, .. } => label,
        }
    }
}

pub fn root_prompt(label: &str, turn: u64) -> String {
    format!("{ROOT_MARK} {label} turn {turn}")
}

pub fn final_text(label: &str, turn: u64) -> String {
    format!("done {label} turn {turn}")
}

fn child_task(label: &str, depth: u64) -> String {
    format!("{CHILD_MARK} {label} depth {depth}")
}

fn word_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let at = text.find(marker)? + marker.len();
    text[at..].split_whitespace().next()
}

fn number_after(text: &str, marker: &str) -> Option<u64> {
    word_after(text, marker)?
        .trim_end_matches(|character: char| !character.is_ascii_digit())
        .parse()
        .ok()
}

pub fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                Some("tool_result") => text_of(block.get("content").unwrap_or(&Value::Null)),
                _ => block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn canonical(message: &Value) -> Value {
    let blocks = match message.get("content") {
        Some(Value::Array(blocks)) => blocks.clone(),
        Some(Value::String(text)) => vec![json!({"type": "text", "text": text})],
        _ => Vec::new(),
    };
    Value::Array(
        blocks
            .into_iter()
            .filter_map(|block| match block.get("type").and_then(Value::as_str) {
                Some("text") => Some(json!({"text": block.get("text")})),
                Some("tool_use") => Some(json!({
                    "id": block.get("id"),
                    "name": block.get("name"),
                    "input": block.get("input"),
                })),
                _ => None,
            })
            .collect(),
    )
}

pub fn message_digest(label: &str, message: &Value) -> u64 {
    fnv(
        fnv(FNV_OFFSET, label.as_bytes()),
        canonical(message).to_string().as_bytes(),
    )
}

struct Conversation {
    agent: Agent,
    steps: u64,
    running_cell: Option<String>,
    prefix: u64,
}

fn read(messages: &[Value]) -> Option<Conversation> {
    let first = text_of(messages.first()?.get("content")?);
    let agent = if let Some(label) = word_after(&first, CHILD_MARK) {
        Agent::Child {
            label: label.to_owned(),
            depth: number_after(&first, " depth ").unwrap_or(0),
        }
    } else {
        let label = word_after(&first, ROOT_MARK)?.to_owned();
        let turn = messages
            .iter()
            .rev()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .map(|message| text_of(message.get("content").unwrap_or(&Value::Null)))
            .find_map(|text| {
                let at = text.rfind(ROOT_MARK)?;
                number_after(&text[at..], " turn ")
            })?;
        Agent::Root { label, turn }
    };
    let opened = match &agent {
        Agent::Root { turn, label } => {
            let prompt = root_prompt(label, *turn);
            messages
                .iter()
                .rposition(|message| {
                    message.get("role").and_then(Value::as_str) == Some("user")
                        && text_of(message.get("content").unwrap_or(&Value::Null)).contains(&prompt)
                })
                .unwrap_or(0)
        }
        Agent::Child { .. } => 0,
    };
    let steps = messages[opened..]
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter(|message| {
            message
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("tool_use")
                            && block.get("name").and_then(Value::as_str) == Some("exec")
                    })
                })
        })
        .count();
    let last = messages.last()?;
    let running_cell = (last.get("role").and_then(Value::as_str) == Some("user"))
        .then(|| text_of(last.get("content").unwrap_or(&Value::Null)))
        .and_then(|text| {
            let at = text.find(RUNNING_MARK)? + RUNNING_MARK.len();
            text[at..]
                .split(|character: char| !character.is_ascii_alphanumeric())
                .next()
                .map(str::to_owned)
        });
    let prefix = messages.iter().fold(FNV_OFFSET, |hash, message| {
        fnv(hash, canonical(message).to_string().as_bytes())
    });
    Some(Conversation {
        agent,
        steps: u64::try_from(steps).expect("a step count fits in u64"),
        running_cell,
        prefix,
    })
}

fn plan(seed: u64, agent: &Agent) -> Vec<Action> {
    let mut rng = match agent {
        Agent::Root { label, turn } => Rng::derive(seed, &format!("plan {label} {turn}")),
        Agent::Child { label, .. } => Rng::derive(seed, &format!("plan {label}")),
    };
    let mut actions = Vec::new();
    let base = match agent {
        Agent::Root { label, turn } => format!("{label}.t{turn}"),
        Agent::Child { label, .. } => label.clone(),
    };
    if rng.chance(EFFECT_PPM) {
        let count = rng.range(1, EFFECTS_MAX);
        actions.push(Action::Effects(
            (0..count).map(|index| format!("{base}.e{index}")).collect(),
        ));
    }
    let depth = match agent {
        Agent::Root { .. } => 2,
        Agent::Child { depth, .. } => *depth,
    };
    if depth > 0 && rng.chance(SPAWN_PPM) {
        let count = rng.range(1, CHILDREN_MAX);
        actions.push(Action::Spawn(
            (0..count)
                .map(|index| child_task(&format!("{base}.c{index}"), depth - 1))
                .collect(),
        ));
    }
    if matches!(agent, Agent::Child { .. }) {
        if rng.chance(MESSAGE_PPM) {
            actions.push(Action::Message);
        }
        actions.push(Action::Submit);
    }
    actions
}

fn code(action: &Action, label: &str) -> String {
    match action {
        Action::Effects(keys) => format!(
            "for (const key of {}) {{ await tools.{EFFECT_TOOL}({{ key }}); }}\ntext(\"effects done\");",
            json!(keys)
        ),
        Action::Spawn(tasks) => format!(
            "const settled = await Promise.allSettled({}.map((task) => tools.spawn_agent({{ role: \"worker\", task, harness: null, model: null, thinking: null, reasoning_mode: null, output_contract: {{ kind: \"string\" }} }})));\nconst ids = settled.filter((r) => r.status === \"fulfilled\").map((r) => r.value.agent_id);\nconst waited = await Promise.allSettled(ids.map((id) => tools.wait_agent({{ agent_ids: [id], timeout_ms: {WAIT_AGENT_TIMEOUT_MS} }})));\ntext(\"spawned \" + ids.length + \" waited \" + waited.filter((r) => r.status === \"fulfilled\").length);",
            json!(tasks)
        ),
        Action::Message => format!(
            "const listed = await tools.list_agents({{}});\nconst peers = (listed.agents ?? []).map((agent) => agent.agent_id);\nlet sent = 0;\nfor (const id of peers.slice(0, 2)) {{ try {{ await tools.send_agent_message({{ agent_id: id, message: \"hello from {label}\" }}); sent += 1; }} catch (error) {{}} }}\ntext(\"sent \" + sent);"
        ),
        Action::Submit => format!(
            "const result = await tools.submit_result({{ output: \"{label} done\" }});\ntext(JSON.stringify(result));"
        ),
    }
}

fn events(model: &str, id: &str, block: &Value, delta: Option<Value>, stop: &str) -> String {
    let mut list = vec![
        (
            "message_start",
            json!({"type":"message_start","message":{"id":format!("msg_{id}"),"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":0}}}),
        ),
        (
            "content_block_start",
            json!({"type":"content_block_start","index":0,"content_block":block}),
        ),
    ];
    if let Some(delta) = delta {
        list.push((
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":delta}),
        ));
    }
    list.push((
        "content_block_stop",
        json!({"type":"content_block_stop","index":0}),
    ));
    list.push((
        "message_delta",
        json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":1}}),
    ));
    list.push(("message_stop", json!({"type":"message_stop"})));
    list.into_iter()
        .map(|(name, event)| format!("event: {name}\ndata: {event}\n\n"))
        .collect()
}

pub struct Reply {
    pub body: String,
    pub label: String,
}

pub struct ModelServer {
    seed: u64,
    world: Arc<World>,
    mutation: WireMutation,
}

fn blocks(message: &Value) -> &[Value] {
    message
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

fn of_type<'a>(blocks: &'a [Value], kind: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    blocks
        .iter()
        .filter(move |block| block.get("type").and_then(Value::as_str) == Some(kind))
        .filter_map(move |block| block.get(key).and_then(Value::as_str))
}

fn duplicate_first_tool_result(messages: &mut [Value]) {
    for message in messages {
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        if let Some(at) = blocks
            .iter()
            .position(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        {
            let copy = blocks[at].clone();
            blocks.insert(at + 1, copy);
            return;
        }
    }
}

impl ModelServer {
    pub const fn new(seed: u64, world: Arc<World>, mutation: WireMutation) -> Self {
        Self {
            seed,
            world,
            mutation,
        }
    }

    fn check_tool_results(&self, label: &str, messages: &[Value]) {
        let mut pending: Vec<&str> = Vec::new();
        let mut answered: BTreeSet<&str> = BTreeSet::new();
        for message in messages {
            match role(message) {
                Some("assistant") => {
                    if let Some(id) = pending.first() {
                        self.world.wrong(format!(
                            "{label}'s request has an assistant message after tool_use {id} without its result"
                        ));
                    }
                    pending = of_type(blocks(message), "tool_use", "id").collect();
                }
                Some("user") => {
                    for id in of_type(blocks(message), "tool_result", "tool_use_id") {
                        if !answered.insert(id) {
                            self.world.wrong(format!(
                                "{label}'s request carries the result of tool_use {id} more than once"
                            ));
                        }
                        if !pending.contains(&id) {
                            self.world.wrong(format!(
                                "{label}'s request carries a result for tool_use {id}, which the message before did not call"
                            ));
                        }
                    }
                    if let Some(id) = pending.iter().find(|id| !answered.contains(*id)) {
                        self.world.wrong(format!(
                            "{label}'s request leaves tool_use {id} without a result in the next message"
                        ));
                    }
                    pending.clear();
                }
                _ => {}
            }
        }
    }

    fn check(&self, label: &str, messages: &[Value]) {
        for message in messages
            .iter()
            .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        {
            let digest = message_digest(label, message);
            if !self.world.was_sent(label, digest) {
                self.world.wrong(format!(
                    "{label}'s request carries an assistant message the model never sent: {}",
                    canonical(message)
                ));
            }
        }
    }

    fn check_root(&self, label: &str, turn: u64, messages: &[Value]) {
        let texts: Vec<String> = messages
            .iter()
            .map(|message| text_of(message.get("content").unwrap_or(&Value::Null)))
            .collect();
        for (acked, answer) in self.world.acked(label) {
            if acked >= turn {
                continue;
            }
            let prompt = root_prompt(label, acked);
            if !texts.iter().any(|text| text.contains(&prompt)) {
                self.world.wrong(format!(
                    "{label} turn {turn}'s request lost acknowledged turn {acked}'s prompt"
                ));
            }
            if !texts.iter().any(|text| text.contains(&answer)) {
                self.world.wrong(format!(
                    "{label} turn {turn}'s request lost acknowledged turn {acked}'s answer"
                ));
            }
        }
        for cancelled in self.world.cancelled_turns(label) {
            if cancelled >= turn {
                continue;
            }
            let prompt = root_prompt(label, cancelled);
            if !texts.iter().any(|text| text.contains(&prompt)) {
                self.world.wrong(format!(
                    "{label} turn {turn}'s request lost cancelled turn {cancelled}'s prompt"
                ));
            }
        }
    }

    fn drop_cancelled_prompts(&self, label: &str, messages: &mut [Value]) {
        for cancelled in self.world.cancelled_turns(label) {
            let prompt = root_prompt(label, cancelled);
            let dropped = format!("{ROOT_MARK} {label} turn-{cancelled}");
            for block in messages
                .iter_mut()
                .filter(|message| role(message) == Some("user"))
                .filter_map(|message| message.get_mut("content").and_then(Value::as_array_mut))
                .flatten()
            {
                if let Some(Value::String(text)) = block.get_mut("text") {
                    *text = text.replace(&prompt, &dropped);
                }
            }
        }
    }

    pub fn respond(&self, body: &str) -> Result<Reply, String> {
        let request: Value =
            serde_json::from_str(body).map_err(|error| format!("unparsable request: {error}"))?;
        let model = request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("claude")
            .to_owned();
        let mut messages = request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if self.mutation == WireMutation::ToolResultTwice {
            duplicate_first_tool_result(&mut messages);
        }
        let Some(conversation) = read(&messages) else {
            return Err("a request with no simulated agent in it".to_owned());
        };
        let label = conversation.agent.label().to_owned();
        if self.mutation == WireMutation::DropCancelledPrompt {
            self.drop_cancelled_prompts(&label, &mut messages);
        }
        self.world.count(|counts| counts.requests += 1);
        self.check(&label, &messages);
        self.check_tool_results(&label, &messages);
        if let Agent::Root { turn, .. } = conversation.agent {
            self.world.see(&label, turn);
            self.check_root(&label, turn, &messages);
        }
        let id = format!("{:016x}", fnv(conversation.prefix, label.as_bytes()));
        let (block, delta, stop, message) = self.decide(&conversation, &id);
        self.world.sent(&label, message_digest(&label, &message));
        self.world.trace(&format!(
            "model {label} steps {} -> {}",
            conversation.steps,
            canonical(&message)
        ));
        Ok(Reply {
            body: events(&model, &id, &block, delta, stop),
            label,
        })
    }

    fn decide(&self, conversation: &Conversation, id: &str) -> (Value, Option<Value>, &'static str, Value) {
        let tool_id = format!("toolu_{id}");
        if let Some(cell) = &conversation.running_cell {
            self.world.count(|counts| counts.waits += 1);
            let input = json!({"cell_id": cell, "yield_time_ms": WAIT_YIELD_MS});
            return tool_use(&tool_id, "wait", &input);
        }
        let label = conversation.agent.label();
        let actions = plan(self.seed, &conversation.agent);
        let step = usize::try_from(conversation.steps).expect("a step count fits in usize");
        if let Some(action) = actions.get(step) {
            match action {
                Action::Spawn(tasks) => self.world.count(|counts| {
                    counts.spawned += u64::try_from(tasks.len()).expect("a count fits in u64");
                }),
                Action::Submit => self.world.count(|counts| counts.submitted += 1),
                _ => {}
            }
            let input = json!({"code": code(action, label)});
            return tool_use(&tool_id, "exec", &input);
        }
        let text = match &conversation.agent {
            Agent::Root { label, turn } => final_text(label, *turn),
            Agent::Child { label, .. } => format!("finished {label}"),
        };
        (
            json!({"type": "text", "text": ""}),
            Some(json!({"type": "text_delta", "text": text})),
            "end_turn",
            json!({"role": "assistant", "content": [{"type": "text", "text": text}]}),
        )
    }
}

fn tool_use(id: &str, name: &str, input: &Value) -> (Value, Option<Value>, &'static str, Value) {
    (
        json!({"type": "tool_use", "id": id, "name": name, "input": {}}),
        Some(json!({"type": "input_json_delta", "partial_json": input.to_string()})),
        "tool_use",
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}),
    )
}

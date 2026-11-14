use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    io::{self, Write},
};

use nanocode::agent::events::{AgentEvent, AgentEventKind};
use serde::Deserialize;
use serde_json::Value;

#[derive(Eq, PartialEq)]
enum Block {
    Reasoning(u64),
    Answer(String),
    Notice,
}

#[derive(Default, Deserialize)]
struct Payload<'a> {
    #[serde(default, borrow)]
    text: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    item_id: Option<Cow<'a, str>>,
    #[serde(default)]
    model_call_index: Option<u64>,
    #[serde(default, borrow)]
    tool: Option<Cow<'a, str>>,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    observational: Option<Value>,
}

impl Payload<'_> {
    fn key(&self) -> Cow<'_, str> {
        match &self.item_id {
            Some(id) => Cow::Borrowed(id),
            None => Cow::Owned(match self.model_call_index {
                Some(index) => format!("call-{index}"),
                None => "call-null".to_owned(),
            }),
        }
    }
}

pub struct Printer<O> {
    answers: O,
    live: bool,
    notices: Vec<u8>,
    block: Option<Block>,
    open: bool,
    streamed: HashSet<String>,
    calls: HashMap<String, Option<u64>>,
    held: Vec<(String, String)>,
}

impl<O: Write> Printer<O> {
    pub fn new(answers: O, live: bool) -> Self {
        Self {
            answers,
            live,
            notices: Vec::new(),
            block: None,
            open: false,
            streamed: HashSet::new(),
            calls: HashMap::new(),
            held: Vec::new(),
        }
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if !self.notices.is_empty() {
            let mut stderr = io::stderr().lock();
            stderr.write_all(&self.notices)?;
            stderr.flush()?;
            self.notices.clear();
        }
        self.answers.flush()
    }

    pub fn event(&mut self, event: &AgentEvent) -> io::Result<()> {
        if !matches!(
            event.kind,
            AgentEventKind::ReasoningSummaryDelta
                | AgentEventKind::AssistantDelta
                | AgentEventKind::AssistantMessage
                | AgentEventKind::ToolCall
                | AgentEventKind::RunError
                | AgentEventKind::ModelAttemptRetrying
        ) {
            return Ok(());
        }
        if event.kind == AgentEventKind::ModelAttemptRetrying {
            let payload = serde_json::from_str::<Value>(event.payload.get()).unwrap_or_default();
            return self.retrying(&payload);
        }
        let payload = serde_json::from_str::<Payload>(event.payload.get()).unwrap_or_default();
        let text = payload.text.as_deref().unwrap_or_default();
        match event.kind {
            AgentEventKind::ReasoningSummaryDelta => self.write(
                Block::Reasoning(payload.model_call_index.unwrap_or_default()),
                text,
            ),
            AgentEventKind::AssistantDelta if !self.live => {
                self.hold(&payload.key(), payload.model_call_index, text);
                Ok(())
            }
            AgentEventKind::AssistantDelta => {
                self.calls
                    .insert(payload.key().into_owned(), payload.model_call_index);
                self.answer(&payload.key(), text)
            }
            AgentEventKind::AssistantMessage if !self.streamed.contains(&*payload.key()) => {
                self.held.retain(|(key, _)| *key != payload.key());
                self.answer(&payload.key(), text)
            }
            AgentEventKind::ToolCall => {
                self.notice(&format!("· {}", payload.tool.as_deref().unwrap_or("tool")))
            }
            AgentEventKind::RunError
                if payload.observational.as_ref() == Some(&Value::Bool(true)) =>
            {
                self.notice(&format!(
                    "warning: {}",
                    payload
                        .error
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ))
            }
            _ => Ok(()),
        }
    }

    pub fn finish(mut self, final_message: &str) -> io::Result<()> {
        for (key, text) in std::mem::take(&mut self.held) {
            self.answer(&key, &text)?;
        }
        self.close()?;
        if self.streamed.is_empty() {
            writeln!(self.answers, "{final_message}")?;
        }
        self.flush()
    }

    fn hold(&mut self, key: &str, call: Option<u64>, text: &str) {
        self.calls.insert(key.to_owned(), call);
        match self.held.iter_mut().find(|(held, _)| held == key) {
            Some((_, held)) => held.push_str(text),
            None => self.held.push((key.to_owned(), text.to_owned())),
        }
    }

    fn retrying(&mut self, payload: &Value) -> io::Result<()> {
        let call = payload["model_call_index"].as_u64();
        let dead: Vec<String> = self
            .calls
            .iter()
            .filter(|(_, owner)| **owner == call)
            .map(|(key, _)| key.clone())
            .collect();
        let mut shown = false;
        for key in dead {
            self.calls.remove(&key);
            self.held.retain(|(held, _)| *held != key);
            shown |= self.streamed.remove(&key);
        }
        let notice = crate::events::retry_notice(payload);
        if shown {
            self.notice(&format!("nanocode: {notice}; the partial reply above is discarded"))
        } else {
            self.notice(&format!("nanocode: {notice}"))
        }
    }

    fn answer(&mut self, key: &str, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if matches!(&self.block, Some(Block::Answer(current)) if current == key) {
            self.answers.write_all(text.as_bytes())?;
            self.open = !text.ends_with('\n');
            return Ok(());
        }
        self.streamed.insert(key.to_owned());
        self.write(Block::Answer(key.to_owned()), text)
    }

    fn write(&mut self, block: Block, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if self.block.as_ref() != Some(&block) {
            self.close()?;
        }
        if matches!(block, Block::Reasoning(_)) {
            self.notices.extend_from_slice(text.as_bytes());
        } else {
            self.answers.write_all(text.as_bytes())?;
        }
        self.block = Some(block);
        self.open = !text.ends_with('\n');
        Ok(())
    }

    fn notice(&mut self, line: &str) -> io::Result<()> {
        self.close()?;
        self.block = Some(Block::Notice);
        writeln!(self.notices, "{line}")
    }

    fn close(&mut self) -> io::Result<()> {
        match self.block {
            Some(Block::Reasoning(_)) => {
                if self.open {
                    self.notices.push(b'\n');
                }
                self.notices.push(b'\n');
            }
            Some(Block::Answer(_)) if self.open => {
                self.answers.write_all(b"\n")?;
            }
            _ => {}
        }
        self.block = None;
        self.open = false;
        self.flush()
    }
}

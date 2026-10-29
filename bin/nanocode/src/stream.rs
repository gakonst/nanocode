use std::{
    borrow::Cow,
    collections::HashSet,
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
    block: Option<Block>,
    open: bool,
    streamed: HashSet<String>,
}

impl<O: Write> Printer<O> {
    pub fn new(answers: O) -> Self {
        Self {
            answers,
            block: None,
            open: false,
            streamed: HashSet::new(),
        }
    }

    pub fn event(&mut self, event: &AgentEvent) -> io::Result<()> {
        if !matches!(
            event.kind,
            AgentEventKind::ReasoningSummaryDelta
                | AgentEventKind::AssistantDelta
                | AgentEventKind::AssistantMessage
                | AgentEventKind::ToolCall
                | AgentEventKind::RunError
        ) {
            return Ok(());
        }
        let payload = serde_json::from_str::<Payload>(event.payload.get()).unwrap_or_default();
        let text = payload.text.as_deref().unwrap_or_default();
        match event.kind {
            AgentEventKind::ReasoningSummaryDelta => self.write(
                Block::Reasoning(payload.model_call_index.unwrap_or_default()),
                text,
            ),
            AgentEventKind::AssistantDelta => self.answer(&payload.key(), text),
            AgentEventKind::AssistantMessage if !self.streamed.contains(&*payload.key()) => {
                self.answer(&payload.key(), text)
            }
            AgentEventKind::ToolCall => self.notice(&format!(
                "· {}",
                payload.tool.as_deref().unwrap_or("tool")
            )),
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
        self.close()?;
        if self.streamed.is_empty() {
            writeln!(self.answers, "{final_message}")?;
        }
        self.answers.flush()
    }

    fn answer(&mut self, key: &str, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        if matches!(&self.block, Some(Block::Answer(current)) if current == key) {
            self.answers.write_all(text.as_bytes())?;
            self.answers.flush()?;
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
            let mut stderr = io::stderr().lock();
            stderr.write_all(text.as_bytes())?;
            stderr.flush()?;
        } else {
            self.answers.write_all(text.as_bytes())?;
            self.answers.flush()?;
        }
        self.block = Some(block);
        self.open = !text.ends_with('\n');
        Ok(())
    }

    fn notice(&mut self, line: &str) -> io::Result<()> {
        self.close()?;
        self.block = Some(Block::Notice);
        let mut stderr = io::stderr().lock();
        writeln!(stderr, "{line}")?;
        stderr.flush()
    }

    fn close(&mut self) -> io::Result<()> {
        match self.block {
            Some(Block::Reasoning(_)) => {
                let mut stderr = io::stderr().lock();
                if self.open {
                    stderr.write_all(b"\n")?;
                }
                stderr.write_all(b"\n")?;
                stderr.flush()?;
            }
            Some(Block::Answer(_)) if self.open => {
                self.answers.write_all(b"\n")?;
                self.answers.flush()?;
            }
            _ => {}
        }
        self.block = None;
        self.open = false;
        Ok(())
    }
}

use std::{
    collections::HashSet,
    io::{self, Write},
};

use nanocode::agent::{
    AgentEvents, PromptRequest,
    events::{AgentEvent, AgentEventKind},
};
use serde_json::Value;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub async fn prompt(
    agent: &nanocode::Agent,
    events: &mut AgentEvents,
    request: impl Into<PromptRequest>,
) -> Result<()> {
    let mut result = std::pin::pin!(agent.prompt(request).await?.result());
    let mut printer = Printer::default();
    let result = loop {
        nanocode_sync::select! {
            Some(event) = events.recv() => printer.event(&event)?,
            result = &mut result => break result?,
        }
    };
    while let Some(event) = events.try_recv() {
        printer.event(&event)?;
    }
    printer.finish(result.final_message())?;
    Ok(())
}

#[derive(Eq, PartialEq)]
enum Block {
    Reasoning(u64),
    Answer(String),
    Notice,
}

#[derive(Default)]
struct Printer {
    block: Option<Block>,
    open: bool,
    streamed: HashSet<String>,
}

impl Printer {
    fn event(&mut self, event: &AgentEvent) -> io::Result<()> {
        let payload = serde_json::from_str::<Value>(event.payload.get()).unwrap_or_default();
        let text = payload["text"].as_str().unwrap_or_default();
        match event.kind {
            AgentEventKind::ReasoningSummaryDelta => self.write(
                Block::Reasoning(payload["model_call_index"].as_u64().unwrap_or_default()),
                text,
            ),
            AgentEventKind::AssistantDelta => self.answer(&payload, text),
            AgentEventKind::AssistantMessage if !self.streamed.contains(&key(&payload)) => {
                self.answer(&payload, text)
            }
            AgentEventKind::ToolCall => {
                self.notice(&format!("· {}", payload["tool"].as_str().unwrap_or("tool")))
            }
            AgentEventKind::RunError if payload["observational"] == true => self.notice(&format!(
                "warning: {}",
                payload["error"].as_str().unwrap_or_default()
            )),
            _ => Ok(()),
        }
    }

    fn finish(mut self, final_message: &str) -> io::Result<()> {
        self.close()?;
        let mut stdout = io::stdout().lock();
        if self.streamed.is_empty() {
            writeln!(stdout, "{final_message}")?;
        }
        stdout.flush()
    }

    fn answer(&mut self, payload: &Value, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let key = key(payload);
        self.streamed.insert(key.clone());
        self.write(Block::Answer(key), text)
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
            let mut stdout = io::stdout().lock();
            stdout.write_all(text.as_bytes())?;
            stdout.flush()?;
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
                let mut stdout = io::stdout().lock();
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
            _ => {}
        }
        self.block = None;
        self.open = false;
        Ok(())
    }
}

fn key(payload: &Value) -> String {
    payload["item_id"].as_str().map_or_else(
        || format!("call-{}", payload["model_call_index"]),
        str::to_owned,
    )
}

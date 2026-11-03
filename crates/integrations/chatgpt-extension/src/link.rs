use crate::{read_body, write_body};
use async_trait::async_trait;
use nanocode_sync::{Atom, Slot};
use nanocode_tool_contracts::{
    Tool, ToolContext, ToolDefinition, ToolError, ToolInput, ToolOutput, ToolResult,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, Write},
    os::unix::{fs::PermissionsExt, net},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError, sync_channel},
    },
    thread,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, mpsc, oneshot},
};

const STEP_TIMEOUT: Duration = Duration::from_secs(45);
const MAX_CONNECTIONS: usize = 8;
const OUTBOX: usize = 64;
const MAX_INPUT: usize = 32 * 1024;

pub struct ExtensionLink {
    name: String,
    absent: String,
    next: AtomicU64,
    live: Atom<Option<Arc<Live>>>,
    unavailable: Atom<Option<String>>,
}

struct Live {
    id: u64,
    out: mpsc::Sender<String>,
    pending: Atom<HashMap<String, Arc<Slot<oneshot::Sender<Value>>>>>,
}

impl Live {
    fn answer(&self, call: &str) -> Option<oneshot::Sender<Value>> {
        self.pending
            .try_update(|pending| pending.remove(call))
            .and_then(|waiting| waiting.take())
    }
}

impl ExtensionLink {
    pub fn new(name: impl Into<String>, absent: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            absent: absent.into(),
            next: AtomicU64::new(0),
            live: Atom::default(),
            unavailable: Atom::default(),
        })
    }

    pub fn connected(&self) -> bool {
        self.live.read(Option::is_some)
    }

    pub fn set_unavailable(&self, reason: Option<String>) {
        self.unavailable.set(reason);
    }

    fn absent(&self) -> ToolOutput {
        ToolOutput::error(match self.unavailable.get().as_deref() {
            Some(reason) => format!("{} {reason}", self.absent),
            None => self.absent.clone(),
        })
    }

    pub async fn serve(self: Arc<Self>, path: PathBuf) -> io::Result<()> {
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        loop {
            let permit = Arc::clone(&slots)
                .acquire_owned()
                .await
                .map_err(io::Error::other)?;
            let (stream, _) = listener.accept().await?;
            let link = Arc::clone(&self);
            tokio::spawn(async move {
                link.connection(stream).await;
                drop(permit);
            });
        }
    }

    async fn connection(self: Arc<Self>, stream: UnixStream) {
        let (read, mut write) = stream.into_split();
        let (out, mut lines_out) = mpsc::channel::<String>(OUTBOX);
        let live = Arc::new(Live {
            id: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            out: out.clone(),
            pending: Atom::default(),
        });
        let claimed = self.live.try_update(|current| {
            if current.is_some() {
                return None;
            }
            *current = Some(Arc::clone(&live));
            Some(())
        });
        if claimed.is_none() {
            let _ = write
                .write_all(b"{\"type\":\"status\",\"connection\":\"other_browser\"}\n")
                .await;
            return;
        }
        let _ = out
            .send(json!({"type": "hello", "hand": self.name}).to_string())
            .await;
        drop(out);
        let writer = tokio::spawn(async move {
            while let Some(line) = lines_out.recv().await {
                if write.write_all(line.as_bytes()).await.is_err()
                    || write.write_all(b"\n").await.is_err()
                {
                    break;
                }
            }
        });
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(call) = reply["id"].as_str().filter(|_| reply["type"] == "result") else {
                continue;
            };
            if let Some(waiting) = live.answer(call) {
                let _ = waiting.send(reply);
            }
        }
        self.live.update(|current| {
            if current
                .as_ref()
                .is_some_and(|current| current.id == live.id)
            {
                *current = None;
            }
        });
        live.pending.set(HashMap::new());
        writer.abort();
    }

    async fn turn_ended(&self, session: &str, turn: &str) {
        let out = self
            .live
            .read(|live| live.as_ref().map(|live| live.out.clone()));
        if let Some(out) = out {
            let frame = json!({"type": "turn_ended", "session": session, "turn": turn});
            let _ = out.send(frame.to_string()).await;
        }
    }

    async fn step(&self, context: ToolContext<'_>, input: Value) -> ToolOutput {
        let call = context.call_id().to_owned();
        let frame = json!({
            "type": "call",
            "id": call,
            "session": context.session_id(),
            "turn": context.turn_id(),
            "input": input,
        })
        .to_string();
        let Some(live) = Option::clone(&self.live.get()) else {
            return self.absent();
        };
        let (sender, receiver) = oneshot::channel();
        let waiting = Arc::new(Slot::new(Some(sender)));
        live.pending.update(|pending| {
            pending.insert(call.clone(), Arc::clone(&waiting));
        });
        drop(waiting);
        let out = live.out.clone();
        let live = Arc::downgrade(&live);
        let sent_and_answered = tokio::time::timeout(STEP_TIMEOUT, async {
            if out.send(frame).await.is_err() {
                return None;
            }
            Some(receiver.await)
        })
        .await;
        let reply = match sent_and_answered {
            Ok(None) => {
                if let Some(live) = live.upgrade() {
                    drop(live.answer(&call));
                }
                return self.absent();
            }
            Ok(Some(Ok(reply))) => reply,
            Ok(Some(Err(_))) => {
                return ToolOutput::error(
                    "The browser disconnected during this step; whether it took effect is unknown. Take a snapshot before trying again.",
                );
            }
            Err(_) => {
                if let Some(live) = live.upgrade() {
                    drop(live.answer(&call));
                }
                return ToolOutput::error(
                    "The browser did not finish this step within 45 seconds; whether it took effect is unknown. Take a snapshot before trying again.",
                );
            }
        };
        let summary = reply["summary"]
            .as_str()
            .unwrap_or("Browser step")
            .to_owned();
        if reply["ok"] == true {
            ToolOutput::text(summary).with_structured_result(reply["result"].clone())
        } else {
            ToolOutput::error(
                reply["error"]
                    .as_str()
                    .unwrap_or("The browser step failed.")
                    .to_owned(),
            )
        }
    }
}

pub struct ExtensionTool {
    link: Arc<ExtensionLink>,
    definition: ToolDefinition,
}

impl ExtensionTool {
    pub const fn new(link: Arc<ExtensionLink>, definition: ToolDefinition) -> Self {
        Self { link, definition }
    }
}

#[async_trait]
impl Tool for ExtensionTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input: Value = input.decode_json()?;
        if !input.is_object() || input.to_string().len() > MAX_INPUT {
            return Ok(ToolOutput::error(
                "expected browser arguments of at most 32 KiB",
            ));
        }
        Ok(self.link.step(context, input).await)
    }

    async fn end_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        _hook_event_name: &str,
    ) -> Result<(), ToolError> {
        self.link.turn_ended(session_id, turn_id).await;
        Ok(())
    }
}

fn emit(message: &[u8]) -> bool {
    write_body(&mut io::stdout().lock(), message).is_ok()
}

fn forward(stream: net::UnixStream, browser: &Receiver<Vec<u8>>) -> io::Result<bool> {
    let reader = stream.try_clone()?;
    let (closed, done) = sync_channel::<()>(1);
    let reading = thread::spawn(move || {
        for line in io::BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if !emit(line.as_bytes()) {
                std::process::exit(0);
            }
        }
        let _ = closed.send(());
    });
    let mut writer = stream;
    let mut gone = false;
    loop {
        match browser.recv_timeout(Duration::from_millis(250)) {
            Ok(message) => {
                if writer
                    .write_all(&message)
                    .and_then(|()| writer.write_all(b"\n"))
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                gone = true;
                break;
            }
        }
        if done.try_recv().is_ok() {
            break;
        }
    }
    let _ = writer.shutdown(std::net::Shutdown::Both);
    let _ = reading.join();
    Ok(gone)
}

pub fn relay_host(claimed: impl Fn() -> bool, socket: &Path) -> io::Result<()> {
    let (from_browser, browser) = sync_channel::<Vec<u8>>(16);
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        loop {
            let Ok(message) = read_body(&mut stdin) else {
                std::process::exit(0);
            };
            if from_browser.send(message).is_err() {
                std::process::exit(0);
            }
        }
    });
    let mut reported = "";
    loop {
        let status = if !claimed() {
            "unclaimed"
        } else {
            match net::UnixStream::connect(socket) {
                Ok(stream) => {
                    reported = "";
                    if forward(stream, &browser)? {
                        return Ok(());
                    }
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
                Err(_) => "hand_offline",
            }
        };
        if status != reported {
            reported = status;
            let message = json!({"type": "status", "connection": status}).to_string();
            if !emit(message.as_bytes()) {
                return Ok(());
            }
        }
        while browser.try_recv().is_ok() {}
        thread::sleep(Duration::from_secs(2));
    }
}

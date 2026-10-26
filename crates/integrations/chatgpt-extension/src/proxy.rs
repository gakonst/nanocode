use crate::{
    Bridge, EDGE_EXTENSION_ID, EDGE_ORIGIN, EXTENSION_ID, Error, ORIGIN,
    bridge::{host_path, random_prefix, started_path},
    native::{frame, read_body, standalone_reply, write_body},
    steps::Tabs,
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const RESPONSES: usize = 256;
/// Frames waiting for the stdout writer before senders block, so a browser
/// that stops reading pushes back on OpenAI's host instead of growing memory;
/// the stdout writes the proxy replaces were synchronous.
const OUTGOING: usize = 64;

pub(crate) struct Extension {
    responses: mpsc::Receiver<Value>,
    output: mpsc::SyncSender<Vec<u8>>,
    connected: Arc<AtomicBool>,
    upstream: Option<Child>,
    prefix: String,
    next: u64,
}

impl Extension {
    fn start(original: Option<&Path>, args: &[String], namespace: &str) -> Result<Self, Error> {
        let (sender, responses) = mpsc::sync_channel(RESPONSES);
        let (output, outgoing) = mpsc::sync_channel::<Vec<u8>>(OUTGOING);
        let connected = Arc::new(AtomicBool::new(true));
        let prefix = random_prefix(namespace)?;
        let own_prefix = prefix.clone();
        let writer_connected = Arc::clone(&connected);
        thread::spawn(move || {
            let mut stdout = io::stdout();
            for body in outgoing {
                if write_body(&mut stdout, &body).is_err() {
                    writer_connected.store(false, Ordering::Release);
                    break;
                }
            }
        });
        let (upstream, mut to_original) = match original {
            Some(original) => {
                let mut upstream = Command::new(original)
                    .args(args)
                    .current_dir(
                        original
                            .parent()
                            .ok_or("original host has no executable directory")?,
                    )
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()?;
                let to_original = upstream.stdin.take().ok_or("original host has no stdin")?;
                let mut from_original = upstream
                    .stdout
                    .take()
                    .ok_or("original host has no stdout")?;
                let forward = output.clone();
                let live = Arc::clone(&connected);
                thread::spawn(move || {
                    while let Ok(body) = read_body(&mut from_original) {
                        if forward.send(body).is_err() {
                            break;
                        }
                    }
                    live.store(false, Ordering::Release);
                });
                (Some(upstream), Some(to_original))
            }
            None => (None, None),
        };
        let answer = output.clone();
        let reader_connected = Arc::clone(&connected);
        thread::spawn(move || {
            let mut input = io::stdin().lock();
            while let Ok(body) = read_body(&mut input) {
                let message = serde_json::from_slice::<Value>(&body);
                let ours = message.as_ref().is_ok_and(|message| {
                    message["method"].is_null()
                        && message["id"]
                            .as_str()
                            .is_some_and(|id| id.starts_with(&own_prefix))
                });
                if ours {
                    if let Ok(message) = message
                        && sender.send(message).is_err()
                    {
                        break;
                    }
                } else if let Some(ref mut to_original) = to_original {
                    if write_body(to_original, &body).is_err() {
                        break;
                    }
                } else if let Some(reply) = message.ok().as_ref().and_then(standalone_reply)
                    && frame(&reply)
                        .map_err(drop)
                        .and_then(|body| answer.send(body).map_err(drop))
                        .is_err()
                {
                    break;
                }
            }
            reader_connected.store(false, Ordering::Release);
        });
        Ok(Self {
            responses,
            output,
            connected,
            upstream,
            prefix,
            next: 0,
        })
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub(crate) fn rpc(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.next += 1;
        let id = format!("{}{}", self.prefix, self.next);
        let body = frame(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        // The output channel is bounded: wait for room only until the deadline.
        let mut body = body;
        loop {
            match self.output.try_send(body) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(returned)) if Instant::now() < deadline => {
                    body = returned;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(mpsc::TrySendError::Full(_)) => {
                    return Err(format!("{method}: the browser is not reading").into());
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "extension disconnected",
                    )
                    .into());
                }
            }
        }
        loop {
            let reply = self
                .responses
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            if reply["id"] != id {
                continue;
            }
            if !reply["error"].is_null() {
                return Err(format!(
                    "{method}: {}",
                    reply["error"]["message"]
                        .as_str()
                        .unwrap_or("extension request failed")
                )
                .into());
            }
            return Ok(reply["result"].clone());
        }
    }

    pub(crate) fn cdp(
        &mut self,
        session: &str,
        turn: &str,
        tab: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, Error> {
        self.rpc(
            "executeCdp",
            json!({"session_id":session,"turn_id":turn,
            "target":{"tabId":tab},"method":method,"commandParams":params}),
        )
    }

    pub(crate) fn evaluate(
        &mut self,
        session: &str,
        turn: &str,
        tab: u64,
        expression: &str,
    ) -> Result<Value, Error> {
        let result = self.cdp(
            session,
            turn,
            tab,
            "Runtime.evaluate",
            json!({"expression":expression,"returnByValue":true,"awaitPromise":true}),
        )?;
        if !result["exceptionDetails"].is_null() {
            return Err(format!(
                "page evaluation failed: {}",
                result["exceptionDetails"]["text"]
            )
            .into());
        }
        Ok(result["result"]["value"].clone())
    }
}

impl Drop for Extension {
    fn drop(&mut self) {
        self.connected.store(false, Ordering::Release);
        if let Some(ref mut upstream) = self.upstream {
            let _ = upstream.kill();
            let _ = upstream.wait();
        }
    }
}

pub struct Connection {
    extension: Extension,
    controls: bool,
    /// The official extension did not answer the handshake.
    refused: bool,
    record: PathBuf,
    manifest: PathBuf,
    launcher: PathBuf,
    owner: String,
    namespace: String,
}

impl Bridge {
    /// Records that the browser started the launcher, and why it stopped, for [`Bridge::launches`].
    pub fn connect(
        &self,
        manifest: &Path,
        browser: &str,
        args: &[String],
    ) -> Result<Connection, Error> {
        let marker = started_path(&self.record_path(&self.browser_root(browser)?)?);
        let _ = fs::write(&marker, "");
        let result = self.open(manifest, browser, args);
        let failure = match result {
            Err(ref error) => Some(error.to_string()),
            Ok(ref connection) if connection.refused => {
                Some("the ChatGPT extension did not answer the getInfo handshake".into())
            }
            Ok(_) => None,
        };
        if let Some(failure) = failure {
            let _ = fs::write(&marker, failure);
        }
        result
    }

    fn open(&self, manifest: &Path, browser: &str, args: &[String]) -> Result<Connection, Error> {
        let origin = args.first().ok_or("missing extension origin")?;
        let root = self.browser_root(browser)?;
        if manifest != host_path(&root) {
            return Err("native-host registration does not belong to this browser".into());
        }
        let record_path = self.record_path(&root)?;
        let launcher = self.launcher_path(&root)?;
        let record = self.registration(&root)?;
        if !self.owns(manifest, &launcher) {
            return Err(format!(
                "{} no longer owns this native-host registration",
                self.owner_name()
            )
            .into());
        }
        let upstream = self.upstream_of(&record)?;
        if !upstream
            .origins
            .as_array()
            .is_some_and(|allowed| allowed.iter().any(|entry| entry == origin))
        {
            return Err("original OpenAI host does not allow this extension".into());
        }
        let mut extension =
            Extension::start(upstream.executable.as_deref(), args, self.namespace_name())?;
        let chatgpt = origin == ORIGIN || origin == EDGE_ORIGIN;
        let controls = chatgpt && {
            let expected = if origin == EDGE_ORIGIN {
                EDGE_EXTENSION_ID
            } else {
                EXTENSION_ID
            };
            extension
                .rpc("getInfo", json!({}))
                .is_ok_and(|info| info["metadata"]["extensionId"] == expected)
        };
        Ok(Connection {
            extension,
            controls,
            refused: chatgpt && !controls,
            record: record_path,
            manifest: manifest.to_path_buf(),
            launcher,
            owner: self.owner_name().to_string(),
            namespace: self.namespace_name().to_string(),
        })
    }
}

impl Connection {
    pub const fn controls(&self) -> bool {
        self.controls
    }

    pub fn connected(&self) -> bool {
        self.extension.connected()
    }

    pub fn owned(&self) -> bool {
        self.record.exists() && Bridge::still_ours(&self.manifest, &self.launcher)
    }

    pub fn live(&self) -> bool {
        self.controls && self.connected() && self.owned()
    }

    pub fn step(
        &mut self,
        tabs: &mut Tabs,
        session: &str,
        turn: Option<&str>,
        input: &Value,
    ) -> Result<(String, Value), Error> {
        self.controlled()?;
        tabs.step(
            &mut self.extension,
            &self.namespace,
            &self.owner,
            session,
            turn,
            input,
        )
    }

    pub fn name_session(
        &mut self,
        tabs: &mut Tabs,
        session: &str,
        turn: Option<&str>,
        title: &str,
    ) -> Result<(), Error> {
        self.controlled()?;
        tabs.name(&mut self.extension, &self.namespace, session, turn, title)
    }

    pub fn end_turn(&mut self, tabs: &mut Tabs, session: &str, turn: &str) -> Result<(), Error> {
        self.controlled()?;
        tabs.end_turn(&mut self.extension, &self.namespace, session, turn)
    }

    pub fn end_idle(&mut self, tabs: &mut Tabs, idle: Duration) -> Result<(), Error> {
        self.controlled()?;
        tabs.end_idle(&mut self.extension, &self.namespace, idle)
    }

    fn controlled(&self) -> Result<(), Error> {
        if self.controls {
            Ok(())
        } else {
            Err("this extension connection only forwards to the original host".into())
        }
    }

    pub fn finish(self) {
        while self.extension.connected() {
            thread::sleep(Duration::from_millis(250));
        }
    }
}

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use nanocode_mcp::Session;
use serde_json::{Map, Value, json};
use tokio::process::Command;

pub use nanocode_mcp::Error as McpError;

const SEARCH: &[&str] = &[
    "/Applications/CuaDriver.app/Contents/MacOS/cua-driver",
    ".local/bin/cua-driver",
    "/usr/local/bin/cua-driver",
    "/opt/homebrew/bin/cua-driver",
];
const LAUNCH: Duration = Duration::from_secs(20);
const FIRST_READ: Duration = Duration::from_millis(60);
const SETTLE: Duration = Duration::from_millis(400);
const IMAGE_EDGES: [u32; 3] = [1366, 1024, 768];
const MAX_KEY_STEPS: usize = 600;
const STEPS_PER_CALL: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub path: PathBuf,
    pub direct: bool,
}

impl Install {
    pub fn new(path: impl Into<PathBuf>, direct: bool) -> Self {
        Self {
            path: path.into(),
            direct,
        }
    }

    pub fn installed() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        if let Some(path) = crate::fork_driver(&home).filter(|path| path.is_file()) {
            return Some(Self::new(path, true));
        }
        SEARCH
            .iter()
            .map(|path| home.join(path))
            .find(|path| path.is_file())
            .map(|path| Self::new(path, false))
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.path);
        command.arg("mcp");
        if self.direct {
            command.arg("--direct");
        }
        command
    }
}

pub struct McpClient {
    session: Session,
}

impl McpClient {
    pub async fn start(command: Command, name: &str, version: &str) -> Result<Self, String> {
        let (session, outgoing, inbound) = Session::new();
        let stderr = nanocode_mcp_tools::stdio::spawn(command, outgoing, inbound)
            .map_err(|error| error.to_string())?;
        if let Some(mut stderr) = stderr {
            drop(tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            }));
        }
        session
            .request_value(
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": name, "version": version},
                })),
            )
            .await
            .map_err(|error| error.to_string())?;
        session
            .notify("notifications/initialized", None)
            .map_err(|error| error.to_string())?;
        Ok(Self { session })
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        self.session.request_value(method, Some(params)).await
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))
            .await
    }
}

pub fn text_of(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "text")
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn image_of(result: &Value) -> Option<String> {
    result["content"]
        .as_array()?
        .iter()
        .find(|item| item["type"] == "image")?["data"]
        .as_str()
        .map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub sent: bool,
}

impl Failure {
    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            sent: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub text: String,
    pub screenshot: Option<String>,
    pub screenshot_omitted: bool,
}

pub type Policy = Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

#[derive(Default)]
struct App {
    name: String,
    window: Option<i64>,
    snapshot: Option<String>,
    rows: HashMap<u64, String>,
    scope: String,
    acted: Option<Instant>,
}

pub struct Driver {
    install: Install,
    client: (String, String),
    policy: Option<Policy>,
    session: Option<McpClient>,
    apps: HashMap<i64, App>,
}

pub fn find_app<'a>(apps: &'a [Value], wanted: &str) -> Result<Option<&'a Value>, String> {
    let lower = wanted.to_lowercase();
    let base = Path::new(wanted)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(wanted)
        .to_lowercase();
    let key = |app: &Value| {
        app["bundle_id"]
            .as_str()
            .or(app["launch_path"].as_str())
            .or(app["name"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let name = |app: &Value| app["name"].as_str().unwrap_or_default().to_lowercase();
    let pick = |list: Vec<&'a Value>| -> Result<Option<&'a Value>, String> {
        let mut ids: Vec<String> = list.iter().map(|app| key(app)).collect();
        ids.sort();
        ids.dedup();
        let running: Vec<&&Value> = list.iter().filter(|app| app["running"] == true).collect();
        if ids.len() <= 1 {
            return Ok(running.first().copied().copied().or(list.first().copied()));
        }
        let mut running_ids: Vec<String> = running.iter().map(|app| key(app)).collect();
        running_ids.sort();
        running_ids.dedup();
        if running_ids.len() == 1 {
            return Ok(running.first().copied().copied());
        }
        Err(format!(
            "\"{wanted}\" matches several apps ({}); pass a bundle identifier or app path",
            ids.into_iter().take(8).collect::<Vec<_>>().join(", ")
        ))
    };
    let by_id: Vec<&Value> = apps
        .iter()
        .filter(|app| {
            app["bundle_id"]
                .as_str()
                .is_some_and(|id| id.eq_ignore_ascii_case(wanted))
                || app["launch_path"] == wanted
        })
        .collect();
    if !by_id.is_empty() {
        return pick(by_id);
    }
    let by_name: Vec<&Value> = apps
        .iter()
        .filter(|app| name(app) == lower || (wanted.contains('/') && name(app) == base))
        .collect();
    if !by_name.is_empty() {
        return pick(by_name);
    }
    let partial: Vec<&Value> = apps
        .iter()
        .filter(|app| app["bundle_id"].is_string() && name(app).contains(&lower))
        .collect();
    if partial.is_empty() {
        return Ok(None);
    }
    pick(partial)
}

pub fn chord(spec: &str) -> Result<(String, Vec<&'static str>), String> {
    let parts: Vec<&str> = if spec.len() > 1 {
        spec.split('+').collect()
    } else {
        vec![spec]
    };
    let (key, modifiers) = parts.split_last().ok_or("missing key")?;
    let key = if key.is_empty() { "+" } else { key.trim() };
    let modifiers = modifiers
        .iter()
        .map(|part| match part.trim().to_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" | "win" => Ok("cmd"),
            "ctrl" | "control" => Ok("ctrl"),
            "alt" | "option" | "opt" => Ok("alt"),
            "shift" => Ok("shift"),
            other => Err(format!("unknown modifier {other} in {spec}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lower = key.to_lowercase();
    let name = match lower.as_str() {
        "return" | "enter" | "kp_enter" => "return",
        "tab" => "tab",
        "escape" | "esc" => "escape",
        "backspace" => "backspace",
        "delete" | "del" => "delete",
        "home" => "home",
        "end" => "end",
        "page_up" | "pageup" | "prior" => "pageup",
        "page_down" | "pagedown" | "next" => "pagedown",
        "up" | "arrowup" => "up",
        "down" | "arrowdown" => "down",
        "left" | "arrowleft" => "left",
        "right" | "arrowright" => "right",
        "space" => "space",
        "minus" => "-",
        "plus" => "+",
        "equal" => "=",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        _ if lower.starts_with("kp_") && lower.len() == 4 => &lower[3..],
        _ if lower.starts_with('f')
            && lower[1..]
                .parse::<u8>()
                .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            lower.as_str()
        }
        _ if key.chars().count() == 1 => key,
        _ => return Err(format!("unknown key {key}")),
    };
    Ok((name.to_owned(), modifiers))
}

pub fn ax_action(name: &str) -> String {
    let compact: String = name
        .chars()
        .filter(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_lowercase();
    match compact.trim_start_matches("ax") {
        "press" => "press",
        "showmenu" => "show_menu",
        "pick" => "pick",
        "confirm" => "confirm",
        "cancel" => "cancel",
        "open" => "open",
        "showalternateui" => "show_alternate_ui",
        "showdefaultui" => "show_default_ui",
        _ => return name.to_owned(),
    }
    .to_owned()
}

impl Driver {
    pub fn new(install: Install) -> Self {
        Self {
            install,
            client: ("nanocode-cua".into(), env!("CARGO_PKG_VERSION").into()),
            policy: None,
            session: None,
            apps: HashMap::new(),
        }
    }

    pub fn client(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client = (name.into(), version.into());
        self
    }

    pub fn policy(
        mut self,
        policy: impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.policy = Some(Arc::new(policy));
        self
    }

    pub fn install(&self) -> &Install {
        &self.install
    }

    async fn call(&mut self, name: &str, arguments: Value) -> Result<Value, Failure> {
        if self.session.is_none() {
            self.apps.clear();
            let started = McpClient::start(self.install.command(), &self.client.0, &self.client.1)
                .await
                .map_err(|error| {
                    Failure::refused(format!(
                        "cua-driver could not start ({error}), so nothing ran."
                    ))
                })?;
            self.session = Some(started);
        }
        let Some(session) = self.session.as_ref() else {
            return Err(Failure::refused(
                "cua-driver is not running, so nothing ran.",
            ));
        };
        match session.call_tool(name, arguments).await {
            Ok(result) if result["isError"] == true => Err(Failure {
                message: text_of(&result),
                sent: true,
            }),
            Ok(result) => Ok(result),
            Err(McpError::Rpc(error)) => Err(Failure {
                message: error.message,
                sent: true,
            }),
            Err(McpError::Transport(error)) if error.is_unsent() => {
                self.session = None;
                Err(Failure::refused(format!(
                    "cua-driver stopped ({}), so nothing ran.",
                    error.message()
                )))
            }
            Err(error) => {
                self.session = None;
                Err(Failure {
                    message: format!(
                        "cua-driver stopped during this step ({error}); whether it took effect is unknown. Read the app's state before trying again."
                    ),
                    sent: true,
                })
            }
        }
    }

    pub fn reset(&mut self) {
        self.session = None;
        self.apps.clear();
    }

    pub async fn list_apps(&mut self) -> Result<Vec<Value>, Failure> {
        let result = self.call("list_apps", json!({})).await?;
        Ok(result["structuredContent"]["apps"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    pub async fn resolve(&mut self, wanted: &str) -> Result<i64, Failure> {
        let wanted = wanted.trim();
        if wanted.is_empty() {
            return Err(Failure::refused("app is required"));
        }
        let apps = self.list_apps().await?;
        let app = find_app(&apps, wanted)
            .map_err(Failure::refused)?
            .cloned()
            .ok_or_else(|| {
                Failure::refused(format!(
                    "No app matches \"{wanted}\"; list_apps shows installed apps and their bundle identifiers"
                ))
            })?;
        let bundle = app["bundle_id"].as_str().unwrap_or_default().to_owned();
        let name = app["name"].as_str().unwrap_or(wanted).to_owned();
        if let Some(policy) = &self.policy {
            policy(&name, &bundle).map_err(Failure::refused)?;
        }
        let pid = match app["pid"]
            .as_i64()
            .filter(|pid| *pid > 0 && app["running"] == true)
        {
            Some(pid) => pid,
            None => self.launch(&app, &bundle, &name).await?,
        };
        self.apps.entry(pid).or_default().name = name;
        Ok(pid)
    }

    async fn launch(&mut self, app: &Value, bundle: &str, name: &str) -> Result<i64, Failure> {
        let arguments = if bundle.is_empty() {
            json!({"launch_path": app["launch_path"]})
        } else {
            json!({"bundle_id": bundle})
        };
        let launched = self.call("launch_app", arguments).await?;
        let deadline = Instant::now() + LAUNCH;
        let mut pid = launched["structuredContent"]["pid"].as_i64();
        while Instant::now() < deadline {
            if pid.is_none() {
                let apps = self.list_apps().await?;
                pid = apps
                    .iter()
                    .find(|entry| entry["running"] == true && entry["bundle_id"] == bundle)
                    .and_then(|entry| entry["pid"].as_i64());
            }
            if let Some(pid) = pid
                && !self.windows(pid).await?.is_empty()
            {
                return Ok(pid);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(Failure {
            message: format!(
                "{name} did not open a window within {} seconds",
                LAUNCH.as_secs()
            ),
            sent: true,
        })
    }

    async fn windows(&mut self, pid: i64) -> Result<Vec<Value>, Failure> {
        let result = self.call("list_windows", json!({"pid": pid})).await?;
        Ok(result["structuredContent"]["windows"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    async fn window(&mut self, pid: i64, refresh: bool) -> Result<i64, Failure> {
        let known = self.apps.get(&pid).and_then(|app| app.window);
        if let (Some(window), false) = (known, refresh) {
            return Ok(window);
        }
        let mut windows = self.windows(pid).await?;
        let name = self
            .apps
            .get(&pid)
            .map(|app| app.name.clone())
            .unwrap_or_default();
        if windows.is_empty() {
            return Err(Failure::refused(format!(
                "{name} has no window to read; open one in the app first"
            )));
        }
        windows.sort_by_key(|window| {
            std::cmp::Reverse((
                window["is_on_screen"] == true,
                window["title"]
                    .as_str()
                    .is_some_and(|title| !title.is_empty()),
                window["z_index"].as_i64().unwrap_or(-1),
            ))
        });
        let chosen = windows
            .iter()
            .find(|window| window["window_id"].as_i64() == known && window["is_on_screen"] == true)
            .unwrap_or(&windows[0])["window_id"]
            .as_i64()
            .ok_or_else(|| Failure::refused("cua-driver listed a window without an id"))?;
        self.apps.entry(pid).or_default().window = Some(chosen);
        Ok(chosen)
    }

    async fn window_state(
        &mut self,
        pid: i64,
        mut arguments: Map<String, Value>,
    ) -> Result<Value, Failure> {
        let window = self.window(pid, false).await?;
        arguments.insert("pid".into(), pid.into());
        arguments.insert("window_id".into(), window.into());
        match self
            .call("get_window_state", Value::Object(arguments.clone()))
            .await
        {
            Ok(result) => Ok(result),
            Err(failure) if self.session.is_some() => {
                let Ok(fresh) = self.window(pid, true).await else {
                    return Err(failure);
                };
                arguments.remove("since");
                arguments.insert("window_id".into(), fresh.into());
                self.call("get_window_state", Value::Object(arguments))
                    .await
            }
            Err(failure) => Err(failure),
        }
    }

    pub async fn read(
        &mut self,
        pid: i64,
        scope: &str,
        full: bool,
        screenshot: Option<usize>,
    ) -> Result<Step, Failure> {
        let (acted, diff) = {
            let app = self.apps.entry(pid).or_default();
            let acted = app.acted.take();
            let diff = !full && app.scope == scope && app.snapshot.is_some();
            app.scope = scope.to_owned();
            (acted, diff)
        };
        if let Some(acted) = acted {
            let wait = (acted + FIRST_READ).saturating_duration_since(Instant::now());
            tokio::time::sleep(wait).await;
        }
        let mut arguments = Map::new();
        arguments.insert("include_screenshot".into(), false.into());
        if diff {
            arguments.insert("since".into(), "latest".into());
        }
        let mut result = self.window_state(pid, arguments.clone()).await?;
        if let Some(acted) = acted
            && result["structuredContent"]["since_status"] == "no_change"
            && Instant::now() < acted + SETTLE
        {
            tokio::time::sleep((acted + SETTLE).saturating_duration_since(Instant::now())).await;
            result = self.window_state(pid, arguments).await?;
        }
        let state = &result["structuredContent"];
        let app = self.apps.entry(pid).or_default();
        if let Some(snapshot) = state["snapshot_id"].as_str() {
            app.snapshot = Some(snapshot.to_owned());
        }
        let status = state["since_status"].as_str().unwrap_or_default();
        if status != "diff" && status != "no_change" {
            app.rows.clear();
        }
        for line in state["tree_markdown"]
            .as_str()
            .or(state["tree_diff"].as_str())
            .unwrap_or_default()
            .lines()
        {
            if let Some(index) = line
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .and_then(|(index, _)| index.parse::<u64>().ok())
            {
                app.rows.insert(index, line.to_owned());
            }
        }
        let title = state["window_title"].as_str().unwrap_or_default();
        let header = if title.is_empty() {
            format!("{} (pid {pid})", app.name)
        } else {
            format!("{} (pid {pid}) — window \"{title}\"", app.name)
        };
        let body = match status {
            "no_change" => "No accessibility-tree change since the previous state.".to_owned(),
            "diff" => state["tree_diff"]
                .as_str()
                .unwrap_or_default()
                .lines()
                .map(|line| {
                    line.split(" Use element_tokens")
                        .next()
                        .unwrap_or(line)
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_owned(),
            _ => state["tree_markdown"]
                .as_str()
                .map(|tree| tree.trim_end().to_owned())
                .unwrap_or_else(|| text_of(&result)),
        };
        let mut text = format!("{header}\n{body}");
        if state["truncated"] == true {
            text.push_str(&format!(
                "\n(tree cut at {} of {} elements)",
                state["returned_element_count"], state["total_element_count"]
            ));
        }
        let mut step = Step {
            text,
            ..Step::default()
        };
        if let Some(budget) = screenshot {
            for edge in IMAGE_EDGES {
                let mut arguments = Map::new();
                arguments.insert("include_screenshot".into(), true.into());
                arguments.insert("include_accessibility_tree".into(), false.into());
                arguments.insert("display_only".into(), true.into());
                arguments.insert("max_image_dimension".into(), edge.into());
                let shot = self.window_state(pid, arguments).await?;
                match image_of(&shot) {
                    Some(data) if data.len() <= budget => {
                        step.screenshot = Some(data);
                        break;
                    }
                    Some(_) => step.screenshot_omitted = true,
                    None => break,
                }
            }
            if step.screenshot.is_some() {
                step.screenshot_omitted = false;
            }
        }
        Ok(step)
    }

    fn token(&self, pid: i64, input: &Value) -> Result<String, Failure> {
        let index = input["element_index"]
            .as_u64()
            .ok_or_else(|| Failure::refused("element_index is required"))?;
        let snapshot = self
            .apps
            .get(&pid)
            .and_then(|app| app.snapshot.clone())
            .ok_or_else(|| Failure::refused("Read the app first with get_app_state; element indexes come from its latest state"))?;
        Ok(format!("{snapshot}:{index}"))
    }

    async fn act(
        &mut self,
        pid: i64,
        name: &str,
        mut arguments: Map<String, Value>,
    ) -> Result<(), Failure> {
        if !arguments.contains_key("element_token") {
            let window = self.window(pid, false).await?;
            arguments.insert("pid".into(), pid.into());
            arguments.insert("window_id".into(), window.into());
        }
        let result = self.call(name, Value::Object(arguments)).await;
        self.apps.entry(pid).or_default().acted = Some(Instant::now());
        result.map(drop)
    }

    fn point(input: &Value, x: &str, y: &str) -> Result<(f64, f64), Failure> {
        match (input[x].as_f64(), input[y].as_f64()) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(Failure::refused(format!(
                "pass element_index, or {x} and {y}"
            ))),
        }
    }

    pub async fn step(
        &mut self,
        action: &str,
        input: &Value,
        scope: &str,
        max_image: usize,
    ) -> Result<Step, Failure> {
        if action == "list_apps" {
            let mut apps = self.list_apps().await?;
            apps.retain(|app| app["bundle_id"].is_string() || app["launch_path"].is_string());
            apps.sort_by_key(|app| {
                (
                    app["running"] != true,
                    app["name"].as_str().unwrap_or_default().to_lowercase(),
                )
            });
            let text = apps
                .iter()
                .map(|app| {
                    format!(
                        "{} — {}{}",
                        app["name"].as_str().unwrap_or_default(),
                        app["bundle_id"]
                            .as_str()
                            .or(app["launch_path"].as_str())
                            .unwrap_or_default(),
                        if app["running"] == true {
                            " (running)"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(Step {
                text,
                ..Step::default()
            });
        }
        let pid = self
            .resolve(input["app"].as_str().unwrap_or_default())
            .await?;
        let wants_screenshot = input["screenshot"]
            .as_bool()
            .unwrap_or(action == "get_app_state");
        let budget = wants_screenshot.then_some(max_image);
        if action == "get_app_state" {
            return self.read(pid, scope, input["full"] == true, budget).await;
        }
        if self
            .apps
            .get(&pid)
            .is_none_or(|app| app.snapshot.is_none() || app.scope != scope)
            && input.get("element_index").is_some()
        {
            return Err(Failure::refused(
                "Read the app first with get_app_state in this turn; element indexes come from its latest state",
            ));
        }
        let mut arguments = Map::new();
        let has_index = input["element_index"].is_u64();
        match action {
            "click" => {
                let button = input["mouse_button"].as_str().unwrap_or("left");
                let count = input["click_count"].as_u64().unwrap_or(1);
                if has_index {
                    arguments.insert("element_token".into(), self.token(pid, input)?.into());
                    let tool = match (button, count) {
                        ("left", 2) => "double_click",
                        ("right", _) => "right_click",
                        _ => {
                            if button != "left" {
                                arguments.insert("button".into(), button.into());
                            }
                            if count != 1 {
                                arguments.insert("count".into(), count.into());
                            }
                            "click"
                        }
                    };
                    self.act(pid, tool, arguments).await?;
                } else {
                    let (x, y) = Self::point(input, "x", "y")?;
                    arguments.insert("x".into(), x.into());
                    arguments.insert("y".into(), y.into());
                    arguments.insert("button".into(), button.into());
                    arguments.insert("count".into(), count.into());
                    self.act(pid, "click", arguments).await?;
                }
            }
            "drag" => {
                let (from_x, from_y) = Self::point(input, "from_x", "from_y")?;
                let (to_x, to_y) = Self::point(input, "to_x", "to_y")?;
                for (key, value) in [
                    ("from_x", from_x),
                    ("from_y", from_y),
                    ("to_x", to_x),
                    ("to_y", to_y),
                ] {
                    arguments.insert(key.into(), value.into());
                }
                self.act(pid, "drag", arguments).await?;
            }
            "type_text" => {
                let text = input["text"]
                    .as_str()
                    .ok_or_else(|| Failure::refused("text is required"))?;
                arguments.insert("text".into(), text.into());
                self.act(pid, "type_text", arguments).await?;
            }
            "press_key" => {
                let (key, modifiers) =
                    chord(input["key"].as_str().unwrap_or_default()).map_err(Failure::refused)?;
                arguments.insert("key".into(), key.into());
                if !modifiers.is_empty() {
                    arguments.insert("modifiers".into(), json!(modifiers));
                }
                self.act(pid, "press_key", arguments).await?;
            }
            "set_value" => {
                arguments.insert("element_token".into(), self.token(pid, input)?.into());
                arguments.insert(
                    "value".into(),
                    input["value"].as_str().unwrap_or_default().into(),
                );
                self.act(pid, "set_value", arguments).await?;
            }
            "scroll" => {
                if has_index {
                    arguments.insert("element_token".into(), self.token(pid, input)?.into());
                } else {
                    let (x, y) = Self::point(input, "x", "y")?;
                    arguments.insert("x".into(), x.into());
                    arguments.insert("y".into(), y.into());
                }
                arguments.insert(
                    "direction".into(),
                    input["direction"].as_str().unwrap_or("down").into(),
                );
                arguments.insert("by".into(), "page".into());
                arguments.insert(
                    "amount".into(),
                    (input["pages"].as_f64().unwrap_or(1.0).round().max(1.0) as u64).into(),
                );
                self.act(pid, "scroll", arguments).await?;
            }
            "perform_secondary_action" => {
                arguments.insert("element_token".into(), self.token(pid, input)?.into());
                arguments.insert(
                    "action".into(),
                    ax_action(input["ax_action"].as_str().unwrap_or_default()).into(),
                );
                self.act(pid, "click", arguments).await?;
            }
            "select_text" => self.select_text(pid, input).await?,
            other => return Err(Failure::refused(format!("unknown action {other}"))),
        }
        self.read(pid, scope, false, budget).await
    }

    async fn select_text(&mut self, pid: i64, input: &Value) -> Result<(), Failure> {
        let index = input["element_index"]
            .as_u64()
            .ok_or_else(|| Failure::refused("element_index is required"))?;
        let text = input["text"].as_str().unwrap_or_default();
        let prefix = input["prefix"].as_str().unwrap_or_default();
        let suffix = input["suffix"].as_str().unwrap_or_default();
        let row = self
            .apps
            .get(&pid)
            .and_then(|app| app.rows.get(&index))
            .cloned()
            .unwrap_or_default();
        let value = row
            .split_once(" value=\"")
            .map(|(_, rest)| rest)
            .or_else(|| row.split_once('"').map(|(_, rest)| rest))
            .and_then(|rest| rest.split_once('"'))
            .map(|(value, _)| value.replace("\\\"", "\"").replace("\\n", "\n"))
            .ok_or_else(|| Failure::refused(format!("Element {index} shows no text value in the latest state; click into it and use press_key instead")))?;
        let wanted = format!("{prefix}{text}{suffix}");
        let found = value.find(&wanted).ok_or_else(|| {
            Failure::refused(format!("\"{text}\" was not found in element {index}"))
        })?;
        if value[found + 1..].contains(&wanted) {
            return Err(Failure::refused(format!(
                "\"{wanted}\" occurs more than once in element {index}; pass prefix or suffix to pick one"
            )));
        }
        let start = value[..found + prefix.len()].chars().count();
        let length = text.chars().count();
        let (caret, selected) = match input["selection_type"].as_str().unwrap_or("text") {
            "cursor_before" => (start, 0),
            "cursor_after" => (start + length, 0),
            _ => (start, length),
        };
        let mut steps = vec![
            json!({"tool": "click", "args": {"element_token": self.token(pid, input)?}}),
            json!({"tool": "press_key", "args": {"key": "up", "modifiers": ["cmd"]}}),
        ];
        steps.extend((0..caret).map(|_| json!({"tool": "press_key", "args": {"key": "right"}})));
        steps.extend((0..selected).map(
            |_| json!({"tool": "press_key", "args": {"key": "right", "modifiers": ["shift"]}}),
        ));
        if steps.len() > MAX_KEY_STEPS {
            return Err(Failure::refused(format!(
                "The text is too far into element {index} to select with keys; click near it instead"
            )));
        }
        let window = self.window(pid, false).await?;
        for chunk in steps.chunks(STEPS_PER_CALL) {
            let result = self
                .call(
                    "run_steps",
                    json!({"pid": pid, "window_id": window, "steps": chunk}),
                )
                .await;
            self.apps.entry(pid).or_default().acted = Some(Instant::now());
            result?;
        }
        Ok(())
    }
}

pub struct Leases {
    idle: Duration,
    held: HashMap<String, Lease>,
}

struct Lease {
    session: String,
    turn: String,
    used: Instant,
}

impl Leases {
    pub fn new(idle: Duration) -> Self {
        Self {
            idle,
            held: HashMap::new(),
        }
    }

    pub fn take(&mut self, app: &str, session: &str, turn: &str, now: Instant) -> bool {
        let idle = self.idle;
        self.held
            .retain(|_, lease| now.duration_since(lease.used) < idle);
        let key = app_key(app);
        if self
            .held
            .get(&key)
            .is_some_and(|lease| lease.session != session)
        {
            return false;
        }
        self.held.insert(
            key,
            Lease {
                session: session.to_owned(),
                turn: turn.to_owned(),
                used: now,
            },
        );
        true
    }

    pub fn end(&mut self, session: &str, turn: &str) {
        self.held
            .retain(|_, lease| lease.session != session || lease.turn != turn);
    }
}

pub fn app_key(app: &str) -> String {
    let name = app.trim().trim_end_matches('/');
    let name = name.rsplit('/').next().unwrap_or(name).to_lowercase();
    let name = name.strip_suffix(".app").unwrap_or(&name);
    match name {
        "chrome" | "com.google.chrome" => "google chrome",
        "chrome beta" | "com.google.chrome.beta" => "google chrome beta",
        "chrome canary" | "com.google.chrome.canary" => "google chrome canary",
        "chromium" | "org.chromium.chromium" => "chromium",
        "arc" | "company.thebrowser.browser" => "arc",
        "brave" | "com.brave.browser" => "brave browser",
        "edge" | "com.microsoft.edgemac" => "microsoft edge",
        "safari" | "com.apple.safari" => "safari",
        "firefox" | "org.mozilla.firefox" => "firefox",
        "dia" | "company.thebrowser.dia" => "dia",
        other => other,
    }
    .to_owned()
}

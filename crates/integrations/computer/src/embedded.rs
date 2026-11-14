use crate::{
    App, Backend, Limits, Native, Target,
    engine::{Change, Engine},
    macos::{self, Ax},
    tool::STOPPED,
};
use nanocode_io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use nanocode_mcp::RpcError;
use nanocode_sync::Owned;
use serde_json::{Map, Value, json};
use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    sync::Arc,
};

const PROTOCOL_VERSION: &str = "2025-06-18";
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INTERNAL_ERROR: i64 = -32603;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub accessibility: bool,
    pub screen_recording: bool,
}

pub fn permissions() -> Permissions {
    Permissions {
        accessibility: macos::trusted(),
        screen_recording: macos::screen_recording(),
    }
}

pub fn request_permissions() -> Permissions {
    Permissions {
        accessibility: macos::request_trust(),
        screen_recording: macos::request_screen_recording(),
    }
}

struct Shim {
    engine: Engine<Ax>,
    epochs: HashMap<i64, u64>,
    epoch: u64,
}

pub struct Server {
    shim: Arc<Owned<Shim>>,
    tools: Value,
}

impl Server {
    pub async fn start(_label: &str) -> Result<Self, String> {
        Ok(Self {
            shim: Arc::new(Owned::new(Shim {
                engine: Native::default().pointer(true).engine(Ax::shown()),
                epochs: HashMap::new(),
                epoch: 0,
            })),
            tools: tools(),
        })
    }

    pub const fn tools(&self) -> &Value {
        &self.tools
    }

    pub async fn request(&self, message: Value) -> Option<Value> {
        let method = message["method"].as_str();
        let id = message.get("id").cloned();
        let result = match (method, &id) {
            (Some(_), None) => return None,
            (None, _) => Err((INVALID_REQUEST, "Invalid Request: no method".to_owned())),
            (Some("initialize"), Some(_)) => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "nanocode-cua-driver", "version": env!("CARGO_PKG_VERSION")},
            })),
            (Some("ping"), Some(_)) => Ok(json!({})),
            (Some("tools/list"), Some(_)) => Ok(self.tools.clone()),
            (Some("tools/call"), Some(_)) => self
                .call_tool(
                    message["params"]["name"].as_str().unwrap_or_default(),
                    message["params"]["arguments"].clone(),
                )
                .await
                .map_err(|error| (error.code, error.message)),
            (Some(other), Some(_)) => Err((METHOD_NOT_FOUND, format!("Method not found: {other}"))),
        };
        let id = id.unwrap_or(Value::Null);
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        })
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, RpcError> {
        let mut shim = self.shim.clone().acquire_owned().await;
        let name = name.to_owned();
        match nanocode_platform::spawn_blocking(move || shim.call(&name, &arguments)).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Ok(json!({
                "content": [{"type": "text", "text": message}],
                "isError": true,
            })),
            Err(_) => Err(RpcError::new(INTERNAL_ERROR, STOPPED)),
        }
    }
}

fn done(text: impl Into<String>, structured: Value) -> Value {
    json!({
        "content": [{"type": "text", "text": text.into()}],
        "structuredContent": structured,
    })
}

fn record(app: &App, running: bool) -> Value {
    let mut record = json!({"name": app.name, "launch_path": app.path, "running": running});
    if !app.id.is_empty() {
        record["bundle_id"] = app.id.clone().into();
    }
    if running {
        record["pid"] = app.pid.into();
    }
    record
}

fn secondary(action: &str) -> String {
    match action {
        "press" => "AXPress",
        "show_menu" => "AXShowMenu",
        "pick" => "AXPick",
        "confirm" => "AXConfirm",
        "cancel" => "AXCancel",
        "open" => "AXOpen",
        "show_alternate_ui" => "AXShowAlternateUI",
        "show_default_ui" => "AXShowDefaultUI",
        other => other,
    }
    .to_owned()
}

impl Shim {
    fn call(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "list_apps" => Ok(self.list_apps()),
            "launch_app" => self.launch(args),
            "list_windows" => Ok(self.windows(args)),
            "get_window_state" => self.window_state(args),
            "clipboard_read" => clipboard_read(),
            "clipboard_write" => clipboard_write(args),
            "press_key" => self.press_key(args),
            "type_text" => self.type_text(args),
            "drag" => self.drag(args),
            "click" | "double_click" | "right_click" | "set_value" | "select_text" | "scroll" => {
                self.element(name, args)
            }
            other => Err(format!(
                "nanocode-cua-driver has no {other} tool, so nothing ran."
            )),
        }
    }

    fn list_apps(&mut self) -> Value {
        let running: Vec<App> = self
            .engine
            .backend()
            .apps()
            .into_iter()
            .filter(|app| !app.background || app.windows)
            .collect();
        let installed = macos::catalog().into_iter().filter(|app| {
            !app.background
                && !running
                    .iter()
                    .any(|other| other.path == app.path || !app.id.is_empty() && other.id == app.id)
        });
        let apps: Vec<Value> = running
            .iter()
            .map(|app| record(app, true))
            .chain(installed.map(|app| record(&app, false)))
            .collect();
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
        done(text, json!({"apps": apps}))
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        let wanted = ["bundle_id", "launch_path", "name"]
            .into_iter()
            .find_map(|key| args[key].as_str().filter(|value| !value.trim().is_empty()))
            .ok_or("launch_app needs bundle_id, launch_path or name, so nothing ran.")?;
        let app = self.engine.open(wanted)?;
        Ok(done(
            format!("{} runs with pid {}.", app.name, app.pid),
            json!({"pid": app.pid, "name": app.name, "bundle_id": app.id}),
        ))
    }

    fn windows(&mut self, args: &Value) -> Value {
        let wanted = args["pid"].as_i64();
        let windows: Vec<Value> = self
            .engine
            .backend()
            .apps()
            .into_iter()
            .filter(|app| wanted.map_or(app.windows, |pid| app.pid == pid))
            .filter_map(|app| {
                let window = macos::window(app.pid)?;
                Some(json!({
                    "window_id": macos::window_id(&window)?,
                    "pid": app.pid,
                    "app_name": app.name,
                    "title": macos::title(&window).unwrap_or_default(),
                    "is_on_screen": app.windows,
                }))
            })
            .collect();
        let text = windows
            .iter()
            .map(|window| {
                format!(
                    "{} {} (pid {}) {}",
                    window["window_id"], window["app_name"], window["pid"], window["title"]
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        done(text, json!({"windows": windows}))
    }

    fn snapshot(&mut self, pid: i64, change: Change) -> String {
        let epoch = match (change, self.epochs.get(&pid)) {
            (Change::Diff | Change::Same, Some(&epoch)) => epoch,
            _ => {
                self.epoch += 1;
                self.epochs.insert(pid, self.epoch);
                self.epoch
            }
        };
        format!("{pid}.{epoch}")
    }

    fn token(&self, token: &str) -> Result<(i64, u64), String> {
        let malformed =
            || format!("{token} is not an element token from get_window_state, so nothing ran.");
        let (snapshot, index) = token.rsplit_once(':').ok_or_else(malformed)?;
        let (pid, epoch) = snapshot.split_once('.').ok_or_else(malformed)?;
        let pid = pid.parse::<i64>().map_err(|_| malformed())?;
        let epoch = epoch.parse::<u64>().map_err(|_| malformed())?;
        let index = index.parse::<u64>().map_err(|_| malformed())?;
        if self.epochs.get(&pid) != Some(&epoch) {
            return Err(format!(
                "{token} is from an earlier state of that app whose indexes have changed since, so nothing ran. Read it again with get_window_state."
            ));
        }
        Ok((pid, index))
    }

    fn app(&mut self, args: &Value) -> Result<App, String> {
        let pid = args["pid"]
            .as_i64()
            .ok_or("pid is required, so nothing ran.")?;
        self.engine.running(pid)
    }

    fn window_state(&mut self, args: &Value) -> Result<Value, String> {
        let app = self.app(args)?;
        let tree = args["include_accessibility_tree"] != false && args["display_only"] != true;
        let mut structured = Map::new();
        let mut text = Vec::new();
        let window = if tree {
            let view = self.engine.observe(&app, args["since"] == "latest")?;
            let body = [&view.heading[1..], view.body.as_slice()]
                .concat()
                .join("\n");
            let (status, key) = match view.change {
                Change::Whole => ("full", "tree_markdown"),
                Change::Diff => ("diff", "tree_diff"),
                Change::Same => ("no_change", "tree_diff"),
            };
            structured.insert(
                "snapshot_id".into(),
                self.snapshot(app.pid, view.change).into(),
            );
            structured.insert("since_status".into(), status.into());
            structured.insert(key.into(), body.into());
            if let Some(title) = &view.title {
                structured.insert("window_title".into(), title.clone().into());
            }
            text.push(view.text());
            view.window
        } else {
            macos::window(app.pid)
        };
        let mut content = Vec::new();
        if args["include_screenshot"] == true {
            let shot = match &window {
                Some(window) => {
                    self.engine
                        .backend()
                        .screenshot(&app, window, Limits::default().screenshot)
                }
                None => Err(format!("{} has no window to capture", app.name)),
            };
            match shot {
                Ok(Some(data)) => {
                    content.push(json!({"type": "image", "data": data, "mimeType": "image/jpeg"}));
                }
                Ok(None) => {
                    structured.insert("screenshot_omitted".into(), true.into());
                    text.push("(screenshot omitted: too large)".into());
                }
                Err(why) => text.push(format!("(no screenshot: {why})")),
            }
        }
        content.insert(0, json!({"type": "text", "text": text.join("\n")}));
        Ok(json!({"content": content, "structuredContent": structured}))
    }

    fn press_key(&mut self, args: &Value) -> Result<Value, String> {
        let app = self.app(args)?;
        let key = args["key"]
            .as_str()
            .ok_or("key is required, so nothing was pressed.")?;
        let key = if key.eq_ignore_ascii_case("delete") {
            "forwarddelete"
        } else {
            key
        };
        let chord: Vec<&str> = args["modifiers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .chain([key])
            .collect();
        self.engine.refocus(&app, macos::focused(app.pid));
        let pressed = self
            .engine
            .perform("press_key", &json!({"key": chord.join("+")}), &app)?;
        Ok(done(pressed, json!({})))
    }

    fn type_text(&mut self, args: &Value) -> Result<Value, String> {
        let app = self.app(args)?;
        let text = args["text"]
            .as_str()
            .ok_or("text is required, so nothing was typed.")?;
        let focused = macos::focused(app.pid);
        if focused.is_none() {
            macos::type_keys(app.pid, text);
            return Ok(done("Typed text as keys.", json!({})));
        }
        self.engine.refocus(&app, focused);
        let typed = self
            .engine
            .perform("type_text", &json!({"text": text}), &app)?;
        Ok(done(typed, json!({})))
    }

    fn drag(&mut self, args: &Value) -> Result<Value, String> {
        let app = self.app(args)?;
        let point = |x: &str, y: &str| match (args[x].as_f64(), args[y].as_f64()) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => Err(format!("drag needs {x} and {y}, so nothing ran.")),
        };
        let pointer = crate::Pointer::Drag {
            from: point("from_x", "from_y")?,
            to: point("to_x", "to_y")?,
        };
        let window = macos::window(app.pid);
        let dragged = self
            .engine
            .backend()
            .pointer(&app, window.as_ref(), pointer)?;
        Ok(done(dragged, json!({})))
    }

    fn element(&mut self, name: &str, args: &Value) -> Result<Value, String> {
        let (app, index) = match args["element_token"].as_str() {
            Some(token) => {
                let (pid, index) = self.token(token)?;
                (self.engine.running(pid)?, Some(index))
            }
            None => (self.app(args)?, None),
        };
        let mut input = Map::new();
        if let Some(index) = index {
            input.insert("element_index".into(), index.into());
        }
        let text = |key: &str| args[key].as_str().unwrap_or_default().to_owned();
        let action = match name {
            "set_value" => {
                input.insert("value".into(), text("value").into());
                "set_value"
            }
            "select_text" => {
                for key in ["text", "prefix", "suffix", "selection_type"] {
                    input.insert(key.into(), args[key].clone());
                }
                "select_text"
            }
            "scroll" => {
                if args["by"] == "pixel" || index.is_none() {
                    return self.scroll(&app, index, args);
                }
                input.insert("direction".into(), args["direction"].clone());
                input.insert("pages".into(), args["amount"].clone());
                "scroll"
            }
            _ if args["action"].is_string() => {
                input.insert("ax_action".into(), secondary(&text("action")).into());
                "perform_secondary_action"
            }
            _ => {
                if args["modifiers"]
                    .as_array()
                    .is_some_and(|keys| !keys.is_empty())
                {
                    return Err(
                        "Clicks with modifier keys are not supported here, so nothing ran.".into(),
                    );
                }
                let (button, count) = match name {
                    "double_click" => ("left".to_owned(), 2),
                    "right_click" => ("right".to_owned(), 1),
                    _ => (
                        args["button"].as_str().unwrap_or("left").to_owned(),
                        args["count"].as_u64().unwrap_or(1),
                    ),
                };
                if index.is_none() {
                    return self.click(&app, args, &button, count);
                }
                input.insert("mouse_button".into(), button.into());
                input.insert("click_count".into(), count.into());
                "click"
            }
        };
        let acted = self.engine.perform(action, &Value::Object(input), &app)?;
        Ok(done(acted, json!({})))
    }

    fn click(
        &mut self,
        app: &App,
        args: &Value,
        button: &str,
        count: u64,
    ) -> Result<Value, String> {
        let (Some(x), Some(y)) = (args["x"].as_f64(), args["y"].as_f64()) else {
            return Err("click needs element_token, or x and y, so nothing ran.".into());
        };
        let button = match button {
            "right" => crate::Button::Right,
            "middle" => crate::Button::Middle,
            _ => crate::Button::Left,
        };
        let count = u32::try_from(count)
            .map_err(|_| format!("{count} clicks is not a click count, so nothing ran."))?;
        let window = macos::window(app.pid);
        let clicked = self.engine.backend().pointer(
            app,
            window.as_ref(),
            crate::Pointer::Click {
                at: Target::Point(x, y),
                button,
                count,
            },
        )?;
        Ok(done(clicked, json!({})))
    }

    fn scroll(&mut self, app: &App, index: Option<u64>, args: &Value) -> Result<Value, String> {
        let window = macos::window(app.pid);
        let amount = args["amount"].as_f64().unwrap_or(1.0);
        let (horizontal, vertical) = if args["by"] == "pixel" {
            (amount, amount)
        } else {
            let (width, height) = macos::page(window.as_ref()).ok_or(
                "The app's window has no place on screen to scroll by pages, so nothing ran.",
            )?;
            (width * amount, height * amount)
        };
        let (horizontal, vertical) = match args["direction"].as_str().unwrap_or("down") {
            "up" => (0.0, vertical),
            "left" => (horizontal, 0.0),
            "right" => (-horizontal, 0.0),
            _ => (0.0, -vertical),
        };
        let element = match index {
            Some(index) => Some(self.engine.element(app, index)?),
            None => None,
        };
        let at = match &element {
            Some(element) => Target::Element(element),
            None => match (args["x"].as_f64(), args["y"].as_f64()) {
                (Some(x), Some(y)) => Target::Point(x, y),
                _ => return Err("scroll needs element_token, or x and y, so nothing ran.".into()),
            },
        };
        let scrolled = macos::scroll(app.pid, window.as_ref(), at, horizontal, vertical)?;
        Ok(done(scrolled, json!({})))
    }
}

fn pasteboard(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env("LANG", "en_US.UTF-8");
    command
}

fn clipboard_read() -> Result<Value, String> {
    let output = pasteboard("/usr/bin/pbpaste")
        .output()
        .map_err(|error| format!("The clipboard could not be read ({error})."))?;
    if !output.status.success() {
        return Err(format!(
            "The clipboard could not be read (pbpaste {}).",
            output.status
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "The clipboard's text is not UTF-8.".to_owned())?;
    Ok(done(text.clone(), json!({"text": text})))
}

fn clipboard_write(args: &Value) -> Result<Value, String> {
    let status = match (args["text"].as_str(), args["image_path"].as_str()) {
        (Some(text), _) => {
            let mut child = pasteboard("/usr/bin/pbcopy")
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|error| format!("The clipboard could not be written ({error})."))?;
            let written = child
                .stdin
                .take()
                .map(|mut stdin| stdin.write_all(text.as_bytes()));
            let status = child
                .wait()
                .map_err(|error| format!("The clipboard could not be written ({error})."))?;
            if let Some(Err(error)) = written {
                return Err(format!("The clipboard could not be written ({error})."));
            }
            status
        }
        (None, Some(path)) => {
            let lower = path.to_lowercase();
            let class = if lower.ends_with(".png") {
                "«class PNGf»"
            } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
                "«class JPEG»"
            } else {
                return Err(format!(
                    "{path} is not a PNG or JPEG image, so nothing was written."
                ));
            };
            Command::new("/usr/bin/osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    &format!(
                        "set the clipboard to (read (POSIX file (item 1 of argv)) as {class})"
                    ),
                    "-e",
                    "end run",
                    path,
                ])
                .stdout(Stdio::null())
                .status()
                .map_err(|error| format!("The clipboard could not be written ({error})."))?
        }
        (None, None) => {
            return Err("clipboard_write needs text or image_path, so nothing was written.".into());
        }
    };
    if !status.success() {
        return Err(format!("The clipboard could not be written ({status})."));
    }
    Ok(done("Wrote the clipboard.", json!({})))
}

fn tools() -> Value {
    let object = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required});
    let token = json!({"type": "string", "description": "<snapshot_id>:<index> from the window's latest get_window_state"});
    let integer = json!({"type": "integer"});
    let number = json!({"type": "number"});
    let string = json!({"type": "string"});
    let boolean = json!({"type": "boolean"});
    let direction = json!({"type": "string", "enum": ["up", "down", "left", "right"]});
    let tool = |name: &str, description: &str, schema: Value| json!({"name": name, "description": description, "inputSchema": schema});
    json!({"tools": [
        tool("list_apps", "Running apps with their pids, and installed apps, with bundle identifiers and paths.", object(json!({}), &[])),
        tool("launch_app", "Start an app in the background, by bundle identifier, path or name, and return its pid.", object(json!({"bundle_id": string, "launch_path": string, "name": string}), &[])),
        tool("list_windows", "The window each app's state is read from (its focused, else main window), for one pid or every app with a window on screen.", object(json!({"pid": integer}), &[])),
        tool("get_window_state", "The app's window as an accessibility tree with element indexes, or what changed since its latest state, and optionally a JPEG screenshot.", object(json!({"pid": integer, "window_id": integer, "include_accessibility_tree": boolean, "include_screenshot": boolean, "display_only": boolean, "since": {"type": "string", "enum": ["latest"]}}), &["pid"])),
        tool("click", "Click an element (its primary action, or action), or the window at screenshot coordinates.", object(json!({"element_token": token, "pid": integer, "window_id": integer, "x": number, "y": number, "button": {"type": "string", "enum": ["left", "right", "middle"]}, "count": integer, "action": string}), &[])),
        tool("double_click", "Double-click an element.", object(json!({"element_token": token}), &["element_token"])),
        tool("right_click", "Open an element's menu.", object(json!({"element_token": token}), &["element_token"])),
        tool("drag", "Drag between two points of the window's screenshot.", object(json!({"pid": integer, "window_id": integer, "from_x": number, "from_y": number, "to_x": number, "to_y": number}), &["pid", "from_x", "from_y", "to_x", "to_y"])),
        tool("press_key", "Press a key with modifiers in the app.", object(json!({"pid": integer, "window_id": integer, "key": string, "modifiers": {"type": "array", "items": {"type": "string", "enum": ["cmd", "ctrl", "alt", "shift"]}}}), &["pid", "key"])),
        tool("type_text", "Type text into the app's focused element.", object(json!({"pid": integer, "window_id": integer, "text": string}), &["pid", "text"])),
        tool("set_value", "Set an element's value.", object(json!({"element_token": token, "value": string}), &["element_token", "value"])),
        tool("select_text", "Select text in an element, or put the caret before or after it.", object(json!({"element_token": token, "text": string, "prefix": string, "suffix": string, "selection_type": {"type": "string", "enum": ["text", "cursor_before", "cursor_after"]}}), &["element_token", "text"])),
        tool("scroll", "Scroll an element or the window at screenshot coordinates, by pages or pixels.", object(json!({"element_token": token, "pid": integer, "window_id": integer, "x": number, "y": number, "direction": direction, "by": {"type": "string", "enum": ["page", "pixel"]}, "amount": number}), &["direction"])),
        tool("clipboard_read", "The clipboard's text.", object(json!({"include_text": boolean}), &[])),
        tool("clipboard_write", "Put text, or a PNG or JPEG image file, on the clipboard.", object(json!({"text": string, "image_path": string}), &[])),
    ]})
}

pub async fn serve_stdio() -> Result<(), String> {
    let server = Server::start("mcp").await?;
    let mut reader = BufReader::new(nanocode_executor::fs::stdin());
    let mut writer = nanocode_executor::fs::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?
            == 0
        {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line.trim()) {
            Ok(message) => server.request(message).await,
            Err(error) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": PARSE_ERROR, "message": format!("Parse error: {error}")},
            })),
        };
        let Some(response) = response else { continue };
        let mut bytes = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        writer
            .write_all(&bytes)
            .await
            .map_err(|error| error.to_string())?;
        writer.flush().await.map_err(|error| error.to_string())?;
    }
}

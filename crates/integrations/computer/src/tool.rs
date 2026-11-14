use crate::{
    backend::Capabilities,
    driver::Driver,
    engine::{Desktop, Engine},
    mcp::Mcp,
    policy::{Leases, Policy, app_key},
};
use nanocode_sync::{Atom, Owned};
use nanocode_tool_contracts::{
    Tool, ToolContext, ToolDefinition, ToolError, ToolInput, ToolOutput, ToolResult,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::timeout;

pub const ACTIONS: [&str; 10] = [
    "list_apps",
    "get_app_state",
    "click",
    "drag",
    "type_text",
    "press_key",
    "set_value",
    "select_text",
    "scroll",
    "perform_secondary_action",
];

const TOO_LARGE: &str = "The window's screenshot was too large to return; use the state.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub step: Duration,
    pub start: Duration,
    pub input: usize,
    pub state: usize,
    pub screenshot: usize,
    pub lease_idle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(45),
            start: Duration::from_secs(10),
            input: 32 * 1024,
            state: 192 * 1024,
            screenshot: 480 * 1024,
            lease_idle: Duration::from_secs(180),
        }
    }
}

impl Limits {
    pub(crate) fn clip(&self, mut text: String) -> String {
        if text.len() > self.state {
            text.truncate(text.floor_char_boundary(self.state));
            text.push_str(&format!(
                "\n(state cut at {} KiB; read a smaller part of the app)",
                self.state / 1024
            ));
        }
        text
    }

    pub(crate) fn slow(&self) -> String {
        format!(
            "Computer use did not finish this step within {} seconds; whether it took effect is unknown. Read the app's state before trying again.",
            self.step.as_secs()
        )
    }
}

pub(crate) const STOPPED: &str = "Computer use stopped during this step; whether it took effect is unknown. Read the app's state before trying again.";

#[derive(Clone, Default)]
pub struct Native {
    policy: Option<Policy>,
    pointer: bool,
    host: Option<String>,
}

impl Native {
    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.policy = Some(policy);
        self
    }

    #[must_use]
    pub const fn pointer(mut self, allowed: bool) -> Self {
        self.pointer = allowed;
        self
    }

    #[must_use]
    pub fn host(mut self, name: impl Into<String>) -> Self {
        self.host = Some(name.into());
        self
    }

    pub fn engine<B: crate::Backend>(self, backend: B) -> Engine<B> {
        let mut engine = Engine::new(backend).pointer(self.pointer);
        if let Some(policy) = self.policy {
            engine = engine.policy(policy);
        }
        match self.host {
            Some(host) => engine.host(host),
            None => engine,
        }
    }
}

type Shared = Arc<Owned<Box<dyn Desktop>>>;

type Make = Box<dyn Fn() -> Option<Box<dyn Desktop>> + Send + Sync>;

#[derive(Clone)]
struct Lane {
    desktop: Shared,
    used: Instant,
}

pub struct Lanes {
    front: Shared,
    make: Option<Make>,
    apps: Atom<HashMap<String, Lane>>,
}

impl Lanes {
    fn new(desktop: Box<dyn Desktop>, make: Option<Make>) -> Self {
        Self {
            front: Arc::new(Owned::new(desktop)),
            make,
            apps: Atom::new(HashMap::new()),
        }
    }

    fn lane(&self, key: &str, idle: Duration) -> Shared {
        let Some(make) = &self.make else {
            return self.front.clone();
        };
        let now = Instant::now();
        let key = app_key(key);
        self.apps.update(|apps| {
            apps.retain(|_, lane| {
                Arc::strong_count(&lane.desktop) > 1 || now.duration_since(lane.used) < idle
            });
            let lane = match apps.get_mut(&key) {
                Some(lane) => lane,
                None => match make() {
                    Some(desktop) => apps.entry(key.clone()).or_insert(Lane {
                        desktop: Arc::new(Owned::new(desktop)),
                        used: now,
                    }),
                    None => return self.front.clone(),
                },
            };
            lane.used = now;
            lane.desktop.clone()
        })
    }
}

pub enum Provider {
    Native {
        lanes: Arc<Lanes>,
        capabilities: Capabilities,
    },
    Driver(Owned<Driver>),
    Mcp(Mcp),
    Missing(String),
}

impl Provider {
    pub fn native(native: Native) -> Self {
        match crate::platform(native.clone()) {
            Ok(desktop) => Self::lanes(
                desktop,
                Some(Box::new(move || crate::platform(native.clone()).ok())),
            ),
            Err(missing) => Self::Missing(missing),
        }
    }

    pub fn desktop(desktop: Box<dyn Desktop>) -> Self {
        Self::lanes(desktop, None)
    }

    fn lanes(desktop: Box<dyn Desktop>, make: Option<Make>) -> Self {
        Self::Native {
            capabilities: desktop.capabilities(),
            lanes: Arc::new(Lanes::new(desktop, make)),
        }
    }

    pub fn driver(driver: Driver) -> Self {
        Self::Driver(Owned::new(driver))
    }

    pub const fn mcp(mcp: Mcp) -> Self {
        Self::Mcp(mcp)
    }

    pub fn missing(reason: impl Into<String>) -> Self {
        Self::Missing(reason.into())
    }

    pub const fn capabilities(&self) -> Capabilities {
        match self {
            Self::Native { capabilities, .. } => *capabilities,
            Self::Driver(_) | Self::Mcp(_) => Capabilities {
                pointer: true,
                screenshots: true,
                diffs: true,
                policy: true,
            },
            Self::Missing(_) => Capabilities {
                pointer: false,
                screenshots: false,
                diffs: false,
                policy: false,
            },
        }
    }

    async fn lease_key(&self, app: &str, limits: &Limits) -> Option<String> {
        let Self::Native { lanes, .. } = self else {
            return None;
        };
        let mut engine = timeout(limits.step, lanes.front.clone().acquire_owned())
            .await
            .ok()?;
        let app = app.to_owned();
        let key = nanocode_platform::spawn_blocking(move || engine.lease_key(&app));
        timeout(limits.step, key).await.ok()?.ok()?
    }

    async fn step(
        &self,
        action: &str,
        input: &Value,
        scope: &str,
        key: Option<&str>,
        limits: &Limits,
    ) -> ToolOutput {
        match self {
            Self::Missing(reason) => ToolOutput::error(reason.clone()),
            Self::Native { lanes, .. } => {
                let desktop = lanes.lane(key.unwrap_or_default(), limits.lease_idle);
                native(&desktop, action, input, limits).await
            }
            Self::Driver(driver) => {
                let mut driver = driver.acquire().await;
                let step = driver.step(action, input, scope, limits.screenshot);
                let step = match timeout(limits.step, step).await {
                    Ok(Ok(step)) => step,
                    Ok(Err(failure)) => return ToolOutput::error(failure.message),
                    Err(_) => {
                        driver.reset();
                        return ToolOutput::error(limits.slow());
                    }
                };
                let media_type = step.screenshot_mime.unwrap_or_else(|| "image/png".into());
                let screenshot = step
                    .screenshot
                    .map(|data| json!({"media_type": media_type, "data": data}));
                stepped(
                    input,
                    limits.clip(step.text),
                    screenshot,
                    step.screenshot_omitted,
                )
            }
            Self::Mcp(mcp) => mcp.step(action, input, scope, limits).await,
        }
    }
}

async fn native(desktop: &Shared, action: &str, input: &Value, limits: &Limits) -> ToolOutput {
    let Ok(mut engine) = timeout(limits.step, desktop.clone().acquire_owned()).await else {
        return ToolOutput::error(format!(
            "Computer use was busy with other steps for {} seconds, so nothing ran. Try again.",
            limits.step.as_secs()
        ));
    };
    let (owned_action, owned_input, max) = (action.to_owned(), input.clone(), limits.screenshot);
    let step =
        nanocode_platform::spawn_blocking(move || engine.step(&owned_action, &owned_input, max));
    match timeout(limits.step, step).await {
        Ok(Ok(Ok(step))) => {
            let screenshot = step
                .screenshot
                .map(|data| json!({"media_type": "image/jpeg", "data": data}));
            stepped(input, limits.clip(step.text), screenshot, step.omitted)
        }
        Ok(Ok(Err(message))) => ToolOutput::error(limits.clip(message)),
        Ok(Err(_)) => ToolOutput::error(STOPPED),
        Err(_) => ToolOutput::error(limits.slow()),
    }
}

type Refusal = Arc<dyn Fn(&str) -> String + Send + Sync>;

pub struct Computer {
    providers: Vec<(String, Provider)>,
    leases: Owned<Leases>,
    limits: Limits,
    refusal: Refusal,
    definition: Option<ToolDefinition>,
}

impl Computer {
    pub fn new(provider: Provider) -> Self {
        let limits = Limits::default();
        Self {
            providers: vec![(String::new(), provider)],
            leases: Owned::new(Leases::new(limits.lease_idle)),
            limits,
            refusal: Arc::new(|app| {
                format!(
                    "Another session is operating {app} on this computer, so nothing ran: both would act on the same windows and tabs. Try again once that session's turn ends, or continue without {app}."
                )
            }),
            definition: None,
        }
    }

    #[must_use]
    pub fn or(mut self, name: impl Into<String>, provider: Provider) -> Self {
        self.providers.push((name.into(), provider));
        self
    }

    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Self {
        self.leases = Owned::new(Leases::new(limits.lease_idle));
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn refusal(mut self, refusal: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        self.refusal = Arc::new(refusal);
        self
    }

    #[must_use]
    pub fn definition(mut self, definition: ToolDefinition) -> Self {
        self.definition = Some(definition);
        self
    }

    pub fn capabilities(&self) -> Capabilities {
        self.providers
            .first()
            .map_or_else(Capabilities::default, |(_, provider)| {
                provider.capabilities()
            })
    }

    fn provider(&self, input: &Value) -> &Provider {
        let wanted = input["_provider"].as_str().unwrap_or_default();
        self.providers
            .iter()
            .find(|(name, _)| !wanted.is_empty() && name == wanted)
            .or_else(|| self.providers.first())
            .map(|(_, provider)| provider)
            .unwrap_or(&MISSING)
    }

    async fn key(&self, input: &Value) -> Option<String> {
        let app = input["app"].as_str().filter(|app| !app.trim().is_empty())?;
        Some(
            self.provider(input)
                .lease_key(app, &self.limits)
                .await
                .unwrap_or_else(|| app.to_owned()),
        )
    }

    pub async fn step(&self, input: &Value, scope: &str) -> ToolOutput {
        let key = self.key(input).await;
        self.keyed(input, scope, key.as_deref()).await
    }

    async fn keyed(&self, input: &Value, scope: &str, key: Option<&str>) -> ToolOutput {
        let action = input["action"].as_str().unwrap_or_default();
        if !ACTIONS.contains(&action) {
            return ToolOutput::error(format!("action must be one of {}", ACTIONS.join(", ")));
        }
        self.provider(input)
            .step(action, input, scope, key, &self.limits)
            .await
    }
}

static MISSING: Provider = Provider::Missing(String::new());

impl Tool for Computer {
    fn definition(&self) -> ToolDefinition {
        self.definition
            .clone()
            .unwrap_or_else(|| definition(self.capabilities()))
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input: Value = input.decode_json()?;
                if !input.is_object() || input.to_string().len() > self.limits.input {
                    return Ok(ToolOutput::error(format!(
                        "expected computer arguments of at most {} KiB",
                        self.limits.input / 1024
                    )));
                }
                let key = self.key(&input).await;
                if let Some(key) = &key
                    && !self.leases.acquire().await.take(
                        key,
                        context.session_id(),
                        context.turn_id().unwrap_or_default(),
                        Instant::now(),
                    )
                {
                    return Ok(ToolOutput::error((self.refusal)(
                        input["app"].as_str().unwrap_or(key),
                    )));
                }
                let scope = json!([context.session_id(), context.turn_id()]).to_string();
                Ok(self.keyed(&input, &scope, key.as_deref()).await)
            };
            output
        })
    }

    fn end_turn<'life0, 'life1, 'life2, 'life3, 'async_trait>(
        &'life0 self,
        session_id: &'life1 str,
        turn_id: &'life2 str,
        _: &'life3 str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        'life3: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: Result<(), ToolError> = {
                self.leases.acquire().await.end(session_id, turn_id);
                Ok(())
            };
            output
        })
    }
}

pub(crate) fn stepped(
    input: &Value,
    state: String,
    screenshot: Option<Value>,
    omitted: bool,
) -> ToolOutput {
    let mut structured = json!({"state": state});
    if let Some(screenshot) = screenshot {
        structured["screenshot"] = screenshot;
    } else if omitted {
        structured["screenshot_omitted"] = TOO_LARGE.into();
    }
    ToolOutput::text(step_line(input)).with_structured_result(structured)
}

pub fn step_line(input: &Value) -> String {
    let app = input["app"].as_str().unwrap_or("the app");
    let target = input["element_index"].as_i64().map_or_else(
        || match (input["x"].as_f64(), input["y"].as_f64()) {
            (Some(x), Some(y)) => format!("({x:.0}, {y:.0})"),
            _ => "the app".into(),
        },
        |index| format!("element {index}"),
    );
    let line = match input["action"].as_str().unwrap_or_default() {
        "list_apps" => return "Listed apps".into(),
        "get_app_state" => return format!("Read {app}"),
        "click" => format!("Clicked {target}"),
        "drag" => "Dragged".into(),
        "type_text" => "Typed text".into(),
        "press_key" => format!("Pressed {}", input["key"].as_str().unwrap_or("a key")),
        "set_value" => format!("Set the value of {target}"),
        "select_text" => format!("Selected text in {target}"),
        "scroll" => format!(
            "Scrolled {} {target}",
            input["direction"].as_str().unwrap_or_default()
        ),
        "perform_secondary_action" => format!(
            "{} on {target}",
            input["ax_action"].as_str().unwrap_or("Action")
        ),
        other => other.to_owned(),
    };
    format!("{line} in {app}").chars().take(200).collect()
}

pub fn definition(capabilities: Capabilities) -> ToolDefinition {
    ToolDefinition::function("computer", description(capabilities), schema())
}

fn description(capabilities: Capabilities) -> String {
    let click = if capabilities.pointer {
        "click (element_index, or x and y in the latest screenshot's pixels; mouse_button, click_count); drag (from_x, from_y, to_x, to_y); "
    } else {
        "click (element_index; mouse_button left or right); "
    };
    let changes = if capabilities.diffs {
        "the app's new state, or only what changed, where lines it leaves out keep their index"
    } else {
        "the app's whole new state"
    };
    let mut text = format!(
        "Operates this computer's apps through accessibility, with window screenshots as a fallback. action: list_apps; get_app_state (app) reads the app's accessibility tree, one element per line as [index] Role \"title\" …, launching it if needed; {click}type_text (text) into the focused element; press_key (key such as Return, Escape, ctrl+a, or cmd+shift+z on a Mac); set_value (element_index, value); select_text (element_index, text, prefix, suffix, selection_type); scroll (element_index, direction, pages); perform_secondary_action (element_index, ax_action such as ShowMenu, Increment or activate). Every action but list_apps needs app: a display name, a bundle identifier or .app path on macOS, or a .desktop id or executable name on Linux. Each action returns {changes}; use indexes only from the latest state."
    );
    if capabilities.screenshots {
        text.push_str(" screenshot adds a JPEG of the app's window; get_app_state includes one unless screenshot is false.");
    }
    if capabilities.policy {
        text.push_str(
            " Terminals, password managers and system authentication dialogs are refused.",
        );
    }
    text.push_str(
        " Ask before submitting, sending, buying or deleting anything the user did not ask for.",
    );
    text
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": {"type": "string", "enum": ACTIONS},
            "app": {"type": "string"},
            "element_index": {"type": "integer"},
            "x": {"type": "number"},
            "y": {"type": "number"},
            "mouse_button": {"type": "string", "enum": ["left", "right", "middle"]},
            "click_count": {"type": "integer"},
            "from_x": {"type": "number"},
            "from_y": {"type": "number"},
            "to_x": {"type": "number"},
            "to_y": {"type": "number"},
            "text": {"type": "string"},
            "key": {"type": "string"},
            "value": {"type": "string"},
            "prefix": {"type": "string"},
            "suffix": {"type": "string"},
            "selection_type": {"type": "string", "enum": ["text", "cursor_before", "cursor_after"]},
            "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
            "pages": {"type": "number"},
            "ax_action": {"type": "string"},
            "full": {"type": "boolean", "description": "get_app_state: the whole tree instead of what changed."},
            "screenshot": {"type": "boolean"}
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

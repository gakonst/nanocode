use crate::{
    backend::{App, Backend, Button, Capabilities, Direction, Pointer, Press, Target},
    keys::Chord,
    policy::{AppIdentity, Policy},
    state::{Selection, render},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    thread,
    time::{Duration, Instant},
};

const SETTLE: Duration = Duration::from_millis(150);
const LAUNCH: Duration = Duration::from_secs(10);
const LAUNCH_POLL: Duration = Duration::from_millis(250);
const LAUNCH_SETTLE: Duration = Duration::from_millis(500);
const POINTER: &str = "needs the pointer, which this computer's engine does not take here, so nothing ran. Use element_index with click, set_value, scroll or perform_secondary_action.";

pub struct Step {
    pub text: String,
    pub screenshot: Option<String>,
    pub omitted: bool,
}

struct Snapshot<E> {
    elements: Vec<E>,
    focused: Option<E>,
    window: Option<E>,
}

pub struct Engine<B: Backend> {
    backend: B,
    apps: HashMap<i64, Snapshot<B::Element>>,
    policy: Option<Policy>,
    pointer: bool,
    host: String,
    settle: Duration,
    launch: Duration,
}

pub trait Desktop: Send + 'static {
    fn capabilities(&self) -> Capabilities;

    fn step(&mut self, action: &str, input: &Value, max_screenshot: usize) -> Result<Step, String>;
}

impl<B: Backend> Desktop for Engine<B> {
    fn capabilities(&self) -> Capabilities {
        let mut capabilities = self.backend.capabilities();
        capabilities.pointer &= self.pointer;
        capabilities.policy = self.policy.is_some();
        capabilities
    }

    fn step(&mut self, action: &str, input: &Value, max_screenshot: usize) -> Result<Step, String> {
        self.run(action, input, max_screenshot)
    }
}

impl<B: Backend> Engine<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            apps: HashMap::new(),
            policy: None,
            pointer: false,
            host: "this app".into(),
            settle: SETTLE,
            launch: LAUNCH,
        }
    }

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
        self.host = name.into();
        self
    }

    #[must_use]
    pub const fn settle(mut self, settle: Duration, launch: Duration) -> Self {
        self.settle = settle;
        self.launch = launch;
        self
    }

    pub const fn backend(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn run(
        &mut self,
        action: &str,
        input: &Value,
        max_screenshot: usize,
    ) -> Result<Step, String> {
        self.backend.ready(&self.host)?;
        if action == "list_apps" {
            return Ok(Step {
                text: self.list_apps(),
                screenshot: None,
                omitted: false,
            });
        }
        let wanted = input["app"]
            .as_str()
            .filter(|app| !app.trim().is_empty())
            .ok_or("app is required: a display name or the app's identifier")?;
        let app = self.resolve(wanted)?;
        self.permitted(&app)
            .map_err(|why| format!("{}: {why}", app.name))?;
        let done = if action == "get_app_state" {
            None
        } else {
            let done = self.act(action, input, &app)?;
            thread::sleep(self.settle);
            Some(done)
        };
        let (mut text, window) = self.read(&app)?;
        if let Some(done) = done {
            text = format!("{done}\n{text}");
        }
        let mut step = Step {
            text,
            screenshot: None,
            omitted: false,
        };
        let wanted = input["screenshot"]
            .as_bool()
            .unwrap_or(action == "get_app_state");
        if wanted
            && self.backend.capabilities().screenshots
            && let Some(window) = &window
        {
            match self.backend.screenshot(&app, window, max_screenshot) {
                Ok(Some(jpeg)) => step.screenshot = Some(jpeg),
                Ok(None) => step.omitted = true,
                Err(why) => step.text.push_str(&format!("\n(no screenshot: {why})")),
            }
        }
        Ok(step)
    }

    fn permitted(&self, app: &App) -> Result<(), String> {
        let Some(policy) = &self.policy else {
            return Ok(());
        };
        std::iter::once(&app.id)
            .chain(&app.aliases)
            .try_for_each(|id| {
                policy(AppIdentity {
                    name: &app.name,
                    id,
                })
            })
    }

    fn list_apps(&mut self) -> String {
        let mut apps = self.backend.apps();
        apps.retain(|app| !app.background || app.windows);
        apps.sort_by_key(|app| app.name.to_lowercase());
        let lines: Vec<String> = apps
            .iter()
            .map(|app| {
                let windows = if app.windows {
                    ", windows on screen"
                } else {
                    ""
                };
                format!("{} ({}) pid {}{windows}", app.name, app.id, app.pid)
            })
            .collect();
        if lines.is_empty() {
            "No running apps.".into()
        } else {
            lines.join("\n")
        }
    }

    fn resolve(&mut self, wanted: &str) -> Result<App, String> {
        if let Some(app) = find(self.backend.apps(), wanted) {
            return Ok(app);
        }
        let wanted = wanted.trim().trim_end_matches('/');
        let installed = self.backend.installed(wanted).ok_or_else(|| {
            format!(
                "{wanted} is not running and isn't an installed app, so nothing ran. Give its bundle identifier, path or executable name, or list running apps with list_apps."
            )
        })?;
        self.permitted(&installed)
            .map_err(|why| format!("{}: {why}", installed.name))?;
        self.backend.launch(&installed)?;
        let deadline = Instant::now() + self.launch;
        while Instant::now() < deadline {
            thread::sleep(LAUNCH_POLL);
            let apps = self.backend.apps();
            if let Some(app) = find(apps.clone(), wanted)
                .or_else(|| find(apps, &installed.id).filter(|_| !installed.id.is_empty()))
            {
                thread::sleep(LAUNCH_SETTLE.min(self.launch));
                return Ok(app);
            }
        }
        Err(format!(
            "{wanted} was launched but did not start within {} seconds, so nothing ran.",
            self.launch.as_secs()
        ))
    }

    fn read(&mut self, app: &App) -> Result<(String, Option<B::Element>), String> {
        let read = self.backend.read(app)?;
        let title = match &read.title {
            Some(title) => format!("{} — window \"{title}\"", app.name),
            None => app.name.clone(),
        };
        let (text, elements) = render(&title, read.root, read.complete);
        self.apps.insert(
            app.pid,
            Snapshot {
                elements,
                focused: read.focused,
                window: read.window.clone(),
            },
        );
        Ok((text, read.window))
    }

    fn target(&mut self, app: &App, input: &Value) -> Result<B::Element, String> {
        let index = input["element_index"]
            .as_u64()
            .ok_or("element_index is required: this engine acts on elements, not coordinates")?;
        let element = self
            .apps
            .get(&app.pid)
            .and_then(|state| state.elements.get(index as usize))
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Element {index} is not in {}'s latest state, so nothing ran. Read it with get_app_state.",
                    app.name
                )
            })?;
        if !self.backend.alive(&element) {
            return Err(format!(
                "Element {index} is no longer in {}, so nothing ran. Read it again with get_app_state and use its new indexes.",
                app.name
            ));
        }
        Ok(element)
    }

    fn focus_unchanged(&mut self, app: &App) -> Result<Option<B::Element>, String> {
        let Some(state) = self.apps.get(&app.pid) else {
            return Err(format!(
                "Read {} with get_app_state before typing, so keys reach the element you expect. Nothing was sent.",
                app.name
            ));
        };
        let then = state.focused.clone();
        if !self.backend.focus_unchanged(app, then.as_ref()) {
            return Err(format!(
                "{}'s focused element changed since its latest state, so nothing was sent. Read it with get_app_state first.",
                app.name
            ));
        }
        Ok(then)
    }

    fn pointer_allowed(&self) -> bool {
        self.pointer && self.backend.capabilities().pointer
    }

    fn window(&self, app: &App) -> Option<B::Element> {
        self.apps
            .get(&app.pid)
            .and_then(|state| state.window.clone())
    }

    fn act(&mut self, action: &str, input: &Value, app: &App) -> Result<String, String> {
        let text = |key: &str| input[key].as_str().unwrap_or_default().to_owned();
        match action {
            "click" => self.click(input, app),
            "drag" => {
                if !self.pointer_allowed() {
                    return Err(format!("Dragging {POINTER}"));
                }
                let point = |x: &str, y: &str| match (input[x].as_f64(), input[y].as_f64()) {
                    (Some(x), Some(y)) => Ok((x, y)),
                    _ => Err(format!("drag needs {x} and {y}, so nothing ran.")),
                };
                let (from, to) = (point("from_x", "from_y")?, point("to_x", "to_y")?);
                let window = self.window(app);
                self.backend
                    .pointer(app, window.as_ref(), Pointer::Drag { from, to })
            }
            "set_value" => {
                let element = self.target(app, input)?;
                self.backend.set_value(&element, &text("value"))
            }
            "type_text" => {
                let focused = self.focus_unchanged(app)?.ok_or_else(|| {
                    format!(
                        "No element of {} is focused, so nothing was typed. Click a field first.",
                        app.name
                    )
                })?;
                self.backend.type_text(app, &focused, &text("text"))
            }
            "press_key" => {
                self.focus_unchanged(app)?;
                let chord = Chord::parse(&text("key"))?;
                self.backend.press_key(app, chord)?;
                Ok(format!("Pressed {}.", text("key")))
            }
            "select_text" => {
                let element = self.target(app, input)?;
                let value = self
                    .backend
                    .value(&element)
                    .ok_or("That element has no text to select, so nothing was selected.")?;
                let selection = Selection::find(
                    &value,
                    &text("text"),
                    &text("prefix"),
                    &text("suffix"),
                    input["selection_type"].as_str().unwrap_or("text"),
                )?;
                self.backend.select(&element, &value, selection)
            }
            "scroll" => {
                let element = self.target(app, input)?;
                self.backend.scroll(
                    &element,
                    Direction::parse(input["direction"].as_str().unwrap_or("down")),
                    input["pages"].as_f64().unwrap_or(1.0),
                )
            }
            "perform_secondary_action" => {
                let element = self.target(app, input)?;
                self.backend.perform(&element, &text("ax_action"))
            }
            other => Err(format!(
                "{other} is not supported by this computer's engine"
            )),
        }
    }

    fn click(&mut self, input: &Value, app: &App) -> Result<String, String> {
        let count = input["click_count"].as_u64().unwrap_or(1).clamp(1, 3) as u32;
        let button = match input["mouse_button"].as_str() {
            Some("right") => Button::Right,
            Some("middle") => Button::Middle,
            _ => Button::Left,
        };
        let pointer = self.pointer_allowed();
        if input["element_index"].is_null() {
            if !pointer {
                return Err(format!("Clicking coordinates {POINTER}"));
            }
            let (Some(x), Some(y)) = (input["x"].as_f64(), input["y"].as_f64()) else {
                return Err("click needs element_index, or x and y, so nothing ran.".into());
            };
            let window = self.window(app);
            return self.backend.pointer(
                app,
                window.as_ref(),
                Pointer::Click {
                    at: Target::Point(x, y),
                    button,
                    count,
                },
            );
        }
        let element = self.target(app, input)?;
        if count > 1 || button == Button::Middle {
            if !pointer {
                let what = if count > 1 {
                    "A multiple click"
                } else {
                    "A middle click"
                };
                return Err(format!("{what} {POINTER}"));
            }
            let window = self.window(app);
            return self.backend.pointer(
                app,
                window.as_ref(),
                Pointer::Click {
                    at: Target::Element(&element),
                    button,
                    count,
                },
            );
        }
        let press = if button == Button::Right {
            Press::Menu
        } else {
            Press::Primary
        };
        self.backend.press(&element, press)
    }
}

pub fn find(mut apps: Vec<App>, wanted: &str) -> Option<App> {
    let wanted = wanted.trim().trim_end_matches('/');
    let lower = wanted.to_lowercase();
    apps.sort_by_key(|app| app.background);
    let exact = apps.iter().position(|app| {
        let stem = app.path.rsplit('/').next().unwrap_or_default();
        let stem = stem.strip_suffix(".app").unwrap_or(stem);
        app.name.eq_ignore_ascii_case(wanted)
            || app.id.eq_ignore_ascii_case(wanted)
            || app
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(wanted))
            || (!app.path.is_empty() && app.path == wanted)
            || (!stem.is_empty() && stem.eq_ignore_ascii_case(wanted.trim_end_matches(".app")))
    });
    let index = exact.or_else(|| {
        apps.iter()
            .position(|app| app.name.to_lowercase().starts_with(&lower))
    })?;
    Some(apps.swap_remove(index))
}

#[cfg(test)]
mod tests;

use crate::{
    backend::{App, Backend, Button, Capabilities, Direction, Pointer, Press, Target},
    keys::Chord,
    policy::{AppIdentity, Policy},
    state::{Line, NO_WINDOW, Selection, heading, lines},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant},
};

const SETTLE: Duration = Duration::from_millis(150);
const QUIET: Duration = Duration::from_millis(20);
const SINCE: &str = "Since its last state (lines left out are unchanged and keep their index; + added, ~ changed, - removed):";
const UNCHANGED: &str = "No change since its last state.";
const LAUNCH: Duration = Duration::from_secs(10);
const LAUNCH_POLL: Duration = Duration::from_millis(250);
const LAUNCH_SETTLE: Duration = Duration::from_millis(500);
const POINTER: &str = "needs the pointer, which this computer's engine does not take here, so nothing ran. Use element_index with click, set_value, scroll or perform_secondary_action.";

pub struct Step {
    pub text: String,
    pub screenshot: Option<String>,
    pub omitted: bool,
}

struct Shown<E> {
    line: Line<E>,
    index: Option<usize>,
}

struct Snapshot<E> {
    shown: Vec<Shown<E>>,
    next: usize,
    focused: Option<E>,
    window: Option<E>,
    typing: Option<E>,
}

struct Seen<E> {
    text: String,
    snapshot: Snapshot<E>,
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

    fn lease_key(&mut self, _app: &str) -> Option<String> {
        None
    }
}

impl<B: Backend> Desktop for Engine<B> {
    fn capabilities(&self) -> Capabilities {
        let mut capabilities = self.backend.capabilities();
        capabilities.pointer &= self.pointer;
        capabilities.policy = self.policy.is_some();
        capabilities.diffs = true;
        capabilities
    }

    fn step(&mut self, action: &str, input: &Value, max_screenshot: usize) -> Result<Step, String> {
        self.run(action, input, max_screenshot)
    }

    fn lease_key(&mut self, app: &str) -> Option<String> {
        if !self.backend.keys_by_bundle() {
            return None;
        }
        find(self.backend.apps(), app)
            .or_else(|| self.backend.installed(app))
            .map(|app| app.id)
            .filter(|id| !id.is_empty())
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
    pub fn pointer(mut self, allowed: bool) -> Self {
        self.pointer = allowed;
        self.backend.watched(!allowed);
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
        let app = self.resolve(wanted, action == "get_app_state")?;
        self.permitted(&app)
            .map_err(|why| format!("{}: {why}", app.name))?;
        let (done, watching) = if action == "get_app_state" {
            (None, false)
        } else {
            let watching = self.backend.watch(&app);
            let done = self.act(action, input, &app);
            match &done {
                Ok(_) if watching => {
                    self.backend.settle(self.settle, QUIET);
                }
                Ok(_) => thread::sleep(self.settle),
                Err(_) => self.backend.unwatch(),
            }
            (Some(done?), watching)
        };
        let read = self.read(&app, done.is_some(), watching);
        if watching {
            self.backend.unwatch();
        }
        let (mut text, window) = match (read, &done) {
            (Ok(read), _) => read,
            (Err(why), Some(_)) => (why, None),
            (Err(why), None) => return Err(why),
        };
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
        let mut apps = self.backend.listing();
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

    fn resolve(&mut self, wanted: &str, launch: bool) -> Result<App, String> {
        let running = self.backend.apps();
        if let Some(app) = find(running.clone(), wanted) {
            return Ok(app);
        }
        let wanted = wanted.trim().trim_end_matches('/');
        let installed = self.backend.installed(wanted).ok_or_else(|| {
            format!(
                "{wanted} is not running and isn't an installed app, so nothing ran. Give its bundle identifier, path or executable name, or list running apps with list_apps."
            )
        })?;
        if !installed.id.is_empty()
            && let Some(app) = find(running, &installed.id)
        {
            return Ok(app);
        }
        if !launch {
            return Err(format!(
                "{wanted} is not running, so nothing ran; it may have quit. Read it with get_app_state, which starts it."
            ));
        }
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
                self.backend.launched(&app);
                thread::sleep(LAUNCH_SETTLE.min(self.launch));
                self.backend.launched(&app);
                return Ok(app);
            }
        }
        Err(format!(
            "{wanted} was launched but did not start within {} seconds, so nothing ran.",
            self.launch.as_secs()
        ))
    }

    fn read(
        &mut self,
        app: &App,
        diff: bool,
        watching: bool,
    ) -> Result<(String, Option<B::Element>), String> {
        let before = self.apps.remove(&app.pid);
        let since = before.as_ref().filter(|_| diff);
        let mut seen = self.seen(app, since);
        if watching && seen.is_ok() && self.backend.settle(Duration::ZERO, QUIET) {
            seen = self.seen(app, since);
        }
        match seen {
            Ok(Seen { text, mut snapshot }) => {
                snapshot.typing = before.and_then(|before| before.typing);
                let window = snapshot.window.clone();
                self.apps.insert(app.pid, snapshot);
                Ok((text, window))
            }
            Err(why) => {
                if let Some(before) = before {
                    self.apps.insert(app.pid, before);
                }
                Err(why)
            }
        }
    }

    fn seen(
        &mut self,
        app: &App,
        since: Option<&Snapshot<B::Element>>,
    ) -> Result<Seen<B::Element>, String> {
        let read = self.backend.read(app)?;
        let title = match &read.title {
            Some(title) => format!("{} — window \"{title}\"", app.name),
            None => app.name.clone(),
        };
        let mut text = heading(&title, read.complete);
        let Some(root) = read.root else {
            text.push(read.note.unwrap_or_else(|| NO_WINDOW.into()));
            return Ok(Seen {
                text: text.join("\n"),
                snapshot: Snapshot {
                    shown: Vec::new(),
                    next: 0,
                    focused: read.focused,
                    window: read.window,
                    typing: None,
                },
            });
        };
        let lines = lines(root);
        let (fresh, count, shown) = numbered(&lines, None);
        let size = |lines: &[String]| lines.iter().map(|line| line.len() + 1).sum::<usize>();
        let changed = since
            .filter(|since| since.window.is_some() && since.window == read.window)
            .map(|since| numbered(&lines, Some(since)))
            .filter(|(_, _, changes)| size(changes) < size(&shown));
        let (indexes, next) = match changed {
            Some((indexes, next, changes)) => {
                if changes.is_empty() {
                    text.push(UNCHANGED.into());
                } else {
                    text.push(SINCE.into());
                    text.extend(changes);
                }
                (indexes, next)
            }
            None => {
                text.extend(shown);
                (fresh, count)
            }
        };
        Ok(Seen {
            text: text.join("\n"),
            snapshot: Snapshot {
                shown: lines
                    .into_iter()
                    .zip(indexes)
                    .map(|(line, index)| Shown { line, index })
                    .collect(),
                next,
                focused: read.focused,
                window: read.window,
                typing: None,
            },
        })
    }

    fn target(&mut self, app: &App, input: &Value) -> Result<B::Element, String> {
        let index = input["element_index"]
            .as_u64()
            .ok_or("element_index is required: this engine acts on elements, not coordinates")?;
        let element = self
            .apps
            .get(&app.pid)
            .and_then(|state| {
                state
                    .shown
                    .iter()
                    .find(|shown| shown.index == usize::try_from(index).ok())
            })
            .map(|shown| shown.line.element.clone())
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

    fn typing(&mut self, app: &App, element: Option<&B::Element>) {
        let typing = element
            .filter(|element| self.backend.types_into(element))
            .cloned();
        if let Some(state) = self.apps.get_mut(&app.pid) {
            state.typing = typing;
        }
    }

    fn typed(&mut self, app: &App) -> Option<B::Element> {
        let typing = self.apps.get(&app.pid)?.typing.clone()?;
        self.backend.alive(&typing).then_some(typing)
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
                let done = self.backend.set_value(&element, &text("value"))?;
                self.typing(app, Some(&element));
                Ok(done)
            }
            "type_text" => {
                if let Some(target) = self.typed(app) {
                    return self.backend.type_text(app, &target, &text("text"));
                }
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
                let done = self.backend.select(&element, &value, selection)?;
                self.typing(app, Some(&element));
                Ok(done)
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
        if button == Button::Right {
            return self.backend.press(&element, Press::Menu);
        }
        let done = self.backend.press(&element, Press::Primary)?;
        self.typing(app, Some(&element));
        Ok(done)
    }
}

fn numbered<E: Eq + std::hash::Hash>(
    lines: &[Line<E>],
    since: Option<&Snapshot<E>>,
) -> (Vec<Option<usize>>, usize, Vec<String>) {
    let Some(since) = since else {
        let mut next = 0;
        let indexes: Vec<Option<usize>> = lines
            .iter()
            .map(|line| {
                next += usize::from(line.actionable);
                line.actionable.then_some(next - 1)
            })
            .collect();
        let shown = lines
            .iter()
            .zip(&indexes)
            .map(|(line, index)| line.shown(*index))
            .collect();
        return (indexes, next, shown);
    };
    let known: HashMap<&E, &Shown<E>> = since
        .shown
        .iter()
        .map(|shown| (&shown.line.element, shown))
        .collect();
    let mut next = since.next;
    let mut changes = Vec::new();
    let indexes = lines
        .iter()
        .map(|line| {
            let old = known.get(&line.element);
            let index = line.actionable.then(|| {
                old.and_then(|old| old.index).unwrap_or_else(|| {
                    next += 1;
                    next - 1
                })
            });
            match old {
                Some(old)
                    if old.index == index
                        && old.line.depth == line.depth
                        && old.line.text == line.text => {}
                Some(old) if index.is_some() && old.index == index => {
                    changes.push(format!("~ {}", line.shown(index)));
                }
                Some(old) => {
                    changes.push(format!("- {}", old.line.shown(old.index)));
                    changes.push(format!("+ {}", line.shown(index)));
                }
                None => changes.push(format!("+ {}", line.shown(index))),
            }
            index
        })
        .collect();
    let kept: HashSet<&E> = lines.iter().map(|line| &line.element).collect();
    changes.extend(
        since
            .shown
            .iter()
            .filter(|shown| !kept.contains(&shown.line.element))
            .map(|shown| format!("- {}", shown.line.shown(shown.index))),
    );
    (indexes, next, changes)
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

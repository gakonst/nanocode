mod ax;

use crate::{
    App, Backend, Capabilities, Chord, Direction, Node, Pointer, Press, Read, Selection, Target,
    keys::{Key, Modifiers, mac},
};
use ax::{
    AX_ACTION_UNSUPPORTED, AX_API_DISABLED, AX_ATTRIBUTE_UNSUPPORTED, AX_CANNOT_COMPLETE,
    AX_INVALID, AX_NOT_IMPLEMENTED, AX_SUCCESS, AXError, Element,
};
use nanocode_sys::core_foundation::Object;
use std::{
    env, fs,
    process::Command,
    thread,
    time::{Duration, Instant},
};

const MAX_NODES: usize = 3000;
const MAX_DEPTH: usize = 40;
const MAX_CHILDREN: usize = 400;
const READ_BUDGET: Duration = Duration::from_secs(5);
const INTERACTIVE: &str = "AXButton AXCheckBox AXRadioButton AXPopUpButton AXMenuButton AXComboBox AXTextField AXTextArea AXSlider AXIncrementor AXLink AXMenuItem AXMenuBarItem AXDisclosureTriangle AXRow AXCell AXScrollArea AXColorWell AXDateField AXTimeField AXSegmentedControl AXTab";
const ACTIVATING: &str = "AXPress AXShowMenu AXConfirm AXPick AXIncrement AXOpen";
const SECURE: &str = "AXSecureTextField";
const UNMAPPED_KEY: u16 = 0;
const KEYS_PER_PAUSE: usize = 16;
const TYPING_PAUSE: Duration = Duration::from_millis(8);
const TEXT_ENTRY: &str = "AXTextField AXTextArea AXComboBox";
const LOCKED: &str = "The screen is locked, and macOS hides every app's windows from accessibility until it is unlocked, so nothing ran. Try again once the Mac is unlocked, or continue without computer use.";
const NODE: [&str; 9] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXEnabled",
    "AXSelected",
    "AXExpanded",
    "AXChildren",
];

#[derive(Default)]
pub struct Ax {
    host: String,
    watching: Option<ax::Observer>,
    shown: bool,
}

impl Ax {
    pub(crate) fn shown() -> Self {
        Self {
            shown: true,
            ..Self::default()
        }
    }

    fn no_permission(&self) -> String {
        format!(
            "Computer use needs Accessibility permission for {} (System Settings › Privacy & Security › Accessibility), so nothing ran. Ask the user to grant it, or continue without it.",
            self.host
        )
    }

    fn outcome(&self, error: AXError, done: &str) -> Result<String, String> {
        match error {
            AX_SUCCESS => Ok(format!("{done}.")),
            AX_INVALID => Err("That element is gone, so nothing ran. Read the app again with get_app_state.".into()),
            AX_ACTION_UNSUPPORTED | AX_ATTRIBUTE_UNSUPPORTED => Err("That element does not support this action, so nothing ran.".into()),
            AX_API_DISABLED => Err(self.no_permission()),
            AX_CANNOT_COMPLETE => Err("The app did not answer in time; whether it took effect is unknown. Read its state before trying again.".into()),
            other => Err(format!("The app refused it (accessibility error {other}); whether it took effect is unknown. Read its state before trying again.")),
        }
    }
}

impl Backend for Ax {
    type Element = Element;

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            pointer: true,
            screenshots: true,
            diffs: false,
            policy: false,
        }
    }

    fn ready(&mut self, host: &str) -> Result<(), String> {
        host.clone_into(&mut self.host);
        if !ax::trusted() {
            Err(self.no_permission())
        } else if ax::locked() {
            Err(LOCKED.into())
        } else {
            Ok(())
        }
    }

    fn apps(&mut self) -> Vec<App> {
        let shown = ax::on_screen();
        ax::processes()
            .into_iter()
            .filter_map(|(pid, path)| {
                let bundle = app_bundle(&path)?;
                let (id, name, background) = ax::bundle_info(bundle);
                Some(App {
                    pid: pid.into(),
                    name,
                    id,
                    aliases: Vec::new(),
                    path: bundle.to_owned(),
                    background,
                    windows: shown.contains(&pid),
                })
            })
            .collect()
    }

    fn keys_by_bundle(&self) -> bool {
        true
    }

    fn installed(&mut self, wanted: &str) -> Option<App> {
        let by_bundle = wanted.contains('.')
            && !wanted.contains('/')
            && !wanted.contains(' ')
            && !wanted.ends_with(".app");
        if by_bundle {
            return Some(App {
                name: wanted.to_owned(),
                id: wanted.to_owned(),
                ..App::default()
            });
        }
        let path = if wanted.starts_with('/') {
            Some(wanted.to_owned()).filter(|path| path.ends_with(".app"))
        } else {
            installed(wanted)
        }?;
        let (id, name, background) = ax::bundle_info(&path);
        Some(App {
            name,
            id,
            path,
            background,
            ..App::default()
        })
    }

    fn launch(&mut self, app: &App) -> Result<(), String> {
        let (flag, target) = if app.path.is_empty() {
            ("-b", &app.id)
        } else {
            ("-a", &app.path)
        };
        let hidden = if self.shown { None } else { Some("-j") };
        let launched = Command::new("/usr/bin/open")
            .arg("-g")
            .args(hidden)
            .args([flag, target])
            .output()
            .map_err(|error| {
                format!(
                    "{} could not be launched ({error}), so nothing ran.",
                    app.name
                )
            })?;
        if launched.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{} is not running and no app by that name could be launched, so nothing ran. List apps with list_apps.",
                app.name
            ))
        }
    }

    fn read(&mut self, app: &App) -> Result<Read<Element>, String> {
        let ax = Element::application(app.pid as i32)
            .ok_or_else(|| format!("{} could not be reached, so nothing ran.", app.name))?;
        let window = window_of(&ax);
        let focused = ax.element("AXFocusedUIElement");
        let mut budget = Budget {
            nodes: 0,
            deadline: Instant::now() + READ_BUDGET,
            complete: true,
        };
        let title = window.as_ref().and_then(|window| window.text("AXTitle"));
        let note = (window.is_none() && ax.flag("AXHidden") == Some(true)).then(|| {
            format!(
                "(no open window: {0} runs hidden, which keeps it off the user's screen, and has no window to read. A document app shows one once it opens a file: open the file with it in the background (open -g -j -a \"{0}\" FILE in a shell), or ask the user to bring {0} to the front.)",
                app.name
            )
        });
        let root = window
            .clone()
            .map(|window| read_node(window, 0, focused.as_ref(), &mut budget, &mut Vec::new()));
        Ok(Read {
            title,
            root,
            window,
            focused,
            complete: budget.complete,
            note,
        })
    }

    fn alive(&mut self, element: &Element) -> bool {
        element.alive()
    }

    fn watch(&mut self, app: &App) -> bool {
        self.watching = ax::Observer::new(app.pid as i32);
        self.watching.is_some()
    }

    fn settle(&mut self, first: Duration, quiet: Duration) -> bool {
        self.watching
            .as_mut()
            .is_some_and(|watching| watching.settle(first, quiet))
    }

    fn unwatch(&mut self) {
        self.watching = None;
    }

    fn focus_unchanged(&mut self, app: &App, then: Option<&Element>) -> bool {
        let now =
            Element::application(app.pid as i32).and_then(|ax| ax.element("AXFocusedUIElement"));
        now.as_ref() == then
    }

    fn press(&mut self, element: &Element, press: Press) -> Result<String, String> {
        let actions = element.actions();
        let wanted: &[&str] = match press {
            Press::Menu => &["AXShowMenu"],
            Press::Primary => &["AXPress", "AXConfirm", "AXPick", "AXOpen"],
        };
        if let Some(chosen) = wanted
            .iter()
            .find(|name| actions.iter().any(|a| a == *name))
        {
            return self.outcome(element.perform(chosen), "Clicked");
        }
        if press == Press::Primary && element.flag("AXSelected").is_some() {
            return self.outcome(element.set_true("AXSelected"), "Selected");
        }
        Err(format!(
            "That element takes no click (its actions: {}), so nothing ran.",
            list(&actions)
        ))
    }

    fn set_value(&mut self, element: &Element, value: &str) -> Result<String, String> {
        let numeric = element.numeric("AXValue");
        let error = match value.trim().parse::<f64>() {
            Ok(number) if numeric => element.set_number("AXValue", number),
            _ => element.set_text("AXValue", value),
        };
        self.outcome(error, "Set the value")?;
        Ok(match element.text("AXValue") {
            Some(now) if now != value && !numeric => {
                "Set the value, but the element now reads differently; check the state.".into()
            }
            _ => "Set the value.".into(),
        })
    }

    fn value(&mut self, element: &Element) -> Option<String> {
        element.text("AXValue")
    }

    fn select(
        &mut self,
        element: &Element,
        value: &str,
        selection: Selection,
    ) -> Result<String, String> {
        let (location, length) = selection.utf16(value);
        self.outcome(
            element.set_range("AXSelectedTextRange", location, length),
            "Selected text",
        )
    }

    fn scroll(
        &mut self,
        element: &Element,
        direction: Direction,
        pages: f64,
    ) -> Result<String, String> {
        let mut area = element.clone();
        for _ in 0..12 {
            if area.text("AXRole").as_deref() == Some("AXScrollArea") {
                break;
            }
            match area.element("AXParent") {
                Some(parent) => area = parent,
                None => {
                    area = element.clone();
                    break;
                }
            }
        }
        let (action, bar, sign) = match direction {
            Direction::Up => ("AXScrollUpByPage", "AXVerticalScrollBar", -1.0),
            Direction::Left => ("AXScrollLeftByPage", "AXHorizontalScrollBar", -1.0),
            Direction::Right => ("AXScrollRightByPage", "AXHorizontalScrollBar", 1.0),
            Direction::Down => ("AXScrollDownByPage", "AXVerticalScrollBar", 1.0),
        };
        let name = direction.name();
        if area.actions().iter().any(|a| a == action) {
            for _ in 0..pages.round().clamp(1.0, 20.0) as usize {
                self.outcome(area.perform(action), "Scrolled")?;
            }
            return Ok(format!("Scrolled {name}."));
        }
        let bar = area
            .element(bar)
            .ok_or("That element has nothing to scroll, so nothing ran.")?;
        let position = bar
            .text("AXValue")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        let moved = (position + sign * 0.15 * pages.max(0.1)).clamp(0.0, 1.0);
        self.outcome(bar.set_number("AXValue", moved), "Scrolled")?;
        Ok(format!("Scrolled {name}."))
    }

    fn perform(&mut self, element: &Element, action: &str) -> Result<String, String> {
        let wanted = ax_action(action);
        let actions = element.actions();
        if !actions.contains(&wanted) {
            return Err(format!(
                "That element has no {wanted} (its actions: {}), so nothing ran.",
                list(&actions)
            ));
        }
        self.outcome(element.perform(&wanted), &format!("Performed {wanted}"))
    }

    fn type_text(&mut self, app: &App, focused: &Element, text: &str) -> Result<String, String> {
        let entry = focused
            .text("AXRole")
            .is_some_and(|role| TEXT_ENTRY.split(' ').any(|known| known == role));
        if entry && focused.text("AXSubrole").as_deref() != Some(SECURE) {
            match focused.set_text("AXSelectedText", text) {
                AX_SUCCESS => return Ok("Typed text.".into()),
                AX_ATTRIBUTE_UNSUPPORTED | AX_NOT_IMPLEMENTED => {}
                _ => {
                    return Err(format!(
                        "{} didn't confirm the typing, so whether the text went in is unknown. Read it with get_app_state before typing again.",
                        app.name
                    ));
                }
            }
        }
        type_keys(app.pid, text);
        Ok("Typed text as keys.".into())
    }

    fn press_key(&mut self, app: &App, chord: Chord) -> Result<String, String> {
        let (code, flags) = mac::code(&chord)?;
        for down in [true, false] {
            ax::post_key(app.pid as i32, code, flags, down, None);
        }
        Ok("Pressed the key.".into())
    }

    fn screenshot(
        &mut self,
        app: &App,
        window: &Element,
        max: usize,
    ) -> Result<Option<String>, String> {
        if !ax::on_screen().contains(&(app.pid as i32)) {
            return Err(format!(
                "{} has no window on the user's screen (it runs hidden or minimized), so there is nothing to capture; the state above stands without one",
                app.name
            ));
        }
        ax::screenshot(window, max)
    }

    fn pointer(
        &mut self,
        app: &App,
        window: Option<&Element>,
        pointer: Pointer<'_, Element>,
    ) -> Result<String, String> {
        let pid = app.pid as i32;
        let id = window.and_then(Element::window_id);
        match pointer {
            Pointer::Click { at, button, count } => {
                ax::click(pid, id, place(window, at)?, button, count);
                Ok("Clicked.".into())
            }
            Pointer::Drag { from, to } => {
                let from = place(window, Target::Point(from.0, from.1))?;
                let to = place(window, Target::Point(to.0, to.1))?;
                ax::drag(pid, id, from, to);
                Ok("Dragged.".into())
            }
        }
    }
}

fn place(window: Option<&Element>, at: Target<'_, Element>) -> Result<(f64, f64), String> {
    match at {
        Target::Point(x, y) => window
            .and_then(Element::frame)
            .map(|(left, top, _, _)| (left + x, top + y))
            .ok_or_else(|| "The app's window has no place on screen, so nothing ran.".into()),
        Target::Element(element) => element
            .frame()
            .map(|(left, top, width, height)| (left + width / 2.0, top + height / 2.0))
            .ok_or_else(|| "That element has no place on screen, so nothing ran.".into()),
    }
}

fn window_of(ax: &Element) -> Option<Element> {
    ["AXFocusedWindow", "AXMainWindow"]
        .into_iter()
        .find_map(|attribute| ax.element(attribute).filter(Element::is_window))
        .or_else(|| {
            ax.elements("AXWindows", MAX_CHILDREN)
                .into_iter()
                .find(Element::is_window)
        })
}

pub(crate) use ax::{request_screen_recording, request_trust, screen_recording, trusted};

pub(crate) fn window(pid: i64) -> Option<Element> {
    window_of(&Element::application(pid as i32)?)
}

pub(crate) fn focused(pid: i64) -> Option<Element> {
    Element::application(pid as i32)?.element("AXFocusedUIElement")
}

pub(crate) fn title(window: &Element) -> Option<String> {
    window.text("AXTitle")
}

pub(crate) fn window_id(window: &Element) -> Option<u32> {
    window.window_id()
}

pub(crate) fn type_keys(pid: i64, text: &str) {
    let mut units = [0u16; 2];
    for (typed, c) in text.chars().enumerate() {
        let chord = Chord {
            key: Key::Char(c),
            modifiers: Modifiers::default(),
        };
        let (code, flags) = mac::code(&chord).unwrap_or((UNMAPPED_KEY, 0));
        let unit = c.encode_utf16(&mut units);
        for down in [true, false] {
            ax::post_key(pid as i32, code, flags, down, Some(unit));
        }
        if typed % KEYS_PER_PAUSE == KEYS_PER_PAUSE - 1 {
            thread::sleep(TYPING_PAUSE);
        }
    }
}

pub(crate) fn scroll(
    pid: i64,
    window: Option<&Element>,
    at: Target<'_, Element>,
    horizontal: f64,
    vertical: f64,
) -> Result<String, String> {
    let at = place(window, at)?;
    let id = window.and_then(Element::window_id);
    ax::scroll(
        pid as i32,
        id,
        at,
        horizontal.round() as i32,
        vertical.round() as i32,
    );
    Ok("Scrolled.".into())
}

pub(crate) fn page(window: Option<&Element>) -> Option<(f64, f64)> {
    window
        .and_then(Element::frame)
        .map(|(_, _, width, height)| (width, height))
}

pub(crate) fn catalog() -> Vec<App> {
    app_dirs()
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.ends_with(".app")
                .then(|| entry.path().to_string_lossy().into_owned())
        })
        .map(|path| {
            let (id, name, background) = ax::bundle_info(&path);
            App {
                name,
                id,
                path,
                background,
                ..App::default()
            }
        })
        .collect()
}

struct Budget {
    nodes: usize,
    deadline: Instant,
    complete: bool,
}

fn read_node(
    element: Element,
    depth: usize,
    focused: Option<&Element>,
    budget: &mut Budget,
    above: &mut Vec<Element>,
) -> Node<Element> {
    budget.nodes += 1;
    let [
        role,
        subrole,
        title,
        description,
        value,
        enabled,
        selected,
        expanded,
        children,
    ] = element.many(&NODE);
    let text = |value: Option<Object>| value.as_ref().and_then(ax::text_of);
    let label = |value: Option<Object>| value.as_ref().and_then(ax::label_of);
    let flag = |value: Option<Object>| value.as_ref().and_then(ax::flag_of);
    let role = text(role).unwrap_or_else(|| "AXUnknown".into());
    let actions = element.actions();
    let actionable = INTERACTIVE.split(' ').any(|known| known == role)
        || actions
            .iter()
            .any(|a| ACTIVATING.split(' ').any(|known| known == a));
    let name = label(title).or_else(|| label(description));
    let value = label(value);
    let secure = text(subrole).as_deref() == Some(SECURE);
    let mut states = Vec::new();
    if flag(enabled) == Some(false) {
        states.push("disabled");
    }
    if focused.is_some_and(|focused| *focused == element) {
        states.push("focused");
    }
    if flag(selected) == Some(true) {
        states.push("selected");
    }
    match flag(expanded) {
        Some(true) => states.push("expanded"),
        Some(false) if actionable => states.push("collapsed"),
        _ => {}
    }
    let checkable = matches!(role.as_str(), "AXCheckBox" | "AXRadioButton");
    if checkable && value.as_deref() == Some("1") {
        states.push("checked");
    }
    let value = if checkable { None } else { value };
    let mut kids = Vec::new();
    if depth < MAX_DEPTH {
        above.push(element.clone());
        let children = children
            .map(|children| ax::elements_of(&children, MAX_CHILDREN))
            .unwrap_or_default();
        for child in children {
            if budget.nodes >= MAX_NODES || Instant::now() > budget.deadline {
                budget.complete = false;
                break;
            }
            if !above.contains(&child) {
                kids.push(read_node(child, depth + 1, focused, budget, above));
            }
        }
        above.pop();
    } else {
        budget.complete = false;
    }
    let mut node = Node::new(role.strip_prefix("AX").unwrap_or(&role), element);
    node.name = name;
    node.value = value;
    node.states = states;
    node.actionable = actionable;
    node.secure = secure;
    node.children = kids;
    node
}

fn list(actions: &[String]) -> String {
    if actions.is_empty() {
        "none".into()
    } else {
        actions.join(", ")
    }
}

pub fn ax_action(action: &str) -> String {
    let action = action.trim();
    if action.starts_with("AX") {
        action.to_owned()
    } else {
        format!("AX{action}")
    }
}

fn app_bundle(path: &str) -> Option<&str> {
    let index = path.find(".app/Contents/MacOS/")?;
    let bundle = &path[..index + 4];
    let rest = &path[index + ".app/Contents/MacOS/".len()..];
    (!rest.contains('/') && !bundle[..bundle.len() - 4].contains(".app/")).then_some(bundle)
}

fn installed(name: &str) -> Option<String> {
    let file = format!("{}.app", name.trim_end_matches(".app"));
    app_dirs().iter().find_map(|dir| {
        fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
            let entry_name = entry.file_name().into_string().ok()?;
            entry_name
                .eq_ignore_ascii_case(&file)
                .then(|| format!("{dir}/{entry_name}"))
        })
    })
}

fn app_dirs() -> [String; 5] {
    let home = env::var("HOME").unwrap_or_default();
    [
        "/Applications".to_owned(),
        "/Applications/Utilities".to_owned(),
        "/System/Applications".to_owned(),
        "/System/Applications/Utilities".to_owned(),
        format!("{home}/Applications"),
    ]
}

use super::{Desktop, Engine};
use crate::{
    App, Backend, Button, Capabilities, Chord, Direction, Key, Node, Pointer, Press, Read,
    Selection, Target, sensitive,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[derive(Default)]
struct Fake {
    running: Vec<App>,
    installed: Vec<App>,
    generation: usize,
    focused: Option<usize>,
    value: String,
    log: Vec<String>,
    pointer: bool,
    trusted: bool,
    bundles: bool,
}

impl Fake {
    fn new() -> Self {
        Self {
            running: vec![App {
                pid: 7,
                name: "Fixture".into(),
                id: "org.example.fixture".into(),
                path: "/apps/Fixture.app".into(),
                windows: true,
                ..App::default()
            }],
            installed: vec![
                App {
                    name: "Notes".into(),
                    id: "com.example.notes".into(),
                    path: "/apps/Notes.app".into(),
                    ..App::default()
                },
                App {
                    name: "Terminal".into(),
                    id: "com.apple.Terminal".into(),
                    path: "/apps/Terminal.app".into(),
                    ..App::default()
                },
            ],
            focused: Some(2),
            value: "naïve cat".into(),
            trusted: true,
            ..Self::default()
        }
    }

    fn node(&self, id: usize, role: &str, name: &str, actionable: bool) -> Node<usize> {
        let mut node = Node::new(role, self.generation * 100 + id);
        node.name = (!name.is_empty()).then(|| name.to_owned());
        node.actionable = actionable;
        if self.focused == Some(id) {
            node.states.push("focused");
        }
        node
    }
}

impl Backend for Fake {
    type Element = usize;

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            pointer: self.pointer,
            screenshots: true,
            diffs: false,
            policy: false,
        }
    }

    fn ready(&mut self, host: &str) -> Result<(), String> {
        if self.trusted {
            Ok(())
        } else {
            Err(format!(
                "Computer use needs Accessibility permission for {host}"
            ))
        }
    }

    fn apps(&mut self) -> Vec<App> {
        self.running.clone()
    }

    fn keys_by_bundle(&self) -> bool {
        self.bundles
    }

    fn installed(&mut self, wanted: &str) -> Option<App> {
        self.installed
            .iter()
            .find(|app| app.name.eq_ignore_ascii_case(wanted) || app.id == wanted)
            .cloned()
    }

    fn launch(&mut self, app: &App) -> Result<(), String> {
        self.log.push(format!("launch {}", app.id));
        self.running.push(App {
            pid: 9,
            windows: true,
            ..app.clone()
        });
        Ok(())
    }

    fn read(&mut self, app: &App) -> Result<Read<usize>, String> {
        self.log.push(format!("read {}", app.pid));
        let mut window = self.node(0, "Window", &app.name, false);
        let mut field = self.node(2, "TextField", "Name", true);
        field.value = Some(self.value.clone());
        let mut password = self.node(3, "TextField", "Password", true);
        password.value = Some("hunter2".into());
        password.secure = true;
        window.children = vec![self.node(1, "Button", "7", true), field, password];
        Ok(Read {
            title: Some(app.name.clone()),
            root: Some(window),
            window: Some(self.generation * 100),
            focused: self.focused.map(|id| self.generation * 100 + id),
            complete: true,
        })
    }

    fn alive(&mut self, element: &usize) -> bool {
        element / 100 == self.generation
    }

    fn focus_unchanged(&mut self, _: &App, then: Option<&usize>) -> bool {
        then.map(|element| element % 100) == self.focused
    }

    fn press(&mut self, element: &usize, press: Press) -> Result<String, String> {
        self.log.push(format!("press {element} {press:?}"));
        self.generation += 1;
        Ok("Clicked.".into())
    }

    fn set_value(&mut self, element: &usize, value: &str) -> Result<String, String> {
        self.log.push(format!("set {element} {value}"));
        value.clone_into(&mut self.value);
        Ok("Set the value.".into())
    }

    fn value(&mut self, _: &usize) -> Option<String> {
        Some(self.value.clone())
    }

    fn select(
        &mut self,
        element: &usize,
        value: &str,
        selection: Selection,
    ) -> Result<String, String> {
        self.log
            .push(format!("select {element} {:?}", selection.utf16(value)));
        Ok("Selected text.".into())
    }

    fn scroll(
        &mut self,
        element: &usize,
        direction: Direction,
        pages: f64,
    ) -> Result<String, String> {
        self.log
            .push(format!("scroll {element} {} {pages}", direction.name()));
        Ok("Scrolled.".into())
    }

    fn perform(&mut self, element: &usize, action: &str) -> Result<String, String> {
        self.log.push(format!("perform {element} {action}"));
        Ok(format!("Performed {action}."))
    }

    fn type_text(&mut self, _: &App, focused: &usize, text: &str) -> Result<String, String> {
        self.log.push(format!("type {focused} {}", text.len()));
        Ok("Typed text.".into())
    }

    fn press_key(&mut self, _: &App, chord: Chord) -> Result<String, String> {
        self.log.push(format!("key {:?}", chord.key));
        Ok(String::new())
    }

    fn screenshot(&mut self, _: &App, _: &usize, max: usize) -> Result<Option<String>, String> {
        Ok((max >= 8).then(|| "/9j/AAAA".into()))
    }

    fn pointer(
        &mut self,
        _: &App,
        window: Option<&usize>,
        pointer: Pointer<'_, usize>,
    ) -> Result<String, String> {
        let line = match pointer {
            Pointer::Click {
                at: Target::Point(x, y),
                button,
                count,
            } => {
                format!("pointer click {x} {y} {button:?} {count} in {window:?}")
            }
            Pointer::Click {
                at: Target::Element(element),
                button,
                count,
            } => {
                format!("pointer click element {element} {button:?} {count}")
            }
            Pointer::Drag { from, to } => format!("pointer drag {from:?} {to:?}"),
        };
        self.log.push(line);
        Ok("Clicked with the pointer.".into())
    }
}

fn engine(fake: Fake) -> Engine<Fake> {
    Engine::new(fake)
        .policy(Arc::new(|app| {
            if sensitive(app.id) {
                Err(format!("{} is blocked", app.id))
            } else {
                Ok(())
            }
        }))
        .host("Tester")
        .settle(Duration::ZERO, Duration::from_millis(600))
}

fn index(text: &str, label: &str) -> u64 {
    text.lines()
        .find(|line| line.contains(label))
        .and_then(|line| {
            line.trim()
                .strip_prefix('[')?
                .split_once(']')?
                .0
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("{label} in {text}"))
}

#[test]
fn a_state_numbers_elements_masks_secrets_and_screenshots_by_default() {
    let mut engine = engine(Fake::new());
    let step = engine
        .run("get_app_state", &json!({"app": "fixture"}), 1024)
        .unwrap();
    assert_eq!(
        step.text,
        "Fixture — window \"Fixture\"\nWindow \"Fixture\"\n  [0] Button \"7\"\n  [1] TextField \"Name\" value=\"naïve cat\" (focused)\n  [2] TextField \"Password\" value=\"••••\""
    );
    assert_eq!(step.screenshot.as_deref(), Some("/9j/AAAA"));
    let small = engine
        .run("get_app_state", &json!({"app": "Fixture"}), 4)
        .unwrap();
    assert!(small.screenshot.is_none() && small.omitted);
    let none = engine
        .run(
            "get_app_state",
            &json!({"app": "Fixture", "screenshot": false}),
            1024,
        )
        .unwrap();
    assert!(none.screenshot.is_none() && !none.omitted);
}

#[test]
fn every_action_returns_the_whole_new_state_and_old_indexes_die() {
    let mut engine = engine(Fake::new());
    let read = engine
        .run("get_app_state", &json!({"app": "Fixture"}), 1024)
        .unwrap();
    let seven = index(&read.text, "Button \"7\"");
    let clicked = engine
        .run(
            "click",
            &json!({"app": "Fixture", "element_index": seven}),
            1024,
        )
        .unwrap();
    assert!(
        clicked.text.starts_with("Clicked.\nFixture — window"),
        "{}",
        clicked.text
    );
    assert!(clicked.screenshot.is_none());
    assert!(engine.backend().log.contains(&"press 1 Primary".to_owned()));
    engine.backend().generation += 1;
    let stale = engine
        .run(
            "click",
            &json!({"app": "Fixture", "element_index": seven}),
            1024,
        )
        .err()
        .unwrap();
    assert!(stale.contains("no longer in Fixture"), "{stale}");
    let missing = engine
        .run(
            "click",
            &json!({"app": "Fixture", "element_index": 40}),
            1024,
        )
        .err()
        .unwrap();
    assert!(
        missing.contains("not in Fixture's latest state"),
        "{missing}"
    );
}

#[test]
fn actions_map_to_backend_calls() {
    let mut engine = engine(Fake::new());
    engine
        .run("get_app_state", &json!({"app": "Fixture"}), 0)
        .unwrap();
    let field = 1;
    engine
        .run(
            "click",
            &json!({"app": "Fixture", "element_index": 0, "mouse_button": "right"}),
            0,
        )
        .unwrap();
    engine.backend().generation = 0;
    engine
        .run("get_app_state", &json!({"app": "Fixture"}), 0)
        .unwrap();
    engine
        .run(
            "select_text",
            &json!({"app": "Fixture", "element_index": field, "text": "cat"}),
            0,
        )
        .unwrap();
    engine
        .run(
            "set_value",
            &json!({"app": "Fixture", "element_index": field, "value": "x"}),
            0,
        )
        .unwrap();
    engine
        .run(
            "scroll",
            &json!({"app": "Fixture", "element_index": field, "direction": "up", "pages": 2}),
            0,
        )
        .unwrap();
    engine
        .run(
            "perform_secondary_action",
            &json!({"app": "Fixture", "element_index": field, "ax_action": "ShowMenu"}),
            0,
        )
        .unwrap();
    let pressed = engine
        .run("press_key", &json!({"app": "Fixture", "key": "cmd+a"}), 0)
        .unwrap();
    assert!(
        pressed.text.starts_with("Pressed cmd+a."),
        "{}",
        pressed.text
    );
    assert_eq!(
        engine
            .backend()
            .log
            .iter()
            .filter(|line| !line.starts_with("read"))
            .cloned()
            .collect::<Vec<_>>(),
        [
            "press 1 Menu",
            "select 2 (6, 3)",
            "set 2 x",
            "scroll 2 up 2",
            "perform 2 ShowMenu",
            &format!("key {:?}", Key::Char('a')),
        ]
    );
}

#[test]
fn typing_needs_a_read_state_and_unchanged_focus() {
    let mut engine = engine(Fake::new());
    let unread = engine
        .run("type_text", &json!({"app": "Fixture", "text": "secret"}), 0)
        .err()
        .unwrap();
    assert!(unread.contains("before typing"), "{unread}");
    engine
        .run("get_app_state", &json!({"app": "Fixture"}), 0)
        .unwrap();
    let typed = engine
        .run("type_text", &json!({"app": "Fixture", "text": "secret"}), 0)
        .unwrap();
    assert!(typed.text.starts_with("Typed text."));
    assert!(!typed.text.contains("secret"));
    engine.backend().focused = Some(3);
    for action in ["type_text", "press_key"] {
        let moved = engine
            .run(
                action,
                &json!({"app": "Fixture", "text": "x", "key": "Return"}),
                0,
            )
            .err()
            .unwrap();
        assert!(moved.contains("focused element changed"), "{moved}");
    }
    engine.backend().focused = None;
    engine
        .run("get_app_state", &json!({"app": "Fixture"}), 0)
        .unwrap();
    let unfocused = engine
        .run("type_text", &json!({"app": "Fixture", "text": "x"}), 0)
        .err()
        .unwrap();
    assert!(
        unfocused.contains("No element of Fixture is focused"),
        "{unfocused}"
    );
}

#[test]
fn the_pointer_is_refused_unless_the_backend_and_host_allow_it() {
    let pointer_inputs = [
        ("click", json!({"app": "Fixture", "x": 10, "y": 20})),
        (
            "click",
            json!({"app": "Fixture", "element_index": 0, "click_count": 2}),
        ),
        (
            "click",
            json!({"app": "Fixture", "element_index": 0, "mouse_button": "middle"}),
        ),
        (
            "drag",
            json!({"app": "Fixture", "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4}),
        ),
    ];
    for (pointer, allowed) in [(false, true), (true, false)] {
        let mut fake = Fake::new();
        fake.pointer = pointer;
        let mut engine = engine(fake).pointer(allowed);
        assert!(!engine.capabilities().pointer);
        engine
            .run("get_app_state", &json!({"app": "Fixture"}), 0)
            .unwrap();
        for (action, input) in &pointer_inputs {
            let why = engine.run(action, input, 0).err().unwrap();
            assert!(why.contains("pointer"), "{why}");
        }
        assert!(
            engine
                .backend()
                .log
                .iter()
                .all(|line| !line.starts_with("pointer"))
        );
    }
    let mut fake = Fake::new();
    fake.pointer = true;
    let mut engine = engine(fake).pointer(true);
    assert!(engine.capabilities().pointer);
    engine
        .run("get_app_state", &json!({"app": "Fixture"}), 0)
        .unwrap();
    for (action, input) in &pointer_inputs {
        engine.run(action, input, 0).unwrap();
        engine.backend().generation = 0;
        engine
            .run("get_app_state", &json!({"app": "Fixture"}), 0)
            .unwrap();
    }
    let pointed: Vec<_> = engine
        .backend()
        .log
        .iter()
        .filter(|line| line.starts_with("pointer"))
        .cloned()
        .collect();
    assert_eq!(
        pointed,
        [
            format!("pointer click 10 20 {:?} 1 in Some(0)", Button::Left),
            format!("pointer click element 1 {:?} 2", Button::Left),
            format!("pointer click element 1 {:?} 1", Button::Middle),
            "pointer drag (1.0, 2.0) (3.0, 4.0)".to_owned(),
        ]
    );
}

#[test]
fn apps_resolve_launch_and_follow_the_policy() {
    let mut engine = engine(Fake::new());
    let notes = engine
        .run(
            "get_app_state",
            &json!({"app": "notes", "screenshot": false}),
            0,
        )
        .unwrap();
    assert!(notes.text.starts_with("Notes — window"), "{}", notes.text);
    assert_eq!(engine.backend().log[0], "launch com.example.notes");
    let blocked = engine
        .run("get_app_state", &json!({"app": "Terminal"}), 0)
        .err()
        .unwrap();
    assert!(
        blocked.contains("com.apple.Terminal is blocked"),
        "{blocked}"
    );
    assert!(
        !engine
            .backend()
            .log
            .iter()
            .any(|line| line.contains("Terminal"))
    );
    let missing = engine
        .run("get_app_state", &json!({"app": "Xcode"}), 0)
        .err()
        .unwrap();
    assert!(missing.contains("isn't an installed app"), "{missing}");
    assert!(
        engine
            .run("get_app_state", &json!({}), 0)
            .err()
            .unwrap()
            .contains("app is required")
    );
    let listed = engine.run("list_apps", &json!({}), 0).unwrap();
    assert_eq!(
        listed.text,
        "Fixture (org.example.fixture) pid 7, windows on screen\nNotes (com.example.notes) pid 9, windows on screen"
    );
    engine.backend().running.push(App {
        pid: 11,
        name: "Terminal".into(),
        id: "com.apple.Terminal".into(),
        ..App::default()
    });
    let running = engine
        .run("get_app_state", &json!({"app": "com.apple.terminal"}), 0)
        .err()
        .unwrap();
    assert!(
        running.starts_with("Terminal: com.apple.Terminal is blocked"),
        "{running}"
    );
}

#[test]
fn an_app_is_refused_when_any_of_its_identities_is() {
    let mut fake = Fake::new();
    fake.running.push(App {
        pid: 12,
        name: "Calm".into(),
        id: "org.example.Calm".into(),
        aliases: vec!["calm.py".into(), "xterm".into()],
        ..App::default()
    });
    fake.installed.push(App {
        name: "Harmless".into(),
        id: "org.example.Harmless".into(),
        aliases: vec!["xterm".into()],
        path: "/apps/org.example.Harmless.desktop".into(),
        ..App::default()
    });
    let mut engine = engine(fake);
    let running = engine
        .run("get_app_state", &json!({"app": "calm.py"}), 0)
        .err()
        .unwrap();
    assert!(running.starts_with("Calm: xterm is blocked"), "{running}");
    let installed = engine
        .run("get_app_state", &json!({"app": "Harmless"}), 0)
        .err()
        .unwrap();
    assert!(
        installed.starts_with("Harmless: xterm is blocked"),
        "{installed}"
    );
    assert!(
        engine.backend().log.is_empty(),
        "{:?}",
        engine.backend().log
    );
}

#[test]
fn an_unready_backend_runs_nothing() {
    let mut fake = Fake::new();
    fake.trusted = false;
    let mut engine = engine(fake);
    for action in ["list_apps", "get_app_state", "click"] {
        let why = engine
            .run(action, &json!({"app": "Fixture", "element_index": 0}), 0)
            .err()
            .unwrap();
        assert!(why.contains("Accessibility permission for Tester"), "{why}");
    }
    assert!(engine.backend().log.is_empty());
}

#[test]
fn background_apps_without_windows_are_not_listed_and_found_last() {
    let mut fake = Fake::new();
    fake.running.insert(
        0,
        App {
            pid: 3,
            name: "Fixture Helper".into(),
            id: "org.example.helper".into(),
            background: true,
            ..App::default()
        },
    );
    let mut engine = engine(fake);
    let listed = engine.run("list_apps", &json!({}), 0).unwrap();
    assert!(!listed.text.contains("Helper"), "{}", listed.text);
    let found = super::find(engine.backend().running.clone(), "fix").unwrap();
    assert_eq!(found.pid, 7);
    let by_path = super::find(engine.backend().running.clone(), "/apps/Fixture.app/").unwrap();
    assert_eq!(by_path.pid, 7);
}

#[test]
fn leases_key_on_the_bundle_only_where_the_backend_has_bundles() {
    let mut bundles = Engine::new(Fake {
        bundles: true,
        ..Fake::new()
    });
    assert_eq!(
        bundles.lease_key("Fixture").as_deref(),
        Some("org.example.fixture"),
        "a running app by name"
    );
    assert_eq!(
        bundles.lease_key("/apps/Fixture.app").as_deref(),
        Some("org.example.fixture"),
        "by path"
    );
    assert_eq!(
        bundles.lease_key("org.example.fixture").as_deref(),
        Some("org.example.fixture"),
        "by bundle"
    );
    assert_eq!(
        bundles.lease_key("Notes").as_deref(),
        Some("com.example.notes"),
        "an installed app that isn't running"
    );
    assert_eq!(
        bundles.lease_key("Nothing"),
        None,
        "unresolved: the name is the key"
    );
    assert!(
        bundles
            .backend
            .log
            .iter()
            .all(|entry| !entry.starts_with("launch")),
        "resolving launches nothing"
    );
    let mut names = Engine::new(Fake::new());
    assert_eq!(
        names.lease_key("Fixture"),
        None,
        "without bundles the name is the key"
    );
}

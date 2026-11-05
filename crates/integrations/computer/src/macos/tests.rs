use super::{Ax, app_bundle, ax, ax_action, installed};
use crate::{Backend, Engine, policy::sensitive};
use serde_json::json;
use std::{process::Command, sync::Arc};

fn engine() -> Engine<Ax> {
    Engine::new(Ax::default()).policy(Arc::new(|app| {
        if sensitive(app.id) || app.id.is_empty() {
            Err(format!("{} is blocked by this test's policy", app.id))
        } else {
            Ok(())
        }
    }))
}

#[test]
fn only_an_apps_main_executable_counts() {
    assert_eq!(
        app_bundle("/System/Applications/Calculator.app/Contents/MacOS/Calculator"),
        Some("/System/Applications/Calculator.app")
    );
    assert_eq!(
        app_bundle(
            "/Applications/Slack.app/Contents/Frameworks/Slack Helper.app/Contents/MacOS/Slack Helper"
        ),
        None
    );
    assert_eq!(app_bundle("/usr/bin/login"), None);
}

#[test]
fn installed_apps_are_read_without_permissions() {
    let calculator = installed("calculator").unwrap();
    assert!(calculator.ends_with("/Calculator.app"), "{calculator}");
    let (bundle, name, background) = ax::bundle_info(&calculator);
    assert_eq!(
        (bundle.as_str(), name.as_str(), background),
        ("com.apple.calculator", "Calculator", false)
    );
    let (bundle, _, _) = ax::bundle_info("/System/Applications/Utilities/Terminal.app");
    assert!(sensitive(&bundle), "{bundle}");
    assert!(installed("No Such App 38").is_none());
    let mut backend = Ax::default();
    assert!(
        backend
            .apps()
            .iter()
            .any(|app| app.id == "com.apple.finder")
    );
    let by_bundle = backend.installed("com.apple.calculator").unwrap();
    assert_eq!(
        (by_bundle.id.as_str(), by_bundle.path.as_str()),
        ("com.apple.calculator", "")
    );
    let by_name = backend.installed("Calculator").unwrap();
    assert_eq!(by_name.id, "com.apple.calculator");
    assert!(by_name.path.ends_with("/Calculator.app"));
}

#[test]
fn actions_take_their_ax_prefix() {
    assert_eq!(ax_action("ShowMenu"), "AXShowMenu");
    assert_eq!(ax_action("AXIncrement"), "AXIncrement");
}

#[test]
fn an_untrusted_process_runs_nothing() {
    let trusted = ax::trusted();
    let refused = engine().run("drag", &json!({"app": "com.apple.Terminal"}), 1024);
    let why = refused.err().unwrap();
    if trusted {
        assert!(why.contains("blocked"), "{why}");
    } else {
        assert!(why.contains("Accessibility permission"), "{why}");
    }
}

fn running(bundle: &str) -> bool {
    Ax::default().apps().iter().any(|app| app.id == bundle)
}

fn quit_unless(was_running: bool) {
    if !was_running {
        let _ = Command::new("/usr/bin/osascript")
            .args(["-e", "tell application id \"com.apple.calculator\" to quit"])
            .status();
    }
}

#[test]
#[ignore = "needs macOS, a desktop session and Accessibility permission"]
fn live_desktop_reads_and_operates_calculator() {
    let was_running = running("com.apple.calculator");
    let mut engine = engine();
    let read = engine
        .run(
            "get_app_state",
            &json!({"app": "Calculator", "screenshot": false}),
            1 << 20,
        )
        .unwrap();
    let index = |text: &str, label: &str| {
        text.lines()
            .find(|line| line.contains(label))
            .and_then(|line| {
                line.trim()
                    .strip_prefix('[')?
                    .split_once(']')?
                    .0
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or_else(|| panic!("{text}"))
    };
    let seven = index(&read.text, "Button \"7\"");
    let clicked = engine
        .run(
            "click",
            &json!({"app": "Calculator", "element_index": seven}),
            1 << 20,
        )
        .unwrap();
    assert!(
        clicked.text.starts_with("Clicked.\nCalculator") && clicked.text.contains("Button \"7\""),
        "{}",
        clicked.text
    );
    for refused in [
        json!({"app": "Calculator", "x": 10, "y": 10}),
        json!({"app": "Calculator", "element_index": seven, "click_count": 2}),
        json!({"app": "Calculator", "element_index": seven, "mouse_button": "middle"}),
    ] {
        let why = engine.run("click", &refused, 1 << 20).err().unwrap();
        assert!(why.contains("pointer"), "{why}");
    }
    let dragged = engine.run(
        "drag",
        &json!({"app": "Calculator", "from_x": 1, "from_y": 1, "to_x": 9, "to_y": 9}),
        1 << 20,
    );
    assert!(dragged.err().unwrap().contains("pointer"));
    let stale = engine
        .run(
            "click",
            &json!({"app": "Calculator", "element_index": 100_000}),
            1 << 20,
        )
        .err()
        .unwrap();
    assert!(stale.contains("latest state"), "{stale}");
    let pressed = engine
        .run(
            "press_key",
            &json!({"app": "Calculator", "key": "Escape"}),
            1 << 20,
        )
        .unwrap();
    assert!(
        pressed.text.starts_with("Pressed Escape."),
        "{}",
        pressed.text
    );
    assert!(
        engine
            .run(
                "get_app_state",
                &json!({"app": "com.apple.Terminal"}),
                1 << 20
            )
            .err()
            .unwrap()
            .contains("blocked")
    );
    quit_unless(was_running);
}

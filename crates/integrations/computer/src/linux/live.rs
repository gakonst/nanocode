use super::AtSpi;
use crate::{Backend, Chord, Computer, Native, Provider};
use nanocode_tool_contracts::ToolOutput;
use serde_json::{Value, json};
use std::time::Duration;

const APP: &str = "fixture";

async fn step(computer: &Computer, mut input: Value) -> (bool, String, Value) {
    if input["app"].is_null() {
        input["app"] = APP.into();
    }
    let output: ToolOutput = computer.step(&input, "live").await;
    let structured = output.structured_result();
    let text = structured["state"]
        .as_str()
        .or_else(|| structured.as_str())
        .unwrap_or_default()
        .to_owned();
    (output.success, text, structured)
}

fn index(state: &str, line: &str) -> u64 {
    state
        .lines()
        .find(|row| row.contains(line))
        .and_then(|row| row.trim().strip_prefix('['))
        .and_then(|row| row.split(']').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no element {line} in\n{state}"))
}

fn line<'a>(state: &'a str, wanted: &str) -> &'a str {
    state
        .lines()
        .find(|row| row.contains(wanted))
        .unwrap_or_else(|| panic!("no line {wanted} in\n{state}"))
}

async fn ok(computer: &Computer, input: Value) -> String {
    let (success, text, _) = step(computer, input.clone()).await;
    assert!(success, "{input} failed: {text}");
    text
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a session without a desktop"]
async fn refuses_cleanly_without_a_desktop() {
    let computer = Computer::new(Provider::native(Native::default()));
    assert!(!computer.capabilities().screenshots && !computer.capabilities().pointer);
    let started = std::time::Instant::now();
    let (success, refused, _) = step(&computer, json!({"action": "get_app_state"})).await;
    assert!(
        !success && refused.starts_with("Computer use needs a desktop session"),
        "{refused}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an X11 display and the AT-SPI bus"]
async fn drives_a_gtk_app_over_at_spi_and_x11() {
    let computer = Computer::new(Provider::native(Native::default()));
    assert!(computer.capabilities().screenshots);
    let listed = computer.step(&json!({"action": "list_apps"}), "live").await;
    let listed = listed.structured_result()["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(listed.contains(APP), "{listed}");

    let started = std::time::Instant::now();
    let (success, state, structured) = step(&computer, json!({"action": "get_app_state"})).await;
    let elapsed = started.elapsed();
    let nodes = state.lines().count();
    println!(
        "get_app_state with a screenshot in {elapsed:?}, {nodes} lines:\n{}",
        state.lines().take(16).collect::<Vec<_>>().join("\n")
    );
    assert!(nodes > 1000 && !state.contains("cut short"), "{state}");
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert!(success, "{state}");
    assert!(state.starts_with("fixture — window \"Fixture\""), "{state}");
    let screenshot = &structured["screenshot"];
    assert_eq!(screenshot["media_type"], "image/jpeg");
    assert!(
        screenshot["data"]
            .as_str()
            .is_some_and(|data| data.starts_with("/9j/"))
    );

    let seven = index(&state, "PushButton \"7\"");
    let state = ok(
        &computer,
        json!({"action": "click", "element_index": seven}),
    )
    .await;
    assert!(state.starts_with("Clicked."), "{state}");
    assert!(state.contains("Label \"Display: 7\""), "{state}");

    let name = index(&state, "\"Name\"");
    let state = ok(
        &computer,
        json!({"action": "set_value", "element_index": name, "value": "hello"}),
    )
    .await;
    assert!(
        line(&state, "\"Name\"").contains("value=\"hello\""),
        "{state}"
    );
    assert!(line(&state, "\"Name\"").contains("focused"), "{state}");
    let state = ok(&computer, json!({"action": "type_text", "text": " world"})).await;
    assert!(state.starts_with("Typed text."), "{state}");
    assert!(line(&state, "\"Name\"").contains("world"), "{state}");
    let name = index(&state, "\"Name\"");
    let state = ok(
        &computer,
        json!({"action": "select_text", "element_index": name, "text": "world"}),
    )
    .await;
    assert!(state.starts_with("Selected text."), "{state}");
    let state = ok(&computer, json!({"action": "type_text", "text": "there"})).await;
    assert!(
        line(&state, "\"Name\"").contains("value=\"hello there\""),
        "{state}"
    );

    let password = index(&state, "\"Password\"");
    let state = ok(
        &computer,
        json!({"action": "set_value", "element_index": password, "value": "hunter2"}),
    )
    .await;
    assert!(
        line(&state, "\"Password\"").contains("value=\"••••\""),
        "{state}"
    );
    assert!(!state.contains("hunter2"), "{state}");
    let state = ok(
        &computer,
        json!({"action": "click", "element_index": password}),
    )
    .await;
    assert!(state.starts_with("Focused it."), "{state}");
    assert!(line(&state, "\"Password\"").contains("focused"), "{state}");
    assert!(!line(&state, "\"Name\"").contains("focused"), "{state}");
    let state = ok(&computer, json!({"action": "type_text", "text": "!"})).await;
    assert!(
        line(&state, "\"Password\"").contains("value=\"••••\""),
        "{state}"
    );
    let name = index(&state, "\"Name\"");
    ok(&computer, json!({"action": "click", "element_index": name})).await;

    let check = index(&state, "\"Remember me\"");
    assert!(
        !line(&state, "\"Remember me\"").contains("checked"),
        "{state}"
    );
    let state = ok(
        &computer,
        json!({"action": "click", "element_index": check}),
    )
    .await;
    assert!(
        line(&state, "\"Remember me\"").contains("(checked"),
        "{state}"
    );

    let volume = index(&state, "\"Volume\"");
    let state = ok(
        &computer,
        json!({"action": "set_value", "element_index": volume, "value": "42"}),
    )
    .await;
    assert!(
        line(&state, "\"Volume\"").contains("value=\"42\""),
        "{state}"
    );

    let bar = |state: &str| {
        state
            .lines()
            .filter(|row| row.contains("ScrollBar"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    };
    let before = bar(&state);
    let notes = index(&state, "\"Notes\"");
    let state = ok(
        &computer,
        json!({"action": "scroll", "element_index": notes, "direction": "down", "pages": 2}),
    )
    .await;
    assert!(state.starts_with("Scrolled down."), "{state}");
    assert_ne!(bar(&state), before, "{state}");

    let state = ok(&computer, json!({"action": "press_key", "key": "ctrl+F5"})).await;
    assert!(state.starts_with("Pressed ctrl+F5."), "{state}");
    assert!(state.contains("Label \"Key: ctrl+F5\""), "{state}");
    let state = ok(&computer, json!({"action": "press_key", "key": "€"})).await;
    assert!(state.contains("Label \"Key: U+20AC\""), "{state}");
    assert!(
        line(&state, "\"Name\"").contains("value=\"hello there€\""),
        "{state}"
    );

    let vanish = index(&state, "PushButton \"Vanish\"");
    let clicked = std::time::Instant::now();
    let state = ok(
        &computer,
        json!({"action": "click", "element_index": vanish}),
    )
    .await;
    println!("click and read in {:?}", clicked.elapsed());
    assert!(state.contains("PushButton \"Vanish\""), "{state}");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (success, refused, _) = step(
        &computer,
        json!({"action": "click", "element_index": vanish}),
    )
    .await;
    assert!(!success && refused.contains("is no longer in"), "{refused}");
    let (success, refused, _) =
        step(&computer, json!({"action": "click", "element_index": 9999})).await;
    assert!(!success && refused.contains("is not in"), "{refused}");

    let (success, refused, _) = step(&computer, json!({"action": "click", "x": 30, "y": 20})).await;
    assert!(
        !success && refused.contains("needs the pointer"),
        "{refused}"
    );

    let pointer = Computer::new(Provider::native(Native::default().pointer(true)));
    let state = ok(
        &pointer,
        json!({"action": "get_app_state", "screenshot": false}),
    )
    .await;
    assert!(state.contains("Label \"Display: 7\""), "{state}");
    let state = ok(&pointer, json!({"action": "click", "x": 30, "y": 20})).await;
    assert!(state.contains("Label \"Display: 77\""), "{state}");
    let seven = index(&state, "PushButton \"7\"");
    let state = ok(
        &pointer,
        json!({"action": "click", "element_index": seven, "click_count": 2}),
    )
    .await;
    assert!(state.contains("Label \"Display: 7777\""), "{state}");
    assert!(
        line(&state, "PushButton \"7\"").contains("focused"),
        "{state}"
    );
    let state = ok(&pointer, json!({"action": "type_text", "text": "q"})).await;
    assert!(state.starts_with("Typed text as keys."), "{state}");
    assert!(state.contains("Label \"Key: q\""), "{state}");
    let state = ok(
        &pointer,
        json!({"action": "drag", "from_x": 30, "from_y": 20, "to_x": 120, "to_y": 40}),
    )
    .await;
    assert!(state.starts_with("Dragged."), "{state}");

    let state = ok(
        &computer,
        json!({"action": "get_app_state", "app": "Second", "screenshot": false}),
    )
    .await;
    assert!(state.starts_with("second — window \"Second\""), "{state}");
    let listed = computer.step(&json!({"action": "list_apps"}), "live").await;
    let listed = listed.structured_result()["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    println!("list_apps:\n{listed}");
    assert!(
        listed.contains("second (python3") && listed.contains("windows on screen"),
        "{listed}"
    );
    let (success, refused, _) =
        step(&computer, json!({"action": "press_key", "key": "Return"})).await;
    println!("press_key without focus: {refused}");
    assert!(
        !success && refused.contains("nothing was sent"),
        "{refused}"
    );
    let refused = std::thread::spawn(|| {
        let mut backend = AtSpi::new();
        backend.ready("live")?;
        let app = backend
            .apps()
            .into_iter()
            .find(|app| app.name == APP)
            .ok_or("no fixture")?;
        backend.press_key(&app, Chord::parse("Return")?)
    })
    .join()
    .unwrap();
    assert_eq!(
        refused,
        Err("press_key reaches only the window with keyboard focus, and fixture doesn't have it, so nothing was sent.".into())
    );
}

use super::{Computer, Provider, definition, step_line};
use crate::{Capabilities, Desktop, Step};
use serde_json::{Value, json};

#[test]
fn step_lines_name_the_action_and_app_but_not_typed_text() {
    let line = |input| step_line(&json!(input));
    assert_eq!(
        line(json!({"action": "click", "app": "Calculator", "element_index": 12})),
        "Clicked element 12 in Calculator"
    );
    assert_eq!(
        line(json!({"action": "click", "app": "Notes", "x": 10.4, "y": 20})),
        "Clicked (10, 20) in Notes"
    );
    assert_eq!(
        line(json!({"action": "type_text", "app": "Notes", "text": "secret"})),
        "Typed text in Notes"
    );
    assert_eq!(
        line(json!({"action": "get_app_state", "app": "Notes"})),
        "Read Notes"
    );
    assert_eq!(line(json!({"action": "list_apps"})), "Listed apps");
    assert_eq!(
        line(json!({"action": "press_key", "app": "x".repeat(300), "key": "Return"}))
            .chars()
            .count(),
        200
    );
}

#[cfg(target_os = "macos")]
#[test]
fn the_mac_definition_is_pyramids_pinned_one() {
    let pinned: Value = serde_json::from_str(include_str!("pinned.json")).unwrap();
    let full = Capabilities {
        pointer: true,
        screenshots: true,
        diffs: true,
        policy: true,
    };
    assert_eq!(serde_json::to_value(definition(full)).unwrap(), pinned);
}

#[test]
fn one_schema_on_every_platform_with_capabilities_in_the_description() {
    let pinned: Value = serde_json::from_str(include_str!("pinned.json")).unwrap();
    let bare = serde_json::to_value(definition(Capabilities::default())).unwrap();
    assert_eq!(bare["parameters"], pinned["parameters"]);
    assert_eq!(bare["name"], "computer");
    let text = bare["description"].as_str().unwrap();
    assert!(
        !text.contains("x and y") && !text.contains("drag ("),
        "{text}"
    );
    assert!(
        !text.contains("screenshot adds") && !text.contains("are refused"),
        "{text}"
    );
    assert!(text.contains("whole new state"), "{text}");
    let pointer = serde_json::to_value(definition(Capabilities {
        pointer: true,
        screenshots: true,
        diffs: false,
        policy: true,
    }))
    .unwrap();
    let text = pointer["description"].as_str().unwrap();
    assert!(
        text.contains("x and y") && text.contains("screenshot adds") && text.contains("refused")
    );
}

struct Canned(&'static str);

impl Desktop for Canned {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn step(&mut self, action: &str, input: &Value, max: usize) -> Result<Step, String> {
        if input["fail"] == true {
            return Err(format!("{} failed {action}", self.0));
        }
        Ok(Step {
            text: format!("{} ran {action} for {}", self.0, input["app"]),
            screenshot: (max > 0 && input["screenshot"] == true).then(|| "/9j/".into()),
            omitted: input["omit"] == true,
        })
    }
}

fn said(output: &nanocode_tool_contracts::ToolOutput) -> String {
    format!(
        "{:?} {}",
        output.output,
        serde_json::to_string(&output.structured_result()).unwrap()
    )
}

#[tokio::test]
async fn steps_route_to_the_named_provider_and_shape_their_output() {
    let computer = Computer::new(Provider::desktop(Box::new(Canned("main"))))
        .or("native", Provider::desktop(Box::new(Canned("own"))))
        .or(
            "gone",
            Provider::missing("Computer use is not installed here."),
        );
    let main = computer
        .step(
            &json!({"action": "get_app_state", "app": "Notes", "screenshot": true}),
            "s",
        )
        .await;
    assert!(main.success);
    assert_eq!(
        main.structured_result(),
        json!({"state": "main ran get_app_state for \"Notes\"", "screenshot": {"media_type": "image/jpeg", "data": "/9j/"}})
    );
    let own = computer
        .step(
            &json!({"action": "click", "app": "Notes", "_provider": "native", "omit": true}),
            "s",
        )
        .await;
    assert_eq!(
        own.structured_result(),
        json!({"state": "own ran click for \"Notes\"", "screenshot_omitted": "The window's screenshot was too large to return; use the state."})
    );
    let unknown = computer
        .step(&json!({"action": "list_apps", "_provider": "nope"}), "s")
        .await;
    assert!(said(&unknown).contains("main ran list_apps"));
    let gone = computer
        .step(&json!({"action": "list_apps", "_provider": "gone"}), "s")
        .await;
    assert!(!gone.success && said(&gone).contains("not installed here"));
    let failed = computer
        .step(&json!({"action": "list_apps", "fail": true}), "s")
        .await;
    assert!(!failed.success && said(&failed).contains("main failed list_apps"));
    let bad = computer.step(&json!({"action": "launch"}), "s").await;
    assert!(!bad.success && said(&bad).contains("action must be one of list_apps, get_app_state"));
}

#[tokio::test]
async fn a_computer_without_a_provider_runs_nothing() {
    let computer = Computer::new(Provider::missing(
        "Computer use is not installed with this Hand, so nothing ran.",
    ));
    let output = computer.step(&json!({"action": "list_apps"}), "turn").await;
    assert!(!output.success);
    assert!(said(&output).contains("not installed"));
}

#[tokio::test]
async fn long_states_are_clipped_on_a_character_boundary() {
    struct Long;
    impl Desktop for Long {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn step(&mut self, _: &str, _: &Value, _: usize) -> Result<Step, String> {
            Ok(Step {
                text: "é".repeat(200 * 1024),
                screenshot: None,
                omitted: false,
            })
        }
    }
    let output = Computer::new(Provider::desktop(Box::new(Long)))
        .step(&json!({"action": "list_apps"}), "s")
        .await;
    let state = output.structured_result()["state"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(state.len() < 192 * 1024 + 100, "{}", state.len());
    assert!(state.ends_with("(state cut at 192 KiB; read a smaller part of the app)"));
}

#[test]
fn engines_box_as_desktops() {
    fn desktop<D: Desktop>(_: &D) {}
    #[cfg(target_os = "macos")]
    desktop(&crate::Engine::new(crate::macos::Ax::default()));
    #[cfg(target_os = "linux")]
    desktop(&crate::Engine::new(crate::linux::AtSpi::new()));
    let _ = desktop::<Canned>;
}

use super::{
    a11y::{Interfaces, Role, States},
    atspi::{
        self, ACCESSIBLE, COMPONENT, COORD_TYPE_SCREEN, EDITABLE_TEXT, Element, SELECTION, TEXT,
        VALUE, children, extents, interfaces, parent, proxy, role, states, timed,
    },
    dbus::{Bus, Error},
    wire::Value,
};
use crate::{Direction, Press, Selection};
use std::{future::Future, time::Duration};
use tokio::time::timeout;

const ACTION: Duration = Duration::from_secs(5);
const GONE: &str = "That element is gone, so nothing ran. Read the app again with get_app_state.";
const UNSUPPORTED: &str = "That element does not support this action, so nothing ran.";
const NOT_SETTABLE: &str =
    "That field can't be set directly, so nothing ran. Click it, then use type_text.";
const SLOW: &str = "The app did not answer in time; whether it took effect is unknown. Read its state before trying again.";
const REFUSED: &str = "The app refused it, so nothing ran.";
const NOTHING: &str = "That element has nothing to scroll, so nothing ran.";
const FRONT: &str = "Typing into that field needs its window in front of the user's, and it takes no text directly, so nothing ran. For a web page use the browser tool.";
const KEPT: &str = "Chose it for type_text, which now types into it; its window stays in the background, so the keyboard focus the state shows is unchanged.";
const CLICK: [&str; 2] = ["click", "press"];
const PRIMARY: [&str; 4] = ["activate", "toggle", "jump", "open"];
const MENU: [&str; 4] = ["popup", "menu", "showmenu", "contextmenu"];
const SCROLL_TOP_EDGE: u32 = 2;
const SCROLL_BOTTOM_EDGE: u32 = 3;
const SCROLL_LEFT_EDGE: u32 = 4;
const SCROLL_RIGHT_EDGE: u32 = 5;

async fn acted<T>(work: impl Future<Output = Result<T, Error>>) -> Result<T, String> {
    match timeout(ACTION, work).await {
        Err(_) => Err(SLOW.into()),
        Ok(Ok(value)) => Ok(value),
        Ok(Err(Error::Method(name))) => {
            let name = name.as_str();
            Err(
                if name.ends_with("UnknownMethod")
                    || name.ends_with("UnknownInterface")
                    || name.ends_with("NotSupported")
                {
                    UNSUPPORTED.into()
                } else if name.ends_with("UnknownObject")
                    || name.ends_with("ServiceUnknown")
                    || name.ends_with("NameHasNoOwner")
                {
                    GONE.into()
                } else {
                    format!(
                        "The app refused it ({name}); whether it took effect is unknown. Read its state before trying again."
                    )
                },
            )
        }
        Ok(Err(error)) => Err(format!(
            "The app did not take it ({error}); whether it took effect is unknown. Read its state before trying again."
        )),
    }
}

fn done(ok: bool, done: &str) -> Result<String, String> {
    if ok {
        Ok(format!("{done}."))
    } else {
        Err(REFUSED.into())
    }
}

pub fn normal(action: &str) -> String {
    let action = action.trim();
    let action = action.strip_prefix("AX").unwrap_or(action);
    action
        .chars()
        .filter(|c| !matches!(c, ' ' | '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn list(actions: &[String]) -> String {
    if actions.is_empty() {
        "none".into()
    } else {
        actions.join(", ")
    }
}

pub fn chosen(actions: &[String], wanted: &[&str]) -> Option<usize> {
    wanted
        .iter()
        .find_map(|wanted| actions.iter().position(|action| normal(action) == *wanted))
}

async fn perform_at(
    bus: &Bus,
    element: &Element,
    index: usize,
    what: &str,
) -> Result<String, String> {
    let action = proxy(bus, element).ok_or(GONE)?;
    let index = Value::I32(index as i32);
    done(
        acted(action.call(atspi::ACTION, "DoAction", &[index])).await?,
        what,
    )
}

pub async fn press(
    bus: &Bus,
    element: &Element,
    press: Press,
    grab: bool,
) -> Result<String, String> {
    let actions = atspi::actions(bus, element).await;
    if press == Press::Menu {
        return match chosen(&actions, &MENU) {
            Some(index) => perform_at(bus, element, index, "Opened its menu").await,
            None => Err(format!(
                "That element has no menu to open (its actions: {}), so nothing ran.",
                list(&actions)
            )),
        };
    }
    let (interfaces, role) = tokio::join!(interfaces(bus, element), role(bus, element));
    if !grab && role == Some(Role::COMBO_BOX) {
        let names: Vec<String> = options(bus, element)
            .await
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        return Err(format!(
            "Opening its list would show a menu over the user's windows, so nothing ran. Choose an option with set_value and its text (its options: {}).",
            list(&names)
        ));
    }
    let entry = interfaces.contains(Interfaces::EDITABLE_TEXT)
        || interfaces.contains(Interfaces::TEXT)
            && matches!(role, Some(Role::ENTRY | Role::PASSWORD_TEXT));
    if entry && !grab {
        if !interfaces.contains(Interfaces::EDITABLE_TEXT) {
            return Err(FRONT.into());
        }
        caret_to_end(bus, element).await;
        return Ok(KEPT.into());
    }
    if let Some(index) = chosen(&actions, &CLICK) {
        return perform_at(bus, element, index, "Clicked").await;
    }
    if entry {
        let component = proxy(bus, element).ok_or(GONE)?;
        done(
            acted(component.call(COMPONENT, "GrabFocus", &[])).await?,
            "Focused it",
        )?;
        caret_to_end(bus, element).await;
        return Ok("Focused it.".into());
    }
    if let Some(index) = chosen(&actions, &PRIMARY) {
        return perform_at(bus, element, index, "Clicked").await;
    }
    if let Some(selected) = select_in_parent(bus, element).await {
        return selected;
    }
    Err(format!(
        "That element takes no click (its actions: {}), so nothing ran.",
        list(&actions)
    ))
}

async fn caret_to_end(bus: &Bus, element: &Element) {
    if let Some(text) = proxy(bus, element)
        && let Some(count) = timed(text.get::<i32>(TEXT, "CharacterCount")).await
    {
        timed(text.call::<bool>(TEXT, "SetCaretOffset", &[Value::I32(count)])).await;
    }
}

async fn select_in_parent(bus: &Bus, element: &Element) -> Option<Result<String, String>> {
    let parent = parent(bus, element).await?;
    if !interfaces(bus, &parent)
        .await
        .contains(Interfaces::SELECTION)
    {
        return None;
    }
    let accessible = proxy(bus, element)?;
    let index: i32 = timed(accessible.call(ACCESSIBLE, "GetIndexInParent", &[])).await?;
    let selection = proxy(bus, &parent)?;
    let chosen = selection.call(SELECTION, "SelectChild", &[Value::I32(index)]);
    Some(match acted(chosen).await {
        Ok(ok) => done(ok, "Selected"),
        Err(why) => Err(why),
    })
}

pub async fn perform(bus: &Bus, element: &Element, wanted: &str) -> Result<String, String> {
    let actions = atspi::actions(bus, element).await;
    let normal = normal(wanted);
    match chosen(&actions, &[normal.as_str()]) {
        Some(index) => {
            let name = actions[index].clone();
            perform_at(bus, element, index, &format!("Performed {name}")).await
        }
        None => Err(format!(
            "That element has no {} (its actions: {}), so nothing ran.",
            wanted.trim(),
            list(&actions)
        )),
    }
}

async fn options(bus: &Bus, element: &Element) -> Vec<(Element, String)> {
    let mut found = Vec::new();
    let (kids, _) = children(bus, element, -1).await;
    for kid in kids {
        let nested = if matches!(role(bus, &kid).await, Some(Role::MENU | Role::LIST)) {
            children(bus, &kid, -1).await.0
        } else {
            vec![kid]
        };
        for item in nested {
            if let Some(accessible) = proxy(bus, &item)
                && let Some(name) = timed(accessible.get::<String>(ACCESSIBLE, "Name")).await
                && !name.trim().is_empty()
            {
                found.push((item, name.trim().to_owned()));
            }
        }
    }
    found
}

async fn choose(
    bus: &Bus,
    element: &Element,
    interfaces: Interfaces,
    wanted: &str,
) -> Result<String, String> {
    let options = options(bus, element).await;
    let Some((item, name)) = options
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(wanted.trim()))
    else {
        let names: Vec<String> = options.into_iter().map(|(_, name)| name).collect();
        return Err(format!(
            "That list has no option {}, so nothing ran (its options: {}).",
            wanted.trim(),
            list(&names)
        ));
    };
    let done = format!("Chose {name}");
    let list = if interfaces.contains(Interfaces::SELECTION) {
        Some(element.clone())
    } else {
        parent(bus, item).await.filter(|list| list != element)
    };
    if let Some(list) = list
        && self::interfaces(bus, &list)
            .await
            .contains(Interfaces::SELECTION)
        && let Some(accessible) = proxy(bus, item)
        && let Some(index) =
            timed(accessible.call::<i32>(ACCESSIBLE, "GetIndexInParent", &[])).await
        && let Some(selection) = proxy(bus, &list)
        && acted(selection.call(SELECTION, "SelectChild", &[Value::I32(index)])).await == Ok(true)
    {
        return Ok(format!("{done}."));
    }
    let actions = atspi::actions(bus, item).await;
    match chosen(&actions, &["select", "click", "press", "activate"]) {
        Some(index) => perform_at(bus, item, index, &done).await,
        None => Err(UNSUPPORTED.into()),
    }
}

pub async fn set_value(bus: &Bus, element: &Element, wanted: &str) -> Result<String, String> {
    let (interfaces, role) = tokio::join!(interfaces(bus, element), role(bus, element));
    if role == Some(Role::COMBO_BOX) && !interfaces.contains(Interfaces::EDITABLE_TEXT) {
        return choose(bus, element, interfaces, wanted).await;
    }
    if interfaces.contains(Interfaces::VALUE)
        && let Ok(number) = wanted.trim().parse::<f64>()
    {
        let value = proxy(bus, element).ok_or(GONE)?;
        acted(value.set(VALUE, "CurrentValue", Value::F64(number))).await?;
        return Ok("Set the value.".into());
    }
    if !interfaces.contains(Interfaces::EDITABLE_TEXT) {
        return Err(if matches!(role, Some(Role::ENTRY | Role::PASSWORD_TEXT)) {
            NOT_SETTABLE.into()
        } else {
            UNSUPPORTED.into()
        });
    }
    let editable = proxy(bus, element).ok_or(GONE)?;
    let contents = [Value::Str(wanted.into())];
    done(
        acted(editable.call(EDITABLE_TEXT, "SetTextContents", &contents)).await?,
        "Set the value",
    )?;
    caret_to_end(bus, element).await;
    if role == Some(Role::PASSWORD_TEXT) {
        return Ok("Set the value.".into());
    }
    Ok(match atspi::text(bus, element, i32::MAX).await {
        Some(now) if now != wanted => {
            "Set the value, but the element now reads differently; check the state.".into()
        }
        _ => "Set the value.".into(),
    })
}

pub async fn value(bus: &Bus, element: &Element) -> Option<String> {
    let (interfaces, role) = tokio::join!(interfaces(bus, element), role(bus, element));
    if role == Some(Role::PASSWORD_TEXT) || !interfaces.contains(Interfaces::TEXT) {
        return None;
    }
    atspi::text(bus, element, i32::MAX).await
}

pub async fn select(
    bus: &Bus,
    element: &Element,
    value: &str,
    selection: Selection,
) -> Result<String, String> {
    let (start, end) = selection.chars(value);
    let (start, end) = (start as i32, end as i32);
    let text = proxy(bus, element).ok_or(GONE)?;
    if start == end {
        return done(
            acted(text.call(TEXT, "SetCaretOffset", &[Value::I32(start)])).await?,
            "Moved the cursor",
        );
    }
    let selections: i32 = timed(text.call(TEXT, "GetNSelections", &[]))
        .await
        .unwrap_or(0);
    let ok = if selections > 0 {
        let range = [Value::I32(0), Value::I32(start), Value::I32(end)];
        acted(text.call(TEXT, "SetSelection", &range)).await?
    } else {
        let range = [Value::I32(start), Value::I32(end)];
        acted(text.call(TEXT, "AddSelection", &range)).await?
    };
    done(ok, "Selected text")
}

pub async fn type_text(bus: &Bus, element: &Element, typed: &str) -> Result<bool, String> {
    if !interfaces(bus, element)
        .await
        .contains(Interfaces::EDITABLE_TEXT)
    {
        return Ok(false);
    }
    let text = proxy(bus, element).ok_or(GONE)?;
    let (selections, caret, count) = tokio::join!(
        timed(text.call::<i32>(TEXT, "GetNSelections", &[])),
        timed(text.get::<i32>(TEXT, "CaretOffset")),
        timed(text.get::<i32>(TEXT, "CharacterCount")),
    );
    let mut caret = caret.filter(|&caret| caret >= 0).or(count).unwrap_or(0);
    if selections.unwrap_or(0) > 0
        && let Some((start, end)) =
            timed(text.call::<(i32, i32)>(TEXT, "GetSelection", &[Value::I32(0)])).await
        && start != end
    {
        let range = [Value::I32(start.min(end)), Value::I32(start.max(end))];
        done(
            acted(text.call(EDITABLE_TEXT, "DeleteText", &range)).await?,
            "",
        )?;
        caret = start.min(end);
    }
    let length = i32::try_from(typed.len())
        .map_err(|_| "That text is too long to type, so nothing was typed.")?;
    let inserted = [
        Value::I32(caret),
        Value::Str(typed.into()),
        Value::I32(length),
    ];
    done(
        acted(text.call(EDITABLE_TEXT, "InsertText", &inserted)).await?,
        "",
    )?;
    let moved = caret + typed.chars().count() as i32;
    timed(text.call::<bool>(TEXT, "SetCaretOffset", &[Value::I32(moved)])).await;
    Ok(true)
}

pub async fn focused(bus: &Bus, element: &Element) -> bool {
    states(bus, element)
        .await
        .is_some_and(|states| states.contains(States::FOCUSED))
}

pub fn moved(current: f64, minimum: f64, maximum: f64, sign: f64, pages: f64) -> f64 {
    (current + sign * pages.max(0.1) * (maximum - minimum) * 0.15)
        .clamp(minimum, maximum.max(minimum))
}

pub async fn scroll(
    bus: &Bus,
    element: &Element,
    direction: Direction,
    pages: f64,
) -> Result<String, String> {
    let mut area = element.clone();
    let mut pane = None;
    for _ in 0..12 {
        match role(bus, &area).await {
            Some(Role::SCROLL_PANE) => {
                pane = Some(area);
                break;
            }
            Some(Role::APPLICATION) | None => break,
            Some(_) => match parent(bus, &area).await {
                Some(parent) => area = parent,
                None => break,
            },
        }
    }
    let pane = pane.ok_or(NOTHING)?;
    let vertical = matches!(direction, Direction::Up | Direction::Down);
    let sign = if matches!(direction, Direction::Up | Direction::Left) {
        -1.0
    } else {
        1.0
    };
    let (bars, _) = children(bus, &pane, -1).await;
    for bar in bars {
        if role(bus, &bar).await != Some(Role::SCROLL_BAR) {
            continue;
        }
        let states = states(bus, &bar).await.unwrap_or_default();
        let upright = if states.contains(States::VERTICAL) {
            true
        } else if states.contains(States::HORIZONTAL) {
            false
        } else {
            extents(bus, &bar)
                .await
                .is_some_and(|(_, _, width, height)| height > width)
        };
        if upright != vertical {
            continue;
        }
        let value = proxy(bus, &bar).ok_or(GONE)?;
        let (current, minimum, maximum) = tokio::join!(
            timed(value.get::<f64>(VALUE, "CurrentValue")),
            timed(value.get::<f64>(VALUE, "MinimumValue")),
            timed(value.get::<f64>(VALUE, "MaximumValue")),
        );
        let (Some(current), Some(minimum), Some(maximum)) = (current, minimum, maximum) else {
            continue;
        };
        let to = Value::F64(moved(current, minimum, maximum, sign, pages));
        acted(value.set(VALUE, "CurrentValue", to)).await?;
        return Ok(format!("Scrolled {}.", direction.name()));
    }
    let component = proxy(bus, element).ok_or(NOTHING)?;
    let edge = match direction {
        Direction::Up => SCROLL_TOP_EDGE,
        Direction::Down => SCROLL_BOTTOM_EDGE,
        Direction::Left => SCROLL_LEFT_EDGE,
        Direction::Right => SCROLL_RIGHT_EDGE,
    };
    match timed(component.call::<bool>(COMPONENT, "ScrollTo", &[Value::U32(edge)])).await {
        Some(true) => Ok(format!("Scrolled {}.", direction.name())),
        _ => Err(NOTHING.into()),
    }
}

pub async fn center(bus: &Bus, element: &Element) -> Option<(i32, i32)> {
    let component = proxy(bus, element)?;
    let screen = [Value::U32(COORD_TYPE_SCREEN)];
    let (x, y, width, height) =
        timed(component.call::<(i32, i32, i32, i32)>(COMPONENT, "GetExtents", &screen)).await?;
    (width > 0 && height > 0).then_some((x + width / 2, y + height / 2))
}

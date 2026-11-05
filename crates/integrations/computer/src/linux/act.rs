use super::atspi::{
    self, Element, children, extents, interfaces, parent, proxy, role, states, timed,
};
use crate::{Direction, Press, Selection};
use atspi_common::{CoordType, Interface, Role, ScrollType, State};
use atspi_proxies::{
    accessible::AccessibleProxy, action::ActionProxy, component::ComponentProxy,
    editable_text::EditableTextProxy, selection::SelectionProxy, text::TextProxy,
    value::ValueProxy,
};
use std::{future::Future, time::Duration};
use tokio::time::timeout;
use zbus::Connection;

const ACTION: Duration = Duration::from_secs(5);
const GONE: &str = "That element is gone, so nothing ran. Read the app again with get_app_state.";
const UNSUPPORTED: &str = "That element does not support this action, so nothing ran.";
const SLOW: &str = "The app did not answer in time; whether it took effect is unknown. Read its state before trying again.";
const REFUSED: &str = "The app refused it, so nothing ran.";
const NOTHING: &str = "That element has nothing to scroll, so nothing ran.";
const CLICK: [&str; 2] = ["click", "press"];
const PRIMARY: [&str; 4] = ["activate", "toggle", "jump", "open"];
const MENU: [&str; 4] = ["popup", "menu", "showmenu", "contextmenu"];

async fn acted<T>(work: impl Future<Output = zbus::Result<T>>) -> Result<T, String> {
    match timeout(ACTION, work).await {
        Err(_) => Err(SLOW.into()),
        Ok(Ok(value)) => Ok(value),
        Ok(Err(zbus::Error::MethodError(name, _, _))) => {
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
    bus: &Connection,
    element: &Element,
    index: usize,
    what: &str,
) -> Result<String, String> {
    let action = proxy::<ActionProxy>(bus, element).await.ok_or(GONE)?;
    done(acted(action.do_action(index as i32)).await?, what)
}

pub async fn press(bus: &Connection, element: &Element, press: Press) -> Result<String, String> {
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
    if let Some(index) = chosen(&actions, &CLICK) {
        return perform_at(bus, element, index, "Clicked").await;
    }
    if interfaces(bus, element)
        .await
        .contains(Interface::EditableText)
    {
        let component = proxy::<ComponentProxy>(bus, element).await.ok_or(GONE)?;
        done(acted(component.grab_focus()).await?, "Focused it")?;
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

async fn caret_to_end(bus: &Connection, element: &Element) {
    if let Some(text) = proxy::<TextProxy>(bus, element).await
        && let Some(count) = timed(text.character_count()).await
    {
        timed(text.set_caret_offset(count)).await;
    }
}

async fn select_in_parent(bus: &Connection, element: &Element) -> Option<Result<String, String>> {
    let parent = parent(bus, element).await?;
    if !interfaces(bus, &parent)
        .await
        .contains(Interface::Selection)
    {
        return None;
    }
    let accessible = proxy::<AccessibleProxy>(bus, element).await?;
    let index = timed(accessible.get_index_in_parent()).await?;
    let selection = proxy::<SelectionProxy>(bus, &parent).await?;
    Some(match acted(selection.select_child(index)).await {
        Ok(ok) => done(ok, "Selected"),
        Err(why) => Err(why),
    })
}

pub async fn perform(bus: &Connection, element: &Element, wanted: &str) -> Result<String, String> {
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

pub async fn set_value(
    bus: &Connection,
    element: &Element,
    wanted: &str,
) -> Result<String, String> {
    let (interfaces, role) = tokio::join!(interfaces(bus, element), role(bus, element));
    if interfaces.contains(Interface::Value)
        && let Ok(number) = wanted.trim().parse::<f64>()
    {
        let value = proxy::<ValueProxy>(bus, element).await.ok_or(GONE)?;
        acted(value.set_current_value(number)).await?;
        return Ok("Set the value.".into());
    }
    if !interfaces.contains(Interface::EditableText) {
        return Err(UNSUPPORTED.into());
    }
    let editable = proxy::<EditableTextProxy>(bus, element).await.ok_or(GONE)?;
    done(
        acted(editable.set_text_contents(wanted)).await?,
        "Set the value",
    )?;
    caret_to_end(bus, element).await;
    if role == Some(Role::PasswordText) {
        return Ok("Set the value.".into());
    }
    Ok(match atspi::text(bus, element, i32::MAX).await {
        Some(now) if now != wanted => {
            "Set the value, but the element now reads differently; check the state.".into()
        }
        _ => "Set the value.".into(),
    })
}

pub async fn value(bus: &Connection, element: &Element) -> Option<String> {
    let (interfaces, role) = tokio::join!(interfaces(bus, element), role(bus, element));
    if role == Some(Role::PasswordText) || !interfaces.contains(Interface::Text) {
        return None;
    }
    atspi::text(bus, element, i32::MAX).await
}

pub async fn select(
    bus: &Connection,
    element: &Element,
    value: &str,
    selection: Selection,
) -> Result<String, String> {
    let (start, end) = selection.chars(value);
    let (start, end) = (start as i32, end as i32);
    let text = proxy::<TextProxy>(bus, element).await.ok_or(GONE)?;
    if start == end {
        return done(
            acted(text.set_caret_offset(start)).await?,
            "Moved the cursor",
        );
    }
    let selections = timed(text.get_n_selections()).await.unwrap_or(0);
    let ok = if selections > 0 {
        acted(text.set_selection(0, start, end)).await?
    } else {
        acted(text.add_selection(start, end)).await?
    };
    done(ok, "Selected text")
}

pub async fn type_text(bus: &Connection, element: &Element, typed: &str) -> Result<bool, String> {
    if !interfaces(bus, element)
        .await
        .contains(Interface::EditableText)
    {
        return Ok(false);
    }
    let text = proxy::<TextProxy>(bus, element).await.ok_or(GONE)?;
    let editable = proxy::<EditableTextProxy>(bus, element).await.ok_or(GONE)?;
    let (selections, caret, count) = tokio::join!(
        timed(text.get_n_selections()),
        timed(text.caret_offset()),
        timed(text.character_count()),
    );
    let mut caret = caret.filter(|&caret| caret >= 0).or(count).unwrap_or(0);
    if selections.unwrap_or(0) > 0
        && let Some((start, end)) = timed(text.get_selection(0)).await
        && start != end
    {
        done(
            acted(editable.delete_text(start.min(end), start.max(end))).await?,
            "",
        )?;
        caret = start.min(end);
    }
    let length = i32::try_from(typed.len())
        .map_err(|_| "That text is too long to type, so nothing was typed.")?;
    done(acted(editable.insert_text(caret, typed, length)).await?, "")?;
    let moved = caret + typed.chars().count() as i32;
    timed(text.set_caret_offset(moved)).await;
    Ok(true)
}

pub async fn focused(bus: &Connection, element: &Element) -> bool {
    states(bus, element)
        .await
        .is_some_and(|states| states.contains(State::Focused))
}

pub fn moved(current: f64, minimum: f64, maximum: f64, sign: f64, pages: f64) -> f64 {
    (current + sign * pages.max(0.1) * (maximum - minimum) * 0.15)
        .clamp(minimum, maximum.max(minimum))
}

pub async fn scroll(
    bus: &Connection,
    element: &Element,
    direction: Direction,
    pages: f64,
) -> Result<String, String> {
    let mut area = element.clone();
    let mut pane = None;
    for _ in 0..12 {
        match role(bus, &area).await {
            Some(Role::ScrollPane) => {
                pane = Some(area);
                break;
            }
            Some(Role::Application) | None => break,
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
        if role(bus, &bar).await != Some(Role::ScrollBar) {
            continue;
        }
        let states = states(bus, &bar).await.unwrap_or_default();
        let upright = if states.contains(State::Vertical) {
            true
        } else if states.contains(State::Horizontal) {
            false
        } else {
            extents(bus, &bar)
                .await
                .is_some_and(|(_, _, width, height)| height > width)
        };
        if upright != vertical {
            continue;
        }
        let value = proxy::<ValueProxy>(bus, &bar).await.ok_or(GONE)?;
        let (current, minimum, maximum) = tokio::join!(
            timed(value.current_value()),
            timed(value.minimum_value()),
            timed(value.maximum_value()),
        );
        let (Some(current), Some(minimum), Some(maximum)) = (current, minimum, maximum) else {
            continue;
        };
        acted(value.set_current_value(moved(current, minimum, maximum, sign, pages))).await?;
        return Ok(format!("Scrolled {}.", direction.name()));
    }
    let component = proxy::<ComponentProxy>(bus, element).await.ok_or(NOTHING)?;
    let edge = match direction {
        Direction::Up => ScrollType::TopEdge,
        Direction::Down => ScrollType::BottomEdge,
        Direction::Left => ScrollType::LeftEdge,
        Direction::Right => ScrollType::RightEdge,
    };
    match timed(component.scroll_to(edge)).await {
        Some(true) => Ok(format!("Scrolled {}.", direction.name())),
        _ => Err(NOTHING.into()),
    }
}

pub async fn center(bus: &Connection, element: &Element) -> Option<(i32, i32)> {
    let component = proxy::<ComponentProxy>(bus, element).await?;
    let (x, y, width, height) = timed(component.get_extents(CoordType::Screen)).await?;
    (width > 0 && height > 0).then_some((x + width / 2, y + height / 2))
}

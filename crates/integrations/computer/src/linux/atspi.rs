use crate::state::Node;
use atspi_common::{Interface, InterfaceSet, Role, State, StateSet};
use atspi_proxies::{
    accessible::AccessibleProxy, action::ActionProxy, bus::BusProxy, component::ComponentProxy,
    text::TextProxy, value::ValueProxy,
};
use futures_util::{StreamExt, stream};
use std::{future::Future, time::Duration};
use tokio::time::{Instant, timeout, timeout_at};
use zbus::{
    Connection,
    fdo::DBusProxy,
    names::BusName,
    proxy::{Builder, CacheProperties, Defaults},
    zvariant::OwnedObjectPath,
};

pub const CALL: Duration = Duration::from_secs(1);
const READ: Duration = Duration::from_secs(5);
const MAX_NODES: usize = 3000;
const MAX_DEPTH: usize = 40;
const MAX_CHILDREN: usize = 400;
const MAX_TEXT: i32 = 1000;
const PARALLEL: usize = 64;
const REGISTRY: &str = "org.a11y.atspi.Registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const NULL: &str = "/org/a11y/atspi/null";
const WINDOWS: [Role; 4] = [Role::Frame, Role::Window, Role::Dialog, Role::Alert];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    pub bus: String,
    pub path: String,
}

impl Element {
    fn of((bus, path): (String, OwnedObjectPath)) -> Option<Self> {
        let path = path.as_str().to_owned();
        (!bus.is_empty() && path != NULL).then_some(Self { bus, path })
    }
}

pub struct Info {
    pub role: Role,
    pub name: Option<String>,
    pub states: StateSet,
    pub interfaces: InterfaceSet,
    pub actions: Vec<String>,
    pub value: Option<String>,
    pub children: Vec<Element>,
    pub shown: bool,
    pub cut: bool,
}

pub struct Tree {
    pub root: Option<Node<Element>>,
    pub focused: Option<Element>,
    pub complete: bool,
}

pub struct Registered {
    pub element: Element,
    pub pid: u32,
    pub name: String,
    pub windows: bool,
}

pub async fn timed<T, E>(work: impl Future<Output = Result<T, E>>) -> Option<T> {
    timeout(CALL, work).await.ok()?.ok()
}

pub async fn proxy<'a, P>(bus: &Connection, element: &'a Element) -> Option<P>
where
    P: From<zbus::Proxy<'a>> + Defaults,
{
    Builder::<P>::new(bus)
        .cache_properties(CacheProperties::No)
        .destination(element.bus.as_str())
        .ok()?
        .path(element.path.as_str())
        .ok()?
        .build()
        .await
        .ok()
}

pub async fn connect(fallback: Option<String>) -> Option<Connection> {
    let address = match std::env::var("AT_SPI_BUS_ADDRESS") {
        Ok(address) if !address.is_empty() => Some(address),
        _ => session_address().await.or(fallback),
    }?;
    let builder = zbus::connection::Builder::address(address.as_str()).ok()?;
    timeout(CALL * 2, builder.build()).await.ok()?.ok()
}

async fn session_address() -> Option<String> {
    let session = timeout(CALL * 2, Connection::session()).await.ok()?.ok()?;
    let bus = BusProxy::builder(&session)
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .ok()?;
    timed(bus.get_address()).await
}

pub async fn ping(bus: &Connection) -> bool {
    let ping = bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus.Peer"),
        "Ping",
        &(),
    );
    timed(ping).await.is_some()
}

pub async fn children(bus: &Connection, element: &Element, count: i32) -> (Vec<Element>, bool) {
    let Some(accessible) = proxy::<AccessibleProxy>(bus, element).await else {
        return (Vec::new(), true);
    };
    let accessible = accessible.inner();
    if usize::try_from(count).map_or(true, |count| count <= MAX_CHILDREN) {
        let refs: Option<Vec<(String, OwnedObjectPath)>> =
            timed(accessible.call("GetChildren", &())).await;
        let Some(refs) = refs else {
            return (Vec::new(), true);
        };
        let cut = refs.len() > MAX_CHILDREN;
        let children = refs
            .into_iter()
            .take(MAX_CHILDREN)
            .filter_map(Element::of)
            .collect();
        return (children, cut);
    }
    let children: Vec<Option<Element>> = stream::iter(0..MAX_CHILDREN as i32)
        .map(|index| async move {
            timed(accessible.call("GetChildAtIndex", &(index,)))
                .await
                .and_then(Element::of)
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    (children.into_iter().flatten().collect(), true)
}

pub async fn role(bus: &Connection, element: &Element) -> Option<Role> {
    let accessible = proxy::<AccessibleProxy>(bus, element).await?;
    let role: u32 = timed(accessible.inner().call("GetRole", &())).await?;
    Some(Role::try_from(role).unwrap_or(Role::Unknown))
}

pub async fn states(bus: &Connection, element: &Element) -> Option<StateSet> {
    let accessible = proxy::<AccessibleProxy>(bus, element).await?;
    timed(accessible.get_state()).await
}

pub async fn interfaces(bus: &Connection, element: &Element) -> InterfaceSet {
    match proxy::<AccessibleProxy>(bus, element).await {
        Some(accessible) => timed(accessible.get_interfaces()).await.unwrap_or_default(),
        None => InterfaceSet::empty(),
    }
}

pub async fn parent(bus: &Connection, element: &Element) -> Option<Element> {
    let accessible = proxy::<AccessibleProxy>(bus, element).await?;
    let parent = timed(accessible.parent()).await?;
    let bus = parent.name_as_str()?.to_owned();
    let path = parent.path_as_str().to_owned();
    (path != NULL && path != ROOT).then_some(Element { bus, path })
}

pub async fn actions(bus: &Connection, element: &Element) -> Vec<String> {
    let Some(action) = proxy::<ActionProxy>(bus, element).await else {
        return Vec::new();
    };
    timed(action.get_actions())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|action| action.name)
        .collect()
}

pub async fn extents(bus: &Connection, element: &Element) -> Option<(i32, i32, i32, i32)> {
    let component = proxy::<ComponentProxy>(bus, element).await?;
    let extents = timed(component.get_extents(atspi_common::CoordType::Screen)).await?;
    (extents.2 > 0 && extents.3 > 0).then_some(extents)
}

pub async fn text(bus: &Connection, element: &Element, limit: i32) -> Option<String> {
    let text = proxy::<TextProxy>(bus, element).await?;
    let count = timed(text.character_count()).await?;
    timed(text.get_text(0, count.min(limit))).await
}

pub fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

pub fn textual(role: Role, interfaces: InterfaceSet) -> bool {
    interfaces.contains(Interface::Text)
        && (interfaces.contains(Interface::EditableText)
            || matches!(
                role,
                Role::Text | Role::Entry | Role::PasswordText | Role::Terminal
            ))
}

async fn value(
    bus: &Connection,
    element: &Element,
    role: Role,
    interfaces: InterfaceSet,
) -> Option<String> {
    if role == Role::PasswordText {
        let text = proxy::<TextProxy>(bus, element).await?;
        let count = timed(text.character_count()).await?;
        return (count > 0).then(String::new);
    }
    if textual(role, interfaces) {
        return text(bus, element, MAX_TEXT)
            .await
            .filter(|text| !text.is_empty());
    }
    if interfaces.contains(Interface::Value) {
        let value = proxy::<ValueProxy>(bus, element).await?;
        return timed(value.current_value()).await.map(number);
    }
    None
}

pub async fn info(bus: &Connection, element: &Element, deep: bool) -> Option<Info> {
    let accessible = proxy::<AccessibleProxy>(bus, element).await?;
    let (role, name, states, interfaces, count) = tokio::join!(
        timed(accessible.inner().call::<_, _, u32>("GetRole", &())),
        timed(accessible.name()),
        timed(accessible.get_state()),
        timed(accessible.get_interfaces()),
        timed(accessible.child_count()),
    );
    let role = Role::try_from(role?).unwrap_or(Role::Unknown);
    let states = states.unwrap_or_default();
    let interfaces = interfaces.unwrap_or_default();
    let shown = listed(states);
    let count = if shown { count.unwrap_or(-1) } else { 0 };
    let name = name.filter(|name| !name.trim().is_empty());
    let mut info = Info {
        role,
        name,
        states,
        interfaces,
        actions: Vec::new(),
        value: None,
        children: Vec::new(),
        shown,
        cut: !deep && count > 0,
    };
    if !shown {
        return Some(info);
    }
    let unnamed = info.name.is_none() && interfaces.contains(Interface::Action);
    let (actions, value, children, description) = tokio::join!(
        async {
            if interfaces.contains(Interface::Action) {
                actions(bus, element).await
            } else {
                Vec::new()
            }
        },
        value(bus, element, role, interfaces),
        async {
            if deep && count != 0 {
                children(bus, element, count).await
            } else {
                (Vec::new(), false)
            }
        },
        async {
            if unnamed {
                timed(accessible.description()).await
            } else {
                None
            }
        },
    );
    info.actions = actions;
    info.value = value;
    (info.children, info.cut) = (children.0, info.cut || children.1);
    if info.name.is_none() {
        info.name = description.filter(|name| !name.trim().is_empty());
    }
    Some(info)
}

pub fn listed(states: StateSet) -> bool {
    !states.contains(State::Defunct)
        && (states.contains(State::Visible) || states.contains(State::Showing))
}

pub fn camel(role: &str) -> String {
    role.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect()
}

pub fn role_name(role: Role) -> String {
    match role {
        Role::Button => "PushButton".into(),
        role => camel(role.name()),
    }
}

pub fn flags(states: StateSet, actionable: bool) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if actionable && !states.contains(State::Enabled) && !states.contains(State::Sensitive) {
        flags.push("disabled");
    }
    if states.contains(State::Focused) {
        flags.push("focused");
    }
    if states.contains(State::Selected) {
        flags.push("selected");
    }
    if states.contains(State::Expandable) {
        flags.push(if states.contains(State::Expanded) {
            "expanded"
        } else {
            "collapsed"
        });
    }
    if states.contains(State::Checked) {
        flags.push("checked");
    }
    flags
}

pub fn node(element: Element, info: Info) -> Node<Element> {
    let actionable = !info.actions.is_empty()
        || info.interfaces.contains(Interface::EditableText)
        || info.interfaces.contains(Interface::Value)
        || info.states.contains(State::Selectable);
    let mut node = Node::new(role_name(info.role), element);
    node.name = info.name;
    node.value = info.value;
    node.states = flags(info.states, actionable);
    node.actionable = actionable;
    node.secure = info.role == Role::PasswordText;
    node
}

pub async fn tree(bus: &Connection, window: &Element) -> Tree {
    let deadline = Instant::now() + READ;
    let mut slots: Vec<(Element, Info, Vec<usize>)> = Vec::new();
    let mut level = vec![(window.clone(), None::<usize>, 0)];
    let mut complete = true;
    while !level.is_empty() {
        let room = MAX_NODES.saturating_sub(slots.len());
        if level.len() > room {
            level.truncate(room);
            complete = false;
        }
        if level.is_empty() || Instant::now() >= deadline {
            complete = false;
            break;
        }
        let infos: Vec<Option<Info>> = stream::iter(&level)
            .map(|(element, _, depth)| async move {
                timeout_at(deadline, info(bus, element, *depth < MAX_DEPTH))
                    .await
                    .ok()
                    .flatten()
            })
            .buffered(PARALLEL)
            .collect()
            .await;
        let mut next = Vec::new();
        for ((element, parent, depth), info) in level.into_iter().zip(infos) {
            let Some(info) = info else {
                complete = false;
                continue;
            };
            if parent.is_some() && !info.shown {
                continue;
            }
            complete &= !info.cut;
            let index = slots.len();
            if let Some(parent) = parent {
                slots[parent].2.push(index);
            }
            next.extend(
                info.children
                    .iter()
                    .map(|child| (child.clone(), Some(index), depth + 1)),
            );
            slots.push((element, info, Vec::new()));
        }
        level = next;
    }
    let focused = slots
        .iter()
        .find(|(_, info, _)| info.states.contains(State::Focused))
        .map(|(element, _, _)| element.clone());
    let mut built: Vec<Option<Node<Element>>> = slots.iter().map(|_| None).collect();
    for (index, (element, info, kids)) in slots.into_iter().enumerate().rev() {
        let mut node = node(element, info);
        node.children = kids.iter().filter_map(|&kid| built[kid].take()).collect();
        built[index] = Some(node);
    }
    Tree {
        root: built.into_iter().next().flatten(),
        focused,
        complete,
    }
}

pub async fn registered(bus: &Connection) -> Vec<Registered> {
    let root = Element {
        bus: REGISTRY.into(),
        path: ROOT.into(),
    };
    let (apps, _) = children(bus, &root, -1).await;
    let Ok(builder) = DBusProxy::builder(bus)
        .cache_properties(CacheProperties::No)
        .build()
        .await
    else {
        return Vec::new();
    };
    let dbus = &builder;
    let found: Vec<Option<Registered>> = stream::iter(apps)
        .map(|element| async move {
            let name = BusName::try_from(element.bus.as_str()).ok()?;
            let (pid, accessible, windows) = tokio::join!(
                timed(dbus.get_connection_unix_process_id(name)),
                proxy::<AccessibleProxy>(bus, &element),
                windows(bus, &element),
            );
            let name = match accessible {
                Some(accessible) => timed(accessible.name()).await.unwrap_or_default(),
                None => String::new(),
            };
            Some(Registered {
                pid: pid?,
                name,
                windows: windows
                    .iter()
                    .any(|(_, _, states)| states.contains(State::Showing)),
                element,
            })
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    found.into_iter().flatten().collect()
}

pub async fn windows(bus: &Connection, app: &Element) -> Vec<(Element, Role, StateSet)> {
    let (children, _) = children(bus, app, -1).await;
    let found: Vec<Option<(Element, Role, StateSet)>> = stream::iter(children)
        .map(|element| async move {
            let (role, states) = tokio::join!(role(bus, &element), states(bus, &element));
            let role = role.filter(|role| WINDOWS.contains(role))?;
            Some((element, role, states.unwrap_or_default()))
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    found.into_iter().flatten().collect()
}

pub async fn window(bus: &Connection, apps: &[Element]) -> Option<(Element, Option<String>)> {
    let mut all = Vec::new();
    for app in apps {
        all.extend(windows(bus, app).await);
    }
    let (window, _, _) = all
        .iter()
        .find(|(_, _, states)| states.contains(State::Active))
        .or_else(|| {
            all.iter()
                .find(|(_, _, states)| states.contains(State::Showing))
        })?;
    let accessible = proxy::<AccessibleProxy>(bus, window).await?;
    let title = timed(accessible.name())
        .await
        .filter(|title| !title.trim().is_empty());
    Some((window.clone(), title))
}

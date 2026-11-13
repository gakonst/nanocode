use super::{
    a11y::{Interfaces, Role, States},
    dbus::{BUS, BUS_PATH, Bus, Error, Message, PEER, PROPERTIES},
    wire::Value,
};
use crate::state::Node;
use futures_util::{StreamExt, stream};
use std::{collections::HashMap, future::Future, time::Duration};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    time::{Instant, timeout, timeout_at},
};

pub const CALL: Duration = Duration::from_secs(1);
const READ: Duration = Duration::from_secs(5);
const MAX_NODES: usize = 3000;
const MAX_DEPTH: usize = 40;
const MAX_CHILDREN: usize = 400;
const MAX_TEXT: i32 = 1000;
const PARALLEL: usize = 64;
const REGISTRY: &str = "org.a11y.atspi.Registry";
const REGISTRY_PATH: &str = "/org/a11y/atspi/registry";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const NULL: &str = "/org/a11y/atspi/null";
const A11Y_BUS: &str = "org.a11y.Bus";
const A11Y_BUS_PATH: &str = "/org/a11y/bus";
pub const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub const ACTION: &str = "org.a11y.atspi.Action";
const APPLICATION: &str = "org.a11y.atspi.Application";
pub const COMPONENT: &str = "org.a11y.atspi.Component";
pub const EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";
pub const SELECTION: &str = "org.a11y.atspi.Selection";
pub const TEXT: &str = "org.a11y.atspi.Text";
pub const VALUE: &str = "org.a11y.atspi.Value";
pub const COORD_TYPE_SCREEN: u32 = 0;
const WINDOWS: [Role; 4] = [Role::FRAME, Role::WINDOW, Role::DIALOG, Role::ALERT];
const GTK4_ACTIVATION: Duration = Duration::from_millis(250);
const EVENTS: [&str; 9] = [
    "object:state-changed",
    "object:property-change",
    "object:children-changed",
    "object:text-changed",
    "object:text-caret-moved",
    "object:text-selection-changed",
    "object:selection-changed",
    "window:",
    "focus:",
];
const CHANGES: [&str; 7] = [
    "StateChanged",
    "PropertyChange",
    "ChildrenChanged",
    "TextChanged",
    "TextCaretMoved",
    "TextSelectionChanged",
    "SelectionChanged",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    pub bus: String,
    pub path: String,
}

impl Element {
    fn of((bus, path): (String, String)) -> Option<Self> {
        (!bus.is_empty() && path != NULL).then_some(Self { bus, path })
    }
}

pub struct Info {
    pub role: Role,
    pub name: Option<String>,
    pub states: States,
    pub interfaces: Interfaces,
    pub actionable: bool,
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
    pub activation: Duration,
    pub windows: bool,
}

pub trait One: Sized {
    fn one(value: Value) -> Option<Self>;
}

impl One for () {
    fn one(_: Value) -> Option<Self> {
        Some(())
    }
}

impl One for Value {
    fn one(value: Value) -> Option<Self> {
        Some(value)
    }
}

impl One for bool {
    fn one(value: Value) -> Option<Self> {
        value.bool()
    }
}

impl One for i32 {
    fn one(value: Value) -> Option<Self> {
        value.i32()
    }
}

impl One for u32 {
    fn one(value: Value) -> Option<Self> {
        value.u32()
    }
}

impl One for f64 {
    fn one(value: Value) -> Option<Self> {
        value.f64()
    }
}

impl One for String {
    fn one(value: Value) -> Option<Self> {
        match value {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }
}

impl One for (String, String) {
    fn one(value: Value) -> Option<Self> {
        match value.fields()? {
            [Value::Str(bus), Value::Path(path)] => Some((bus.clone(), path.clone())),
            _ => None,
        }
    }
}

impl One for (i32, i32) {
    fn one(value: Value) -> Option<Self> {
        match value.fields()? {
            [Value::I32(start), Value::I32(end)] => Some((*start, *end)),
            _ => None,
        }
    }
}

impl One for (i32, i32, i32, i32) {
    fn one(value: Value) -> Option<Self> {
        match value.fields()? {
            [
                Value::I32(x),
                Value::I32(y),
                Value::I32(width),
                Value::I32(height),
            ] => Some((*x, *y, *width, *height)),
            _ => None,
        }
    }
}

pub struct ActionName(String);

impl One for ActionName {
    fn one(value: Value) -> Option<Self> {
        match value.fields()? {
            [Value::Str(name), Value::Str(_), Value::Str(_)] => Some(Self(name.clone())),
            _ => None,
        }
    }
}

impl<T: One> One for Vec<T> {
    fn one(value: Value) -> Option<Self> {
        match value {
            Value::Array(items) => items.into_iter().map(T::one).collect(),
            _ => None,
        }
    }
}

impl One for States {
    fn one(value: Value) -> Option<Self> {
        Self::of(&Vec::<u32>::one(value)?)
    }
}

impl One for Interfaces {
    fn one(value: Value) -> Option<Self> {
        Some(Self::of(
            Vec::<String>::one(value)?.iter().map(String::as_str),
        ))
    }
}

impl One for HashMap<String, Value> {
    fn one(value: Value) -> Option<Self> {
        match value {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::Entry(entry) => match *entry {
                        (Value::Str(key), Value::Variant(inner)) => Some((key, *inner)),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => None,
        }
    }
}

fn read<T: One>(message: &Message) -> Result<T, Error> {
    let mut values = message.body()?;
    let value = if values.len() == 1 {
        values.remove(0)
    } else {
        Value::Struct(values)
    };
    T::one(value).ok_or(Error::Malformed("the reply has another type"))
}

pub struct Object<'a> {
    bus: &'a Bus,
    element: &'a Element,
}

impl Object<'_> {
    pub fn call<T: One>(
        &self,
        interface: &str,
        member: &str,
        args: &[Value],
    ) -> impl Future<Output = Result<T, Error>> + use<T> {
        let reply = self.bus.call(
            &self.element.bus,
            &self.element.path,
            interface,
            member,
            args,
        );
        async move { read(&reply.await?) }
    }

    pub fn get<T: One>(
        &self,
        interface: &str,
        property: &str,
    ) -> impl Future<Output = Result<T, Error>> + use<T> {
        let args = [Value::Str(interface.into()), Value::Str(property.into())];
        let reply = self.bus.call(
            &self.element.bus,
            &self.element.path,
            PROPERTIES,
            "Get",
            &args,
        );
        async move {
            match read::<Value>(&reply.await?)? {
                Value::Variant(inner) => {
                    T::one(*inner).ok_or(Error::Malformed("the property has another type"))
                }
                _ => Err(Error::Malformed("the property is not a variant")),
            }
        }
    }

    pub fn set(
        &self,
        interface: &str,
        property: &str,
        value: Value,
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let args = [
            Value::Str(interface.into()),
            Value::Str(property.into()),
            Value::Variant(Box::new(value)),
        ];
        let reply = self.bus.call(
            &self.element.bus,
            &self.element.path,
            PROPERTIES,
            "Set",
            &args,
        );
        async move { reply.await.map(|_| ()) }
    }
}

pub async fn timed<T, E>(work: impl Future<Output = Result<T, E>>) -> Option<T> {
    timeout(CALL, work).await.ok()?.ok()
}

pub fn proxy<'a>(bus: &'a Bus, element: &'a Element) -> Option<Object<'a>> {
    (super::dbus::valid_name(&element.bus) && super::dbus::valid_path(&element.path))
        .then_some(Object { bus, element })
}

pub async fn connect(fallback: Option<String>) -> Option<Bus> {
    let address = match std::env::var("AT_SPI_BUS_ADDRESS") {
        Ok(address) if !address.is_empty() => Some(address),
        _ => session_address().await.or(fallback),
    }?;
    timeout(CALL * 2, Bus::connect(&address, PARALLEL))
        .await
        .ok()?
        .ok()
}

fn session() -> String {
    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(address) => address,
        Err(_) => {
            let runtime = std::env::var("XDG_RUNTIME_DIR")
                .unwrap_or_else(|_| format!("/run/user/{}", nanocode_sys::unix::user_id()));
            format!("unix:path={runtime}/bus")
        }
    }
}

async fn session_address() -> Option<String> {
    let session = timeout(CALL * 2, Bus::connect(&session(), PARALLEL))
        .await
        .ok()?
        .ok()?;
    let asked = session.call(A11Y_BUS, A11Y_BUS_PATH, A11Y_BUS, "GetAddress", &[]);
    timed(async { read::<String>(&asked.await?) }).await
}

pub async fn register(bus: &Bus) -> bool {
    for event in EVENTS {
        let registered = bus.call(
            REGISTRY,
            REGISTRY_PATH,
            REGISTRY,
            "RegisterEvent",
            &[Value::Str(event.into())],
        );
        if timed(registered).await.is_none() {
            return false;
        }
    }
    true
}

pub async fn changes(bus: &Bus, sender: &str, changes: UnboundedSender<()>) -> Option<()> {
    timed(bus.watch(sender, changed, changes)).await
}

pub fn changed(message: &Message) -> bool {
    let (Some(interface), Some(member)) = (&message.interface, &message.member) else {
        return false;
    };
    match interface.as_str() {
        "org.a11y.atspi.Event.Window" | "org.a11y.atspi.Event.Focus" => true,
        "org.a11y.atspi.Event.Object" => CHANGES
            .iter()
            .any(|change| change.eq_ignore_ascii_case(member)),
        _ => false,
    }
}

pub async fn settle(changes: &mut UnboundedReceiver<()>, first: Duration, quiet: Duration) -> bool {
    let start = Instant::now();
    let cap = start + first + quiet;
    let mut until = start + first;
    let mut changed = false;
    while let Ok(Some(())) = timeout_at(until, changes.recv()).await {
        changed = true;
        until = (Instant::now() + quiet).min(cap);
    }
    changed
}

pub async fn ping(bus: &Bus) -> bool {
    timed(bus.call(BUS, BUS_PATH, PEER, "Ping", &[]))
        .await
        .is_some()
}

pub async fn children(bus: &Bus, element: &Element, count: i32) -> (Vec<Element>, bool) {
    let Some(accessible) = proxy(bus, element) else {
        return (Vec::new(), true);
    };
    if usize::try_from(count).map_or(true, |count| count <= MAX_CHILDREN) {
        let refs: Option<Vec<(String, String)>> =
            timed(accessible.call(ACCESSIBLE, "GetChildren", &[])).await;
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
    let accessible = &accessible;
    let children: Vec<Option<Element>> = stream::iter(0..MAX_CHILDREN as i32)
        .map(|index| async move {
            timed(accessible.call(ACCESSIBLE, "GetChildAtIndex", &[Value::I32(index)]))
                .await
                .and_then(Element::of)
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    (children.into_iter().flatten().collect(), true)
}

pub async fn role(bus: &Bus, element: &Element) -> Option<Role> {
    let accessible = proxy(bus, element)?;
    let role: u32 = timed(accessible.call(ACCESSIBLE, "GetRole", &[])).await?;
    Some(Role::of(role))
}

pub async fn states(bus: &Bus, element: &Element) -> Option<States> {
    let accessible = proxy(bus, element)?;
    timed(accessible.call(ACCESSIBLE, "GetState", &[])).await
}

pub async fn interfaces(bus: &Bus, element: &Element) -> Interfaces {
    match proxy(bus, element) {
        Some(accessible) => timed(accessible.call(ACCESSIBLE, "GetInterfaces", &[]))
            .await
            .unwrap_or_default(),
        None => Interfaces::default(),
    }
}

pub async fn parent(bus: &Bus, element: &Element) -> Option<Element> {
    let accessible = proxy(bus, element)?;
    let parent = timed(accessible.get(ACCESSIBLE, "Parent")).await?;
    Element::of(parent).filter(|parent| parent.path != ROOT)
}

pub async fn actions(bus: &Bus, element: &Element) -> Vec<String> {
    let Some(action) = proxy(bus, element) else {
        return Vec::new();
    };
    let listed: Vec<ActionName> = timed(action.call(ACTION, "GetActions", &[]))
        .await
        .unwrap_or_default();
    let mut names = Vec::with_capacity(listed.len());
    for (index, ActionName(listed)) in (0..).zip(listed) {
        names.push(if listed.is_empty() {
            timed(action.call(ACTION, "GetName", &[Value::I32(index)]))
                .await
                .unwrap_or_default()
        } else {
            listed
        });
    }
    names
}

pub async fn extents(bus: &Bus, element: &Element) -> Option<(i32, i32, i32, i32)> {
    let component = proxy(bus, element)?;
    let extents: (i32, i32, i32, i32) =
        timed(component.call(COMPONENT, "GetExtents", &[Value::U32(COORD_TYPE_SCREEN)])).await?;
    (extents.2 > 0 && extents.3 > 0).then_some(extents)
}

pub async fn text(bus: &Bus, element: &Element, limit: i32) -> Option<String> {
    let text = proxy(bus, element)?;
    let count: i32 = timed(text.get(TEXT, "CharacterCount")).await?;
    timed(text.call(
        TEXT,
        "GetText",
        &[Value::I32(0), Value::I32(count.min(limit))],
    ))
    .await
}

pub fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

pub fn textual(role: Role, interfaces: Interfaces) -> bool {
    interfaces.contains(Interfaces::TEXT)
        && (interfaces.contains(Interfaces::EDITABLE_TEXT)
            || [Role::TEXT, Role::ENTRY, Role::PASSWORD_TEXT, Role::TERMINAL].contains(&role))
}

pub async fn selected(bus: &Bus, element: &Element) -> Option<String> {
    let selection = proxy(bus, element)?;
    let chosen = timed(selection.call(SELECTION, "GetSelectedChild", &[Value::I32(0)])).await?;
    let chosen = Element::of(chosen)?;
    let accessible = proxy(bus, &chosen)?;
    timed(accessible.get::<String>(ACCESSIBLE, "Name"))
        .await
        .filter(|name| !name.trim().is_empty())
}

async fn value(bus: &Bus, element: &Element, role: Role, interfaces: Interfaces) -> Option<String> {
    if role == Role::PASSWORD_TEXT {
        let text = proxy(bus, element)?;
        let count: i32 = timed(text.get(TEXT, "CharacterCount")).await?;
        return (count > 0).then(String::new);
    }
    if textual(role, interfaces) {
        return text(bus, element, MAX_TEXT)
            .await
            .filter(|text| !text.is_empty());
    }
    if role == Role::COMBO_BOX {
        if interfaces.contains(Interfaces::SELECTION) {
            return selected(bus, element).await;
        }
        let (kids, _) = children(bus, element, 1).await;
        let list = kids.first()?;
        return if self::interfaces(bus, list)
            .await
            .contains(Interfaces::SELECTION)
        {
            selected(bus, list).await
        } else {
            None
        };
    }
    if interfaces.contains(Interfaces::VALUE) {
        let value = proxy(bus, element)?;
        return timed(value.get::<f64>(VALUE, "CurrentValue"))
            .await
            .map(number);
    }
    None
}

async fn summary(accessible: &Object<'_>) -> (Option<String>, Option<Option<String>>, Option<i32>) {
    let all: Option<HashMap<String, Value>> =
        timed(accessible.call(PROPERTIES, "GetAll", &[Value::Str(ACCESSIBLE.into())])).await;
    if let Some(all) = all {
        let text = |key: &str| all.get(key).and_then(Value::str).map(str::to_owned);
        let count = all.get("ChildCount").and_then(Value::i32);
        return (text("Name"), Some(text("Description")), count);
    }
    let (name, count) = tokio::join!(
        timed(accessible.get::<String>(ACCESSIBLE, "Name")),
        timed(accessible.get::<i32>(ACCESSIBLE, "ChildCount"))
    );
    (name, None, count)
}

pub async fn info(bus: &Bus, element: &Element, deep: bool) -> Option<Info> {
    let accessible = proxy(bus, element)?;
    let (role, states, interfaces, (name, described, count)) = tokio::join!(
        timed(accessible.call::<u32>(ACCESSIBLE, "GetRole", &[])),
        timed(accessible.call::<States>(ACCESSIBLE, "GetState", &[])),
        timed(accessible.call::<Interfaces>(ACCESSIBLE, "GetInterfaces", &[])),
        summary(&accessible),
    );
    let role = Role::of(role?);
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
        actionable: false,
        value: None,
        children: Vec::new(),
        shown,
        cut: !deep && count > 0,
    };
    if !shown {
        return Some(info);
    }
    let unnamed = info.name.is_none() && interfaces.contains(Interfaces::ACTION);
    let (actionable, value, children, description) = tokio::join!(
        async {
            if !interfaces.contains(Interfaces::ACTION) {
                return false;
            }
            timed(accessible.call::<Vec<ActionName>>(ACTION, "GetActions", &[]))
                .await
                .is_some_and(|listed| !listed.is_empty())
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
            match described {
                _ if !unnamed => None,
                Some(description) => description,
                None => timed(accessible.get::<String>(ACCESSIBLE, "Description")).await,
            }
        },
    );
    info.actionable = actionable;
    info.value = value;
    (info.children, info.cut) = (children.0, info.cut || children.1);
    if info.name.is_none() {
        info.name = description.filter(|name| !name.trim().is_empty());
    }
    Some(info)
}

pub const fn listed(states: States) -> bool {
    !states.contains(States::DEFUNCT)
        && (states.contains(States::VISIBLE) || states.contains(States::SHOWING))
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
        Role::PUSH_BUTTON => "PushButton".into(),
        role => camel(role.name()),
    }
}

pub fn flags(states: States, actionable: bool) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if actionable && !states.contains(States::ENABLED) && !states.contains(States::SENSITIVE) {
        flags.push("disabled");
    }
    if states.contains(States::FOCUSED) {
        flags.push("focused");
    }
    if states.contains(States::SELECTED) {
        flags.push("selected");
    }
    if states.contains(States::EXPANDABLE) {
        flags.push(if states.contains(States::EXPANDED) {
            "expanded"
        } else {
            "collapsed"
        });
    }
    if states.contains(States::CHECKED) {
        flags.push("checked");
    }
    flags
}

pub fn node(element: Element, info: Info) -> Node<Element> {
    let actionable = info.actionable
        || info.interfaces.contains(Interfaces::EDITABLE_TEXT)
        || info.interfaces.contains(Interfaces::VALUE)
        || info.states.contains(States::SELECTABLE);
    let mut node = Node::new(role_name(info.role), element);
    node.name = info.name;
    node.value = info.value;
    node.states = flags(info.states, actionable);
    node.actionable = actionable;
    node.secure = info.role == Role::PASSWORD_TEXT;
    node
}

pub async fn tree(bus: &Bus, window: &Element) -> Tree {
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
        .find(|(_, info, _)| info.states.contains(States::FOCUSED))
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

pub async fn registered(
    bus: &Bus,
    known: &HashMap<String, (u32, String, Duration)>,
    shown: bool,
) -> Vec<Registered> {
    let root = Element {
        bus: REGISTRY.into(),
        path: ROOT.into(),
    };
    let (apps, _) = children(bus, &root, -1).await;
    let found: Vec<Option<Registered>> = stream::iter(apps)
        .map(|element| async move {
            let windows = async {
                if shown {
                    windows(bus, &element).await.unwrap_or_default()
                } else {
                    Vec::new()
                }
            };
            let (pid, name, activation) = match known.get(&element.bus) {
                Some((pid, name, activation)) => (Some(*pid), name.clone(), *activation),
                None => {
                    let pid = async {
                        let asked = bus.call(
                            BUS,
                            BUS_PATH,
                            BUS,
                            "GetConnectionUnixProcessID",
                            &[Value::Str(element.bus.clone())],
                        );
                        timed(async { read::<u32>(&asked.await?) }).await
                    };
                    let name = async {
                        let accessible = proxy(bus, &element)?;
                        timed(accessible.get::<String>(ACCESSIBLE, "Name")).await
                    };
                    let (pid, name, activation) =
                        tokio::join!(pid, name, activation(bus, &element));
                    (pid, name.unwrap_or_default(), activation)
                }
            };
            let windows = windows.await;
            Some(Registered {
                pid: pid?,
                name,
                activation,
                windows: windows
                    .iter()
                    .any(|(_, _, states)| states.contains(States::SHOWING)),
                element,
            })
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    found.into_iter().flatten().collect()
}

async fn activation(bus: &Bus, app: &Element) -> Duration {
    let Some(application) = proxy(bus, app) else {
        return Duration::ZERO;
    };
    let (toolkit, version) = tokio::join!(
        timed(application.get::<String>(APPLICATION, "ToolkitName")),
        timed(application.get::<String>(APPLICATION, "Version"))
    );
    if toolkit.as_deref() == Some("GTK") && version.is_some_and(|version| version.starts_with("4."))
    {
        GTK4_ACTIVATION
    } else {
        Duration::ZERO
    }
}

pub async fn windows(bus: &Bus, app: &Element) -> Option<Vec<(Element, Role, States)>> {
    let (children, cut) = children(bus, app, -1).await;
    if cut && children.is_empty() {
        return None;
    }
    let found: Vec<Option<(Element, Role, States)>> = stream::iter(children)
        .map(|element| async move {
            let (role, states) = tokio::join!(role(bus, &element), states(bus, &element));
            let role = role.filter(|role| WINDOWS.contains(role))?;
            Some((element, role, states.unwrap_or_default()))
        })
        .buffered(PARALLEL)
        .collect()
        .await;
    Some(found.into_iter().flatten().collect())
}

pub async fn window(bus: &Bus, apps: &[Element]) -> Result<Option<(Element, Option<String>)>, ()> {
    let mut all = Vec::new();
    let mut answered = apps.is_empty();
    for app in apps {
        if let Some(windows) = windows(bus, app).await {
            answered = true;
            all.extend(windows);
        }
    }
    if !answered {
        return Err(());
    }
    let Some((window, _, _)) = all
        .iter()
        .find(|(_, _, states)| states.contains(States::ACTIVE))
        .or_else(|| {
            all.iter()
                .find(|(_, _, states)| states.contains(States::SHOWING))
        })
    else {
        return Ok(None);
    };
    let title = match proxy(bus, window) {
        Some(accessible) => timed(accessible.get::<String>(ACCESSIBLE, "Name")).await,
        None => None,
    };
    Ok(Some((
        window.clone(),
        title.filter(|title| !title.trim().is_empty()),
    )))
}

use crate::{Error, proxy::Extension};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Reference {
    pub node: u64,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub text: String,
    pub refs: HashMap<String, Reference>,
}

#[derive(Default)]
struct Tab {
    session: String,
    turn: String,
    refs: HashMap<String, Reference>,
}

#[derive(Default)]
pub struct Tabs {
    tabs: HashMap<u64, Tab>,
    titles: HashMap<String, String>,
    active: HashMap<(String, String), Instant>,
    /// Tab groups already collapsed once: a group the member expanded stays open.
    collapsed: HashSet<u64>,
    /// Browser management failed once on this connection (OpenAI's gate is
    /// off for this browser, or the extension didn't answer), so groups
    /// aren't collapsed on it.
    unmanaged: bool,
}

pub const GROUP_TITLE_MAX: usize = 32;

pub fn group_title(title: &str) -> Option<String> {
    let words = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        return None;
    }
    if words.chars().count() <= GROUP_TITLE_MAX {
        return Some(words);
    }
    let cut: String = words.chars().take(GROUP_TITLE_MAX - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

fn idle_turns(
    active: &mut HashMap<(String, String), Instant>,
    now: Instant,
    idle: Duration,
) -> Vec<(String, String)> {
    let ended: Vec<_> = active
        .iter()
        .filter(|(_, last)| now.saturating_duration_since(**last) >= idle)
        .map(|(turn, _)| turn.clone())
        .collect();
    for turn in &ended {
        active.remove(turn);
    }
    ended
}

fn url(value: &str) -> Result<String, Error> {
    let parsed = url::Url::parse(value)?;
    if !["http", "https"].contains(&parsed.scheme())
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("browser URL must be an absolute http(s) URL without credentials".into());
    }
    Ok(parsed.to_string())
}

fn site(value: &str) -> String {
    url::Url::parse(value)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "the page".into())
}

fn ax_text(node: &Value) -> String {
    match &node["value"] {
        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn clipped(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub fn snapshot(nodes: &[Value], offset: usize) -> Snapshot {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n["nodeId"].as_str()?, n)))
        .collect();
    let children: HashSet<&str> = nodes
        .iter()
        .flat_map(|n| {
            n["childIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
        })
        .collect();
    let transparent = [
        "none",
        "generic",
        "GenericContainer",
        "InlineTextBox",
        "LineBreak",
        "RootWebArea",
        "WebArea",
        "Section",
        "presentation",
        "Pre",
        "Div",
        "paragraph",
        "LabelText",
    ];
    let actionable = [
        "button",
        "link",
        "textbox",
        "searchbox",
        "combobox",
        "checkbox",
        "radio",
        "switch",
        "slider",
        "spinbutton",
        "menuitem",
        "menuitemcheckbox",
        "menuitemradio",
        "option",
        "tab",
        "treeitem",
        "listbox",
        "textField",
        "TextField",
    ];
    let leaves = [
        "heading",
        "img",
        "image",
        "cell",
        "columnheader",
        "rowheader",
        "label",
    ];
    struct Walk<'a> {
        by_id: HashMap<&'a str, &'a Value>,
        seen: HashSet<String>,
        lines: Vec<String>,
        refs: HashMap<String, Reference>,
        length: usize,
        cut: bool,
        offset: usize,
        walked: usize,
        numbered: usize,
    }
    fn visit(
        node: &Value,
        depth: usize,
        walk: &mut Walk<'_>,
        transparent: &[&str],
        actionable: &[&str],
        leaves: &[&str],
    ) {
        let id = node["nodeId"].as_str().unwrap_or("");
        if walk.cut || !walk.seen.insert(id.into()) {
            return;
        }
        let role = ax_text(&node["role"]);
        let name = ax_text(&node["name"]);
        let ignored =
            node["ignored"] == true || role.is_empty() || transparent.contains(&role.as_str());
        if !ignored {
            let indent = "  ".repeat(depth.min(30));
            let mut reference = None;
            let mut line = if role == "StaticText" || role == "text" {
                if name.is_empty() {
                    String::new()
                } else {
                    format!("{indent}- text: {}", json!(clipped(&name, 200)))
                }
            } else {
                let mut line = format!("{indent}- {role}");
                if !name.is_empty() {
                    line.push_str(&format!(" {}", json!(clipped(&name, 200))));
                }
                if let Some(properties) = node["properties"].as_array() {
                    for property in properties {
                        let key = property["name"].as_str().unwrap_or("");
                        let value = &property["value"]["value"];
                        if key == "level" && value.is_number() {
                            line.push_str(&format!(" [level={value}]"));
                        } else if [
                            "checked", "disabled", "expanded", "selected", "pressed", "required",
                        ]
                        .contains(&key)
                            && !value.is_null()
                            && value != false
                            && value != "false"
                        {
                            line.push_str(&format!(
                                " [{key}{}]",
                                if value == true || value == "true" {
                                    String::new()
                                } else {
                                    format!("={value}")
                                }
                            ));
                        }
                    }
                }
                let focusable = node["properties"].as_array().is_some_and(|p| {
                    p.iter()
                        .any(|p| p["name"] == "focusable" && p["value"]["value"] == true)
                });
                if (actionable.contains(&role.as_str()) || focusable)
                    && let Some(dom) = node["backendDOMNodeId"].as_u64()
                {
                    walk.numbered += 1;
                    let key = format!("e{}", walk.numbered);
                    line.push_str(&format!(" [ref={key}]"));
                    reference = Some((
                        key,
                        Reference {
                            node: dom,
                            label: if name.is_empty() {
                                role.clone()
                            } else {
                                format!("{role} “{}”", clipped(&name, 60))
                            },
                        },
                    ));
                }
                let url = node["properties"]
                    .as_array()
                    .and_then(|p| p.iter().find(|p| p["name"] == "url"))
                    .map(|p| ax_text(&p["value"]))
                    .unwrap_or_default();
                if !url.is_empty() {
                    line.push_str(&format!(" [url={}]", clipped(&url, 500)));
                }
                let value = ax_text(&node["value"]);
                if !value.is_empty() && value != name && value != url {
                    line.push_str(&format!(": {}", json!(clipped(&value, 200))));
                }
                line
            };
            if !line.is_empty() && walk.walked < walk.offset {
                walk.walked += 1;
            } else if !line.is_empty() {
                if walk.length + line.len() + 1 > 30000 {
                    walk.cut = true;
                    return;
                }
                walk.length += line.len() + 1;
                walk.walked += 1;
                walk.lines.push(std::mem::take(&mut line));
                if let Some((key, target)) = reference {
                    walk.refs.insert(key, target);
                }
            }
        }
        if !ignored
            && !name.is_empty()
            && (actionable.contains(&role.as_str())
                || leaves.contains(&role.as_str())
                || role == "text"
                || role == "StaticText")
        {
            return;
        }
        if let Some(ids) = node["childIds"].as_array() {
            for child in ids {
                if let Some(next) = child.as_str().and_then(|id| walk.by_id.get(id)).copied() {
                    visit(
                        next,
                        depth + usize::from(!ignored),
                        walk,
                        transparent,
                        actionable,
                        leaves,
                    );
                }
            }
        }
    }
    let mut walk = Walk {
        by_id,
        seen: HashSet::new(),
        lines: vec![],
        refs: HashMap::new(),
        length: 0,
        cut: false,
        offset,
        walked: 0,
        numbered: 0,
    };
    for node in nodes {
        if node["nodeId"]
            .as_str()
            .is_some_and(|id| !children.contains(id))
        {
            visit(node, 0, &mut walk, &transparent, &actionable, &leaves);
        }
    }
    if offset > 0 {
        walk.lines.insert(
            0,
            if walk.walked < offset {
                format!(
                    "… offset {offset} is past the end: the snapshot has {} lines; offset 0 reads it from the top.",
                    walk.walked
                )
            } else {
                format!("… {offset} lines above (offset {offset}); offset 0 reads from the top.")
            },
        );
    }
    if walk.cut {
        walk.lines.push(format!(
            "… snapshot cut at 30000 characters after line {0}; snapshot with offset {0} reads on, or open a more specific page.",
            walk.walked
        ));
    }
    Snapshot {
        text: walk.lines.join("\n"),
        refs: walk.refs,
    }
}

impl Tabs {
    fn read(
        &mut self,
        ext: &mut Extension,
        session: &str,
        turn: &str,
        tab: u64,
        screenshot: bool,
        offset: usize,
    ) -> Result<Value, Error> {
        let current_url = ext.evaluate(session, turn, tab, "location.href")?;
        let title = ext.evaluate(session, turn, tab, "document.title")?;
        let tree = ext.cdp(session, turn, tab, "Accessibility.getFullAXTree", json!({}))?;
        let Snapshot { text, refs } = snapshot(
            tree["nodes"]
                .as_array()
                .ok_or("extension returned no accessibility tree")?,
            offset,
        );
        self.tabs.get_mut(&tab).ok_or("tab closed")?.refs = refs;
        let mut result = json!({"tab":tab,"url":current_url,"title":title,"snapshot":text});
        if screenshot {
            let image = ext.cdp(
                session,
                turn,
                tab,
                "Page.captureScreenshot",
                json!({"format":"jpeg","quality":70,"captureBeyondViewport":false}),
            )?;
            result["screenshot"] = json!({"media_type":"image/jpeg","data":image["data"]});
        }
        Ok(result)
    }

    fn settle(ext: &mut Extension, session: &str, turn: &str, tab: u64) -> Result<(), Error> {
        let expression = r#"new Promise(resolve=>{let timer;const done=()=>{observer.disconnect();resolve(true)};const observer=new MutationObserver(()=>{clearTimeout(timer);timer=setTimeout(done,200)});const start=()=>{observer.observe(document,{subtree:true,childList:true,attributes:true,characterData:true});timer=setTimeout(done,200)};if(document.readyState==='complete')start();else addEventListener('load',start,{once:true});setTimeout(done,10000)})"#;
        for _ in 0..3 {
            if ext.evaluate(session, turn, tab, expression).is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err("page did not finish loading".into())
    }

    /// Collapses the new tab's group the first time it is seen, so agent tabs
    /// don't spread across the tab strip. The extension creates groups expanded
    /// and only lets callers change them through its browser management
    /// command, which OpenAI gates per browser. Best effort.
    fn collapse(&mut self, ext: &mut Extension, namespace: &str, common: &Value, tab: u64) {
        if self.unmanaged {
            return;
        }
        let mut call = |module: &str, method: &str, args: Value| {
            let mut params = common.clone();
            params["type"] = "browser_management_call".into();
            params["browser_id"] = namespace.into();
            params["namespace"] = module.into();
            params["method"] = method.into();
            params["args"] = args;
            ext.rpc("executeUnhandledCommand", params)
                .map(|result| result["value"].clone())
        };
        let Ok(info) = call("tabs", "get", json!([tab])) else {
            // Gated off, unsupported or unanswered: don't make every later
            // open wait on it again.
            self.unmanaged = true;
            return;
        };
        let Some(group) = info["groupId"].as_i64().and_then(|g| u64::try_from(g).ok()) else {
            return;
        };
        if !self.collapsed.contains(&group)
            && call("tabGroups", "update", json!([group, {"collapsed": true}])).is_ok()
        {
            self.collapsed.insert(group);
        }
    }

    pub(crate) fn step(
        &mut self,
        ext: &mut Extension,
        namespace: &str,
        owner: &str,
        session: &str,
        turn: Option<&str>,
        input: &Value,
    ) -> Result<(String, Value), Error> {
        let session_id = format!("{namespace}:{session}");
        let fallback = format!("{namespace}-no-turn");
        let turn = turn.unwrap_or(&fallback);
        self.active
            .insert((session.to_owned(), turn.to_owned()), Instant::now());
        let common = json!({"session_id":session_id,"turn_id":turn});
        let action = input["action"].as_str().ok_or("missing browser action")?;
        let opened = action == "open";
        let target_url = if opened {
            Some(url(input["url"].as_str().ok_or("missing URL")?)?)
        } else {
            None
        };
        let tab = if opened {
            let created = ext.rpc("createTab", common.clone())?;
            let tab = created["id"]
                .as_u64()
                .ok_or("extension returned no tab ID")?;
            self.collapse(ext, namespace, &common, tab);
            self.tabs.insert(
                tab,
                Tab {
                    session: session.into(),
                    turn: turn.into(),
                    ..Tab::default()
                },
            );
            tab
        } else {
            let tab = input["tab"].as_u64().ok_or("missing browser tab")?;
            match self.tabs.get_mut(&tab) {
                Some(known) if known.session == session => known.turn = turn.into(),
                _ => {
                    return Err(format!(
                        "tab is not one of this session's {owner} tabs; open one first"
                    )
                    .into());
                }
            }
            tab
        };
        if action == "close" {
            ext.rpc(
                "attach",
                json!({"session_id":session_id,"turn_id":turn,"tabId":tab}),
            )?;
            let result = ext.cdp(&session_id, turn, tab, "Page.close", json!({}));
            let _ = ext.rpc(
                "detach",
                json!({"session_id":session_id,"turn_id":turn,"tabId":tab}),
            );
            result?;
            self.tabs.remove(&tab);
            return Ok((
                "Closed a browser tab".into(),
                json!({"tab":tab,"closed":true}),
            ));
        }
        let result = (|| {
            ext.rpc(
                "attach",
                json!({"session_id":session_id,"turn_id":turn,"tabId":tab}),
            )?;
            ext.cdp(
                &session_id,
                turn,
                tab,
                "Emulation.setFocusEmulationEnabled",
                json!({"enabled":true}),
            )?;
            let current_url = if let Some(ref value) = target_url {
                ext.cdp(
                    &session_id,
                    turn,
                    tab,
                    "Page.navigate",
                    json!({"url":value}),
                )?;
                Self::settle(ext, &session_id, turn, tab)?;
                value.clone()
            } else {
                ext.evaluate(&session_id, turn, tab, "location.href")?
                    .as_str()
                    .ok_or("missing tab URL")?
                    .to_string()
            };
            let summary = match action {
                "open" => format!("Opened {}", site(&current_url)),
                "snapshot" => format!("Read {}", site(&current_url)),
                "click" => {
                    let (node, label) = self.target(tab, input)?;
                    let _ = ext.cdp(
                        &session_id,
                        turn,
                        tab,
                        "DOM.scrollIntoViewIfNeeded",
                        json!({"backendNodeId":node}),
                    );
                    let quads = ext.cdp(
                        &session_id,
                        turn,
                        tab,
                        "DOM.getContentQuads",
                        json!({"backendNodeId":node}),
                    )?;
                    let quad = quads["quads"][0]
                        .as_array()
                        .ok_or("element is not visible; take a new snapshot")?;
                    let values: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
                    if values.len() != 8 {
                        return Err("invalid element position".into());
                    }
                    let x = (values[0] + values[2] + values[4] + values[6]) / 4.0;
                    let y = (values[1] + values[3] + values[5] + values[7]) / 4.0;
                    for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
                        ext.cdp(
                            &session_id,
                            turn,
                            tab,
                            "Input.dispatchMouseEvent",
                            json!({"type":kind,"x":x,"y":y,"button":"left","clickCount":1}),
                        )?;
                    }
                    Self::settle(ext, &session_id, turn, tab)?;
                    format!("Clicked {label} on {}", site(&current_url))
                }
                "type" => {
                    let (node, label) = self.target(tab, input)?;
                    ext.cdp(
                        &session_id,
                        turn,
                        tab,
                        "DOM.focus",
                        json!({"backendNodeId":node}),
                    )?;
                    let object = ext.cdp(
                        &session_id,
                        turn,
                        tab,
                        "DOM.resolveNode",
                        json!({"backendNodeId":node}),
                    )?;
                    let object_id = &object["object"]["objectId"];
                    ext.cdp(&session_id,turn,tab,"Runtime.callFunctionOn",json!({"objectId":object_id,"functionDeclaration":"function(){if(typeof this.select==='function'){this.select();return}const range=document.createRange();range.selectNodeContents(this);const selection=getSelection();selection.removeAllRanges();selection.addRange(range)}"}))?;
                    let text = input["text"].as_str().unwrap_or("");
                    if text.is_empty() {
                        Self::press(ext, &session_id, turn, tab, "Backspace")?;
                    } else {
                        ext.cdp(
                            &session_id,
                            turn,
                            tab,
                            "Input.insertText",
                            json!({"text":text}),
                        )?;
                    }
                    if input["submit"] == true {
                        Self::press(ext, &session_id, turn, tab, "Enter")?;
                    }
                    Self::settle(ext, &session_id, turn, tab)?;
                    format!(
                        "Typed into {label} on {}{}",
                        site(&current_url),
                        if input["submit"] == true {
                            " and pressed Enter"
                        } else {
                            ""
                        }
                    )
                }
                "press" => {
                    let key = input["key"].as_str().ok_or("missing key")?;
                    Self::press(ext, &session_id, turn, tab, key)?;
                    Self::settle(ext, &session_id, turn, tab)?;
                    format!("Pressed {key} on {}", site(&current_url))
                }
                "scroll" => {
                    let sign = if input["direction"] == "up" { -1 } else { 1 };
                    let script = "function(sign){let element=this instanceof Element?this:document.elementFromPoint(innerWidth/2,innerHeight/2);while(element&&element!==document.documentElement){const style=getComputedStyle(element);if(/(auto|scroll|overlay)/.test(style.overflowY)&&element.scrollHeight>element.clientHeight)break;element=element.parentElement}const box=element&&element!==document.documentElement?element:document.scrollingElement;box.scrollBy({top:sign*0.8*(box===document.scrollingElement?innerHeight:box.clientHeight),behavior:'instant'})}";
                    if !input["ref"].is_null() {
                        let (node, _) = self.target(tab, input)?;
                        let object = ext.cdp(
                            &session_id,
                            turn,
                            tab,
                            "DOM.resolveNode",
                            json!({"backendNodeId":node}),
                        )?;
                        ext.cdp(&session_id,turn,tab,"Runtime.callFunctionOn",json!({"objectId":object["object"]["objectId"],"functionDeclaration":script,"arguments":[{"value":sign}]}))?;
                    } else {
                        ext.evaluate(
                            &session_id,
                            turn,
                            tab,
                            &format!("({script}).call(null,{sign})"),
                        )?;
                    }
                    format!(
                        "Scrolled {} on {}",
                        if sign < 0 { "up" } else { "down" },
                        site(&current_url)
                    )
                }
                "navigate" => {
                    let destination = input["url"].as_str().ok_or("missing destination")?;
                    match destination {
                        "back" | "forward" => {
                            ext.evaluate(
                                &session_id,
                                turn,
                                tab,
                                &format!("history.{destination}()"),
                            )?;
                        }
                        "reload" => {
                            ext.cdp(&session_id, turn, tab, "Page.reload", json!({}))?;
                        }
                        other => {
                            ext.cdp(
                                &session_id,
                                turn,
                                tab,
                                "Page.navigate",
                                json!({"url":url(other)?}),
                            )?;
                        }
                    }
                    Self::settle(ext, &session_id, turn, tab)?;
                    format!(
                        "Navigated {}",
                        site(if ["back", "forward", "reload"].contains(&destination) {
                            &current_url
                        } else {
                            destination
                        })
                    )
                }
                _ => return Err(format!("unknown browser action {action}").into()),
            };
            Ok((
                summary,
                self.read(
                    ext,
                    &session_id,
                    turn,
                    tab,
                    input["screenshot"] == true,
                    usize::try_from(input["offset"].as_u64().unwrap_or(0)).unwrap_or(usize::MAX),
                )?,
            ))
        })();
        let _ = ext.rpc(
            "detach",
            json!({"session_id":session_id,"turn_id":turn,"tabId":tab}),
        );
        if result.is_err() && opened {
            let _ = ext.cdp(&session_id, turn, tab, "Page.close", json!({}));
            self.tabs.remove(&tab);
        }
        result
    }

    pub(crate) fn name(
        &mut self,
        ext: &mut Extension,
        namespace: &str,
        session: &str,
        turn: Option<&str>,
        title: &str,
    ) -> Result<(), Error> {
        let Some(title) = group_title(title) else {
            return Ok(());
        };
        if self.titles.get(session) == Some(&title) {
            return Ok(());
        }
        let fallback = format!("{namespace}-no-turn");
        ext.rpc(
            "nameSession",
            json!({"session_id":format!("{namespace}:{session}"),"turn_id":turn.unwrap_or(&fallback),"name":title}),
        )?;
        self.titles.insert(session.into(), title);
        Ok(())
    }

    pub(crate) fn end_turn(
        &mut self,
        ext: &mut Extension,
        namespace: &str,
        session: &str,
        turn: &str,
    ) -> Result<(), Error> {
        self.active.remove(&(session.to_owned(), turn.to_owned()));
        self.tabs
            .retain(|_, tab| tab.session != session || tab.turn != turn);
        ext.rpc(
            "turnEnded",
            json!({"session_id":format!("{namespace}:{session}"),"turn_id":turn}),
        )
        .map(drop)
    }

    pub(crate) fn end_idle(
        &mut self,
        ext: &mut Extension,
        namespace: &str,
        idle: Duration,
    ) -> Result<(), Error> {
        let mut failure = None;
        for (session, turn) in idle_turns(&mut self.active, Instant::now(), idle) {
            if let Err(error) = self.end_turn(ext, namespace, &session, &turn) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn target(&self, tab: u64, input: &Value) -> Result<(u64, String), Error> {
        let key = input["ref"]
            .as_str()
            .ok_or("missing ref; take a new snapshot")?;
        self.tabs
            .get(&tab)
            .and_then(|t| t.refs.get(key))
            .map(|reference| (reference.node, reference.label.clone()))
            .ok_or_else(|| format!("unknown ref {key}; take a new snapshot").into())
    }

    fn press(
        ext: &mut Extension,
        session: &str,
        turn: &str,
        tab: u64,
        spec: &str,
    ) -> Result<(), Error> {
        let mut parts: Vec<&str> = spec.split('+').map(str::trim).collect();
        let key = parts.pop().unwrap_or("");
        let mut modifiers = 0;
        for part in parts {
            modifiers |= match part {
                "Alt" => 1,
                "Control" | "Ctrl" => 2,
                "Meta" | "Cmd" | "Command" => 4,
                "Shift" => 8,
                _ => return Err(format!("unknown key {spec}").into()),
            };
        }
        let named = match key {
            "Enter" => Some(("Enter", 13, "\r")),
            "Tab" => Some(("Tab", 9, "")),
            "Escape" => Some(("Escape", 27, "")),
            "Backspace" => Some(("Backspace", 8, "")),
            "Delete" => Some(("Delete", 46, "")),
            "ArrowUp" => Some(("ArrowUp", 38, "")),
            "ArrowDown" => Some(("ArrowDown", 40, "")),
            "ArrowLeft" => Some(("ArrowLeft", 37, "")),
            "ArrowRight" => Some(("ArrowRight", 39, "")),
            "PageUp" => Some(("PageUp", 33, "")),
            "PageDown" => Some(("PageDown", 34, "")),
            "Space" => Some(("Space", 32, " ")),
            _ => None,
        };
        if named.is_none() && key.chars().count() != 1 {
            return Err(format!("unknown key {spec}").into());
        }
        let upper = key.to_ascii_uppercase();
        let letter = upper.bytes().all(|b| b.is_ascii_uppercase());
        let digit = key.bytes().all(|b| b.is_ascii_digit());
        let code = named.map(|n| n.0.to_string()).unwrap_or_else(|| {
            if letter {
                format!("Key{upper}")
            } else if digit {
                format!("Digit{key}")
            } else {
                String::new()
            }
        });
        let number = named.map(|n| n.1).unwrap_or_else(|| {
            if letter || digit {
                upper.bytes().next().unwrap_or(0) as i32
            } else {
                0
            }
        });
        let typed = named.map(|n| n.2).unwrap_or(key);
        let text = if modifiers & !8 == 0 { typed } else { "" };
        let params = json!({"key":if key=="Space"{" "}else{key},"code":code,"windowsVirtualKeyCode":number,"modifiers":modifiers,"text":text});
        ext.cdp(session,turn,tab,"Input.dispatchKeyEvent",json!({"type":if text.is_empty(){"rawKeyDown"}else{"keyDown"},"key":params["key"],"code":code,"windowsVirtualKeyCode":number,"modifiers":modifiers,"text":text}))?;
        ext.cdp(session,turn,tab,"Input.dispatchKeyEvent",json!({"type":"keyUp","key":params["key"],"code":code,"windowsVirtualKeyCode":number,"modifiers":modifiers}))?;
        Ok(())
    }
}

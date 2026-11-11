const MAX_VALUE: usize = 200;
const MASK: &str = "••••";
const STATIC: [&str; 3] = ["Label", "Static", "StaticText"];
pub const NO_WINDOW: &str = "(no open window)";

pub struct Node<E> {
    pub role: String,
    pub name: Option<String>,
    pub value: Option<String>,
    pub states: Vec<&'static str>,
    pub actionable: bool,
    pub secure: bool,
    pub element: E,
    pub children: Vec<Self>,
}

impl<E> Node<E> {
    pub fn new(role: impl Into<String>, element: E) -> Self {
        Self {
            role: role.into(),
            name: None,
            value: None,
            states: Vec::new(),
            actionable: false,
            secure: false,
            element,
            children: Vec::new(),
        }
    }
}

pub struct Line<E> {
    pub element: E,
    pub depth: usize,
    pub actionable: bool,
    pub text: String,
}

impl<E> Line<E> {
    pub fn shown(&self, index: Option<usize>) -> String {
        let indent = "  ".repeat(self.depth);
        match index {
            Some(index) => format!("{indent}[{index}] {}", self.text),
            None => format!("{indent}{}", self.text),
        }
    }
}

pub fn lines<E>(root: Node<E>) -> Vec<Line<E>> {
    let mut lines = Vec::new();
    walk(root, 0, None, &mut lines);
    lines
}

pub fn heading(title: &str, complete: bool) -> Vec<String> {
    let mut lines = vec![title.to_owned()];
    if !complete {
        lines.push("(tree cut short: it was too large or slow to read whole)".into());
    }
    lines
}

pub fn render<E>(title: &str, root: Option<Node<E>>, complete: bool) -> (String, Vec<E>) {
    let mut text = heading(title, complete);
    let mut elements = Vec::new();
    match root {
        Some(root) => {
            for line in lines(root) {
                let index = line.actionable.then_some(elements.len());
                text.push(line.shown(index));
                if line.actionable {
                    elements.push(line.element);
                }
            }
        }
        None => text.push(NO_WINDOW.into()),
    }
    (text.join("\n"), elements)
}

fn redundant<E>(node: &Node<E>, above: Option<&str>) -> bool {
    node.children.is_empty()
        && node.value.is_none()
        && node.states.is_empty()
        && STATIC.contains(&node.role.as_str())
        && node.name.as_deref().map(str::trim).is_none_or(|name| {
            above.is_some_and(|above| above.trim() == name)
                || !name.chars().any(char::is_alphanumeric)
        })
}

fn walk<E>(node: Node<E>, depth: usize, above: Option<&str>, lines: &mut Vec<Line<E>>) {
    if redundant(&node, above) {
        return;
    }
    let Node {
        role,
        name,
        value,
        states,
        actionable,
        secure,
        element,
        children,
    } = node;
    let value = if secure {
        value.map(|_| MASK.to_owned())
    } else {
        value
    };
    if !(actionable || name.is_some() || value.is_some()) {
        for child in children {
            walk(child, depth, above, lines);
        }
        return;
    }
    let mut text = role;
    if let Some(name) = &name {
        text.push_str(&format!(" \"{}\"", flat(name)));
    }
    if let Some(value) = value {
        text.push_str(&format!(" value=\"{}\"", flat(&value)));
    }
    if !states.is_empty() {
        text.push_str(&format!(" ({})", states.join(", ")));
    }
    lines.push(Line {
        element,
        depth,
        actionable,
        text,
    });
    for child in children {
        walk(child, depth + 1, name.as_deref(), lines);
    }
}

fn flat(text: &str) -> String {
    let mut flat = text.chars().flat_map(|character| match character {
        '\n' => ['\\', 'n'].into_iter().take(2),
        character => [character; 2].into_iter().take(1),
    });
    let shown: String = flat.by_ref().take(MAX_VALUE).collect();
    if flat.next().is_some() {
        format!("{shown}…")
    } else {
        shown
    }
}

#[cfg(target_os = "macos")]
pub fn shown_units(len: usize, unit: impl Fn(usize) -> u16) -> usize {
    let end = len.min(2 * (MAX_VALUE + 1));
    if end < len && (0xD800..0xDC00).contains(&unit(end - 1)) {
        end - 1
    } else {
        end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: usize,
    pub end: usize,
}

impl Selection {
    pub fn find(
        value: &str,
        text: &str,
        prefix: &str,
        suffix: &str,
        selection: &str,
    ) -> Result<Self, String> {
        if text.is_empty() {
            return Err("text is required: the text to select".into());
        }
        let start = value
            .match_indices(text)
            .map(|(start, _)| start)
            .find(|&start| {
                value[..start].ends_with(prefix) && value[start + text.len()..].starts_with(suffix)
            })
            .ok_or("That text is not in the element's value, so nothing was selected.")?;
        let end = start + text.len();
        Ok(match selection {
            "cursor_before" => Self { start, end: start },
            "cursor_after" => Self { start: end, end },
            _ => Self { start, end },
        })
    }

    pub fn utf16(self, value: &str) -> (usize, usize) {
        let units = |bytes: &str| bytes.encode_utf16().count();
        let location = units(&value[..self.start]);
        (location, units(&value[self.start..self.end]))
    }

    pub fn chars(self, value: &str) -> (usize, usize) {
        let location = value[..self.start].chars().count();
        (
            location,
            location + value[self.start..self.end].chars().count(),
        )
    }
}

pub fn text_range(
    value: &str,
    text: &str,
    prefix: &str,
    suffix: &str,
    selection: &str,
) -> Result<(usize, usize), String> {
    Selection::find(value, text, prefix, suffix, selection).map(|found| found.utf16(value))
}

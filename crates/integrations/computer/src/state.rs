const MAX_VALUE: usize = 200;
const MASK: &str = "••••";

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

pub fn render<E>(title: &str, root: Option<Node<E>>, complete: bool) -> (String, Vec<E>) {
    let mut lines = vec![title.to_owned()];
    if !complete {
        lines.push("(tree cut short: it was too large or slow to read whole)".into());
    }
    let mut elements = Vec::new();
    match root {
        Some(root) => walk(root, 0, &mut lines, &mut elements),
        None => lines.push("(no open window)".into()),
    }
    (lines.join("\n"), elements)
}

fn walk<E>(node: Node<E>, depth: usize, lines: &mut Vec<String>, elements: &mut Vec<E>) {
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
            walk(child, depth, lines, elements);
        }
        return;
    }
    let mut line = "  ".repeat(depth);
    if actionable {
        line.push_str(&format!("[{}] ", elements.len()));
        elements.push(element);
    }
    line.push_str(&role);
    if let Some(name) = name {
        line.push_str(&format!(" \"{}\"", flat(&name)));
    }
    if let Some(value) = value {
        line.push_str(&format!(" value=\"{}\"", flat(&value)));
    }
    if !states.is_empty() {
        line.push_str(&format!(" ({})", states.join(", ")));
    }
    lines.push(line);
    for child in children {
        walk(child, depth + 1, lines, elements);
    }
}

fn flat(text: &str) -> String {
    let flat = text.replace('\n', "\\n");
    if flat.chars().count() > MAX_VALUE {
        format!("{}…", flat.chars().take(MAX_VALUE).collect::<String>())
    } else {
        flat
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

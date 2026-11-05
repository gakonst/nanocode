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

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        role: &str,
        name: Option<&str>,
        actionable: bool,
        element: u8,
        children: Vec<Node<u8>>,
    ) -> Node<u8> {
        let mut node = Node::new(role, element);
        node.name = name.map(Into::into);
        node.actionable = actionable;
        node.children = children;
        node
    }

    #[test]
    fn states_number_actionable_elements_and_drop_empty_containers() {
        let mut field = node("TextField", Some("Search"), true, 3, vec![]);
        field.value = Some("line\nnext".into());
        field.states = vec!["focused"];
        let root = node(
            "Window",
            Some("Calculator"),
            false,
            0,
            vec![
                node(
                    "Group",
                    None,
                    false,
                    1,
                    vec![node("Button", Some("7"), true, 2, vec![])],
                ),
                field,
            ],
        );
        let (text, elements) = render("Calculator — window \"Calculator\"", Some(root), true);
        assert_eq!(
            text,
            "Calculator — window \"Calculator\"\nWindow \"Calculator\"\n  [0] Button \"7\"\n  [1] TextField \"Search\" value=\"line\\nnext\" (focused)"
        );
        assert_eq!(elements, [2, 3]);
        let (text, elements) = render::<u8>("Finder", None, false);
        assert!(text.contains("cut short") && text.ends_with("(no open window)"));
        assert!(elements.is_empty());
        let long = "x".repeat(300);
        let (text, _) = render(
            "T",
            Some(node("StaticText", Some(&long), false, 0, vec![])),
            true,
        );
        assert!(
            text.ends_with(&format!("\"{}…\"", "x".repeat(200))),
            "{text}"
        );
    }

    #[test]
    fn secure_values_are_masked_whatever_the_backend_read() {
        let mut password = node("TextField", Some("Password"), true, 1, vec![]);
        password.value = Some("hunter2".into());
        password.secure = true;
        let (text, _) = render("Login", Some(password), true);
        assert!(
            text.ends_with("[0] TextField \"Password\" value=\"••••\""),
            "{text}"
        );
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn selections_are_utf16_ranges_in_context() {
        let value = "naïve cat, brave cat";
        assert_eq!(text_range(value, "cat", "", "", "text"), Ok((6, 3)));
        assert_eq!(text_range(value, "cat", "brave ", "", "text"), Ok((17, 3)));
        assert_eq!(
            text_range(value, "cat", "", ",", "cursor_after"),
            Ok((9, 0))
        );
        assert_eq!(
            text_range(value, "brave", "", "", "cursor_before"),
            Ok((11, 0))
        );
        assert!(text_range(value, "dog", "", "", "text").is_err());
        assert!(text_range(value, "", "", "", "text").is_err());
        assert_eq!(text_range("😀 cat", "cat", "", "", "text"), Ok((3, 3)));
    }

    #[test]
    fn selections_are_character_offsets_for_at_spi() {
        let value = "😀 naïve cat";
        let found = Selection::find(value, "cat", "", "", "text").unwrap();
        assert_eq!(found.chars(value), (8, 11));
        assert_eq!(found.utf16(value), (9, 3));
        let caret = Selection::find(value, "naïve", "", "", "cursor_after").unwrap();
        assert_eq!(caret.chars(value), (7, 7));
    }
}

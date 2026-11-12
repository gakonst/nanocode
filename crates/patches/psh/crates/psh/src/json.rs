//! A small JSON tree and its compact and indented text (D029).

/// A JSON value. Objects keep their keys in insertion order.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    /// An object builder: `Json::obj().set("a", 1)`.
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    pub fn set(mut self, key: &'static str, value: impl Into<Json>) -> Json {
        if let Json::Obj(fields) = &mut self {
            fields.push((key, value.into()));
        }
        self
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}

impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Num(v as u64)
    }
}

impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Num(v as u64)
    }
}

impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}

impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}

impl From<Vec<Json>> for Json {
    fn from(v: Vec<Json>) -> Json {
        Json::Arr(v)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

const PRETTY_INDENT_SPACES: usize = 2;
const PRETTY_LINE_WIDTH_CHARS: usize = 100;

enum Open<'a> {
    Items {
        rest: std::slice::Iter<'a, Json>,
        first: bool,
    },
    Fields {
        rest: std::slice::Iter<'a, (&'static str, Json)>,
        first: bool,
    },
}

enum Layout {
    Compact,
    Pretty,
}

impl Json {
    fn is_container(&self) -> bool {
        matches!(self, Json::Arr(_) | Json::Obj(_))
    }

    fn is_leaf_container(&self) -> bool {
        match self {
            Json::Arr(items) => !items.iter().any(Json::is_container),
            Json::Obj(fields) => !fields.iter().any(|(_, value)| value.is_container()),
            Json::Null | Json::Bool(_) | Json::Num(_) | Json::Str(_) => false,
        }
    }

    fn write_scalar(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => out.push_str(&n.to_string()),
            Json::Str(s) => write_string(s, out),
            Json::Arr(_) | Json::Obj(_) => unreachable!("containers are written by write"),
        }
    }

    fn single_line(&self) -> String {
        let mut out = String::new();
        match self {
            Json::Arr(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    item.write_scalar(&mut out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    write_string(key, &mut out);
                    out.push_str(": ");
                    value.write_scalar(&mut out);
                }
                out.push('}');
            }
            Json::Null | Json::Bool(_) | Json::Num(_) | Json::Str(_) => self.write_scalar(&mut out),
        }
        out
    }

    fn write(&self, layout: Layout) -> String {
        let mut out = String::new();
        let mut stack: Vec<Open<'_>> = Vec::new();
        let mut next = Some((self, 0));
        loop {
            if let Some((value, column)) = next.take() {
                let inline = matches!(layout, Layout::Pretty) && value.is_leaf_container() && {
                    let line = value.single_line();
                    let fits = column + line.chars().count() <= PRETTY_LINE_WIDTH_CHARS;
                    if fits {
                        out.push_str(&line);
                    }
                    fits
                };
                if !inline {
                    match value {
                        Json::Arr(items) => {
                            out.push('[');
                            stack.push(Open::Items {
                                rest: items.iter(),
                                first: true,
                            });
                        }
                        Json::Obj(fields) => {
                            out.push('{');
                            stack.push(Open::Fields {
                                rest: fields.iter(),
                                first: true,
                            });
                        }
                        Json::Null | Json::Bool(_) | Json::Num(_) | Json::Str(_) => {
                            value.write_scalar(&mut out)
                        }
                    }
                }
            }
            let depth = stack.len();
            let Some(top) = stack.last_mut() else {
                break;
            };
            let (entry, first) = match top {
                Open::Items { rest, first } => (rest.next().map(|item| (None, item)), first),
                Open::Fields { rest, first } => {
                    (rest.next().map(|(key, value)| (Some(*key), value)), first)
                }
            };
            let was_first = std::mem::replace(first, false);
            let Some((key, value)) = entry else {
                let close = match top {
                    Open::Items { .. } => ']',
                    Open::Fields { .. } => '}',
                };
                stack.pop();
                if matches!(layout, Layout::Pretty) && !was_first {
                    out.push('\n');
                    push_indent(&mut out, depth - 1);
                }
                out.push(close);
                continue;
            };
            if !was_first {
                out.push(',');
            }
            let mut column = 0;
            if matches!(layout, Layout::Pretty) {
                out.push('\n');
                push_indent(&mut out, depth);
                column = PRETTY_INDENT_SPACES * depth;
            }
            if let Some(key) = key {
                let start = out.len();
                write_string(key, &mut out);
                out.push_str(match layout {
                    Layout::Compact => ":",
                    Layout::Pretty => ": ",
                });
                column += out[start..].chars().count();
            }
            next = Some((value, column));
        }
        out
    }

    pub fn to_pretty(&self) -> String {
        self.write(Layout::Pretty)
    }

    pub fn to_compact(&self) -> String {
        self.write(Layout::Compact)
    }
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..PRETTY_INDENT_SPACES * depth {
        out.push(' ');
    }
}

fn write_string(s: &str, out: &mut String) {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => {
                let byte = c as u8;
                out.push_str("\\u00");
                out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
                out.push(char::from(HEX_DIGITS[usize::from(byte & 0xf)]));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes() {
        let j = Json::obj()
            .set("a", 1u32)
            .set("b", vec![Json::Null, true.into(), "x\"y".into()])
            .set("c", Json::obj());
        assert_eq!(j.to_compact(), r#"{"a":1,"b":[null,true,"x\"y"],"c":{}}"#);
    }
}

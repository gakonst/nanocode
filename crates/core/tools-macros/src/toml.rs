use std::collections::BTreeSet;

#[derive(Debug)]
pub(crate) enum Value {
    String(String),
    Boolean(bool),
    Table(Table),
    Other,
}

#[derive(Debug, Default)]
pub(crate) struct Table {
    entries: Vec<(String, Value)>,
}

impl Table {
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    fn table_mut(&mut self, key: &str) -> Option<&mut Self> {
        if self.get(key).is_none() {
            self.entries
                .push((key.to_owned(), Value::Table(Self::default())));
        }
        match self.entries.iter_mut().find(|(name, _)| name == key) {
            Some((_, Value::Table(table))) => Some(table),
            _ => None,
        }
    }

    fn insert(&mut self, path: &[String], value: Value) -> Option<()> {
        let (last, parents) = path.split_last()?;
        let mut table = self;
        for parent in parents {
            table = table.table_mut(parent)?;
        }
        if table.get(last).is_some() {
            return None;
        }
        table.entries.push((last.clone(), value));
        Some(())
    }
}

impl Value {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    pub(crate) const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) const fn as_table(&self) -> Option<&Table> {
        match self {
            Self::Table(table) => Some(table),
            _ => None,
        }
    }
}

const NESTING_DEPTH_MAX: usize = 128;

pub(crate) fn parse(text: &str) -> Option<Table> {
    let mut parser = Parser {
        chars: text.chars().collect(),
        position: 0,
    };
    parser.document()
}

struct Parser {
    chars: Vec<char>,
    position: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.position).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.position + offset).copied()
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            return true;
        }
        false
    }

    fn eat_str(&mut self, expected: &str) -> bool {
        let matches = expected
            .chars()
            .enumerate()
            .all(|(offset, c)| self.peek_at(offset) == Some(c));
        if matches {
            self.position += expected.chars().count();
        }
        matches
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.position += 1;
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some('#') {
            while !matches!(self.peek(), None | Some('\n')) {
                self.position += 1;
            }
        }
    }

    fn skip_blank(&mut self) {
        loop {
            self.skip_spaces();
            self.skip_comment();
            if !(self.eat('\n') || self.eat_str("\r\n")) {
                return;
            }
        }
    }

    fn end_of_line(&mut self) -> Option<()> {
        self.skip_spaces();
        self.skip_comment();
        if self.peek().is_none() || self.eat('\n') || self.eat_str("\r\n") {
            return Some(());
        }
        None
    }

    fn document(&mut self) -> Option<Table> {
        let mut root = Table::default();
        let mut current: Vec<String> = Vec::new();
        let mut in_array_table = false;
        let mut headers: BTreeSet<Vec<String>> = BTreeSet::new();
        let mut scratch = Table::default();
        loop {
            self.skip_blank();
            if self.peek().is_none() {
                return Some(root);
            }
            if self.eat_str("[[") {
                self.skip_spaces();
                self.key_path()?;
                self.skip_spaces();
                if !self.eat_str("]]") {
                    return None;
                }
                in_array_table = true;
                scratch = Table::default();
            } else if self.eat('[') {
                self.skip_spaces();
                let path = self.key_path()?;
                self.skip_spaces();
                if !self.eat(']') || !headers.insert(path.clone()) {
                    return None;
                }
                let mut table = &mut root;
                for part in &path {
                    table = table.table_mut(part)?;
                }
                current = path;
                in_array_table = false;
            } else {
                let path = self.key_path()?;
                self.skip_spaces();
                if !self.eat('=') {
                    return None;
                }
                self.skip_spaces();
                let value = self.value(0)?;
                if in_array_table {
                    scratch.insert(&path, value)?;
                } else {
                    let mut table = &mut root;
                    for part in &current {
                        table = table.table_mut(part)?;
                    }
                    table.insert(&path, value)?;
                }
            }
            self.end_of_line()?;
        }
    }

    fn key_path(&mut self) -> Option<Vec<String>> {
        let mut path = vec![self.key()?];
        loop {
            let start = self.position;
            self.skip_spaces();
            if !self.eat('.') {
                self.position = start;
                return Some(path);
            }
            self.skip_spaces();
            path.push(self.key()?);
        }
    }

    fn key(&mut self) -> Option<String> {
        match self.peek()? {
            '"' => self.basic_string(),
            '\'' => self.literal_string(),
            _ => {
                let start = self.position;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    self.position += 1;
                }
                (self.position > start).then(|| self.chars[start..self.position].iter().collect())
            }
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > NESTING_DEPTH_MAX {
            return None;
        }
        match self.peek()? {
            '"' if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') => {
                self.multiline_basic_string().map(Value::String)
            }
            '\'' if self.peek_at(1) == Some('\'') && self.peek_at(2) == Some('\'') => {
                self.multiline_literal_string().map(Value::String)
            }
            '"' => self.basic_string().map(Value::String),
            '\'' => self.literal_string().map(Value::String),
            '[' => self.array(depth),
            '{' => self.inline_table(depth),
            _ => self.scalar(),
        }
    }

    fn scalar(&mut self) -> Option<Value> {
        let start = self.position;
        while self
            .peek()
            .is_some_and(|c| !matches!(c, ',' | ']' | '}' | '#' | '\n' | '\r'))
        {
            self.position += 1;
        }
        let text: String = self.chars[start..self.position].iter().collect();
        let text = text.trim_end();
        self.position = start + text.chars().count();
        match text {
            "true" => Some(Value::Boolean(true)),
            "false" => Some(Value::Boolean(false)),
            "" => None,
            _ if text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-_.:".contains(c) || c == ' ') =>
            {
                Some(Value::Other)
            }
            _ => None,
        }
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        self.eat('[');
        loop {
            self.skip_blank();
            if self.eat(']') {
                return Some(Value::Other);
            }
            self.value(depth + 1)?;
            self.skip_blank();
            if self.eat(']') {
                return Some(Value::Other);
            }
            if !self.eat(',') {
                return None;
            }
        }
    }

    fn inline_table(&mut self, depth: usize) -> Option<Value> {
        self.eat('{');
        let mut table = Table::default();
        self.skip_spaces();
        if self.eat('}') {
            return Some(Value::Table(table));
        }
        loop {
            self.skip_spaces();
            let path = self.key_path()?;
            self.skip_spaces();
            if !self.eat('=') {
                return None;
            }
            self.skip_spaces();
            let value = self.value(depth + 1)?;
            table.insert(&path, value)?;
            self.skip_spaces();
            if self.eat('}') {
                return Some(Value::Table(table));
            }
            if !self.eat(',') {
                return None;
            }
        }
    }

    fn literal_string(&mut self) -> Option<String> {
        self.eat('\'');
        let start = self.position;
        while self.peek()? != '\'' {
            if self.peek()? == '\n' {
                return None;
            }
            self.position += 1;
        }
        let text = self.chars[start..self.position].iter().collect();
        self.position += 1;
        Some(text)
    }

    fn multiline_literal_string(&mut self) -> Option<String> {
        self.eat_str("'''");
        self.eat('\n');
        let start = self.position;
        while !self.eat_str("'''") {
            self.peek()?;
            self.position += 1;
        }
        while self.peek() == Some('\'') {
            self.position += 1;
        }
        let end = self.position - 3;
        Some(self.chars[start..end].iter().collect())
    }

    fn escape(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += 1;
        let digits = match c {
            'b' => return Some('\u{8}'),
            't' => return Some('\t'),
            'n' => return Some('\n'),
            'f' => return Some('\u{c}'),
            'r' => return Some('\r'),
            'e' => return Some('\u{1b}'),
            '"' => return Some('"'),
            '\\' => return Some('\\'),
            'x' => 2,
            'u' => 4,
            'U' => 8,
            _ => return None,
        };
        let hex: String = self
            .chars
            .get(self.position..self.position + digits)?
            .iter()
            .collect();
        self.position += digits;
        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
    }

    fn basic_string(&mut self) -> Option<String> {
        self.eat('"');
        let mut text = String::new();
        loop {
            match self.peek()? {
                '"' => {
                    self.position += 1;
                    return Some(text);
                }
                '\n' => return None,
                '\\' => {
                    self.position += 1;
                    text.push(self.escape()?);
                }
                c => {
                    self.position += 1;
                    text.push(c);
                }
            }
        }
    }

    fn multiline_basic_string(&mut self) -> Option<String> {
        self.eat_str("\"\"\"");
        self.eat('\n');
        let mut text = String::new();
        loop {
            if self.eat_str("\"\"\"") {
                while self.peek() == Some('"') {
                    self.position += 1;
                    text.push('"');
                }
                return Some(text);
            }
            match self.peek()? {
                '\\' if matches!(self.peek_at(1), Some(' ' | '\t' | '\n' | '\r')) => {
                    self.position += 1;
                    while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
                        self.position += 1;
                    }
                }
                '\\' => {
                    self.position += 1;
                    text.push(self.escape()?);
                }
                c => {
                    self.position += 1;
                    text.push(c);
                }
            }
        }
    }
}

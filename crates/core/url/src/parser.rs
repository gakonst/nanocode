use std::mem;

use crate::encode::{EncodeSet, encode_char, encode_str};
use crate::host::{HostKind, parse_host};
use crate::{Authority, Components, ParseError, Url, default_port};

const SPECIAL_SCHEMES: [&str; 6] = ["ftp", "file", "http", "https", "ws", "wss"];
const PORT_MAX: u32 = 65_535;

pub(crate) fn is_special(scheme: &str) -> bool {
    SPECIAL_SCHEMES.contains(&scheme)
}

const fn tab_or_newline(value: char) -> bool {
    matches!(value, '\t' | '\n' | '\r')
}

pub(crate) fn encode_component(input: &str, set: EncodeSet) -> String {
    let mut out = String::with_capacity(input.len());
    for value in input.chars().filter(|value| !tab_or_newline(*value)) {
        encode_char(&mut out, value, set);
    }
    out
}

fn is_windows_drive_letter(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':' | '|'), None) if letter.is_ascii_alphabetic()
    )
}

fn is_normalized_windows_drive_letter(text: &str) -> bool {
    is_windows_drive_letter(text) && text.ends_with(':')
}

fn starts_with_windows_drive_letter(chars: &[char]) -> bool {
    match chars {
        [letter, ':' | '|'] => letter.is_ascii_alphabetic(),
        [letter, ':' | '|', next, ..] => {
            letter.is_ascii_alphabetic() && matches!(next, '/' | '\\' | '?' | '#')
        }
        _ => false,
    }
}

fn is_single_dot(segment: &str) -> bool {
    segment == "." || segment.eq_ignore_ascii_case("%2e")
}

fn is_double_dot(segment: &str) -> bool {
    matches!(segment, ".." | ".%2e" | ".%2E")
        || segment.eq_ignore_ascii_case("%2e.")
        || segment.eq_ignore_ascii_case("%2e%2e")
}

#[derive(Clone)]
enum Path {
    Opaque(String),
    Segments(Vec<String>),
}

#[derive(Clone)]
struct Record {
    scheme: String,
    username: String,
    password: String,
    host: Option<(HostKind, String)>,
    port: Option<u16>,
    path: Path,
    query: Option<String>,
    fragment: Option<String>,
}

impl Record {
    const fn empty() -> Self {
        Self {
            scheme: String::new(),
            username: String::new(),
            password: String::new(),
            host: None,
            port: None,
            path: Path::Segments(Vec::new()),
            query: None,
            fragment: None,
        }
    }

    fn from_url(url: &Url) -> Self {
        let path = if url.cannot_be_a_base() {
            Path::Opaque(url.path().to_owned())
        } else {
            Path::Segments(match url.path().strip_prefix('/') {
                Some(rest) => rest.split('/').map(str::to_owned).collect(),
                None => Vec::new(),
            })
        };
        Self {
            scheme: url.scheme().to_owned(),
            username: url.username().to_owned(),
            password: url.password().unwrap_or("").to_owned(),
            host: url.has_authority().then(|| {
                (
                    url.host,
                    url.serialization[url.host_start..url.host_end].to_owned(),
                )
            }),
            port: url.port,
            path,
            query: url.query().map(str::to_owned),
            fragment: url.fragment().map(str::to_owned),
        }
    }

    fn segments(&mut self) -> &mut Vec<String> {
        match &mut self.path {
            Path::Segments(segments) => segments,
            Path::Opaque(_) => panic!("a URL with an opaque path has no path segments"),
        }
    }

    fn shorten_path(&mut self) {
        let file = self.scheme == "file";
        let segments = self.segments();
        if file && segments.len() == 1 && is_normalized_windows_drive_letter(&segments[0]) {
            return;
        }
        segments.pop();
    }

    fn into_url(self) -> Url {
        let path = match &self.path {
            Path::Opaque(path) => path.clone(),
            Path::Segments(segments) => {
                let mut path = String::new();
                for segment in segments {
                    path.push('/');
                    path.push_str(segment);
                }
                path
            }
        };
        Components {
            scheme: &self.scheme,
            authority: self.host.as_ref().map(|(host, text)| Authority {
                username: &self.username,
                password: &self.password,
                host: *host,
                host_text: text,
                port: self.port,
            }),
            path: &path,
            query: self.query.as_deref(),
            fragment: self.fragment.as_deref(),
        }
        .assemble()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    SchemeStart,
    Scheme,
    NoScheme,
    SpecialRelativeOrAuthority,
    PathOrAuthority,
    Relative,
    RelativeSlash,
    SpecialAuthoritySlashes,
    SpecialAuthorityIgnoreSlashes,
    Authority,
    Host,
    Port,
    File,
    FileSlash,
    FileHost,
    PathStart,
    Path,
    OpaquePath,
    Query,
    Fragment,
}

struct Parser {
    chars: Vec<char>,
    base: Option<Record>,
    url: Record,
    buffer: String,
    state: State,
    pointer: isize,
    at_sign_seen: bool,
    inside_brackets: bool,
    password_token_seen: bool,
    setter: bool,
}

pub(crate) fn parse(input: &str, base: Option<&Url>) -> Result<Url, ParseError> {
    let trimmed = input.trim_matches(|value: char| value <= ' ');
    let mut parser = Parser {
        chars: trimmed
            .chars()
            .filter(|value| !tab_or_newline(*value))
            .collect(),
        base: base.map(Record::from_url),
        url: Record::empty(),
        buffer: String::new(),
        state: State::SchemeStart,
        pointer: 0,
        at_sign_seen: false,
        inside_brackets: false,
        password_token_seen: false,
        setter: false,
    };
    parser.run()?;
    Ok(parser.url.into_url())
}

pub(crate) fn path_for_setter(scheme: &str, opaque: bool, input: &str) -> String {
    let mut path = String::new();
    if opaque {
        let rest = match input.strip_prefix('/') {
            Some(rest) => {
                path.push_str("%2F");
                rest
            }
            None => input,
        };
        for value in rest.chars().filter(|value| !tab_or_newline(*value)) {
            encode_char(&mut path, value, EncodeSet::Controls);
        }
        return path;
    }
    let first = input.chars().find(|value| !tab_or_newline(*value));
    if !is_special(scheme) && matches!(first, Some('?' | '#')) {
        return path;
    }
    let mut url = Record::empty();
    url.scheme = scheme.to_owned();
    let mut parser = Parser {
        chars: input
            .chars()
            .filter(|value| !tab_or_newline(*value))
            .collect(),
        base: None,
        url,
        buffer: String::new(),
        state: State::PathStart,
        pointer: 0,
        at_sign_seen: false,
        inside_brackets: false,
        password_token_seen: false,
        setter: true,
    };
    if parser.run().is_err() {
        return path;
    }
    for segment in parser.url.segments().iter() {
        path.push('/');
        path.push_str(segment);
    }
    path
}

impl Parser {
    fn current(&self) -> Option<char> {
        usize::try_from(self.pointer)
            .ok()
            .and_then(|index| self.chars.get(index).copied())
    }

    fn remaining(&self, offset: usize) -> Option<char> {
        usize::try_from(self.pointer)
            .ok()
            .and_then(|index| self.chars.get(index + 1 + offset).copied())
    }

    fn remaining_chars(&self) -> &[char] {
        let start = usize::try_from(self.pointer)
            .unwrap_or(0)
            .min(self.chars.len());
        &self.chars[start..]
    }

    fn special(&self) -> bool {
        is_special(&self.url.scheme)
    }

    const fn back(&mut self) {
        self.pointer -= 1;
    }

    fn run(&mut self) -> Result<(), ParseError> {
        let length = isize::try_from(self.chars.len()).map_err(|_| ParseError::Overflow)?;
        loop {
            let current = self.current();
            self.step(current)?;
            if self.pointer >= length {
                return Ok(());
            }
            self.pointer += 1;
        }
    }

    fn step(&mut self, current: Option<char>) -> Result<(), ParseError> {
        match self.state {
            State::SchemeStart => self.scheme_start(current),
            State::Scheme => self.scheme(current),
            State::NoScheme => return self.no_scheme(current),
            State::SpecialRelativeOrAuthority => {
                if current == Some('/') && self.remaining(0) == Some('/') {
                    self.state = State::SpecialAuthorityIgnoreSlashes;
                    self.pointer += 1;
                } else {
                    self.state = State::Relative;
                    self.back();
                }
            }
            State::PathOrAuthority => {
                if current == Some('/') {
                    self.state = State::Authority;
                } else {
                    self.state = State::Path;
                    self.back();
                }
            }
            State::Relative => self.relative(current),
            State::RelativeSlash => self.relative_slash(current),
            State::SpecialAuthoritySlashes => {
                self.state = State::SpecialAuthorityIgnoreSlashes;
                if current == Some('/') && self.remaining(0) == Some('/') {
                    self.pointer += 1;
                } else {
                    self.back();
                }
            }
            State::SpecialAuthorityIgnoreSlashes => {
                if !matches!(current, Some('/' | '\\')) {
                    self.state = State::Authority;
                    self.back();
                }
            }
            State::Authority => return self.authority(current),
            State::Host => return self.host(current),
            State::Port => return self.port(current),
            State::File => self.file(current),
            State::FileSlash => self.file_slash(current),
            State::FileHost => return self.file_host(current),
            State::PathStart => self.path_start(current),
            State::Path => self.path(current),
            State::OpaquePath => self.opaque_path(current),
            State::Query => self.query(current),
            State::Fragment => {
                if let Some(value) = current {
                    let fragment = self.url.fragment.get_or_insert_with(String::new);
                    encode_char(fragment, value, EncodeSet::Fragment);
                }
            }
        }
        Ok(())
    }

    fn scheme_start(&mut self, current: Option<char>) {
        match current {
            Some(value) if value.is_ascii_alphabetic() => {
                self.buffer.push(value.to_ascii_lowercase());
                self.state = State::Scheme;
            }
            _ => {
                self.state = State::NoScheme;
                self.back();
            }
        }
    }

    fn scheme(&mut self, current: Option<char>) {
        match current {
            Some(value) if value.is_ascii_alphanumeric() || matches!(value, '+' | '-' | '.') => {
                self.buffer.push(value.to_ascii_lowercase());
            }
            Some(':') => {
                self.url.scheme = mem::take(&mut self.buffer);
                let special = self.special();
                let same_as_base = self
                    .base
                    .as_ref()
                    .is_some_and(|base| base.scheme == self.url.scheme);
                if self.url.scheme == "file" {
                    self.state = State::File;
                } else if special && same_as_base {
                    self.state = State::SpecialRelativeOrAuthority;
                } else if special {
                    self.state = State::SpecialAuthoritySlashes;
                } else if self.remaining(0) == Some('/') {
                    self.state = State::PathOrAuthority;
                    self.pointer += 1;
                } else {
                    self.url.path = Path::Opaque(String::new());
                    self.state = State::OpaquePath;
                }
            }
            _ => {
                self.buffer.clear();
                self.state = State::NoScheme;
                self.pointer = -1;
            }
        }
    }

    fn no_scheme(&mut self, current: Option<char>) -> Result<(), ParseError> {
        let Some(base) = &self.base else {
            return Err(ParseError::RelativeUrlWithoutBase);
        };
        if matches!(base.path, Path::Opaque(_)) {
            if current != Some('#') {
                return Err(ParseError::RelativeUrlWithCannotBeABaseBase);
            }
            self.url.scheme = base.scheme.clone();
            self.url.path = base.path.clone();
            self.url.query = base.query.clone();
            self.url.fragment = Some(String::new());
            self.state = State::Fragment;
            return Ok(());
        }
        self.state = if base.scheme == "file" {
            State::File
        } else {
            State::Relative
        };
        self.back();
        Ok(())
    }

    fn copy_authority_from_base(&mut self) {
        if let Some(base) = &self.base {
            self.url.username = base.username.clone();
            self.url.password = base.password.clone();
            self.url.host = base.host.clone();
            self.url.port = base.port;
        }
    }

    fn relative(&mut self, current: Option<char>) {
        let Some(base) = &self.base else {
            panic!("the relative state needs a base URL");
        };
        self.url.scheme = base.scheme.clone();
        let special = self.special();
        match current {
            Some('/') => self.state = State::RelativeSlash,
            Some('\\') if special => self.state = State::RelativeSlash,
            _ => {
                self.copy_authority_from_base();
                if let Some(base) = &self.base {
                    self.url.path = base.path.clone();
                    self.url.query = base.query.clone();
                }
                match current {
                    Some('?') => {
                        self.url.query = Some(String::new());
                        self.state = State::Query;
                    }
                    Some('#') => {
                        self.url.fragment = Some(String::new());
                        self.state = State::Fragment;
                    }
                    Some(_) => {
                        self.url.query = None;
                        self.url.shorten_path();
                        self.state = State::Path;
                        self.back();
                    }
                    None => {}
                }
            }
        }
    }

    fn relative_slash(&mut self, current: Option<char>) {
        if self.special() && matches!(current, Some('/' | '\\')) {
            self.state = State::SpecialAuthorityIgnoreSlashes;
        } else if current == Some('/') {
            self.state = State::Authority;
        } else {
            self.copy_authority_from_base();
            self.state = State::Path;
            self.back();
        }
    }

    fn ends_authority(&self, current: Option<char>) -> bool {
        match current {
            None | Some('/' | '?' | '#') => true,
            Some('\\') => self.special(),
            Some(_) => false,
        }
    }

    fn authority(&mut self, current: Option<char>) -> Result<(), ParseError> {
        if current == Some('@') {
            if self.at_sign_seen {
                self.buffer.insert_str(0, "%40");
            }
            self.at_sign_seen = true;
            let buffer = mem::take(&mut self.buffer);
            for value in buffer.chars() {
                if value == ':' && !self.password_token_seen {
                    self.password_token_seen = true;
                    continue;
                }
                let target = if self.password_token_seen {
                    &mut self.url.password
                } else {
                    &mut self.url.username
                };
                encode_char(target, value, EncodeSet::Userinfo);
            }
        } else if self.ends_authority(current) {
            if self.at_sign_seen && self.buffer.is_empty() {
                return Err(ParseError::EmptyHost);
            }
            let length =
                isize::try_from(self.buffer.chars().count()).map_err(|_| ParseError::Overflow)?;
            self.pointer -= length + 1;
            self.buffer.clear();
            self.state = State::Host;
        } else if let Some(value) = current {
            self.buffer.push(value);
        }
        Ok(())
    }

    fn host(&mut self, current: Option<char>) -> Result<(), ParseError> {
        let special = self.special();
        if current == Some(':') && !self.inside_brackets {
            if self.buffer.is_empty() {
                return Err(ParseError::EmptyHost);
            }
            self.url.host = Some(parse_host(&self.buffer, !special)?);
            self.buffer.clear();
            self.state = State::Port;
        } else if self.ends_authority(current) {
            self.back();
            if special && self.buffer.is_empty() {
                return Err(ParseError::EmptyHost);
            }
            self.url.host = Some(parse_host(&self.buffer, !special)?);
            self.buffer.clear();
            self.state = State::PathStart;
        } else if let Some(value) = current {
            if value == '[' {
                self.inside_brackets = true;
            }
            if value == ']' {
                self.inside_brackets = false;
            }
            self.buffer.push(value);
        }
        Ok(())
    }

    fn port(&mut self, current: Option<char>) -> Result<(), ParseError> {
        match current {
            Some(value) if value.is_ascii_digit() => {
                self.buffer.push(value);
                return Ok(());
            }
            _ if self.ends_authority(current) => {}
            _ => return Err(ParseError::InvalidPort),
        }
        if !self.buffer.is_empty() {
            let mut port = 0u32;
            for digit in self.buffer.chars().filter_map(|value| value.to_digit(10)) {
                port = port * 10 + digit;
                if port > PORT_MAX {
                    return Err(ParseError::InvalidPort);
                }
            }
            let port = u16::try_from(port).map_err(|_| ParseError::InvalidPort)?;
            self.url.port = (default_port(&self.url.scheme) != Some(port)).then_some(port);
            self.buffer.clear();
        }
        self.state = State::PathStart;
        self.back();
        Ok(())
    }

    fn base_file(&self) -> Option<&Record> {
        self.base.as_ref().filter(|base| base.scheme == "file")
    }

    fn file(&mut self, current: Option<char>) {
        self.url.scheme = "file".to_owned();
        self.url.host = Some((HostKind::None, String::new()));
        if matches!(current, Some('/' | '\\')) {
            self.state = State::FileSlash;
            return;
        }
        let Some(base) = self.base_file().cloned() else {
            self.state = State::Path;
            self.back();
            return;
        };
        self.url.host = base.host;
        self.url.path = base.path;
        self.url.query = base.query;
        match current {
            Some('?') => {
                self.url.query = Some(String::new());
                self.state = State::Query;
            }
            Some('#') => {
                self.url.fragment = Some(String::new());
                self.state = State::Fragment;
            }
            Some(_) => {
                self.url.query = None;
                if starts_with_windows_drive_letter(self.remaining_chars()) {
                    self.url.path = Path::Segments(Vec::new());
                } else {
                    self.url.shorten_path();
                }
                self.state = State::Path;
                self.back();
            }
            None => {}
        }
    }

    fn file_slash(&mut self, current: Option<char>) {
        if matches!(current, Some('/' | '\\')) {
            self.state = State::FileHost;
            return;
        }
        if let Some(base) = self.base_file() {
            let host = base.host.clone();
            let drive = match &base.path {
                Path::Segments(segments) => segments
                    .first()
                    .filter(|segment| is_normalized_windows_drive_letter(segment))
                    .cloned(),
                Path::Opaque(_) => None,
            };
            self.url.host = host;
            if let Some(drive) = drive
                && !starts_with_windows_drive_letter(self.remaining_chars())
            {
                self.url.segments().push(drive);
            }
        }
        self.state = State::Path;
        self.back();
    }

    fn file_host(&mut self, current: Option<char>) -> Result<(), ParseError> {
        match current {
            None | Some('/' | '\\' | '?' | '#') => {
                self.back();
                if is_windows_drive_letter(&self.buffer) {
                    self.state = State::Path;
                } else if self.buffer.is_empty() {
                    self.url.host = Some((HostKind::None, String::new()));
                    self.state = State::PathStart;
                } else {
                    let mut host = parse_host(&self.buffer, false)?;
                    if host.1 == "localhost" {
                        host = (HostKind::None, String::new());
                    }
                    self.url.host = Some(host);
                    self.buffer.clear();
                    self.state = State::PathStart;
                }
            }
            Some(value) => self.buffer.push(value),
        }
        Ok(())
    }

    fn path_start(&mut self, current: Option<char>) {
        if self.special() {
            self.state = State::Path;
            if !matches!(current, Some('/' | '\\')) {
                self.back();
            }
        } else if !self.setter && current == Some('?') {
            self.url.query = Some(String::new());
            self.state = State::Query;
        } else if !self.setter && current == Some('#') {
            self.url.fragment = Some(String::new());
            self.state = State::Fragment;
        } else if current.is_some() {
            self.state = State::Path;
            if current != Some('/') {
                self.back();
            }
        }
    }

    fn path(&mut self, current: Option<char>) {
        let special = self.special();
        let slash = current == Some('/') || (special && current == Some('\\'));
        let ends =
            current.is_none() || slash || (!self.setter && matches!(current, Some('?' | '#')));
        if !ends {
            if let Some(value) = current {
                encode_char(&mut self.buffer, value, EncodeSet::Path);
            }
            return;
        }
        let mut segment = mem::take(&mut self.buffer);
        if is_double_dot(&segment) {
            self.url.shorten_path();
            if !slash {
                self.url.segments().push(String::new());
            }
        } else if is_single_dot(&segment) {
            if !slash {
                self.url.segments().push(String::new());
            }
        } else {
            if self.url.scheme == "file"
                && self.url.segments().is_empty()
                && is_windows_drive_letter(&segment)
            {
                segment.replace_range(1..2, ":");
                if self
                    .url
                    .host
                    .as_ref()
                    .is_some_and(|(_, text)| !text.is_empty())
                {
                    self.url.host = Some((HostKind::None, String::new()));
                }
            }
            self.url.segments().push(segment);
        }
        match current {
            Some('?') if !self.setter => {
                self.url.query = Some(String::new());
                self.state = State::Query;
            }
            Some('#') if !self.setter => {
                self.url.fragment = Some(String::new());
                self.state = State::Fragment;
            }
            _ => {}
        }
    }

    fn opaque_path(&mut self, current: Option<char>) {
        match current {
            Some('?') => {
                self.url.query = Some(String::new());
                self.state = State::Query;
            }
            Some('#') => {
                self.url.fragment = Some(String::new());
                self.state = State::Fragment;
            }
            Some(value) => {
                if let Path::Opaque(path) = &mut self.url.path {
                    encode_char(path, value, EncodeSet::Controls);
                }
            }
            None => {}
        }
    }

    fn query(&mut self, current: Option<char>) {
        match current {
            Some('#') | None => {
                let set = if self.special() {
                    EncodeSet::SpecialQuery
                } else {
                    EncodeSet::Query
                };
                let buffer = mem::take(&mut self.buffer);
                encode_str(self.url.query.get_or_insert_with(String::new), &buffer, set);
                if current == Some('#') {
                    self.url.fragment = Some(String::new());
                    self.state = State::Fragment;
                }
            }
            Some(value) => self.buffer.push(value),
        }
    }
}

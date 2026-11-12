use std::io::{IsTerminal, Write};

#[derive(Clone, Copy)]
pub(crate) struct Style(&'static str);

pub(crate) const HEADER: Style = Style("\x1b[1m\x1b[4m");
pub(crate) const USAGE: Style = Style("\x1b[1m\x1b[4m");
pub(crate) const ERROR: Style = Style("\x1b[1m\x1b[31m");
pub(crate) const LITERAL: Style = Style("\x1b[1m");
pub(crate) const VALID: Style = Style("\x1b[32m");
pub(crate) const INVALID: Style = Style("\x1b[33m");
const RESET: &str = "\x1b[0m";

impl Style {
    pub(crate) fn paint(self, out: &mut String, text: &str) {
        out.push_str(self.0);
        out.push_str(text);
        out.push_str(RESET);
    }

    pub(crate) fn painted(self, text: &str) -> String {
        let mut out = String::new();
        self.paint(&mut out, text);
        out
    }
}

pub(crate) fn display_width(text: &str) -> usize {
    let mut width = 0;
    let mut control = false;
    for character in text.chars() {
        if character.is_ascii_control() {
            control = true;
        } else if control && character == 'm' {
            control = false;
            continue;
        }
        if !control {
            width += 1;
        }
    }
    width
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParameter,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    DcsParameter,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    SosPmApcString,
    Utf8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Print,
    Execute,
    BeginUtf8,
    Other,
}

const DELETE: u8 = 0x7f;

const fn is_control(byte: u8) -> bool {
    matches!(byte, 0x00..=0x17 | 0x19 | 0x1c..=0x1f)
}

const fn transition(state: State, byte: u8) -> (Option<State>, Action) {
    match byte {
        0x18 | 0x1a => return (Some(State::Ground), Action::Execute),
        0x1b => return (Some(State::Escape), Action::Other),
        _ => {}
    }
    match state {
        State::Ground => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x20..=0x7f => (None, Action::Print),
            0x80..=0x8f | 0x91..=0x9a | 0x9c => (None, Action::Execute),
            0xc2..=0xf4 => (Some(State::Utf8), Action::BeginUtf8),
            _ => (None, Action::Other),
        },
        State::Escape => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x20..=0x2f => (Some(State::EscapeIntermediate), Action::Other),
            0x5b => (Some(State::CsiEntry), Action::Other),
            0x5d => (Some(State::OscString), Action::Other),
            0x50 => (Some(State::DcsEntry), Action::Other),
            0x58 | 0x5e | 0x5f => (Some(State::SosPmApcString), Action::Other),
            0x30..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::EscapeIntermediate => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x30..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::CsiEntry => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x20..=0x2f => (Some(State::CsiIntermediate), Action::Other),
            0x30..=0x3f => (Some(State::CsiParameter), Action::Other),
            0x40..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::CsiIgnore => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x40..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::CsiParameter => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x3c..=0x3f => (Some(State::CsiIgnore), Action::Other),
            0x20..=0x2f => (Some(State::CsiIntermediate), Action::Other),
            0x40..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::CsiIntermediate => match byte {
            _ if is_control(byte) => (None, Action::Execute),
            0x30..=0x3f => (Some(State::CsiIgnore), Action::Other),
            0x40..=0x7e => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::DcsEntry => match byte {
            0x20..=0x2f => (Some(State::DcsIntermediate), Action::Other),
            0x30..=0x3f => (Some(State::DcsParameter), Action::Other),
            0x40..=0x7e => (Some(State::DcsPassthrough), Action::Other),
            _ => (None, Action::Other),
        },
        State::DcsIntermediate => match byte {
            0x30..=0x3f => (Some(State::DcsIgnore), Action::Other),
            0x40..=0x7e => (Some(State::DcsPassthrough), Action::Other),
            _ => (None, Action::Other),
        },
        State::DcsParameter => match byte {
            0x3c..=0x3f => (Some(State::DcsIgnore), Action::Other),
            0x20..=0x2f => (Some(State::DcsIntermediate), Action::Other),
            0x40..=0x7e => (Some(State::DcsPassthrough), Action::Other),
            _ => (None, Action::Other),
        },
        State::DcsPassthrough | State::DcsIgnore | State::SosPmApcString => match byte {
            0x9c => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::OscString => match byte {
            0x07 => (Some(State::Ground), Action::Other),
            _ => (None, Action::Other),
        },
        State::Utf8 => (None, Action::Other),
    }
}

fn is_printable(action: Action, byte: u8) -> bool {
    (action == Action::Print && byte != DELETE)
        || action == Action::BeginUtf8
        || (action == Action::Execute && byte.is_ascii_whitespace())
}

const fn is_continuation(byte: u8) -> bool {
    matches!(byte, 0x80..=0xbf)
}

pub(crate) fn strip(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut stripped = String::with_capacity(text.len());
    let mut state = State::Ground;
    let mut offset = 0;
    while offset < bytes.len() {
        let start = bytes[offset..].iter().position(|byte| {
            let (next, action) = transition(state, *byte);
            if let Some(next) = next {
                state = next;
            }
            is_printable(action, *byte)
        });
        offset += start.unwrap_or(bytes.len() - offset);
        state = State::Ground;
        let length = bytes[offset..]
            .iter()
            .position(|byte| {
                let (_, action) = transition(State::Ground, *byte);
                !(is_printable(action, *byte) || is_continuation(*byte))
            })
            .unwrap_or(bytes.len() - offset);
        stripped.push_str(&text[offset..offset + length]);
        offset += length;
    }
    stripped
}

const fn continuation_count(lead: u8) -> usize {
    match lead {
        0xf0..=0xf4 => 3,
        0xe0..=0xef => 2,
        _ => 1,
    }
}

pub(crate) fn strip_stream(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut stripped = Vec::with_capacity(bytes.len());
    let mut state = State::Ground;
    let mut continuations = 0;
    let mut offset = 0;
    while offset < bytes.len() {
        while offset < bytes.len() && state != State::Utf8 {
            let (next, action) = transition(state, bytes[offset]);
            if let Some(next) = next {
                state = next;
            }
            if is_printable(action, bytes[offset]) {
                break;
            }
            offset += 1;
        }
        let start = offset;
        while offset < bytes.len() {
            let byte = bytes[offset];
            if state == State::Utf8 {
                if is_continuation(byte) {
                    continuations -= 1;
                    if continuations == 0 {
                        state = State::Ground;
                    }
                } else {
                    continuations = continuation_count(byte);
                }
                offset += 1;
                continue;
            }
            let (next, action) = transition(State::Ground, byte);
            if let Some(next) = next {
                state = next;
            }
            if state == State::Utf8 {
                continuations = continuation_count(byte);
                offset += 1;
                continue;
            }
            if !is_printable(action, byte) {
                break;
            }
            offset += 1;
        }
        stripped.extend_from_slice(&bytes[start..offset]);
    }
    stripped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

fn variable_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn colors(terminal: bool) -> bool {
    let clicolor = std::env::var_os("CLICOLOR").map(|value| value != "0");
    if variable_set("NO_COLOR") {
        return false;
    }
    if variable_set("CLICOLOR_FORCE") {
        return true;
    }
    if clicolor == Some(false) {
        return false;
    }
    let term = std::env::var_os("TERM").map_or(cfg!(windows), |value| value != "dumb");
    terminal && (term || clicolor == Some(true) || std::env::var_os("CI").is_some())
}

pub(crate) fn write(stream: Stream, styled: &str) -> std::io::Result<()> {
    match stream {
        Stream::Stdout => write_to(std::io::stdout().lock(), styled),
        Stream::Stderr => write_to(std::io::stderr().lock(), styled),
    }
}

fn write_to(mut out: impl Write + IsTerminal, styled: &str) -> std::io::Result<()> {
    if colors(out.is_terminal()) {
        out.write_all(styled.as_bytes())?;
    } else {
        out.write_all(&strip_stream(styled))?;
    }
    out.flush()
}

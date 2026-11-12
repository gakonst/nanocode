use crate::unicode;
use std::{borrow::Cow, env, path::Path};

pub fn normalize_line_endings(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(index) = remaining.find('\r') {
        normalized.push_str(&remaining[..index]);
        normalized.push('\n');
        remaining = &remaining[index + 1..];
        if let Some(after_newline) = remaining.strip_prefix('\n') {
            remaining = after_newline;
        }
    }
    normalized.push_str(remaining);
    Cow::Owned(normalized)
}

pub fn sanitize_terminal_text(text: &str) -> Cow<'_, str> {
    sanitize_terminal_text_with_break(text, '\n', "    ")
}

pub fn sanitize_terminal_text_inline(text: &str) -> Cow<'_, str> {
    sanitize_terminal_text_with_break(text, ' ', "    ")
}

pub fn strip_terminal_controls(text: &str) -> Cow<'_, str> {
    sanitize_terminal_text_with_break(text, '\n', "\t")
}

fn sanitize_terminal_text_with_break<'a>(
    text: &'a str,
    line_break: char,
    tab: &str,
) -> Cow<'a, str> {
    let mut offset = 0;
    while offset < text.len() {
        offset += crate::text::control_prefix(&text[offset..], line_break == '\n');
        let Some(character) = text[offset..].chars().next() else {
            return Cow::Borrowed(text);
        };
        if character.is_control() {
            break;
        }
        offset += character.len_utf8();
    }
    if offset == text.len() {
        return Cow::Borrowed(text);
    }
    let mut sanitized = String::with_capacity(text.len());
    sanitized.push_str(&text[..offset]);
    let mut characters = text[offset..].chars();
    loop {
        let remaining = characters.as_str();
        let length = crate::text::control_prefix(remaining, line_break == '\n');
        sanitized.push_str(&remaining[..length]);
        characters = remaining[length..].chars();
        let Some(character) = characters.next() else {
            break;
        };
        match character {
            '\u{1b}' => skip_escape(&mut characters),
            '\u{9b}' => skip_control_sequence(&mut characters),
            '\r' => {
                if characters.as_str().starts_with('\n') {
                    characters.next();
                }
                sanitized.push(line_break);
            }
            '\n' => sanitized.push(line_break),
            '\t' => sanitized.push_str(tab),
            character if character.is_control() => sanitized.push('�'),
            character => sanitized.push(character),
        }
    }
    Cow::Owned(sanitized)
}

fn skip_escape(characters: &mut std::str::Chars<'_>) {
    match characters.next() {
        Some('[') => skip_control_sequence(characters),
        Some(']' | 'P' | 'X' | '^' | '_') => {
            let remaining = characters.as_str();
            if let Some(index) = memchr::memchr2(0x07, 0x1b, remaining.as_bytes()) {
                let mut end = index + 1;
                if remaining.as_bytes()[index] == 0x1b
                    && remaining.as_bytes().get(end) == Some(&b'\\')
                {
                    end += 1;
                }
                *characters = remaining[end..].chars();
            } else {
                *characters = "".chars();
            }
        }
        Some(' '..='/') => {
            while characters
                .as_str()
                .starts_with(|character| (' '..='/').contains(&character))
            {
                characters.next();
            }
            characters.next();
        }
        _ => {}
    }
}

fn skip_control_sequence(characters: &mut std::str::Chars<'_>) {
    let remaining = characters.as_str();
    let end = remaining
        .as_bytes()
        .iter()
        .position(|byte| !(b' '..=b'?').contains(byte))
        .unwrap_or(remaining.len());
    *characters = remaining[end..].chars();
    if characters
        .as_str()
        .starts_with(|character| ('@'..='~').contains(&character))
    {
        characters.next();
    }
}

pub fn terminal_text_width(text: &str) -> usize {
    if crate::text::ascii_prefix(text, false) == text.len() {
        return text.len();
    }
    unicode::graphemes(text)
        .map(|grapheme| match grapheme {
            "\t" => 4,
            grapheme if grapheme.contains(char::is_control) => 1,
            grapheme => unicode::width(grapheme),
        })
        .sum()
}

pub fn format_duration(nanoseconds: u64) -> String {
    if nanoseconds >= 1_000_000_000 {
        let tenths = duration_display_tick(nanoseconds).saturating_sub(1_000);
        return format!("{}.{:01}s", tenths / 10, tenths % 10);
    }
    format!("{}ms", duration_display_tick(nanoseconds))
}

pub fn format_turn_duration(nanoseconds: u64) -> String {
    let total_seconds = nanoseconds / 1_000_000_000;
    let days = total_seconds / 86_400;
    let hours = total_seconds % 86_400 / 3_600;
    let minutes = total_seconds % 3_600 / 60;
    let seconds = total_seconds % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    parts.push(format!("{seconds}s"));
    parts.join(" ")
}

pub const fn duration_display_tick(nanoseconds: u64) -> u64 {
    if nanoseconds < 1_000_000_000 {
        return nanoseconds / 1_000_000;
    }
    1_000_u64.saturating_add(nanoseconds.saturating_add(50_000_000) / 100_000_000)
}

pub fn humanize_tool(name: &str) -> String {
    name.trim_start_matches("mcp__")
        .replace("__", " · ")
        .replace('_', " ")
}

pub fn shorten_home(path: &Path) -> String {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    let Some(home) = home else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_owned();
    }
    let Ok(relative) = path.strip_prefix(&home) else {
        return path.display().to_string();
    };
    format!("~/{}", relative.display())
}

use std::ffi::OsStr;
use std::io::{self, BufRead};

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

pub(crate) fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for ch in chunk.valid().chars() {
            if ch.is_ascii() {
                escape_byte(
                    &mut out,
                    u8::try_from(ch).expect("an ASCII char"),
                );
            } else {
                out.push(ch);
            }
        }
        for &byte in chunk.invalid() {
            escape_byte(&mut out, byte);
        }
    }
    out
}

fn escape_byte(out: &mut String, byte: u8) {
    match byte {
        0x21..=0x5B | 0x5D..=0x7E => out.push(char::from(byte)),
        b'\0' => out.push_str("\\0"),
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        b'\t' => out.push_str("\\t"),
        b'\\' => out.push_str("\\\\"),
        _ => {
            out.push_str("\\x");
            out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(HEX_DIGITS[usize::from(byte & 0xF)]));
        }
    }
}

fn push_char(out: &mut Vec<u8>, ch: char) {
    let mut buffer = [0; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
}

pub(crate) fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_char(&mut out, ch);
            continue;
        }
        let Some(escaped) = chars.next() else {
            out.push(b'\\');
            break;
        };
        match escaped {
            '0' => out.push(b'\0'),
            '\\' => out.push(b'\\'),
            'r' => out.push(b'\r'),
            'n' => out.push(b'\n'),
            't' => out.push(b'\t'),
            'x' => {
                let Some(first) = chars.next() else {
                    out.extend_from_slice(b"\\x");
                    break;
                };
                let Some(high) = first.to_digit(16) else {
                    out.extend_from_slice(b"\\x");
                    push_char(&mut out, first);
                    continue;
                };
                let Some(second) = chars.next() else {
                    out.extend_from_slice(b"\\x");
                    push_char(&mut out, first);
                    break;
                };
                let Some(low) = second.to_digit(16) else {
                    out.extend_from_slice(b"\\x");
                    push_char(&mut out, first);
                    push_char(&mut out, second);
                    continue;
                };
                out.push(
                    u8::try_from(high << 4 | low)
                        .expect("two hex digits make a byte"),
                );
            }
            other => {
                out.push(b'\\');
                push_char(&mut out, other);
            }
        }
    }
    out
}

pub(crate) fn for_each_line<R: BufRead>(
    mut reader: R,
    mut each: impl FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 || !each(&line)? {
            return Ok(());
        }
    }
}

pub(crate) fn trim_terminator(line: &[u8]) -> &[u8] {
    let Some(line) = line.strip_suffix(b"\n") else {
        return line;
    };
    line.strip_suffix(b"\r").unwrap_or(line)
}

pub(crate) fn trim_whitespace(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut position = 0;
    let mut end = 0;
    let mut leading = true;
    for chunk in bytes.utf8_chunks() {
        for ch in chunk.valid().chars() {
            position += ch.len_utf8();
            if !ch.is_whitespace() {
                leading = false;
                end = position;
            } else if leading {
                start = position;
            }
        }
        if !chunk.invalid().is_empty() {
            leading = false;
            position += chunk.invalid().len();
            end = position;
        }
    }
    if end <= start {
        return &[];
    }
    &bytes[start..end]
}

pub(crate) fn os_str(bytes: &[u8]) -> Result<&OsStr, std::str::Utf8Error> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).map(OsStr::new)
    }
}

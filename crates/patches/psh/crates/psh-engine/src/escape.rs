//! Backslash escapes of `echo -e` and `printf` (`$'…'` is psh-syntax's
//! `decode_ansi_c`).

/// Which escape dialect: they differ in octal and `\c`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// `echo -e` and `printf %b`: `\0nnn` octal (0–3 digits after the 0), `\c` stops
    /// all output.
    Echo,
    /// `printf`'s format: `\nnn` octal, `\"`; `\c` is literal.
    Printf,
    /// `printf %b`: as `echo -e`, and `\nnn` without the leading 0 is octal too.
    PercentB,
}

/// The result of decoding: the bytes, and whether `\c` asked to stop the output.
pub(crate) struct Decoded {
    pub bytes: Vec<u8>,
    pub stop: bool,
}

fn push_char(out: &mut Vec<u8>, c: u32) {
    if let Some(ch) = char::from_u32(c) {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

fn hex(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

pub(crate) fn decode(s: &[u8], dialect: Dialect) -> Decoded {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b != b'\\' || i + 1 >= s.len() {
            out.push(b);
            i += 1;
            continue;
        }
        let c = s[i + 1];
        i += 2;
        match c {
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'e' | b'E' => out.push(0x1b),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'\\' => out.push(b'\\'),
            b'"' if dialect == Dialect::Printf => out.push(b'"'),
            b'c' if dialect != Dialect::Printf => {
                return Decoded {
                    bytes: out,
                    stop: true,
                };
            }
            b'0'..=b'7' => {
                let (mut v, max) = if dialect != Dialect::Printf {
                    if c == b'0' {
                        (0u32, 3)
                    } else if dialect == Dialect::PercentB {
                        (u32::from(c - b'0'), 2)
                    } else {
                        // `\1` in echo is not an escape.
                        out.push(b'\\');
                        out.push(c);
                        continue;
                    }
                } else {
                    (u32::from(c - b'0'), 2)
                };
                let mut n = 0;
                while n < max && i < s.len() && (b'0'..=b'7').contains(&s[i]) {
                    v = v * 8 + u32::from(s[i] - b'0');
                    i += 1;
                    n += 1;
                }
                out.push(v as u8);
            }
            b'x' => {
                let mut v = 0u32;
                let mut n = 0;
                while n < 2 && i < s.len() && hex(s[i]).is_some() {
                    v = v * 16 + hex(s[i]).unwrap_or(0);
                    i += 1;
                    n += 1;
                }
                if n == 0 {
                    out.extend_from_slice(b"\\x");
                } else {
                    out.push(v as u8);
                }
            }
            b'u' | b'U' => {
                let max = if c == b'u' { 4 } else { 8 };
                let mut v = 0u32;
                let mut n = 0;
                while n < max && i < s.len() && hex(s[i]).is_some() {
                    v = v * 16 + hex(s[i]).unwrap_or(0);
                    i += 1;
                    n += 1;
                }
                if n == 0 {
                    out.push(b'\\');
                    out.push(c);
                } else {
                    push_char(&mut out, v);
                }
            }
            _ => {
                out.push(b'\\');
                out.push(c);
            }
        }
    }
    Decoded {
        bytes: out,
        stop: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo() {
        let d = decode(br"a\0101b\cdrop", Dialect::Echo);
        assert_eq!(d.bytes, b"aAb");
        assert!(d.stop);
        assert_eq!(
            decode(br"\141|\0141|\1419|\558", Dialect::PercentB).bytes,
            b"a|a|a9|-8"
        );
        assert_eq!(decode(br"\141", Dialect::Echo).bytes, br"\141");
    }
}

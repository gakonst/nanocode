//! Just enough of a sed script parser to find the commands with effects beyond printing:
//! `w`/`W` and the `w` flag of `s` write files, `e` and the `e` flag run commands, `r`/`R`
//! read files. GNU and BSD syntax.

/// What a sed script does besides transforming its input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SedEffects {
    pub writes: bool,
    pub reads: bool,
    pub runs: bool,
}

/// `None` if the script does not parse (the caller treats it as unknown).
pub(crate) fn effects(script: &str) -> Option<SedEffects> {
    let b = script.as_bytes();
    let mut e = SedEffects::default();
    let mut i = 0;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b';') {
            i += 1;
        }
        if i >= b.len() {
            return Some(e);
        }
        if b[i] == b'#' {
            i = line_end(b, i);
            continue;
        }
        // Addresses: `N`, `$`, `/re/`, `\cREc`, `first~step`, `addr,addr`, `addr,+N`.
        i = address(b, i)?;
        skip_blanks(b, &mut i);
        if i < b.len() && b[i] == b',' {
            i += 1;
            skip_blanks(b, &mut i);
            if i < b.len() && (b[i] == b'+' || b[i] == b'~') {
                i += 1;
            }
            i = address(b, i)?;
        }
        skip_blanks(b, &mut i);
        while i < b.len() && b[i] == b'!' {
            i += 1;
            skip_blanks(b, &mut i);
        }
        let &cmd = b.get(i)?;
        i += 1;
        match cmd {
            b'{' | b'}' => {}
            b'=' | b'd' | b'D' | b'g' | b'G' | b'h' | b'H' | b'n' | b'N' | b'p' | b'P' | b'x'
            | b'z' | b'F' | b'l' | b'L' | b'q' | b'Q' => {
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                skip_blanks(b, &mut i);
                if i < b.len() && !matches!(b[i], b';' | b'\n' | b'}' | b'#') {
                    return None;
                }
            }
            b':' | b'b' | b't' | b'T' | b'v' => {
                while i < b.len() && !matches!(b[i], b';' | b'\n') && !(cmd != b':' && b[i] == b'}')
                {
                    i += 1;
                }
            }
            b'a' | b'i' | b'c' => {
                // Text to the end of the line; a trailing backslash continues it.
                loop {
                    let end = line_end(b, i);
                    let continued = end > i && b[end - 1] == b'\\';
                    i = end;
                    if !continued || i >= b.len() {
                        break;
                    }
                    i += 1;
                }
            }
            b'r' | b'R' => {
                e.reads = true;
                i = line_end(b, i);
            }
            b'w' | b'W' => {
                e.writes = true;
                i = line_end(b, i);
            }
            b'e' => {
                e.runs = true;
                i = line_end(b, i);
            }
            b's' => {
                let d = *b.get(i)?;
                if d == b'\n' || d == b'\\' {
                    return None;
                }
                i = delimited(b, i + 1, d, true)?;
                i = delimited(b, i, d, false)?;
                while i < b.len() {
                    match b[i] {
                        b'g' | b'p' | b'i' | b'I' | b'm' | b'M' | b'0'..=b'9' => i += 1,
                        b'e' => {
                            e.runs = true;
                            i += 1;
                        }
                        b'w' => {
                            e.writes = true;
                            i = line_end(b, i);
                        }
                        _ => break,
                    }
                }
            }
            b'y' => {
                let d = *b.get(i)?;
                i = delimited(b, i + 1, d, false)?;
                i = delimited(b, i, d, false)?;
            }
            _ => return None,
        }
    }
}

fn skip_blanks(b: &[u8], i: &mut usize) {
    while *i < b.len() && (b[*i] == b' ' || b[*i] == b'\t') {
        *i += 1;
    }
}

fn line_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'\n' {
        i += 1;
    }
    i
}

fn address(b: &[u8], mut i: usize) -> Option<usize> {
    match b.get(i) {
        Some(c) if c.is_ascii_digit() => {
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'~') {
                i += 1;
            }
        }
        Some(b'$') => i += 1,
        Some(b'/') => i = delimited(b, i + 1, b'/', true)?,
        Some(b'\\') => {
            let d = *b.get(i + 1)?;
            i = delimited(b, i + 2, d, true)?;
        }
        _ => return Some(i),
    }
    while i < b.len() && matches!(b[i], b'I' | b'M') {
        i += 1;
    }
    Some(i)
}

/// Skips to just after the next unescaped `d`; in a regex, `[...]` may contain `d`.
fn delimited(b: &[u8], mut i: usize, d: u8, regex: bool) -> Option<usize> {
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == d {
            return Some(i + 1);
        }
        if regex && c == b'[' {
            // A bracket expression: `]` right after `[` or `[^` is literal.
            let mut j = i + 1;
            if j < b.len() && b[j] == b'^' {
                j += 1;
            }
            if j < b.len() && b[j] == b']' {
                j += 1;
            }
            while j < b.len() && b[j] != b']' {
                if b[j] == b'[' && j + 1 < b.len() && matches!(b[j + 1], b':' | b'.' | b'=') {
                    let close = b[j + 1];
                    j += 2;
                    while j + 1 < b.len() && !(b[j] == close && b[j + 1] == b']') {
                        j += 1;
                    }
                    j += 2;
                    continue;
                }
                j += 1;
            }
            if j >= b.len() {
                return None;
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(s: &str) -> Option<(bool, bool, bool)> {
        effects(s).map(|e| (e.writes, e.reads, e.runs))
    }

    #[test]
    fn plain_scripts() {
        for s in [
            "1,20p",
            "s/foo/bar/g",
            "/^#/d",
            "$!N;s/\\n/ /",
            "s|a/b|c|2",
            "10q",
            "/x/,/y/{s/a/b/;p}",
            "1~2d",
            "/a/,+3d",
            "y/abc/xyz/",
            "s/[/]/x/",
            "a\\\nhello",
            "1i hello",
            "s/x/y/I",
            ":a;N;$!ba;s/\\n/,/g",
            "=",
            "/foo/I p",
            "s/a\\/b/c/",
            "\\,x,d",
            "s/[[:space:]]*$//",
        ] {
            assert_eq!(fx(s), Some((false, false, false)), "{s}");
        }
    }

    #[test]
    fn effects_found() {
        assert_eq!(fx("w out.txt"), Some((true, false, false)));
        assert_eq!(fx("s/a/b/w out"), Some((true, false, false)));
        assert_eq!(fx("/x/W f"), Some((true, false, false)));
        assert_eq!(fx("1e date"), Some((false, false, true)));
        assert_eq!(fx("s/.*/date/e"), Some((false, false, true)));
        assert_eq!(fx("r other"), Some((false, true, false)));
        assert_eq!(fx("s/a/b/;w x"), Some((true, false, false)));
    }

    #[test]
    fn rejected() {
        for s in ["s/a/b", "k", "/unclosed", "s\\a\\b\\", "1,"] {
            assert_eq!(fx(s), None, "{s}");
        }
    }
}

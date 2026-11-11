//! Pathspecs (gitglossary(7)) and git's wildmatch.

/// Whether text matches a glob. Without pathname, a star also matches slashes
/// (a pathspec); with it, only a double star does (an ignore or attribute pattern).
pub fn wildmatch(pattern: &[u8], text: &[u8], pathname: bool) -> bool {
    let (p, t) = (pattern, text);
    let (mut pi, mut ti) = (0, 0);
    while pi < p.len() {
        let c = p[pi];
        match c {
            b'*' => {
                let before = pi;
                while pi < p.len() && p[pi] == b'*' {
                    pi += 1;
                }
                let mut deep = !pathname;
                if pathname
                    && pi - before >= 2
                    && (before == 0 || p[before - 1] == b'/')
                    && (pi == p.len() || p[pi] == b'/')
                {
                    deep = true;
                    // A double star between slashes also matches no directory at all.
                    if pi < p.len() && wildmatch(&p[pi + 1..], &t[ti..], pathname) {
                        return true;
                    }
                }
                if pi == p.len() {
                    return deep || !t[ti..].contains(&b'/');
                }
                let mut k = ti;
                loop {
                    if wildmatch(&p[pi..], &t[k..], pathname) {
                        return true;
                    }
                    if k == t.len() || (!deep && t[k] == b'/') {
                        return false;
                    }
                    k += 1;
                }
            }
            b'?' => {
                if ti >= t.len() || (pathname && t[ti] == b'/') {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
            b'[' => {
                if ti >= t.len() {
                    return false;
                }
                match class(p, pi, t[ti]) {
                    Some((true, next)) if !(pathname && t[ti] == b'/') => {
                        pi = next;
                        ti += 1;
                    }
                    Some(_) => return false,
                    None => {
                        if t[ti] != b'[' {
                            return false;
                        }
                        pi += 1;
                        ti += 1;
                    }
                }
            }
            b'\\' if pi + 1 < p.len() => {
                if ti >= t.len() || t[ti] != p[pi + 1] {
                    return false;
                }
                pi += 2;
                ti += 1;
            }
            _ => {
                if ti >= t.len() || t[ti] != c {
                    return false;
                }
                pi += 1;
                ti += 1;
            }
        }
    }
    ti == t.len()
}

/// A bracket expression at pattern[at]: whether c is in it, and where it ends.
fn class(pattern: &[u8], at: usize, c: u8) -> Option<(bool, usize)> {
    let mut i = at + 1;
    let negate = matches!(pattern.get(i), Some(b'!' | b'^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        let b = *pattern.get(i)?;
        if b == b']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if b == b'[' && pattern.get(i + 1) == Some(&b':') {
            let end = pattern[i + 2..].windows(2).position(|w| w == b":]")? + i + 2;
            let name = &pattern[i + 2..end];
            matched |= match name {
                b"alpha" => c.is_ascii_alphabetic(),
                b"digit" => c.is_ascii_digit(),
                b"alnum" => c.is_ascii_alphanumeric(),
                b"upper" => c.is_ascii_uppercase(),
                b"lower" => c.is_ascii_lowercase(),
                b"space" => c.is_ascii_whitespace(),
                b"punct" => c.is_ascii_punctuation(),
                b"xdigit" => c.is_ascii_hexdigit(),
                b"blank" => c == b' ' || c == b'\t',
                b"cntrl" => c.is_ascii_control(),
                b"print" => (0x20..0x7f).contains(&c),
                b"graph" => (0x21..0x7f).contains(&c),
                _ => false,
            };
            i = end + 2;
            continue;
        }
        let low = if b == b'\\' {
            i += 1;
            *pattern.get(i)?
        } else {
            b
        };
        if pattern.get(i + 1) == Some(&b'-') && pattern.get(i + 2).is_some_and(|&e| e != b']') {
            let mut high = pattern[i + 2];
            i += 2;
            if high == b'\\' {
                i += 1;
                high = *pattern.get(i)?;
            }
            matched |= low <= c && c <= high;
        } else {
            matched |= low == c;
        }
        i += 1;
    }
}

fn has_glob(s: &[u8]) -> bool {
    s.iter().any(|b| matches!(b, b'*' | b'?' | b'[' | b'\\'))
}

struct Item {
    /// From the repository's root, no trailing slash; empty is everything.
    path: Vec<u8>,
    glob: bool,
    exclude: bool,
}

/// The paths a command is limited to.
#[derive(Default)]
pub struct Pathspec {
    items: Vec<Item>,
}

impl Pathspec {
    /// Arguments as the user wrote them, relative to prefix (the working directory
    /// inside the worktree, with its trailing slash). None: an argument reaches out of
    /// the repository or uses pathspec magic this does not have.
    pub fn new(prefix: &[u8], args: &[String]) -> Result<Pathspec, String> {
        let mut items = Vec::new();
        for arg in args {
            let mut arg = arg.as_bytes();
            let mut top = false;
            let mut exclude = false;
            if let Some(rest) = arg.strip_prefix(b":(") {
                let close = rest
                    .iter()
                    .position(|&b| b == b')')
                    .ok_or_else(|| String::from_utf8_lossy(arg).into_owned())?;
                for word in rest[..close].split(|&b| b == b',') {
                    match word {
                        b"top" => top = true,
                        b"exclude" => exclude = true,
                        b"glob" | b"" => {}
                        _ => return Err(String::from_utf8_lossy(arg).into_owned()),
                    }
                }
                arg = &rest[close + 1..];
            } else if arg.first() == Some(&b':') && arg.len() > 1 {
                let mut i = 1;
                while i < arg.len() {
                    match arg[i] {
                        b'/' => top = true,
                        b'!' | b'^' => exclude = true,
                        b':' => {
                            i += 1;
                            break;
                        }
                        _ => break,
                    }
                    i += 1;
                }
                arg = &arg[i..];
            }
            let mut full = if top || arg.first() == Some(&b'/') {
                Vec::new()
            } else {
                prefix.to_vec()
            };
            if arg.first() == Some(&b'/') {
                return Err(String::from_utf8_lossy(arg).into_owned());
            }
            full.extend_from_slice(arg);
            let mut parts: Vec<&[u8]> = Vec::new();
            for part in full.split(|&b| b == b'/') {
                match part {
                    b"" | b"." => {}
                    b".." => {
                        if parts.pop().is_none() {
                            return Err(String::from_utf8_lossy(arg).into_owned());
                        }
                    }
                    _ => parts.push(part),
                }
            }
            let path = parts.join(&b'/');
            items.push(Item {
                glob: has_glob(&path),
                path,
                exclude,
            });
        }
        Ok(Pathspec { items })
    }

    /// The paths, when every item is a plain one.
    pub fn literal(&self) -> Option<Vec<Vec<u8>>> {
        if self.items.iter().any(|i| i.glob || i.exclude) {
            return None;
        }
        Some(self.items.iter().map(|i| i.path.clone()).collect())
    }

    fn item_matches(item: &Item, path: &[u8]) -> bool {
        if item.path.is_empty() {
            return true;
        }
        if path == item.path
            || (path.len() > item.path.len()
                && path.starts_with(&item.path)
                && path[item.path.len()] == b'/')
        {
            return true;
        }
        if !item.glob {
            return false;
        }
        if wildmatch(&item.path, path, false) {
            return true;
        }
        // A pattern that matches a leading directory matches what is under it.
        path.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'/')
            .any(|(i, _)| wildmatch(&item.path, &path[..i], false))
    }

    /// Whether a file (from the repository's root) is one of the paths.
    pub fn matches(&self, path: &[u8]) -> bool {
        let mut any_include = false;
        let mut included = false;
        for item in &self.items {
            if item.exclude {
                if Self::item_matches(item, path) {
                    return false;
                }
            } else {
                any_include = true;
                included |= Self::item_matches(item, path);
            }
        }
        included || !any_include
    }

    /// Whether a directory may hold matching files.
    pub fn may_match_under(&self, dir: &[u8]) -> bool {
        let includes: Vec<&Item> = self.items.iter().filter(|i| !i.exclude).collect();
        if includes.is_empty() || dir.is_empty() {
            return true;
        }
        includes.iter().any(|item| {
            if item.glob {
                return true;
            }
            let p = &item.path;
            p.is_empty()
                || p.as_slice() == dir
                || (p.len() > dir.len() && p.starts_with(dir) && p[dir.len()] == b'/')
                || (dir.len() > p.len() && dir.starts_with(p) && dir[p.len()] == b'/')
        })
    }
}

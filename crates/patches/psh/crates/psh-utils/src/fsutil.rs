//! Helpers of the file utilities (0015): GNU's quoting of names in messages, path
//! resolution (`realpath`, `readlink -f`), joining and lexical cleanup of paths.

use std::io;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::UtilCtx;

/// A name quoted as GNU quotes it in messages (`common::quote`, D1163).
pub fn q(ctx: &UtilCtx<'_>, name: &[u8]) -> String {
    crate::common::quote(ctx, name)
}

/// `NAME: what ‘FILE’: reason` (`cannot create directory ‘x’: File exists`).
pub async fn fail(ctx: &mut UtilCtx<'_>, what: &str, file: &[u8], e: &io::Error) {
    let msg = format!("{what} {}: {}", q(ctx, file), error_text(e));
    ctx.error(&msg).await;
}

/// The bytes as text for messages.
pub fn s(b: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(b)
}

/// `dir/name`, without doubling a slash.
pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = dir.to_vec();
    if !p.is_empty() && p.last() != Some(&b'/') {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

/// The last component of a path, trailing slashes ignored (`a/b/` is `b`; `/` is `/`).
pub fn base_name(p: &[u8]) -> &[u8] {
    let end = p.len() - p.iter().rev().take_while(|&&b| b == b'/').count();
    if end == 0 {
        return if p.is_empty() { b"" } else { b"/" };
    }
    let start = p[..end]
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(0, |i| i + 1);
    &p[start..end]
}

/// What `dirname` prints for a path.
pub fn dir_name(p: &[u8]) -> Vec<u8> {
    let mut end = p.len();
    while end > 1 && p[end - 1] == b'/' {
        end -= 1;
    }
    let Some(slash) = p[..end].iter().rposition(|&b| b == b'/') else {
        return b".".to_vec();
    };
    let mut d = slash;
    while d > 0 && p[d - 1] == b'/' {
        d -= 1;
    }
    if d == 0 {
        return b"/".to_vec();
    }
    p[..d].to_vec()
}

/// The root of an absolute path and its components: `/` (or `C:/` on Windows).
fn split_root(p: &[u8]) -> (Vec<u8>, &[u8]) {
    if cfg!(windows) && p.len() >= 2 && p[0].is_ascii_alphabetic() && p[1] == b':' {
        let rest = &p[2..];
        return (p[..2].to_vec(), rest);
    }
    (Vec::new(), p)
}

fn components(p: &[u8]) -> impl DoubleEndedIterator<Item = &[u8]> {
    p.split(|&b| b == b'/' || (cfg!(windows) && b == b'\\'))
        .filter(|c| !c.is_empty())
}

fn assemble(root: &[u8], parts: &[Vec<u8>]) -> Vec<u8> {
    let mut out = root.to_vec();
    if parts.is_empty() {
        out.push(b'/');
    }
    for c in parts {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    out
}

/// An absolute path with `.`, `..` and repeated slashes resolved by text.
pub fn lexical(abs: &[u8]) -> Vec<u8> {
    let (root, rest) = split_root(abs);
    let mut parts: Vec<Vec<u8>> = Vec::new();
    for c in components(rest) {
        match c {
            b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c.to_vec()),
        }
    }
    assemble(&root, &parts)
}

/// How much of a path must exist for [`resolve`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    /// Every component (`realpath -e`, `readlink -e`).
    All,
    /// All but the last (`realpath`, `readlink -f`).
    AllButLast,
    /// None (`-m`).
    None,
}

/// The physical absolute path of `p`: every symlink resolved, `..` applied after the
/// links before it, as GNU `realpath`/`readlink -f` do.
pub async fn resolve(ctx: &UtilCtx<'_>, p: &[u8], need: Need) -> io::Result<Vec<u8>> {
    let abs = ctx.path(p);
    let (root, rest) = split_root(&abs);
    let mut queue: std::collections::VecDeque<Vec<u8>> =
        components(rest).map(<[u8]>::to_vec).collect();
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut links = 0;
    // Set once a component was missing (with `Need::None`): the rest is taken as text.
    let mut missing = false;
    while let Some(c) = queue.pop_front() {
        if c == b"." {
            continue;
        }
        if c == b".." {
            parts.pop();
            continue;
        }
        parts.push(c);
        if missing {
            continue;
        }
        let here = assemble(&root, &parts);
        let more = queue.iter().any(|c| c != b"." && c != b"..");
        match ctx.stat(&here, false).await {
            Ok(m) if m.kind == FileKind::Symlink => {
                links += 1;
                if links > 40 {
                    return Err(io::Error::other("Too many levels of symbolic links"));
                }
                let target = ctx.read_link(&here).await?;
                parts.pop();
                let (troot, trest) = split_root(&target);
                if target.first() == Some(&b'/') || !troot.is_empty() {
                    parts.clear();
                }
                for t in components(trest).rev() {
                    queue.push_front(t.to_vec());
                }
            }
            Ok(m) => {
                // A non-directory with components after it (`file/x`, `file/`).
                if m.kind != FileKind::Dir && !queue.is_empty() {
                    if need == Need::None {
                        missing = true;
                        continue;
                    }
                    return Err(io::ErrorKind::NotADirectory.into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => match need {
                Need::All => return Err(e),
                Need::AllButLast if more => return Err(e),
                _ => missing = need == Need::None,
            },
            Err(e) => {
                if need != Need::None {
                    return Err(e);
                }
                missing = true;
            }
        }
    }
    // A trailing slash on a path that must be a directory: `realpath f/` checks it.
    if need != Need::None && p.last() == Some(&b'/') && !missing {
        let here = assemble(&root, &parts);
        if let Ok(m) = ctx.stat(&here, true).await
            && m.kind != FileKind::Dir
        {
            return Err(io::ErrorKind::NotADirectory.into());
        }
    }
    Ok(assemble(&root, &parts))
}

/// `path` relative to the directory `base` (both absolute and clean).
pub fn relative_to(path: &[u8], base: &[u8]) -> Vec<u8> {
    let a: Vec<&[u8]> = components(path).collect();
    let b: Vec<&[u8]> = components(base).collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out: Vec<u8> = Vec::new();
    for _ in common..b.len() {
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend_from_slice(b"..");
    }
    for c in &a[common..] {
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// Whether the clean absolute `path` is `base` or below it.
pub fn is_below(path: &[u8], base: &[u8]) -> bool {
    let a: Vec<&[u8]> = components(path).collect();
    let b: Vec<&[u8]> = components(base).collect();
    a.len() >= b.len() && a[..b.len()] == b[..]
}

/// GNU's `-h` sizes: below the base as is, else one decimal under 10 and whole numbers
/// above, always rounded up (`1.0K`, `1.5K`, `12K`, `1.1M`); `base` 1000 is `--si`.
pub fn human(n: u64, base: u64) -> String {
    if n < base {
        return n.to_string();
    }
    let units: &[u8] = if base == 1000 {
        b"kMGTPEZY"
    } else {
        b"KMGTPEZY"
    };
    let mut unit = 0;
    let mut div = base as u128;
    let n = n as u128;
    while n >= div * base as u128 && unit + 1 < units.len() {
        div *= base as u128;
        unit += 1;
    }
    loop {
        let tenths = (n * 10).div_ceil(div);
        if tenths < 100 {
            return format!("{}.{}{}", tenths / 10, tenths % 10, units[unit] as char);
        }
        let whole = n.div_ceil(div);
        if whole < base as u128 || unit + 1 == units.len() {
            return format!("{whole}{}", units[unit] as char);
        }
        div *= base as u128;
        unit += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(base_name(b"a/b/"), b"b");
        assert_eq!(base_name(b"//"), b"/");
        assert_eq!(base_name(b""), b"");
        assert_eq!(base_name(b"x"), b"x");
        assert_eq!(dir_name(b"/a//b//"), b"/a");
        assert_eq!(dir_name(b"a/"), b".");
        assert_eq!(dir_name(b"//"), b"/");
        assert_eq!(dir_name(b"/x"), b"/");
        assert_eq!(dir_name(b""), b".");
        assert_eq!(lexical(b"/a/./b/../c//"), b"/a/c");
        assert_eq!(lexical(b"/.."), b"/");
        assert_eq!(relative_to(b"/tmp/f", b"/usr/bin"), b"../../tmp/f");
        assert_eq!(relative_to(b"/a", b"/a"), b".");
        assert_eq!(relative_to(b"/a/b/c", b"/a"), b"b/c");
        assert_eq!(human(1023, 1024), "1023");
        assert_eq!(human(1024, 1024), "1.0K");
        assert_eq!(human(1025, 1024), "1.1K");
        assert_eq!(human(10240, 1024), "10K");
        assert_eq!(human(10239, 1024), "10K");
        assert_eq!(human(1048575, 1024), "1.0M");
        assert_eq!(human(1500000, 1024), "1.5M");
        assert_eq!(human(1000, 1000), "1.0k");
        assert!(is_below(b"/a/b", b"/a") && !is_below(b"/ab", b"/a") && is_below(b"/x", b"/"));
    }
}

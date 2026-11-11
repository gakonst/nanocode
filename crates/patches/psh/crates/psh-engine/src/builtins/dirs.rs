//! The directory stack: `dirs`, `pushd`, `popd`.
//!
//! Entry 0 is always the current directory (so `cd` replaces it); `Vp::dirstack` holds
//! the entries below it, top first; `DIRSTACK` shows both (a dynamic variable).

use crate::host::{FileKind, error_text};
use crate::state::{R, Vp};

use super::{normalize, s};

const DIRS_USAGE: &[u8] = b"dirs: usage: dirs [-clpv] [+N] [-N]\n";
const PUSHD_USAGE: &[u8] = b"pushd: usage: pushd [-n] [+N | -N | dir]\n";
const POPD_USAGE: &[u8] = b"popd: usage: popd [-n] [+N | -N]\n";

/// The whole stack, entry 0 (the current directory) first.
fn stack(vp: &Vp) -> Vec<Vec<u8>> {
    let mut v = vec![vp.cwd.clone()];
    v.extend(vp.dirstack.iter().cloned());
    v
}

/// `+N` (from the left, 0 is the current directory) or `-N` (from the right).
fn index_arg(a: &[u8]) -> Option<(bool, usize)> {
    let from_left = match a.first() {
        Some(b'+') => true,
        Some(b'-') => false,
        _ => return None,
    };
    let digits = &a[1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n: usize = std::str::from_utf8(digits).ok()?.parse().ok()?;
    Some((from_left, n))
}

/// The position in a stack of `len` entries.
fn position((from_left, n): (bool, usize), len: usize) -> Option<usize> {
    if n >= len {
        None
    } else if from_left {
        Some(n)
    } else {
        Some(len - 1 - n)
    }
}

/// `~` for `HOME` at the start of an entry, as `dirs` shows it without `-l`.
fn tilde(vp: &Vp, dir: &[u8]) -> Vec<u8> {
    if let Some(home) = vp.var("HOME").filter(|h| !h.is_empty() && *h != b"/")
        && let Some(rest) = dir.strip_prefix(home)
        && (rest.is_empty() || rest[0] == b'/')
    {
        let mut out = b"~".to_vec();
        out.extend_from_slice(rest);
        return out;
    }
    dir.to_vec()
}

#[derive(Clone, Copy, Default)]
struct Format {
    long: bool,
    lines: bool,
    numbered: bool,
}

/// Prints the stack (what `dirs` prints, and `pushd`/`popd` after a change).
async fn print(vp: &mut Vp, name: &str, f: Format, only: Option<usize>) -> R {
    let entries = stack(vp);
    let shown = |vp: &Vp, d: &[u8]| if f.long { d.to_vec() } else { tilde(vp, d) };
    let mut buf = Vec::new();
    if let Some(k) = only {
        buf.extend_from_slice(&shown(vp, &entries[k]));
        buf.push(b'\n');
    } else if f.numbered {
        for (k, d) in entries.iter().enumerate() {
            buf.extend_from_slice(format!("{k:2}  ").as_bytes());
            buf.extend_from_slice(&shown(vp, d));
            buf.push(b'\n');
        }
    } else {
        for (k, d) in entries.iter().enumerate() {
            if k > 0 {
                buf.push(if f.lines { b'\n' } else { b' ' });
            }
            buf.extend_from_slice(&shown(vp, d));
        }
        buf.push(b'\n');
    }
    // A listing that cannot be written does not undo the change (bash).
    super::write_fd(vp, 1, name, &buf).await?;
    Ok(0)
}

/// `dirs [-clpv] [+N] [-N]`
pub(super) async fn dirs(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut f = Format::default();
    let mut clear = false;
    let mut index = None;
    for a in &args[1..] {
        match &a[..] {
            b"-c" => clear = true,
            b"-l" => f.long = true,
            b"-p" => f.lines = true,
            b"-v" => f.numbered = true,
            b"--" => {}
            _ => match index_arg(a) {
                Some(i) => index = Some(i),
                None if a.first() == Some(&b'-') => {
                    vp.error(&format!("dirs: {}: invalid number", s(a))).await;
                    vp.error_raw(DIRS_USAGE).await;
                    return Ok(2);
                }
                None => {
                    vp.error(&format!("dirs: {}: invalid option", s(a))).await;
                    vp.error_raw(DIRS_USAGE).await;
                    return Ok(2);
                }
            },
        }
    }
    if clear {
        vp.dirstack.clear();
        return Ok(0);
    }
    let only = match index {
        None => None,
        Some(i) => {
            if vp.dirstack.is_empty() && i.1 > 0 {
                vp.error("dirs: directory stack empty").await;
                return Ok(1);
            }
            match position(i, vp.dirstack.len() + 1) {
                Some(k) => Some(k),
                None => {
                    vp.error(&format!(
                        "dirs: {}: directory stack index out of range",
                        i.1
                    ))
                    .await;
                    return Ok(1);
                }
            }
        }
    };
    print(vp, "dirs", f, only).await
}

/// Changes to `dir` as `cd` does (logical), with `NAME: dir: …` errors.
async fn chdir(vp: &mut Vp, name: &str, dir: &[u8]) -> Result<bool, crate::state::Unwind> {
    let target = normalize(&vp.abs_path(dir));
    match vp.host().stat(&target, true).await {
        Ok(m) if m.kind == FileKind::Dir => {}
        Ok(_) => {
            vp.error(&format!("{name}: {}: Not a directory", s(dir)))
                .await;
            return Ok(false);
        }
        Err(e) => {
            vp.error(&format!("{name}: {}: {}", s(dir), error_text(&e)))
                .await;
            return Ok(false);
        }
    }
    let old = vp.cwd.clone();
    vp.set_var("OLDPWD", old);
    vp.set_var("PWD", target.clone());
    vp.cwd = target;
    Ok(true)
}

/// `pushd [-n] [+N | -N | dir]`
pub(super) async fn pushd(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut no_cd = false;
    let mut operand: Option<&Vec<u8>> = None;
    let mut i = 1;
    while let Some(a) = args.get(i) {
        i += 1;
        if a == b"-n" {
            no_cd = true;
            continue;
        }
        if a == b"--" {
            if let Some(d) = args.get(i) {
                if operand.is_some() || args.len() > i + 1 {
                    vp.error("pushd: too many arguments").await;
                    return Ok(1);
                }
                operand = Some(d);
            }
            break;
        }
        if a.len() > 1 && a[0] == b'-' && index_arg(a).is_none() {
            vp.error(&format!("pushd: {}: invalid number", s(a))).await;
            vp.error_raw(PUSHD_USAGE).await;
            return Ok(2);
        }
        if operand.is_some() {
            vp.error("pushd: too many arguments").await;
            return Ok(1);
        }
        operand = Some(a);
    }
    match operand {
        // No operand: swap the top two.
        None => {
            if vp.dirstack.is_empty() {
                vp.error("pushd: no other directory").await;
                return Ok(1);
            }
            if no_cd {
                return print(vp, "pushd", Format::default(), None).await;
            }
            let next = vp.dirstack[0].clone();
            let here = vp.cwd.clone();
            if !chdir(vp, "pushd", &next).await? {
                return Ok(1);
            }
            vp.dirstack[0] = here;
        }
        Some(a) if let Some(idx) = index_arg(a) => {
            let entries = stack(vp);
            let Some(k) = position(idx, entries.len()) else {
                vp.error(&format!(
                    "pushd: {}: directory stack index out of range",
                    s(a)
                ))
                .await;
                return Ok(1);
            };
            // `-n` with a rotation changes nothing (bash prints nothing either).
            if no_cd {
                return Ok(0);
            }
            // Rotate so entry k is on top.
            let mut rotated = entries[k..].to_vec();
            rotated.extend_from_slice(&entries[..k]);
            if k != 0 && !chdir(vp, "pushd", &rotated[0].clone()).await? {
                return Ok(1);
            }
            vp.dirstack = rotated[1..].to_vec();
        }
        Some(dir) => {
            if no_cd {
                // Below the top, without changing directory.
                let d = normalize(&vp.abs_path(dir));
                vp.dirstack.insert(0, d);
            } else {
                let here = vp.cwd.clone();
                if !chdir(vp, "pushd", dir).await? {
                    return Ok(1);
                }
                vp.dirstack.insert(0, here);
            }
        }
    }
    print(vp, "pushd", Format::default(), None).await
}

/// `popd [-n] [+N | -N]`
pub(super) async fn popd(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let mut no_cd = false;
    let mut index = None;
    for a in &args[1..] {
        match &a[..] {
            b"-n" => no_cd = true,
            b"--" => {}
            _ => match index_arg(a) {
                Some(i) => index = Some(i),
                None if a.first() == Some(&b'-') => {
                    vp.error(&format!("popd: {}: invalid number", s(a))).await;
                    vp.error_raw(POPD_USAGE).await;
                    return Ok(2);
                }
                None => {
                    vp.error(&format!("popd: {}: invalid argument", s(a))).await;
                    vp.error_raw(POPD_USAGE).await;
                    return Ok(2);
                }
            },
        }
    }
    if vp.dirstack.is_empty() {
        vp.error("popd: directory stack empty").await;
        return Ok(1);
    }
    let len = vp.dirstack.len() + 1;
    let k = match index {
        None => {
            if no_cd {
                1
            } else {
                0
            }
        }
        Some(i) => match position(i, len) {
            Some(k) => k,
            None => {
                let sign = if i.0 { '+' } else { '-' };
                vp.error(&format!(
                    "popd: {sign}{}: directory stack index out of range",
                    i.1
                ))
                .await;
                return Ok(1);
            }
        },
    };
    if k == 0 {
        // The top goes: change to the next entry.
        let next = vp.dirstack[0].clone();
        if !chdir(vp, "popd", &next).await? {
            return Ok(1);
        }
        vp.dirstack.remove(0);
    } else {
        vp.dirstack.remove(k - 1);
    }
    print(vp, "popd", Format::default(), None).await
}

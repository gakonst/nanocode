//! `umask [-p] [-S] [mode]`: the file creation mask, octal or symbolic (`u=rwx,g-w`).
//!
//! The mask lives in the vproc (`Vp::umask`; `None` until a `umask` command: the
//! process's, `Host::umask`). It goes to the programs the vproc starts
//! (`SpawnSpec::umask`) and to the files it creates itself (`OpenOptions::mode`).

use crate::host::OpenOptions;
use crate::state::{R, Vp};

use super::{out, s};

const USAGE: &[u8] = b"umask: usage: umask [-p] [-S] [mode]\n";

/// Gives a creating open the exact permission bits the shell's mask leaves (D1069).
/// Without a `umask` command the host's default (the process's mask) is already right.
pub(crate) fn creation_mode(vp: &Vp, mut opts: OpenOptions) -> OpenOptions {
    if (opts.create || opts.create_new)
        && opts.mode.is_none()
        && let Some(mask) = vp.umask
    {
        opts.mode = Some(0o666 & !mask);
    }
    opts
}

pub(super) async fn umask(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut print, mut symbolic) = (false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b'p' => print = true,
                b'S' => symbolic = true,
                _ => {
                    vp.error(&format!("umask: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(USAGE).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    let current = vp.umask.unwrap_or_else(|| vp.host().umask());
    // Further arguments are ignored, as in bash.
    let Some(mode) = args.get(i) else {
        let shown = if symbolic {
            symbolic_text(current)
        } else {
            format!("{current:04o}")
        };
        let line = match (print, symbolic) {
            (true, true) => format!("umask -S {shown}\n"),
            (true, false) => format!("umask {shown}\n"),
            _ => format!("{shown}\n"),
        };
        return out(vp, "umask", line.as_bytes()).await;
    };
    let new = if mode.first().is_some_and(u8::is_ascii_digit) {
        match parse_octal(mode) {
            Some(m) => m,
            None => {
                vp.error(&format!("umask: {}: octal number out of range", s(mode)))
                    .await;
                return Ok(1);
            }
        }
    } else {
        match parse_symbolic(mode, current) {
            Ok(m) => m,
            Err(msg) => {
                vp.error(&format!("umask: {msg}")).await;
                return Ok(1);
            }
        }
    };
    vp.umask = Some(new);
    if symbolic {
        let line = format!("{}\n", symbolic_text(new));
        return out(vp, "umask", line.as_bytes()).await;
    }
    Ok(0)
}

fn parse_octal(m: &[u8]) -> Option<u32> {
    let mut v = 0u32;
    for &c in m {
        if !(b'0'..=b'7').contains(&c) {
            return None;
        }
        v = v * 8 + u32::from(c - b'0');
        if v > 0o7777 {
            return None;
        }
    }
    Some(v)
}

/// `u=rwx,g=rx,o=rx`: the permissions the mask leaves.
fn symbolic_text(mask: u32) -> String {
    let perms = !mask & 0o777;
    let class = |shift: u32| {
        let b = (perms >> shift) & 7;
        let mut t = String::new();
        for (bit, c) in [(4, 'r'), (2, 'w'), (1, 'x')] {
            if b & bit != 0 {
                t.push(c);
            }
        }
        t
    };
    format!("u={},g={},o={}", class(6), class(3), class(0))
}

/// Applies a symbolic mode (`[ugoa]*[-+=][rwxXst]*|[ugo]`, clauses separated by
/// commas) to the permissions the mask leaves, and returns the new mask.
fn parse_symbolic(m: &[u8], mask: u32) -> Result<u32, String> {
    let mut perms = !mask & 0o777;
    // Copies (`a=u`) and `X` look at the permissions before this mode, not at the
    // running value.
    let orig = perms;
    // bash shows a missing operator as the NUL that ends its string.
    let shown = |c: Option<&u8>| c.map_or("\0".to_string(), |&c| (c as char).to_string());
    let mut i = 0;
    loop {
        let mut who = 0u32;
        while let Some(&c) = m.get(i) {
            who |= match c {
                b'u' => 0o700,
                b'g' => 0o070,
                b'o' => 0o007,
                b'a' => 0o777,
                _ => break,
            };
            i += 1;
        }
        if who == 0 {
            who = 0o777;
        }
        if !matches!(m.get(i), Some(b'+' | b'-' | b'=')) {
            return Err(format!(
                "`{}': invalid symbolic mode operator",
                shown(m.get(i))
            ));
        }
        while let Some(&op) = m.get(i).filter(|c| matches!(c, b'+' | b'-' | b'=')) {
            i += 1;
            let mut bits = 0u32;
            match m.get(i) {
                // Copy another class's permissions.
                Some(&c @ (b'u' | b'g' | b'o')) => {
                    let shift = match c {
                        b'u' => 6,
                        b'g' => 3,
                        _ => 0,
                    };
                    bits = ((orig >> shift) & 7) * 0o111;
                    i += 1;
                }
                _ => {
                    while let Some(&c) = m.get(i) {
                        bits |= match c {
                            b'r' => 0o444,
                            b'w' => 0o222,
                            b'x' => 0o111,
                            b'X' if orig & 0o111 != 0 => 0o111,
                            b'X' | b's' | b't' => 0,
                            _ => break,
                        };
                        i += 1;
                    }
                }
            }
            bits &= who;
            match op {
                b'+' => perms |= bits,
                b'-' => perms &= !bits,
                _ => perms = (perms & !who) | bits,
            }
        }
        match m.get(i) {
            None => break,
            Some(b',') => i += 1,
            Some(&c) => return Err(format!("`{}': invalid symbolic mode character", c as char)),
        }
    }
    Ok(!perms & 0o777)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic() {
        assert_eq!(parse_symbolic(b"g-w,o-w", 0o002), Ok(0o022));
        assert_eq!(parse_symbolic(b"=rx", 0o022), Ok(0o222));
        assert_eq!(parse_symbolic(b"u=rw+x", 0o222), Ok(0o022));
        assert_eq!(parse_symbolic(b"a=X", 0o124), Ok(0o666));
        assert_eq!(parse_symbolic(b"u=g", 0o022), Ok(0o222));
        assert_eq!(parse_symbolic(b"a=,a=u", 0o124), Ok(0o111));
        assert!(parse_symbolic(b"b=rwx", 0).is_err());
        assert!(parse_symbolic(b"u-r,,u-r", 0).is_err());
        assert!(parse_symbolic(b"", 0).is_err());
        assert_eq!(symbolic_text(0o022), "u=rwx,g=rx,o=rx");
        assert_eq!(parse_octal(b"8"), None);
        assert_eq!(parse_octal(b"07777"), Some(0o7777));
    }
}

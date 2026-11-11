//! `ulimit [-SHa] [-RcdefilmnpqrstuvxR [limit]] ...`: resource limits.
//!
//! The process's limits come from the Host; what `ulimit` sets lives in the vproc
//! (`Vp::rlimits`, so a subshell's settings stay there) and is given to the programs it
//! starts. The shell itself is not limited: an engine embedded in another process must
//! not lower that process's limits (D1293).

use crate::host::{Limit, Rlimit};
use crate::state::{R, Vp};

use super::{out, s};

const USAGE: &[u8] = b"ulimit: usage: ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]\n";

/// What an option letter names: a real limit, or the pipe size (fixed, read-only).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Res {
    Limit(Rlimit),
    Pipe,
}

/// Bash's table, in its `-a` order: letter, label, unit shown, factor of a value.
const TABLE: &[(u8, &str, &str, u64, Res)] = &[
    (
        b'R',
        "real-time non-blocking time",
        "microseconds",
        1,
        Res::Limit(Rlimit::RtTime),
    ),
    (
        b'c',
        "core file size",
        "blocks",
        1024,
        Res::Limit(Rlimit::Core),
    ),
    (
        b'd',
        "data seg size",
        "kbytes",
        1024,
        Res::Limit(Rlimit::Data),
    ),
    (b'e', "scheduling priority", "", 1, Res::Limit(Rlimit::Nice)),
    (
        b'f',
        "file size",
        "blocks",
        1024,
        Res::Limit(Rlimit::FileSize),
    ),
    (
        b'i',
        "pending signals",
        "",
        1,
        Res::Limit(Rlimit::SigPending),
    ),
    (
        b'l',
        "max locked memory",
        "kbytes",
        1024,
        Res::Limit(Rlimit::MemLock),
    ),
    (
        b'm',
        "max memory size",
        "kbytes",
        1024,
        Res::Limit(Rlimit::Rss),
    ),
    (b'n', "open files", "", 1, Res::Limit(Rlimit::NoFile)),
    (b'p', "pipe size", "512 bytes", 512, Res::Pipe),
    (
        b'q',
        "POSIX message queues",
        "bytes",
        1,
        Res::Limit(Rlimit::MsgQueue),
    ),
    (
        b'r',
        "real-time priority",
        "",
        1,
        Res::Limit(Rlimit::RtPrio),
    ),
    (
        b's',
        "stack size",
        "kbytes",
        1024,
        Res::Limit(Rlimit::Stack),
    ),
    (b't', "cpu time", "seconds", 1, Res::Limit(Rlimit::Cpu)),
    (b'u', "max user processes", "", 1, Res::Limit(Rlimit::NProc)),
    (
        b'v',
        "virtual memory",
        "kbytes",
        1024,
        Res::Limit(Rlimit::As),
    ),
    (b'x', "file locks", "", 1, Res::Limit(Rlimit::Locks)),
];

/// POSIX's `PIPE_BUF` (4096 bytes on the platforms psh runs on) in 512-byte blocks.
const PIPE_BLOCKS: u64 = 8;

fn entry(letter: u8) -> Option<&'static (u8, &'static str, &'static str, u64, Res)> {
    TABLE.iter().find(|e| e.0 == letter)
}

/// The soft and hard limit in effect: what `ulimit` set here, else the process's.
fn current(vp: &Vp, res: Res) -> Option<(Limit, Limit)> {
    match res {
        Res::Pipe => Some((Some(PIPE_BLOCKS * 512), Some(PIPE_BLOCKS * 512))),
        Res::Limit(r) => vp
            .rlimits
            .iter()
            .find(|e| e.0 == r)
            .map(|e| (e.1, e.2))
            .or_else(|| vp.host().rlimit(r)),
    }
}

fn exists(vp: &Vp, res: Res) -> bool {
    current(vp, res).is_some()
}

fn shown(limit: Limit, factor: u64) -> String {
    match limit {
        None => "unlimited".into(),
        Some(v) => (v / factor).to_string(),
    }
}

fn label(e: &(u8, &str, &str, u64, Res)) -> String {
    let tag = if e.2.is_empty() {
        format!("(-{})", e.0 as char)
    } else {
        format!("({}, -{})", e.2, e.0 as char)
    };
    // The label left, the tag right-aligned: 40 columns, or the label and a space.
    let width = 40usize.max(e.1.len() + 2 + tag.len());
    format!("{}{tag:>w$}", e.1, w = width - e.1.len())
}

/// `a > b` for limits, with unlimited the largest.
fn above(a: Limit, b: Limit) -> bool {
    match (a, b) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(x), Some(y)) => x > y,
    }
}

pub(super) async fn ulimit(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut soft, mut hard, mut all) = (false, false, false);
    // Resources in the order given, each with the value to set, if any.
    let mut todo: Vec<(u8, Option<Vec<u8>>)> = Vec::new();
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    let mut options_done = false;
    while let Some(a) = args.get(i) {
        i += 1;
        if options_done || a.len() < 2 || a[0] != b'-' {
            operands.push(a.clone());
            continue;
        }
        if a == b"--" {
            options_done = true;
            continue;
        }
        if a[1] == b'-' {
            vp.error("ulimit: --: invalid option").await;
            vp.error_raw(USAGE).await;
            return Ok(2);
        }
        let mut j = 1;
        while j < a.len() {
            let c = a[j];
            j += 1;
            match c {
                b'S' => soft = true,
                b'H' => hard = true,
                b'a' => all = true,
                _ => {
                    let known = entry(c).filter(|e| exists(vp, e.4));
                    if known.is_none() {
                        vp.error(&format!("ulimit: -{}: invalid option", c as char))
                            .await;
                        vp.error_raw(USAGE).await;
                        return Ok(2);
                    }
                    // A value attached (`-n99`) or in the next word (not an option).
                    let value = if j < a.len() {
                        let v = a[j..].to_vec();
                        j = a.len();
                        Some(v)
                    } else {
                        match args.get(i) {
                            Some(v) if v.first() != Some(&b'-') => {
                                i += 1;
                                Some(v.clone())
                            }
                            _ => None,
                        }
                    };
                    todo.push((c, value));
                }
            }
        }
    }
    // The first operand is the value of the last resource without one (`-f` if none
    // was named); the rest are ignored.
    // With -a, operands are ignored.
    if let Some(v) = operands
        .into_iter()
        .next()
        .filter(|_| !(all && todo.is_empty()))
    {
        if todo.is_empty() {
            todo.push((b'f', Some(v)));
        } else if let Some(last) = todo.last_mut()
            && last.1.is_none()
        {
            last.1 = Some(v);
        }
    }
    if todo.is_empty() && !all {
        todo.push((b'f', None));
    }
    // `-f` by default, which a host may lack too.
    for (letter, _) in &todo {
        if !entry(*letter).is_some_and(|e| exists(vp, e.4)) {
            vp.error(&format!("ulimit: -{}: invalid option", *letter as char))
                .await;
            vp.error_raw(USAGE).await;
            return Ok(2);
        }
    }
    let mut report: Vec<u8> = Vec::new();
    if all {
        report = TABLE
            .iter()
            .filter(|e| exists(vp, e.4))
            .map(|e| e.0)
            .collect();
    }
    let mut status = 0;
    let mut to_set = Vec::new();
    for (letter, value) in &todo {
        match value {
            Some(v) => to_set.push((*letter, v.clone())),
            None if !report.contains(letter) => report.push(*letter),
            None => {}
        }
    }
    for (letter, v) in to_set {
        let e = entry(letter).expect("checked");
        let (old_soft, old_hard) = current(vp, e.4).expect("checked");
        let new: Limit = match &v[..] {
            b"unlimited" => None,
            b"soft" => old_soft,
            b"hard" => old_hard,
            _ => {
                let n = std::str::from_utf8(&v)
                    .ok()
                    .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|t| t.parse::<u64>().ok());
                let Some(n) = n else {
                    vp.error(&format!("ulimit: {}: invalid number", s(&v)))
                        .await;
                    return Ok(1);
                };
                match n.checked_mul(e.3) {
                    Some(n) => Some(n),
                    None => {
                        vp.error(&format!("ulimit: {}: limit out of range", s(&v)))
                            .await;
                        return Ok(1);
                    }
                }
            }
        };
        let Res::Limit(r) = e.4 else {
            vp.error(&format!(
                "ulimit: {}: cannot modify limit: Invalid argument",
                e.1
            ))
            .await;
            status = 1;
            continue;
        };
        let (set_soft, set_hard) = if soft || hard {
            (soft, hard)
        } else {
            (true, true)
        };
        let new_soft = if set_soft { new } else { old_soft };
        let new_hard = if set_hard { new } else { old_hard };
        // Raising a hard limit needs privileges psh does not assume; a soft limit
        // stays at or below the hard one.
        let err = if above(new_hard, old_hard) {
            Some("Operation not permitted")
        } else if above(new_soft, new_hard) {
            Some("Invalid argument")
        } else {
            None
        };
        if let Some(why) = err {
            vp.error(&format!("ulimit: {}: cannot modify limit: {why}", e.1))
                .await;
            status = 1;
            continue;
        }
        vp.rlimits.retain(|x| x.0 != r);
        vp.rlimits.push((r, new_soft, new_hard));
    }
    if report.is_empty() {
        return Ok(status);
    }
    let labeled = report.len() > 1 || all || todo.len() > 1;
    let mut buf = Vec::new();
    for letter in report {
        let e = entry(letter).expect("checked");
        let (cur_soft, cur_hard) = current(vp, e.4).expect("checked");
        let v = shown(if hard && !soft { cur_hard } else { cur_soft }, e.3);
        if labeled {
            buf.extend_from_slice(format!("{} {v}\n", label(e)).as_bytes());
        } else {
            buf.extend_from_slice(format!("{v}\n").as_bytes());
        }
    }
    let st = out(vp, "ulimit", &buf).await?;
    Ok(if st != 0 { st } else { status })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_like_bash() {
        assert_eq!(
            label(entry(b'c').unwrap()),
            "core file size              (blocks, -c)"
        );
        assert_eq!(
            label(entry(b'n').unwrap()),
            "open files                          (-n)"
        );
        assert_eq!(
            label(entry(b'R').unwrap()),
            "real-time non-blocking time  (microseconds, -R)"
        );
        assert_eq!(
            label(entry(b'p').unwrap()),
            "pipe size                (512 bytes, -p)"
        );
    }
}

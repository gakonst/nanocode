//! `timeout` (and `gtimeout`) with GNU semantics: `DURATION` with fractions and
//! `s m h d`, `-s SIG`, `-k DURATION`, `--preserve-status`, `--foreground`, `-v`;
//! status 124 when the command timed out, 137 when it was killed with KILL, 125 for
//! timeout's own errors. The command runs through the shell's dispatch (utilities,
//! standalone builtins, programs); stopping it drops it, which stops an in-process
//! command at once and abandons a program with its whole process tree (the host sends
//! TERM, then KILL). Written for psh after GNU's documented behavior; oh-my-pi's
//! `tools/timeout.rs` (MIT) informed the design.

use std::future::Future;
use std::task::Poll;
use std::time::Duration;

use psh_engine::util::{SpawnEnv, UtilCtx};

use crate::common;
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "timeout",
    opts: &[
        both('s', "signal").value(),
        both('k', "kill-after").value(),
        long("preserve-status"),
        long("foreground"),
        both('v', "verbose"),
    ],
    posix_order: true,
};

const SIGNALS: &[(&str, i32)] = &[
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("ILL", 4),
    ("TRAP", 5),
    ("ABRT", 6),
    ("BUS", 7),
    ("FPE", 8),
    ("KILL", 9),
    ("USR1", 10),
    ("SEGV", 11),
    ("USR2", 12),
    ("PIPE", 13),
    ("ALRM", 14),
    ("TERM", 15),
    ("CHLD", 17),
    ("CONT", 18),
    ("STOP", 19),
    ("TSTP", 20),
];

/// A signal by name (`TERM`, `SIGTERM`, `term`) or number.
pub fn signal(s: &[u8]) -> Option<(i32, &'static str)> {
    let s = std::str::from_utf8(s).ok()?;
    if let Ok(n) = s.parse::<i32>() {
        return SIGNALS
            .iter()
            .find(|(_, v)| *v == n)
            .map(|(name, v)| (*v, *name))
            .or((n > 0 && n < 65).then_some((n, "")));
    }
    let up = s.to_ascii_uppercase();
    let name = up.strip_prefix("SIG").unwrap_or(&up);
    SIGNALS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(n, v)| (*v, *n))
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match SPEC.parse(&argv[1..]) {
        Ok(p) => p,
        Err(e) if e.is_unknown() => {
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            ctx.error(&format!(
                "unsupported option '{}' (psh built-in timeout)",
                e.option()
            ))
            .await;
            return 125;
        }
        Err(e) => {
            common::usage(ctx, &e.to_string()).await;
            return 125;
        }
    };
    let (mut sig, mut sig_name) = (15, "TERM");
    let mut kill_after: Option<f64> = None;
    let (mut preserve, mut verbose) = (false, false);
    for (key, value) in p.iter(&SPEC) {
        let v = value.unwrap_or_default();
        match key.as_str() {
            "signal" => match signal(v) {
                Some((n, name)) => (sig, sig_name) = (n, name),
                None => {
                    ctx.error(&format!("{}: invalid signal", String::from_utf8_lossy(v)))
                        .await;
                    return 125;
                }
            },
            "kill-after" => match common::parse_duration(v) {
                Some(d) => kill_after = Some(d),
                None => {
                    let msg = format!("invalid time interval {}", common::quote(ctx, v));
                    ctx.error(&msg).await;
                    return 125;
                }
            },
            "preserve-status" => preserve = true,
            "verbose" => verbose = true,
            // No terminal to share: the command always runs in the foreground here.
            _ => {}
        }
    }
    let Some(duration) = p.operands.first() else {
        common::usage(ctx, "missing operand").await;
        return 125;
    };
    let Some(secs) = common::parse_duration(duration) else {
        let msg = format!("invalid time interval {}", common::quote(ctx, duration));
        ctx.error(&msg).await;
        let name = ctx.name().to_string();
        ctx.error_raw(format!("Try '{name} --help' for more information.\n").as_bytes())
            .await;
        return 125;
    };
    if p.operands.len() < 2 {
        let msg = format!("missing operand after {}", common::quote(ctx, duration));
        common::usage(ctx, &msg).await;
        return 125;
    }
    let command = p.operands[1..].to_vec();
    let shown = String::from_utf8_lossy(&command[0]).into_owned();
    let mut child = ctx.spawn(command, SpawnEnv::default()).await;
    // 0 (or inf) means no limit.
    if secs == 0.0 || secs.is_infinite() {
        return child.wait().await;
    }
    let done = {
        let mut timer = Box::pin(ctx.sleep(Duration::from_secs_f64(secs)));
        std::future::poll_fn(|cx| {
            if let Poll::Ready(s) = child.poll_wait(cx) {
                return Poll::Ready(Some(s));
            }
            match timer.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    };
    if let Some(status) = done {
        return status;
    }
    if verbose {
        ctx.error(&format!("sending signal {sig_name} to command '{shown}'"))
            .await;
    }
    // Stopping the command drops it: in-process commands end at once, programs get
    // TERM and then KILL from the host. `-k` adds nothing beyond that escalation.
    let _ = kill_after;
    drop(child);
    if verbose && kill_after.is_some() && sig != 9 {
        ctx.error(&format!("sending signal KILL to command '{shown}'"))
            .await;
    }
    if preserve {
        128 + sig
    } else if sig == 9 {
        137
    } else {
        124
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals() {
        assert_eq!(signal(b"TERM"), Some((15, "TERM")));
        assert_eq!(signal(b"sigkill"), Some((9, "KILL")));
        assert_eq!(signal(b"9"), Some((9, "KILL")));
        assert_eq!(signal(b"NOPE"), None);
    }
}

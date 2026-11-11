//! `chmod [-R] [-v|-c] [-f] MODE[,MODE]... FILE...` and `--reference=RFILE`: octal and
//! symbolic modes (`-x`, `go-w`, `a=rX`), GNU's messages. `-R` follows the symlinks
//! named on the command line and skips those it meets inside. Written for psh after the
//! POSIX description and GNU's manual.

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::{LocalBoxFuture, UtilCtx};

use crate::common;
use crate::fsutil::{join, q, s};
use crate::modes::{self, Mode};
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "chmod",
    opts: &[
        both('R', "recursive"),
        both('v', "verbose"),
        both('c', "changes"),
        both('f', "silent"),
        long("quiet"),
        long("reference").value(),
        long("preserve-root"),
        long("no-preserve-root"),
    ],
    posix_order: false,
};

/// Whether an argument is a mode written like an option (`-x`, `-rw`, `-w,u+x`).
fn mode_like(a: &[u8]) -> bool {
    a.len() > 1
        && a[0] == b'-'
        && a[1..].iter().all(|c| b"rwxXstugoa,+-=01234567".contains(c))
        && modes::parse(a).is_some()
}

struct Opts {
    recursive: bool,
    verbose: bool,
    changes: bool,
    quiet: bool,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    // Modes like `-x` are operands, not options: pull the first out before parsing.
    let mut args: Vec<Vec<u8>> = argv.to_vec();
    let mut mode_arg = None;
    if let Some(i) = args
        .iter()
        .skip(1)
        .take_while(|a| a.as_slice() != b"--")
        .position(|a| mode_like(a))
    {
        mode_arg = Some(args.remove(i + 1));
    }
    let p = match common::parse(ctx, &SPEC, &args).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let o = Opts {
        recursive: p.has(&SPEC, "recursive"),
        verbose: p.has(&SPEC, "verbose"),
        changes: p.has(&SPEC, "changes"),
        quiet: p.has(&SPEC, "silent") || p.has(&SPEC, "quiet"),
    };
    let mut operands = p.operands.clone();
    let umask = ctx.umask();
    let mode = if let Some(r) = p.last(&SPEC, "reference") {
        match ctx.stat(r, true).await {
            Ok(m) => Mode::Octal(m.mode & 0o7777, 5),
            Err(e) => {
                let msg = format!(
                    "failed to get attributes of {}: {}",
                    q(ctx, r),
                    error_text(&e)
                );
                ctx.error(&msg).await;
                return 1;
            }
        }
    } else {
        let m = match mode_arg {
            Some(m) => m,
            None if !operands.is_empty() => operands.remove(0),
            None => return common::usage(ctx, "missing operand").await,
        };
        match modes::parse(&m) {
            Some(parsed) => {
                if operands.is_empty() {
                    let msg = format!("missing operand after {}", q(ctx, &m));
                    return common::usage(ctx, &msg).await;
                }
                parsed
            }
            None => {
                let msg = format!("invalid mode: {}", q(ctx, &m));
                return common::usage(ctx, &msg).await;
            }
        }
    };
    if operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let mut ok = true;
    for f in operands {
        ok &= change(ctx, f, &mode, umask, &o, true).await;
    }
    if ok { 0 } else { 1 }
}

fn shown(mode: u32) -> String {
    let s = modes::mode_string(FileKind::File, mode);
    format!("{:04o} ({})", mode & 0o7777, &s[1..])
}

fn change<'a, 'v>(
    ctx: &'a mut UtilCtx<'v>,
    f: Vec<u8>,
    mode: &'a Mode,
    umask: u32,
    o: &'a Opts,
    top: bool,
) -> LocalBoxFuture<'a, bool> {
    Box::pin(async move {
        ctx.check().await;
        // Symlinks met while recursing are not followed (and have no mode of their own).
        if !top
            && ctx
                .stat(&f, false)
                .await
                .is_ok_and(|m| m.kind == FileKind::Symlink)
        {
            return true;
        }
        let m = match ctx.stat(&f, true).await {
            Ok(m) => m,
            Err(e) => {
                if !o.quiet {
                    let msg = format!("cannot access '{}': {}", s(&f), error_text(&e));
                    ctx.error(&msg).await;
                }
                return false;
            }
        };
        let is_dir = m.kind == FileKind::Dir;
        let new = mode.apply(m.mode & 0o7777, is_dir, umask);
        let mut ok = true;
        if new != m.mode & 0o7777 {
            if let Err(e) = ctx.set_mode(&f, new).await {
                if !o.quiet {
                    let msg = format!("changing permissions of '{}': {}", s(&f), error_text(&e));
                    ctx.error(&msg).await;
                }
                ok = false;
            } else if o.verbose || o.changes {
                let line = format!(
                    "mode of '{}' changed from {} to {}\n",
                    s(&f),
                    shown(m.mode),
                    shown(new)
                );
                if ctx.write(line.as_bytes()).await.is_err() {
                    return false;
                }
            }
        } else if o.verbose {
            let line = format!("mode of '{}' retained as {}\n", s(&f), shown(new));
            if ctx.write(line.as_bytes()).await.is_err() {
                return false;
            }
        }
        if o.recursive && is_dir {
            let entries = match ctx.read_dir(&f).await {
                Ok(mut e) => {
                    e.sort_by(|a, b| a.name.cmp(&b.name));
                    e
                }
                Err(e) => {
                    if !o.quiet {
                        let msg = format!("cannot read directory '{}': {}", s(&f), error_text(&e));
                        ctx.error(&msg).await;
                    }
                    return false;
                }
            };
            for e in entries {
                ok &= change(ctx, join(&f, &e.name), mode, umask, o, false).await;
            }
        }
        ok
    })
}

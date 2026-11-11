//! `rm [-f] [-r|-R] [-d] [-v] FILE...` with GNU's messages and safety rules: `.` and `..`
//! are refused, `/` needs `--no-preserve-root`, symlinks (and Windows junctions) are
//! removed, never followed. No prompting: `-i`/`-I` fall through to the real `rm`.
//! Written for psh after the manuals.

use std::io;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::{LocalBoxFuture, UtilCtx};

use crate::common;
use crate::fsutil::{base_name, join, lexical, s};
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "rm",
    opts: &[
        both('f', "force"),
        both('r', "recursive"),
        flag('R'),
        both('d', "dir"),
        both('v', "verbose"),
        long("preserve-root").optional(),
        long("no-preserve-root"),
        long("one-file-system"),
    ],
    posix_order: false,
};

struct Opts {
    force: bool,
    recursive: bool,
    dir: bool,
    verbose: bool,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let o = Opts {
        force: p.has(&SPEC, "force"),
        recursive: p.has(&SPEC, "recursive") || p.has(&SPEC, "R"),
        dir: p.has(&SPEC, "dir"),
        verbose: p.has(&SPEC, "verbose"),
    };
    let mut preserve_root = true;
    for (k, _) in p.iter(&SPEC) {
        match k.as_str() {
            "preserve-root" => preserve_root = true,
            "no-preserve-root" => preserve_root = false,
            _ => {}
        }
    }
    if p.operands.is_empty() {
        if o.force {
            return 0;
        }
        return common::usage(ctx, "missing operand").await;
    }
    let mut ok = true;
    for f in &p.operands {
        let last = base_name(f);
        if last == b"." || last == b".." {
            let msg = format!(
                "refusing to remove '.' or '..' directory: skipping '{}'",
                s(f)
            );
            ctx.error(&msg).await;
            ok = false;
            continue;
        }
        if o.recursive && preserve_root && lexical(&ctx.path(f)) == b"/" {
            ctx.error("it is dangerous to operate recursively on '/'")
                .await;
            ctx.error("use --no-preserve-root to override this failsafe")
                .await;
            ok = false;
            continue;
        }
        ok &= remove(ctx, f.clone(), &o, true).await;
    }
    if ok { 0 } else { 1 }
}

async fn report(ctx: &mut UtilCtx<'_>, f: &[u8], e: &io::Error) {
    let msg = format!("cannot remove '{}': {}", s(f), error_text(e));
    ctx.error(&msg).await;
}

async fn removed(ctx: &mut UtilCtx<'_>, o: &Opts, what: &str, f: &[u8]) -> bool {
    if o.verbose {
        let line = format!("removed {what}'{}'\n", s(f));
        if ctx.write(line.as_bytes()).await.is_err() {
            return false;
        }
    }
    true
}

/// `rmdir`, except through a symlink: `link/` names the directory the link points to,
/// whose contents `-r` removes, but the link is not a directory (as GNU: some systems'
/// `rmdir` would remove the target).
async fn remove_dir(ctx: &mut UtilCtx<'_>, f: &[u8]) -> io::Result<()> {
    let end = f.len() - f.iter().rev().take_while(|&&b| b == b'/').count();
    if end > 0
        && end < f.len()
        && ctx
            .stat(&f[..end], false)
            .await
            .is_ok_and(|m| m.kind == FileKind::Symlink)
    {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    ctx.remove_dir(f).await
}

/// Removes `f` (a tree with `-r`): whether everything went.
fn remove<'a, 'v>(
    ctx: &'a mut UtilCtx<'v>,
    f: Vec<u8>,
    o: &'a Opts,
    top: bool,
) -> LocalBoxFuture<'a, bool> {
    Box::pin(async move {
        ctx.check().await;
        // `name/` names what a symlink points to (POSIX path resolution), on every host.
        let m = match ctx.stat(&f, f.last() == Some(&b'/')).await {
            Ok(m) => m,
            Err(e) => {
                if o.force && e.kind() == io::ErrorKind::NotFound && top {
                    return true;
                }
                report(ctx, &f, &e).await;
                return false;
            }
        };
        if m.kind != FileKind::Dir {
            return match ctx.remove_file(&f).await {
                Ok(()) => removed(ctx, o, "", &f).await,
                Err(e) if o.force && e.kind() == io::ErrorKind::NotFound => true,
                Err(e) => {
                    report(ctx, &f, &e).await;
                    false
                }
            };
        }
        if !o.recursive {
            if o.dir {
                return match remove_dir(ctx, &f).await {
                    Ok(()) => removed(ctx, o, "directory ", &f).await,
                    Err(e) => {
                        report(ctx, &f, &e).await;
                        false
                    }
                };
            }
            let e = io::Error::from(io::ErrorKind::IsADirectory);
            report(ctx, &f, &e).await;
            return false;
        }
        let entries = match ctx.read_dir(&f).await {
            Ok(mut e) => {
                e.sort_by(|a, b| a.name.cmp(&b.name));
                e
            }
            Err(e) => {
                report(ctx, &f, &e).await;
                return false;
            }
        };
        let mut ok = true;
        for e in entries {
            ok &= remove(ctx, join(&f, &e.name), o, false).await;
        }
        if !ok {
            // A child stayed: the directory cannot go, and that was reported already.
            return false;
        }
        match remove_dir(ctx, &f).await {
            Ok(()) => removed(ctx, o, "directory ", &f).await,
            Err(e) => {
                report(ctx, &f, &e).await;
                false
            }
        }
    })
}

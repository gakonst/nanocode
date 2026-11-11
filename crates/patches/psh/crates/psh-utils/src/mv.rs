//! `mv [-f|-n] [-u] [-v] [-b] [-t DIR] [-T] SRC... DEST` with GNU's messages. Across
//! file systems it copies (everything kept) and removes the source. `-i` falls through
//! to the real `mv`. Written for psh after the manuals.

use std::io;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::{LocalBoxFuture, UtilCtx};

use crate::common;
use crate::cp::{self, CopyOpts};
use crate::fsutil::{Need, is_below, join, resolve, s};
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "mv",
    opts: &[
        both('f', "force"),
        both('n', "no-clobber"),
        both('u', "update").optional(),
        both('v', "verbose"),
        both('t', "target-directory").value(),
        both('T', "no-target-directory"),
        both('b', "backup").optional(),
        both('S', "suffix").value(),
        long("strip-trailing-slashes"),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (mut no_clobber, mut update, mut backup) = (false, false, None::<Vec<u8>>);
    let mut suffix = b"~".to_vec();
    let mut want_backup = false;
    for (k, v) in p.iter(&SPEC) {
        match k.as_str() {
            "force" => no_clobber = false,
            "no-clobber" => no_clobber = true,
            "update" => match v {
                None | Some(b"older") => update = true,
                Some(b"all") => update = false,
                Some(b"none") => no_clobber = true,
                Some(w) => {
                    let msg = format!("invalid argument '{}' for '--update'", s(w));
                    return common::usage(ctx, &msg).await;
                }
            },
            "backup" => {
                if let Some(v) = v
                    && !matches!(v, b"simple" | b"never" | b"none" | b"off")
                {
                    if let Some(st) = ctx.run_program(argv.to_vec()).await {
                        return st;
                    }
                    let msg = format!("unsupported backup type '{}' (psh built-in mv)", s(v));
                    ctx.error(&msg).await;
                    return 2;
                }
                want_backup = !matches!(v, Some(b"none" | b"off"));
            }
            "suffix" => {
                suffix = v.unwrap_or_default().to_vec();
                want_backup = true;
            }
            _ => {}
        }
    }
    if want_backup {
        backup = Some(suffix);
    }
    let verbose = p.has(&SPEC, "verbose");
    let strip = p.has(&SPEC, "strip-trailing-slashes");
    let mut p = p;
    if strip {
        for o in &mut p.operands {
            while o.len() > 1 && o.last() == Some(&b'/') {
                o.pop();
            }
        }
    }
    let plan = match cp::targets(ctx, &p, "mv").await {
        Ok(plan) => plan,
        Err(st) => return st,
    };
    let mut ok = true;
    for (src, dst) in plan.pairs(false) {
        ctx.check().await;
        ok &= move_one(
            ctx,
            &src,
            &dst,
            no_clobber,
            update,
            backup.as_deref(),
            verbose,
        )
        .await;
    }
    if ok { 0 } else { 1 }
}

async fn fail(ctx: &mut UtilCtx<'_>, what: &str, f: &[u8], e: &io::Error) {
    let msg = format!("{what} '{}': {}", s(f), error_text(e));
    ctx.error(&msg).await;
}

async fn move_one(
    ctx: &mut UtilCtx<'_>,
    src: &[u8],
    dst: &[u8],
    no_clobber: bool,
    update: bool,
    backup: Option<&[u8]>,
    verbose: bool,
) -> bool {
    let sm = match ctx.stat(src, false).await {
        Ok(m) => m,
        Err(e) => {
            fail(ctx, "cannot stat", src, &e).await;
            return false;
        }
    };
    if let Ok(dm) = ctx.stat(dst, false).await {
        if dm.dev == sm.dev && dm.ino == sm.ino && sm.ino != 0 {
            let msg = format!("'{}' and '{}' are the same file", s(src), s(dst));
            ctx.error(&msg).await;
            return false;
        }
        if no_clobber || (update && dm.mtime_ns >= sm.mtime_ns) {
            return true;
        }
        let d_dir = dm.kind == FileKind::Dir;
        let s_dir = sm.kind == FileKind::Dir;
        if d_dir && !s_dir {
            let msg = format!(
                "cannot overwrite directory '{}' with non-directory '{}'",
                s(dst),
                s(src)
            );
            ctx.error(&msg).await;
            return false;
        }
        if !d_dir && s_dir {
            let msg = format!(
                "cannot overwrite non-directory '{}' with directory '{}'",
                s(dst),
                s(src)
            );
            ctx.error(&msg).await;
            return false;
        }
        if let Some(suffix) = backup {
            let mut b = dst.to_vec();
            b.extend_from_slice(suffix);
            if let Err(e) = ctx.rename(dst, &b).await {
                fail(ctx, "cannot backup", dst, &e).await;
                return false;
            }
        }
    }
    if sm.kind == FileKind::Dir
        && let (Ok(a), Ok(b)) = (
            resolve(ctx, src, Need::All).await,
            resolve(ctx, dst, Need::None).await,
        )
        && a != b
        && is_below(&b, &a)
    {
        let msg = format!(
            "cannot move '{}' to a subdirectory of itself, '{}'",
            s(src),
            s(dst)
        );
        ctx.error(&msg).await;
        return false;
    }
    match ctx.rename(src, dst).await {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            // Another file system: copy everything as it is, then remove the source.
            let o = CopyOpts::for_move();
            if !cp::copy(ctx, src.to_vec(), dst.to_vec(), &o, true).await {
                return false;
            }
            if !remove_tree(ctx, src.to_vec()).await {
                return false;
            }
        }
        Err(e) => {
            let msg = format!(
                "cannot move '{}' to '{}': {}",
                s(src),
                s(dst),
                error_text(&e)
            );
            ctx.error(&msg).await;
            return false;
        }
    }
    if verbose {
        let line = format!("renamed '{}' -> '{}'\n", s(src), s(dst));
        if ctx.write(line.as_bytes()).await.is_err() {
            return false;
        }
    }
    true
}

/// Removes what was copied away (symlinks themselves, never followed).
fn remove_tree<'a, 'v>(ctx: &'a mut UtilCtx<'v>, p: Vec<u8>) -> LocalBoxFuture<'a, bool> {
    Box::pin(async move {
        let is_dir = ctx
            .stat(&p, false)
            .await
            .is_ok_and(|m| m.kind == FileKind::Dir);
        let r = if is_dir {
            if let Ok(entries) = ctx.read_dir(&p).await {
                for e in entries {
                    if !remove_tree(ctx, join(&p, &e.name)).await {
                        return false;
                    }
                }
            }
            ctx.remove_dir(&p).await
        } else {
            ctx.remove_file(&p).await
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                fail(ctx, "cannot remove", &p, &e).await;
                false
            }
        }
    })
}

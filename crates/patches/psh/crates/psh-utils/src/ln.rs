//! `ln [-s] [-f] [-n|-h] [-r] [-v] [-L|-P] [-b] [-t DIR] [-T] TARGET... [LINK]` with
//! GNU's messages. `-n` (BSD `-h`) treats a symlink to a directory as a file, so `ln
//! -sfn new link` replaces the link. On Windows the host makes a junction for a
//! directory when it cannot make a symlink, and fails clearly otherwise; never a copy.
//! `-i` falls through to the real `ln`. Written for psh after the manuals.

use std::io;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::cp::Plan;
use crate::fsutil::{Need, base_name, dir_name, join, relative_to, resolve, s};
use crate::opts::{Spec, both, flag};

pub const SPEC: Spec = Spec {
    name: "ln",
    opts: &[
        both('s', "symbolic"),
        both('f', "force"),
        both('n', "no-dereference"),
        flag('h'),
        both('r', "relative"),
        both('v', "verbose"),
        both('L', "logical"),
        both('P', "physical"),
        both('t', "target-directory").value(),
        both('T', "no-target-directory"),
        both('b', "backup").optional(),
        both('S', "suffix").value(),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let symbolic = p.has(&SPEC, "symbolic");
    let force = p.has(&SPEC, "force");
    let no_deref = p.has(&SPEC, "no-dereference") || p.has(&SPEC, "h");
    let relative = p.has(&SPEC, "relative");
    let verbose = p.has(&SPEC, "verbose");
    let mut logical = false;
    for (k, _) in p.iter(&SPEC) {
        match k.as_str() {
            "logical" => logical = true,
            "physical" => logical = false,
            _ => {}
        }
    }
    let mut want_backup = false;
    let mut suffix = b"~".to_vec();
    for (k, v) in p.iter(&SPEC) {
        match k.as_str() {
            "backup" => {
                if let Some(v) = v
                    && !matches!(v, b"simple" | b"never" | b"none" | b"off")
                {
                    if let Some(st) = ctx.run_program(argv.to_vec()).await {
                        return st;
                    }
                    let msg = format!("unsupported backup type '{}' (psh built-in ln)", s(v));
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
    let backup = want_backup.then_some(suffix);
    let target_dir = p.last(&SPEC, "target-directory").map(<[u8]>::to_vec);
    let no_target = p.has(&SPEC, "no-target-directory");
    let ops = &p.operands;
    // `ln TARGET`: a link of the same name here.
    let plan = if ops.len() == 1 && target_dir.is_none() && !no_target {
        Plan {
            sources: ops.clone(),
            target: b".".to_vec(),
            into_dir: true,
        }
    } else if target_dir.is_none() && !no_target && ops.len() == 2 {
        // A symlink to a directory as the last operand is a file with -n.
        let last = &ops[1];
        let is_dir = if no_deref {
            ctx.stat(last, false)
                .await
                .is_ok_and(|m| m.kind == FileKind::Dir)
        } else {
            ctx.stat(last, true)
                .await
                .is_ok_and(|m| m.kind == FileKind::Dir)
        };
        Plan {
            sources: vec![ops[0].clone()],
            target: last.clone(),
            into_dir: is_dir,
        }
    } else {
        match crate::cp::targets(ctx, &p, "ln").await {
            Ok(plan) => plan,
            Err(st) => return st,
        }
    };
    let mut ok = true;
    for src in &plan.sources {
        let dst = if plan.into_dir {
            join(&plan.target, base_name(src))
        } else {
            plan.target.clone()
        };
        ok &= link_one(
            ctx,
            src,
            &dst,
            symbolic,
            force,
            relative,
            logical,
            backup.as_deref(),
            verbose,
        )
        .await;
    }
    if ok { 0 } else { 1 }
}

#[allow(clippy::too_many_arguments)]
async fn link_one(
    ctx: &mut UtilCtx<'_>,
    src: &[u8],
    dst: &[u8],
    symbolic: bool,
    force: bool,
    relative: bool,
    logical: bool,
    backup: Option<&[u8]>,
    verbose: bool,
) -> bool {
    let kind = if symbolic {
        "symbolic link"
    } else {
        "hard link"
    };
    if !symbolic {
        match ctx.stat(src, logical).await {
            Err(e) => {
                let msg = format!("failed to access '{}': {}", s(src), error_text(&e));
                ctx.error(&msg).await;
                return false;
            }
            Ok(m) if m.kind == FileKind::Dir => {
                let msg = format!("{}: hard link not allowed for directory", s(src));
                ctx.error(&msg).await;
                return false;
            }
            Ok(_) => {}
        }
    }
    if let Ok(dm) = ctx.stat(dst, false).await {
        if !symbolic
            && let Ok(sm) = ctx.stat(src, logical).await
            && sm.dev == dm.dev
            && sm.ino == dm.ino
            && sm.ino != 0
        {
            let msg = format!("'{}' and '{}' are the same file", s(src), s(dst));
            ctx.error(&msg).await;
            return false;
        }
        if dm.kind == FileKind::Dir {
            let msg = format!("{}: cannot overwrite directory", s(dst));
            ctx.error(&msg).await;
            return false;
        }
        if let Some(suffix) = backup {
            let mut b = dst.to_vec();
            b.extend_from_slice(suffix);
            if let Err(e) = ctx.rename(dst, &b).await {
                let msg = format!("cannot backup '{}': {}", s(dst), error_text(&e));
                ctx.error(&msg).await;
                return false;
            }
        } else if force {
            if let Err(e) = ctx.remove_file(dst).await {
                let msg = format!("cannot remove '{}': {}", s(dst), error_text(&e));
                ctx.error(&msg).await;
                return false;
            }
        } else {
            let e = io::Error::from(io::ErrorKind::AlreadyExists);
            let msg = format!("failed to create {kind} '{}': {}", s(dst), error_text(&e));
            ctx.error(&msg).await;
            return false;
        }
    }
    let target = if symbolic && relative {
        // The target relative to the link's directory, both resolved.
        let (Ok(t), Ok(d)) = (
            resolve(ctx, src, Need::None).await,
            resolve(ctx, &dir_name(dst), Need::None).await,
        ) else {
            return false;
        };
        relative_to(&t, &d)
    } else {
        src.to_vec()
    };
    let r = if symbolic {
        ctx.symlink(&target, dst).await
    } else if logical {
        match resolve(ctx, src, Need::All).await {
            Ok(real) => ctx.hard_link(&real, dst).await,
            Err(e) => Err(e),
        }
    } else {
        ctx.hard_link(src, dst).await
    };
    if let Err(e) = r {
        let msg = if symbolic {
            format!("failed to create {kind} '{}': {}", s(dst), error_text(&e))
        } else {
            format!(
                "failed to create {kind} '{}' => '{}': {}",
                s(dst),
                s(src),
                error_text(&e)
            )
        };
        ctx.error(&msg).await;
        return false;
    }
    if verbose {
        let arrow = if symbolic { "->" } else { "=>" };
        let line = format!("'{}' {arrow} '{}'\n", s(dst), s(&target));
        if ctx.write(line.as_bytes()).await.is_err() {
            return false;
        }
    }
    true
}

//! `mkdir [-p] [-m MODE] [-v] DIR...` with GNU's messages. Written for psh after the
//! manuals.

use std::io;

use psh_engine::host::FileKind;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{fail, q, s};
use crate::modes;
use crate::opts::{Spec, both};

pub const SPEC: Spec = Spec {
    name: "mkdir",
    opts: &[
        both('m', "mode").value(),
        both('p', "parents"),
        both('v', "verbose"),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let umask = ctx.umask();
    let mode = match p.last(&SPEC, "mode") {
        Some(m) => match modes::parse(m) {
            // `-m` starts from a=rwx, as chmod would change it.
            Some(parsed) => parsed.apply(0o777, true, umask),
            None => {
                let msg = format!("invalid mode {}", q(ctx, m));
                ctx.error(&msg).await;
                return 1;
            }
        },
        None => 0o777 & !umask,
    };
    if p.operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let parents = p.has(&SPEC, "parents");
    let verbose = p.has(&SPEC, "verbose");
    let mut status = 0;
    for dir in &p.operands {
        let r = if parents {
            make_parents(ctx, dir, mode, umask, verbose).await
        } else {
            make(ctx, dir, mode, verbose).await
        };
        if let Err((at, e)) = r {
            fail(ctx, "cannot create directory", &at, &e).await;
            status = 1;
        }
    }
    status
}

async fn make(
    ctx: &mut UtilCtx<'_>,
    dir: &[u8],
    mode: u32,
    verbose: bool,
) -> Result<(), (Vec<u8>, io::Error)> {
    ctx.mkdir(dir, mode).await.map_err(|e| (dir.to_vec(), e))?;
    if verbose {
        let line = format!("{}: created directory '{}'\n", ctx.name(), s(dir));
        if let Err(e) = ctx.write(line.as_bytes()).await {
            return Err((dir.to_vec(), e));
        }
    }
    Ok(())
}

fn is_drive(c: &[u8]) -> bool {
    cfg!(windows) && c.len() == 2 && c[0].is_ascii_alphabetic() && c[1] == b':'
}

/// `-p`: every missing directory on the way, the parents with `u+wx` so the next level
/// can be made, the last with `mode`; an existing directory is fine.
async fn make_parents(
    ctx: &mut UtilCtx<'_>,
    dir: &[u8],
    mode: u32,
    umask: u32,
    verbose: bool,
) -> Result<(), (Vec<u8>, io::Error)> {
    let parent_mode = (0o777 & !umask) | 0o300;
    let comps: Vec<&[u8]> = dir
        .split(|&b| b == b'/' || (cfg!(windows) && b == b'\\'))
        .collect();
    let last = comps.iter().rposition(|c| !c.is_empty()).unwrap_or(0);
    let mut cur: Vec<u8> = Vec::new();
    for (i, c) in comps.iter().enumerate() {
        if i > 0 {
            cur.push(b'/');
        }
        cur.extend_from_slice(c);
        // A drive (`C:` of `C:/x`) is a root, like the empty first component of `/x`.
        if c.is_empty() || *c == b"." || (i == 0 && is_drive(c)) {
            continue;
        }
        let m = if i == last { mode } else { parent_mode };
        match ctx.stat(&cur, true).await {
            Ok(st) if st.kind == FileKind::Dir => continue,
            Ok(_) => {
                let kind = if i == last {
                    io::ErrorKind::AlreadyExists
                } else {
                    io::ErrorKind::NotADirectory
                };
                return Err((cur, kind.into()));
            }
            Err(_) => {}
        }
        match make(ctx, &cur, m, verbose).await {
            Ok(()) => {}
            // Made by someone else meanwhile.
            Err((_, e))
                if e.kind() == io::ErrorKind::AlreadyExists
                    && ctx
                        .stat(&cur, true)
                        .await
                        .is_ok_and(|m| m.kind == FileKind::Dir) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

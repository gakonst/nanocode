//! `rmdir [-p] [-v] [--ignore-fail-on-non-empty] DIR...` with GNU's messages. Written
//! for psh after the manuals.

use std::io;

use psh_engine::host::error_text;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::s;
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "rmdir",
    opts: &[
        both('p', "parents"),
        both('v', "verbose"),
        long("ignore-fail-on-non-empty"),
    ],
    posix_order: false,
};

fn non_empty(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::DirectoryNotEmpty || (e.kind() == io::ErrorKind::AlreadyExists)
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    if p.operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let parents = p.has(&SPEC, "parents");
    let verbose = p.has(&SPEC, "verbose");
    let ignore = p.has(&SPEC, "ignore-fail-on-non-empty");
    let mut status = 0;
    for dir in &p.operands {
        let mut cur = dir.clone();
        let mut first = true;
        loop {
            if verbose {
                let line = format!("{}: removing directory, '{}'\n", ctx.name(), s(&cur));
                if let Err(e) = ctx.write(line.as_bytes()).await {
                    return common::write_failed(ctx, &e).await;
                }
            }
            if let Err(e) = ctx.remove_dir(&cur).await {
                if !(ignore && non_empty(&e)) {
                    let what = if first { "" } else { "directory " };
                    ctx.error(&format!(
                        "failed to remove {what}'{}': {}",
                        s(&cur),
                        error_text(&e)
                    ))
                    .await;
                    status = 1;
                }
                break;
            }
            first = false;
            if !parents {
                break;
            }
            // The parent, as written: `a/b/c` → `a/b`.
            let trimmed = cur.len() - cur.iter().rev().take_while(|&&b| b == b'/').count();
            match cur[..trimmed].iter().rposition(|&b| b == b'/') {
                Some(i) => {
                    let mut end = i;
                    while end > 0 && cur[end - 1] == b'/' {
                        end -= 1;
                    }
                    // Stop at the root: `/`, or a drive (`C:`).
                    if end == 0
                        || (cfg!(windows)
                            && end == 2
                            && cur[0].is_ascii_alphabetic()
                            && cur[1] == b':')
                    {
                        break;
                    }
                    cur.truncate(end);
                }
                None => break,
            }
        }
    }
    status
}

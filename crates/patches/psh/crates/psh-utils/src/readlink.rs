//! `readlink [-f|-e|-m] [-n] [-q|-s|-v] [-z] FILE...` (GNU; macOS has `-f -n`). Written
//! for psh after the manuals.

use psh_engine::host::error_text;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{Need, resolve, s};
use crate::opts::{Spec, both};

pub const SPEC: Spec = Spec {
    name: "readlink",
    opts: &[
        both('f', "canonicalize"),
        both('e', "canonicalize-existing"),
        both('m', "canonicalize-missing"),
        both('n', "no-newline"),
        both('q', "quiet"),
        both('s', "silent"),
        both('v', "verbose"),
        both('z', "zero"),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut need = None;
    let mut verbose = false;
    for (k, _) in p.iter(&SPEC) {
        match k.as_str() {
            "canonicalize" => need = Some(Need::AllButLast),
            "canonicalize-existing" => need = Some(Need::All),
            "canonicalize-missing" => need = Some(Need::None),
            "verbose" => verbose = true,
            "quiet" | "silent" => verbose = false,
            _ => {}
        }
    }
    if p.operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let mut newline = !p.has(&SPEC, "no-newline");
    if !newline && p.operands.len() > 1 {
        ctx.error("ignoring --no-newline with multiple arguments")
            .await;
        newline = true;
    }
    let end: &[u8] = match (p.has(&SPEC, "zero"), newline) {
        (true, _) => b"\0",
        (false, true) => b"\n",
        (false, false) => b"",
    };
    let mut status = 0;
    for f in &p.operands {
        let r = match need {
            Some(n) => resolve(ctx, f, n).await,
            None => ctx.read_link(f).await,
        };
        match r {
            Ok(mut t) => {
                t.extend_from_slice(end);
                if let Err(e) = ctx.write(&t).await {
                    return common::write_failed(ctx, &e).await;
                }
            }
            Err(e) => {
                status = 1;
                if verbose {
                    ctx.error(&format!("{}: {}", s(f), error_text(&e))).await;
                }
            }
        }
    }
    status
}

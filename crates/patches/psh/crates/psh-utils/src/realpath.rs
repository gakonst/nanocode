//! `realpath [-e|-m] [-L|-P|-s] [-q] [-z] [--relative-to=DIR] [--relative-base=DIR]
//! FILE...` (GNU). BSD's `realpath` with no operand resolves `.`, which psh accepts
//! too (GNU fails there). Written for psh after the manuals.

use psh_engine::host::error_text;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{Need, is_below, lexical, relative_to, resolve, s};
use crate::opts::{Spec, both, long};

pub const SPEC: Spec = Spec {
    name: "realpath",
    opts: &[
        both('e', "canonicalize-existing"),
        both('m', "canonicalize-missing"),
        both('L', "logical"),
        both('P', "physical"),
        both('s', "strip"),
        long("no-symlinks"),
        both('q', "quiet"),
        both('z', "zero"),
        long("relative-to").value(),
        long("relative-base").value(),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq)]
enum Links {
    Physical,
    Logical,
    None,
}

async fn real(ctx: &UtilCtx<'_>, f: &[u8], need: Need, links: Links) -> std::io::Result<Vec<u8>> {
    match links {
        Links::Physical => resolve(ctx, f, need).await,
        // `..` by text first, then the links.
        Links::Logical => resolve(ctx, &lexical(&ctx.path(f)), need).await,
        Links::None => {
            let p = lexical(&ctx.path(f));
            if need == Need::All {
                ctx.stat(&p, false).await?;
            }
            Ok(p)
        }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut need = Need::AllButLast;
    let mut links = Links::Physical;
    for (k, _) in p.iter(&SPEC) {
        match k.as_str() {
            "canonicalize-existing" => need = Need::All,
            "canonicalize-missing" => need = Need::None,
            "logical" => links = Links::Logical,
            "physical" => links = Links::Physical,
            "strip" | "no-symlinks" => links = Links::None,
            _ => {}
        }
    }
    let quiet = p.has(&SPEC, "quiet");
    let end = if p.has(&SPEC, "zero") { b'\0' } else { b'\n' };
    let base_of = |key: &str| p.last(&SPEC, key).map(<[u8]>::to_vec);
    let (rel_to, rel_base) = (base_of("relative-to"), base_of("relative-base"));
    let mut dirs = [None, None];
    for (slot, d) in dirs.iter_mut().zip([&rel_to, &rel_base]) {
        if let Some(d) = d {
            match real(ctx, d, need, links).await {
                Ok(r) => *slot = Some(r),
                Err(e) => {
                    ctx.error(&format!("{}: {}", s(d), error_text(&e))).await;
                    return 1;
                }
            }
        }
    }
    let [mut to, base] = dirs;
    // A --relative-to outside --relative-base is not used.
    if let (Some(t), Some(b)) = (&to, &base)
        && !is_below(t, b)
    {
        to = None;
    }
    let operands = if p.operands.is_empty() {
        vec![b".".to_vec()]
    } else {
        p.operands.clone()
    };
    let mut status = 0;
    for f in &operands {
        match real(ctx, f, need, links).await {
            Ok(r) => {
                let mut out = match (&to, &base) {
                    (Some(t), Some(b)) if is_below(&r, b) => relative_to(&r, t),
                    (Some(_), Some(_)) => r,
                    (Some(t), None) => relative_to(&r, t),
                    (None, Some(b)) if is_below(&r, b) => relative_to(&r, b),
                    _ => r,
                };
                out.push(end);
                if let Err(e) = ctx.write(&out).await {
                    return common::write_failed(ctx, &e).await;
                }
            }
            Err(e) => {
                status = 1;
                if !quiet {
                    ctx.error(&format!("{}: {}", s(f), error_text(&e))).await;
                }
            }
        }
    }
    status
}

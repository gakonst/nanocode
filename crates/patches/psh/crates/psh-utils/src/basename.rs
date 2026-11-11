//! `basename NAME [SUFFIX]`, `-a`, `-s SUFFIX`, `-z` (GNU and BSD agree). Written for
//! psh after the manuals.

use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{base_name, q};
use crate::opts::{Spec, both};

pub const SPEC: Spec = Spec {
    name: "basename",
    opts: &[
        both('a', "multiple"),
        both('s', "suffix").value(),
        both('z', "zero"),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let suffix = p.last(&SPEC, "suffix").map(<[u8]>::to_vec);
    let multiple = p.has(&SPEC, "multiple") || suffix.is_some();
    let end = if p.has(&SPEC, "zero") { b'\0' } else { b'\n' };
    let ops = &p.operands;
    if ops.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let (names, suffix): (&[Vec<u8>], Option<Vec<u8>>) = if multiple {
        (ops, suffix)
    } else if ops.len() > 2 {
        let msg = format!("extra operand {}", q(ctx, &ops[2]));
        return common::usage(ctx, &msg).await;
    } else {
        (&ops[..1], ops.get(1).cloned())
    };
    let mut out = Vec::new();
    for n in names {
        let mut b = base_name(n);
        if let Some(s) = &suffix
            && !s.is_empty()
            && b.len() > s.len()
            && b.ends_with(s)
        {
            b = &b[..b.len() - s.len()];
        }
        out.extend_from_slice(b);
        out.push(end);
    }
    match ctx.write(&out).await {
        Ok(()) => 0,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

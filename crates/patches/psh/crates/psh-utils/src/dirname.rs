//! `dirname NAME...`, `-z` (GNU; BSD takes several names too). Written for psh after
//! the manuals.

use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::dir_name;
use crate::opts::{Spec, both};

pub const SPEC: Spec = Spec {
    name: "dirname",
    opts: &[both('z', "zero")],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    if p.operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let end = if p.has(&SPEC, "zero") { b'\0' } else { b'\n' };
    let mut out = Vec::new();
    for n in &p.operands {
        out.extend_from_slice(&dir_name(n));
        out.push(end);
    }
    match ctx.write(&out).await {
        Ok(()) => 0,
        Err(e) => common::write_failed(ctx, &e).await,
    }
}

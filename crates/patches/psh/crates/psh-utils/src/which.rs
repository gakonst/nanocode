//! `which [-as] NAME...`: where a command name is. The first executable on `PATH` (with
//! `-a` every one), so `$(which env)` stays a path that runs; a name only psh's
//! utility provides prints `NAME: shell builtin` (D1200); with `-a` that line comes
//! first, before the `PATH` hits. A name with a slash is printed when it is an
//! executable file.
//! `-s` prints nothing. Status 1 when a name is not found (or no name is given), as
//! both GNU-userland (debianutils) and BSD `which` do.

use psh_engine::host::{Access, FileKind};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::{Spec, flag};

pub const SPEC: Spec = Spec {
    name: "which",
    opts: &[flag('a'), flag('s')],
    posix_order: true,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let all = p.has(&SPEC, "a");
    let silent = p.has(&SPEC, "s");
    let path = ctx.var(b"PATH").unwrap_or_default().to_vec();
    let mut status = i32::from(p.operands.is_empty());
    let mut out = Vec::new();
    for name in &p.operands {
        let mut hits: Vec<Vec<u8>> = Vec::new();
        if name.contains(&b'/') {
            let ok = matches!(ctx.stat(name, true).await, Ok(m) if m.kind != FileKind::Dir)
                && ctx.access(name, Access::Execute).await.unwrap_or(false);
            if ok {
                hits.push(name.clone());
            }
        } else if !name.is_empty() {
            let utility = ctx.is_utility(name).await;
            let programs = ctx.search_path(name, &path).await;
            if utility && (all || programs.is_empty()) {
                let mut line = name.clone();
                line.extend_from_slice(b": shell builtin");
                hits.push(line);
            }
            hits.extend(programs);
        }
        if hits.is_empty() {
            status = 1;
            continue;
        }
        if !all {
            hits.truncate(1);
        }
        if !silent {
            for h in hits {
                out.extend_from_slice(&h);
                out.push(b'\n');
            }
        }
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    status
}

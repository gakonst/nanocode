//! `touch [-acmh] [-d DATE | -t STAMP | -r FILE] FILE...`: GNU `-d` (a subset of GNU
//! date strings, the rest falls through), POSIX `-t` and `-r`, BSD `-f` (ignored).
//! Written for psh after the manuals.

use std::io;

use psh_engine::host::OpenOptions;
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{q, s};
use crate::opts::{Spec, both, flag, long};
use crate::timefmt;

pub const SPEC: Spec = Spec {
    name: "touch",
    opts: &[
        flag('a'),
        flag('m'),
        both('c', "no-create"),
        both('d', "date").value(),
        both('r', "reference").value(),
        flag('t').value(),
        both('h', "no-dereference"),
        flag('f'),
        long("time").value(),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let (mut atime, mut mtime) = (false, false);
    for (k, v) in p.iter(&SPEC) {
        match (k.as_str(), v) {
            ("a", _) => atime = true,
            ("m", _) => mtime = true,
            ("time", Some(b"access" | b"atime" | b"use")) => atime = true,
            ("time", Some(b"modify" | b"mtime")) => mtime = true,
            ("time", Some(w)) => {
                let msg = format!("invalid argument {} for '--time'", q(ctx, w));
                return common::usage(ctx, &msg).await;
            }
            _ => {}
        }
    }
    if !atime && !mtime {
        (atime, mtime) = (true, true);
    }
    let follow = !p.has(&SPEC, "no-dereference");
    let no_create = p.has(&SPEC, "no-create") || !follow;
    let now = i64::try_from(ctx.now().as_nanos()).unwrap_or(i64::MAX);
    // The times to set: (atime, mtime).
    let mut times = (now, now);
    if let Some(r) = p.last(&SPEC, "reference") {
        match ctx.stat_extra(r, follow).await {
            Ok((m, x)) => times = (x.atime_ns, m.mtime_ns),
            Err(e) => {
                let msg = format!(
                    "failed to get attributes of {}: {}",
                    q(ctx, r),
                    psh_engine::host::error_text(&e)
                );
                ctx.error(&msg).await;
                return 1;
            }
        }
    }
    if let Some(d) = p.last(&SPEC, "date") {
        let base = if p.has(&SPEC, "reference") {
            times.1
        } else {
            now
        };
        match timefmt::parse_date(ctx, d, base) {
            Some(t) => times = (t, t),
            None => return bad_date(ctx, argv, d).await,
        }
    }
    if let Some(t) = p.last(&SPEC, "t") {
        match timefmt::parse_stamp(ctx, t, now) {
            Some(t) => times = (t, t),
            None => {
                let msg = format!("invalid date format {}", q(ctx, t));
                ctx.error(&msg).await;
                return 1;
            }
        }
    }
    if p.operands.is_empty() {
        return common::usage(ctx, "missing file operand").await;
    }
    let mut status = 0;
    for f in &p.operands {
        if f == b"-" {
            // GNU touches the file on stdout; psh's stdout is not a path. Nothing to do.
            continue;
        }
        if let Err((what, e)) = touch(ctx, f, times, (atime, mtime), follow, no_create).await {
            let msg = format!("{what} '{}': {}", s(f), psh_engine::host::error_text(&e));
            ctx.error(&msg).await;
            status = 1;
        }
    }
    status
}

async fn touch(
    ctx: &mut UtilCtx<'_>,
    f: &[u8],
    (at, mt): (i64, i64),
    (atime, mtime): (bool, bool),
    follow: bool,
    no_create: bool,
) -> Result<(), (&'static str, io::Error)> {
    match ctx.stat(f, follow).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if no_create {
                return Ok(());
            }
            let opts = OpenOptions {
                write: true,
                create: true,
                mode: Some(0o666 & !ctx.umask()),
                ..OpenOptions::default()
            };
            ctx.open(f, opts).await.map_err(|e| ("cannot touch", e))?;
        }
        Err(e) => return Err(("cannot touch", e)),
    }
    ctx.set_times(f, atime.then_some(at), mtime.then_some(mt), follow)
        .await
        .map_err(|e| ("setting times of", e))
}

/// A date string psh does not parse: the real `touch` if there is one (its parser is
/// GNU's or BSD's whole grammar), else GNU's message.
async fn bad_date(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], d: &[u8]) -> i32 {
    if let Some(st) = ctx.run_program(argv.to_vec()).await {
        return st;
    }
    let msg = format!("invalid date format {}", q(ctx, d));
    ctx.error(&msg).await;
    1
}

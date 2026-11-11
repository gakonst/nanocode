//! `sleep`: GNU durations (fractions, `s m h d`, several operands add up, `inf`).
//! Cancellable: the wait is a host timer, dropped with the command.

use std::time::Duration;

use psh_engine::util::UtilCtx;

use crate::common;
use crate::opts::Spec;

pub const SPEC: Spec = Spec {
    name: "sleep",
    opts: &[],
    posix_order: true,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    // Negative-looking operands are not options here (`sleep -1` is an invalid
    // interval in GNU's words, not an unknown option).
    let args = &argv[1..];
    let args: &[Vec<u8>] = match args.first() {
        Some(a) if a == b"--" => &args[1..],
        _ => args,
    };
    if args.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let mut total = 0.0f64;
    let mut bad = false;
    for a in args {
        match common::parse_duration(a) {
            Some(d) => total += d,
            None => {
                let msg = format!("invalid time interval {}", common::quote(ctx, a));
                ctx.error(&msg).await;
                bad = true;
            }
        }
    }
    if bad {
        let name = ctx.name().to_string();
        ctx.error_raw(format!("Try '{name} --help' for more information.\n").as_bytes())
            .await;
        return 1;
    }
    let _ = SPEC;
    if total.is_infinite() {
        // Until cancelled.
        loop {
            ctx.sleep(Duration::from_secs(86400)).await;
        }
    }
    ctx.sleep(Duration::from_secs_f64(total)).await;
    0
}

//! `comm [-123iz] [--check-order|--nocheck-order] [--output-delimiter=S] [--total]
//! FILE1 FILE2`. Written for psh after GNU's documented behavior; `-i` is BSD's. Lines
//! compare by bytes, as `sort` orders them (D1160).

use std::cmp::Ordering;

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Spec, both, flag, long};
use crate::textio::{self, LineReader};

pub const SPEC: Spec = Spec {
    name: "comm",
    opts: &[
        flag('1'),
        flag('2'),
        flag('3'),
        flag('i'),
        long("check-order"),
        long("nocheck-order"),
        long("output-delimiter").value(),
        long("total"),
        both('z', "zero-terminated"),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq)]
enum Check {
    /// Report disorder among lines that end up unpaired (GNU's default).
    Default,
    /// Any disorder is fatal.
    Strict,
    Off,
}

struct Side {
    input: Input,
    reader: LineReader,
    line: Vec<u8>,
    prev: Vec<u8>,
    has_prev: bool,
    /// The current line is out of order.
    disordered: bool,
    reported: bool,
    done: bool,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let show = [!p.has(&SPEC, "1"), !p.has(&SPEC, "2"), !p.has(&SPEC, "3")];
    let fold = p.has(&SPEC, "i");
    let mut check = Check::Default;
    for (k, _) in p.iter(&SPEC) {
        match k.as_str() {
            "check-order" => check = Check::Strict,
            "nocheck-order" => check = Check::Off,
            _ => {}
        }
    }
    let delim = p.last(&SPEC, "output-delimiter").unwrap_or(b"\t").to_vec();
    let eol = if p.has(&SPEC, "zero-terminated") {
        0
    } else {
        b'\n'
    };
    match p.operands.len() {
        0 => return common::usage(ctx, "missing operand").await,
        1 => {
            let o = common::quote(ctx, &p.operands[0]);
            return common::usage(ctx, &format!("missing operand after {o}")).await;
        }
        2 => {}
        _ => {
            let o = common::quote(ctx, &p.operands[2]);
            return common::usage(ctx, &format!("extra operand {o}")).await;
        }
    }
    let mut sides = Vec::new();
    for name in &p.operands {
        let Some(input) = textio::open_or_report(ctx, name).await else {
            return 1;
        };
        sides.push(Side {
            input,
            reader: LineReader::new(eol),
            line: Vec::new(),
            prev: Vec::new(),
            has_prev: false,
            disordered: false,
            reported: false,
            done: false,
        });
    }
    let cmp = |a: &[u8], b: &[u8]| {
        if fold {
            a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
        } else {
            a.cmp(b)
        }
    };
    // The column prefixes: a delimiter for each shown column to the left.
    let prefix = |col: usize| -> Vec<u8> {
        let n = show[..col].iter().filter(|&&s| s).count();
        delim.repeat(n)
    };
    let prefixes = [prefix(0), prefix(1), prefix(2)];
    let mut totals = [0u64; 3];
    let mut out = Vec::new();
    let mut status = 0;

    for (k, side) in sides.iter_mut().enumerate() {
        if let Err(e) = advance(ctx, side, &cmp).await {
            common::file_error(ctx, &p.operands[k], &e).await;
            return 1;
        }
    }
    loop {
        let (a, b) = (&sides[0], &sides[1]);
        let col = match (a.done, b.done) {
            (true, true) => break,
            (false, true) => 0,
            (true, false) => 1,
            (false, false) => match cmp(&a.line, &b.line) {
                Ordering::Less => 0,
                Ordering::Greater => 1,
                Ordering::Equal => 2,
            },
        };
        // Disorder is reported when it matters: always with --check-order, for an
        // unpaired line by default.
        for (k, s) in sides.iter_mut().enumerate() {
            if s.disordered
                && !s.reported
                && check != Check::Off
                && (col == k || check == Check::Strict)
            {
                s.reported = true;
                if let Err(e) = ctx.write(&out).await {
                    return common::write_failed(ctx, &e).await;
                }
                out.clear();
                ctx.error(&format!("file {} is not in sorted order", k + 1))
                    .await;
                if check == Check::Strict {
                    return 1;
                }
                status = 1;
            }
        }
        totals[col] += 1;
        if show[col] {
            out.extend_from_slice(&prefixes[col]);
            let line = if col == 1 {
                &sides[1].line
            } else {
                &sides[0].line
            };
            out.extend_from_slice(line);
            out.push(eol);
        }
        let which: &[usize] = match col {
            0 => &[0],
            1 => &[1],
            _ => &[0, 1],
        };
        for &k in which {
            if let Err(e) = advance(ctx, &mut sides[k], &cmp).await {
                common::file_error(ctx, &p.operands[k], &e).await;
                return 1;
            }
        }
        if out.len() >= 32 * 1024 {
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
            out.clear();
        }
        ctx.check().await;
    }
    if p.has(&SPEC, "total") {
        let mut t = Vec::new();
        for n in totals {
            t.extend_from_slice(n.to_string().as_bytes());
            t.extend_from_slice(&delim);
        }
        t.extend_from_slice(b"total");
        t.push(eol);
        out.extend_from_slice(&t);
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    if status != 0 {
        ctx.error("input is not in sorted order").await;
    }
    status
}

/// Reads the next line of a side and notes whether it is out of order.
async fn advance(
    ctx: &mut UtilCtx<'_>,
    s: &mut Side,
    cmp: &impl Fn(&[u8], &[u8]) -> Ordering,
) -> std::io::Result<()> {
    if s.done {
        return Ok(());
    }
    std::mem::swap(&mut s.prev, &mut s.line);
    match s.reader.next(ctx, &s.input, &mut s.line).await? {
        Some(_) => {
            if s.has_prev {
                s.disordered = cmp(&s.prev, &s.line) == Ordering::Greater;
            }
            s.has_prev = true;
        }
        None => s.done = true,
    }
    Ok(())
}

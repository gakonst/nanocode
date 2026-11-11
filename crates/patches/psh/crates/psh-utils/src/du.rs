//! `du [-a|-s] [-d N|--max-depth=N] [-c] [-h|--si|-k|-m|-g|-B SIZE|-b] [-A|--apparent-size]
//! [--exclude=PAT] [-X FILE] [-t SIZE] [-x] [-L|-H|-P] [-l] [-0] FILE...`: GNU's units
//! (1 KiB, D016: BSD counts 512-byte blocks), BSD's `-A` and `-g` too. Hard-linked files
//! and directories reached twice count once. Entries are visited in name order. Written
//! for psh after the manuals.

use std::collections::HashSet;

use psh_engine::host::{FileKind, error_text};
use psh_engine::util::{LocalBoxFuture, UtilCtx};

use crate::common::{self, Input};
use crate::fsutil::{base_name, human, join, s};
use crate::ls::glob;
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "du",
    opts: &[
        both('a', "all"),
        both('s', "summarize"),
        both('d', "max-depth").value(),
        both('c', "total"),
        both('h', "human-readable"),
        long("si"),
        flag('k'),
        flag('m'),
        flag('g'),
        both('B', "block-size").value(),
        both('b', "bytes"),
        both('A', "apparent-size"),
        long("exclude").value(),
        both('X', "exclude-from").value(),
        both('t', "threshold").value(),
        both('x', "one-file-system"),
        both('L', "dereference"),
        both('H', "dereference-args"),
        flag('D'),
        both('P', "no-dereference"),
        both('l', "count-links"),
        both('0', "null"),
    ],
    posix_order: false,
};

struct Opts {
    all: bool,
    max_depth: Option<usize>,
    apparent: bool,
    /// Bytes per printed unit; `None`: human-readable with this base.
    unit: u64,
    human: Option<u64>,
    exclude: Vec<Vec<u8>>,
    threshold: i64,
    one_fs: bool,
    deref: bool,
    deref_args: bool,
    count_links: bool,
    end: u8,
}

struct Walk {
    seen: HashSet<(u64, u64)>,
    out: Vec<u8>,
    status: i32,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut o = Opts {
        all: false,
        max_depth: None,
        apparent: false,
        unit: 1024,
        human: None,
        exclude: Vec::new(),
        threshold: 0,
        one_fs: false,
        deref: false,
        deref_args: false,
        count_links: false,
        end: b'\n',
    };
    let mut summarize = false;
    for (k, v) in p.iter(&SPEC) {
        let v = v.unwrap_or_default();
        match k.as_str() {
            "all" => o.all = true,
            "summarize" => summarize = true,
            "max-depth" => match std::str::from_utf8(v).ok().and_then(|n| n.parse().ok()) {
                Some(n) => o.max_depth = Some(n),
                None => {
                    let msg = format!("invalid maximum depth '{}'", s(v));
                    return common::usage(ctx, &msg).await;
                }
            },
            "human-readable" => o.human = Some(1024),
            "si" => o.human = Some(1000),
            "k" => (o.unit, o.human) = (1024, None),
            "m" => (o.unit, o.human) = (1 << 20, None),
            "g" => (o.unit, o.human) = (1 << 30, None),
            "block-size" => {
                let spec = if v.first().is_some_and(u8::is_ascii_digit) {
                    v.to_vec()
                } else {
                    [&b"1"[..], v].concat()
                };
                match common::parse_count(&spec).filter(|&n| n > 0) {
                    Some(n) => (o.unit, o.human) = (n, None),
                    None => {
                        let msg = format!("invalid -B argument '{}'", s(v));
                        return common::usage(ctx, &msg).await;
                    }
                }
            }
            "bytes" => (o.apparent, o.unit, o.human) = (true, 1, None),
            "apparent-size" => o.apparent = true,
            "exclude" => o.exclude.push(v.to_vec()),
            "exclude-from" => {
                let mut data = Vec::new();
                let r = match Input::open(ctx, v).await {
                    Ok(f) => f.read_to_end(ctx, &mut data).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = r {
                    ctx.error(&format!("{}: {}", s(v), error_text(&e))).await;
                    return 1;
                }
                o.exclude.extend(
                    data.split(|&b| b == b'\n')
                        .filter(|l| !l.is_empty())
                        .map(<[u8]>::to_vec),
                );
            }
            "threshold" => {
                let (neg, n) = match v.strip_prefix(b"-") {
                    Some(r) => (true, r),
                    None => (false, v),
                };
                match common::parse_count(n) {
                    Some(n) => o.threshold = if neg { -(n as i64) } else { n as i64 },
                    None => {
                        let msg = format!("invalid --threshold argument '{}'", s(v));
                        return common::usage(ctx, &msg).await;
                    }
                }
            }
            "one-file-system" => o.one_fs = true,
            "dereference" => o.deref = true,
            "dereference-args" | "D" => o.deref_args = true,
            "no-dereference" => (o.deref, o.deref_args) = (false, false),
            "count-links" => o.count_links = true,
            "null" => o.end = b'\0',
            _ => {}
        }
    }
    if summarize {
        if o.all {
            ctx.error("cannot both summarize and show all entries")
                .await;
            ctx.error_raw(b"Try 'du --help' for more information.\n")
                .await;
            return 1;
        }
        if o.max_depth.is_some_and(|d| d > 0) {
            ctx.error(&format!(
                "warning: summarizing conflicts with --max-depth={}",
                o.max_depth.unwrap_or(0)
            ))
            .await;
            ctx.error_raw(b"Try 'du --help' for more information.\n")
                .await;
            return 1;
        }
        o.max_depth = Some(0);
    }
    let operands = if p.operands.is_empty() {
        vec![b".".to_vec()]
    } else {
        p.operands.clone()
    };
    let mut w = Walk {
        seen: HashSet::new(),
        out: Vec::new(),
        status: 0,
    };
    let mut total = 0u64;
    for op in operands {
        if let Some(n) = walk(ctx, &mut w, op, 0, &o, None).await {
            total += n;
        }
        if w.out.len() > 32 * 1024 {
            let data = std::mem::take(&mut w.out);
            if let Err(e) = ctx.write(&data).await {
                return common::write_failed(ctx, &e).await;
            }
        }
    }
    if p.has(&SPEC, "total") {
        line(&mut w.out, total, b"total", &o);
    }
    if let Err(e) = ctx.write(&w.out).await {
        return common::write_failed(ctx, &e).await;
    }
    w.status
}

fn line(out: &mut Vec<u8>, bytes: u64, path: &[u8], o: &Opts) {
    let text = match o.human {
        Some(base) => human(bytes, base),
        None => bytes.div_ceil(o.unit).to_string(),
    };
    out.extend_from_slice(text.as_bytes());
    out.push(b'\t');
    out.extend_from_slice(path);
    out.push(o.end);
}

fn shown(bytes: u64, o: &Opts) -> bool {
    match o.threshold {
        0 => true,
        t if t > 0 => bytes >= t as u64,
        t => bytes <= t.unsigned_abs(),
    }
}

/// The size of `path` in bytes (allocated or apparent), printing what the options ask
/// for; `None` when it could not be read or was counted already.
fn walk<'a, 'v>(
    ctx: &'a mut UtilCtx<'v>,
    w: &'a mut Walk,
    path: Vec<u8>,
    depth: usize,
    o: &'a Opts,
    dev: Option<u64>,
) -> LocalBoxFuture<'a, Option<u64>> {
    Box::pin(async move {
        ctx.check().await;
        if depth > 0
            && o.exclude
                .iter()
                .any(|pat| glob(pat, base_name(&path)) || glob(pat, &path))
        {
            return Some(0);
        }
        let follow = o.deref || (depth == 0 && o.deref_args);
        let (m, x) = match ctx.stat_extra(&path, follow).await {
            Ok(v) => v,
            Err(e) => {
                ctx.error(&format!("cannot access '{}': {}", s(&path), error_text(&e)))
                    .await;
                w.status = 1;
                return None;
            }
        };
        if o.one_fs && dev.is_some_and(|d| d != m.dev) {
            return Some(0);
        }
        let is_dir = m.kind == FileKind::Dir;
        if (is_dir || x.nlink > 1) && !o.count_links && m.ino != 0 && !w.seen.insert((m.dev, m.ino))
        {
            return Some(0);
        }
        let own = if o.apparent {
            if is_dir { 0 } else { m.size }
        } else {
            x.blocks * 512
        };
        let printed = o.max_depth.is_none_or(|d| depth <= d);
        if !is_dir {
            if (o.all || depth == 0) && printed && shown(own, o) {
                line(&mut w.out, own, &path, o);
            }
            return Some(own);
        }
        let mut total = own;
        match ctx.read_dir(&path).await {
            Ok(mut entries) => {
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                for e in entries {
                    let child = join(&path, &e.name);
                    if let Some(n) = walk(ctx, w, child, depth + 1, o, Some(m.dev)).await {
                        total += n;
                    }
                }
            }
            Err(e) => {
                ctx.error(&format!(
                    "cannot read directory '{}': {}",
                    s(&path),
                    error_text(&e)
                ))
                .await;
                w.status = 1;
            }
        }
        if printed && shown(total, o) {
            line(&mut w.out, total, &path, o);
        }
        Some(total)
    })
}

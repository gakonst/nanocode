//! `mktemp [-d] [-u] [-q] [-p DIR | --tmpdir[=DIR]] [-t] [--suffix=S] [TEMPLATE...]`:
//! exclusive creation, files 0600 and directories 0700, in `$TMPDIR` (else `/tmp`) for
//! `-t`, `-p`, `--tmpdir` and the default template. `-t` per D016: an argument with
//! `XXX` is GNU's template, any other is BSD's prefix (`PREFIX.XXXXXXXXXX`); `-t` last
//! is GNU's flag. Several templates are BSD's. Written for psh after the manuals.

use std::cell::Cell;
use std::io;

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{join, q};
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "mktemp",
    opts: &[
        both('d', "directory"),
        both('u', "dry-run"),
        both('q', "quiet"),
        flag('p').value(),
        long("tmpdir").optional(),
        flag('t').value(),
        long("suffix").value(),
    ],
    posix_order: false,
};

/// A `-t` with nothing after it is GNU's flag: `--tmpdir`.
fn rewrite(argv: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut out = argv.to_vec();
    let n = out.len();
    let last = &out[n - 1];
    if n > 1
        && last.len() >= 2
        && last[0] == b'-'
        && last[1] != b'-'
        && last.ends_with(b"t")
        && !last[1..last.len() - 1].contains(&b'p')
    {
        let mut cluster = out.pop().expect("n > 1");
        cluster.pop();
        if cluster.len() > 1 {
            out.push(cluster);
        }
        out.push(b"--tmpdir".to_vec());
    }
    out
}

thread_local! {
    static STATE: Cell<u64> = const { Cell::new(0) };
}

/// splitmix64 over a state seeded from the clock (and /dev/urandom where there is one).
fn next(seed: u64) -> u64 {
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            x = seed | 1;
        }
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        s.set(x);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    })
}

const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

async fn seed(ctx: &UtilCtx<'_>) -> u64 {
    let mut s = ctx.now().as_nanos() as u64 ^ ((ctx as *const _ as usize as u64) << 17);
    if let Ok(f) = ctx.open(b"/dev/urandom", OpenOptions::read()).await {
        let mut b = [0u8; 8];
        if let Ok(8) = f.read(&mut b).await {
            s ^= u64::from_le_bytes(b);
        }
    }
    s
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let argv = rewrite(argv);
    let p = match common::parse(ctx, &SPEC, &argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let dir_mode = p.has(&SPEC, "directory");
    let dry = p.has(&SPEC, "dry-run");
    let quiet = p.has(&SPEC, "quiet");
    let suffix = p.last(&SPEC, "suffix").unwrap_or_default().to_vec();
    let tmpdir = || {
        ctx.var(b"TMPDIR")
            .filter(|v| !v.is_empty())
            .map_or(b"/tmp".to_vec(), <[u8]>::to_vec)
    };
    // Where templates go: `None` means as written (relative to the cwd).
    let mut base: Option<Vec<u8>> = None;
    let mut templates: Vec<Vec<u8>> = p.operands.clone();
    let mut in_tmpdir = false;
    for (k, v) in p.iter(&SPEC) {
        match k.as_str() {
            "p" => base = Some(v.unwrap_or_default().to_vec()),
            "tmpdir" => {
                in_tmpdir = true;
                if let Some(d) = v {
                    base = Some(d.to_vec());
                }
            }
            "t" => {
                let a = v.unwrap_or_default().to_vec();
                in_tmpdir = true;
                if a.windows(3).any(|w| w == b"XXX") {
                    templates.insert(0, a);
                } else {
                    // BSD: a prefix.
                    let mut t = a;
                    t.extend_from_slice(b".XXXXXXXXXX");
                    templates.insert(0, t);
                }
            }
            _ => {}
        }
    }
    if templates.is_empty() {
        templates.push(b"tmp.XXXXXXXXXX".to_vec());
        in_tmpdir = true;
    }
    if in_tmpdir && base.as_ref().is_none_or(|b| b.is_empty()) {
        base = Some(tmpdir());
    }
    if base.as_ref().is_some_and(|b| b.is_empty()) {
        base = Some(tmpdir());
    }
    let seed = seed(ctx).await;
    let mut status = 0;
    for t in templates {
        if in_tmpdir && t.contains(&b'/') && p.last(&SPEC, "p").is_none() {
            let msg = format!(
                "invalid template, {}, contains directory separator",
                q(ctx, &t)
            );
            ctx.error(&msg).await;
            status = 1;
            continue;
        }
        let mut full = match &base {
            Some(b) if t.first() != Some(&b'/') => join(b, &t),
            _ => t.clone(),
        };
        // The last run of X's (at least three) is replaced; what follows is a suffix.
        let end = match full.iter().rposition(|&b| b == b'X') {
            Some(i) => i + 1,
            None => 0,
        };
        let start = full[..end]
            .iter()
            .rposition(|&b| b != b'X')
            .map_or(0, |i| i + 1);
        let slash = full.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
        let start = start.max(slash);
        if end < start + 3 || end == 0 {
            let msg = format!("too few X's in template {}", q(ctx, &t));
            ctx.error(&msg).await;
            status = 1;
            continue;
        }
        full.extend_from_slice(&suffix);
        let mut made = None;
        let mut last_err = None;
        for _ in 0..100 {
            for b in &mut full[start..end] {
                *b = CHARS[(next(seed) % CHARS.len() as u64) as usize];
            }
            let r = if dry {
                match ctx.stat(&full, false).await {
                    Ok(_) => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                    Err(_) => Ok(()),
                }
            } else if dir_mode {
                ctx.mkdir(&full, 0o700).await
            } else {
                let opts = OpenOptions {
                    write: true,
                    create: true,
                    create_new: true,
                    mode: Some(0o600),
                    ..OpenOptions::default()
                };
                ctx.open(&full, opts).await.map(|_| ())
            };
            match r {
                Ok(()) => {
                    made = Some(full.clone());
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last_err = Some(e),
                Err(e) => {
                    last_err = Some(e);
                    break;
                }
            }
        }
        match made {
            Some(path) => {
                let mut line = path;
                line.push(b'\n');
                if let Err(e) = ctx.write(&line).await {
                    return common::write_failed(ctx, &e).await;
                }
            }
            None => {
                if !quiet {
                    let e = last_err.unwrap_or_else(|| io::ErrorKind::AlreadyExists.into());
                    let what = if dir_mode { "directory" } else { "file" };
                    let msg = format!(
                        "failed to create {what} via template {}: {}",
                        q(ctx, &t),
                        error_text(&e)
                    );
                    ctx.error(&msg).await;
                }
                status = 1;
            }
        }
    }
    status
}

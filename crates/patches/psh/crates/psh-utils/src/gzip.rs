//! gzip, gunzip and zcat (nanocode's addition), after GNU gzip: -c -d -k -f -t -l -n -N
//! -q -v -S and the levels. A file is replaced by its compressed or expanded form,
//! with its mode and time, unless -c or -k says otherwise; no operand, or -, is stdin to
//! stdout. Whole files are held in memory. GNU's messages; where BSD's gzip (macOS)
//! differs, GNU (D016). The deflate stream is miniz_oxide's, so compressed bytes differ
//! from GNU's while each reads the other's.

use psh_engine::host::{FileKind, OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::codec::{self, GzipError};
use crate::common::{self, Input};
use crate::opts::{Spec, both, flag};

pub const SPEC: Spec = Spec {
    name: "gzip",
    opts: &[
        both('c', "stdout"),
        both('c', "to-stdout"),
        both('d', "decompress"),
        both('d', "uncompress"),
        both('k', "keep"),
        both('f', "force"),
        both('t', "test"),
        both('l', "list"),
        both('n', "no-name"),
        both('N', "name"),
        both('q', "quiet"),
        both('v', "verbose"),
        both('S', "suffix").value(),
        flag('1'),
        flag('2'),
        flag('3'),
        flag('4'),
        flag('5'),
        flag('6'),
        flag('7'),
        flag('8'),
        both('9', "best"),
        crate::opts::long("fast"),
    ],
    posix_order: false,
};

/// The name a compressed file expands to, by the suffixes gzip knows.
fn expanded_name(name: &[u8], suffix: &[u8]) -> Option<Vec<u8>> {
    let known: [(&[u8], &[u8]); 7] = [
        (suffix, b""),
        (b".tgz", b".tar"),
        (b".taz", b".tar"),
        (b"-gz", b""),
        (b".z", b""),
        (b"-z", b""),
        (b"_z", b""),
    ];
    for (from, to) in known {
        if name.len() > from.len() && name.ends_with(from) && !from.is_empty() {
            let mut out = name[..name.len() - from.len()].to_vec();
            out.extend_from_slice(to);
            return Some(out);
        }
    }
    None
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let called = ctx.name().to_string();
    let mut decompress = called == "gunzip" || called == "zcat" || called == "gzcat";
    let mut stdout = called == "zcat" || called == "gzcat";
    let (mut keep, mut force, mut test, mut list) = (false, false, false, false);
    let (mut quiet, mut verbose, mut store_name, mut restore_name) = (false, false, true, false);
    let mut level = 6u8;
    let mut suffix = b".gz".to_vec();
    for (key, value) in p.iter(&SPEC) {
        match key.as_str() {
            "stdout" | "to-stdout" => stdout = true,
            "decompress" | "uncompress" => decompress = true,
            "keep" => keep = true,
            "force" => force = true,
            "test" => {
                test = true;
                decompress = true;
            }
            "list" => {
                list = true;
                decompress = true;
            }
            "no-name" => store_name = false,
            "name" => {
                store_name = true;
                restore_name = true;
            }
            "quiet" => quiet = true,
            "verbose" => verbose = true,
            "suffix" => suffix = value.unwrap_or_default().to_vec(),
            "best" => level = 9,
            "fast" => level = 1,
            digit => {
                if let Ok(n) = digit.parse::<u8>() {
                    level = n;
                }
            }
        }
    }
    let mut status = 0;
    if list {
        let _ = ctx
            .write(b"         compressed        uncompressed  ratio uncompressed_name\n")
            .await;
    }
    for name in common::operands_or_stdin(&p) {
        let is_stdin = name == b"-";
        let shown = String::from_utf8_lossy(&name).into_owned();
        let input = match Input::open(ctx, &name).await {
            Ok(input) => input,
            Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => {
                if !quiet {
                    ctx.error(&format!("{shown} is a directory -- ignored"))
                        .await;
                }
                status = status.max(2);
                continue;
            }
            Err(e) => {
                ctx.error(&format!("{shown}: {}", error_text(&e))).await;
                status = 1;
                continue;
            }
        };
        let mut data = Vec::new();
        if let Err(e) = input.read_to_end(ctx, &mut data).await {
            ctx.error(&format!("{shown}: {}", error_text(&e))).await;
            status = 1;
            continue;
        }
        let meta = if is_stdin {
            None
        } else {
            ctx.stat(&name, true).await.ok()
        };
        let label = if is_stdin {
            "stdin".to_string()
        } else {
            shown.clone()
        };
        // Where the result goes: stdout, or a file named after the input.
        let target = if is_stdin || stdout || test || list {
            None
        } else if decompress {
            match expanded_name(&name, &suffix) {
                Some(target) => Some(target),
                None => {
                    if !quiet {
                        ctx.error(&format!("{shown}: unknown suffix -- ignored"))
                            .await;
                    }
                    status = status.max(2);
                    continue;
                }
            }
        } else {
            if name.ends_with(&suffix) && !suffix.is_empty() {
                if !quiet {
                    ctx.error(&format!(
                        "{shown} already has {} suffix -- unchanged",
                        String::from_utf8_lossy(&suffix)
                    ))
                    .await;
                }
                status = status.max(2);
                continue;
            }
            let mut target = name.clone();
            target.extend_from_slice(&suffix);
            Some(target)
        };
        let (out, stored_name) = if decompress {
            match codec::gunzip(&data) {
                Ok((out, trailing)) => {
                    if trailing && !quiet {
                        ctx.error(&format!(
                            "{label}: decompression OK, trailing garbage ignored"
                        ))
                        .await;
                        status = status.max(2);
                    }
                    let stored = codec::gunzip_member(&data)
                        .ok()
                        .and_then(|(m, _)| m.name.map(|name| (name, m.mtime)));
                    (out, stored)
                }
                Err(GzipError::NotGzip) => {
                    ctx.error(&format!("{label}: not in gzip format")).await;
                    status = 1;
                    continue;
                }
                Err(GzipError::Corrupt(what)) => {
                    ctx.error(&format!("{label}: {what}")).await;
                    status = 1;
                    continue;
                }
            }
        } else {
            let base = name.rsplit(|&b| b == b'/').next().unwrap_or(&name);
            let stored = (store_name && !is_stdin).then_some(base);
            let mtime = match (&meta, store_name) {
                (Some(m), true) => (m.mtime_ns / 1_000_000_000) as u32,
                _ => 0,
            };
            (codec::gzip(&data, level, stored, mtime), None)
        };
        if list {
            let ratio = if out.is_empty() {
                0.0
            } else {
                100.0 - (data.len() as f64) * 100.0 / (out.len() as f64)
            };
            let plain = match expanded_name(&name, &suffix) {
                Some(n) => String::from_utf8_lossy(&n).into_owned(),
                None => label.clone(),
            };
            let line = format!(
                "{:>19} {:>19} {ratio:5.1}% {plain}\n",
                data.len(),
                out.len()
            );
            let _ = ctx.write(line.as_bytes()).await;
            continue;
        }
        if test {
            if verbose {
                ctx.error_raw(format!("{shown}:\t OK\n").as_bytes()).await;
            }
            continue;
        }
        let Some(mut target) = target else {
            if let Err(e) = ctx.write(&out).await {
                return common::write_failed(ctx, &e).await;
            }
            continue;
        };
        let mut stamp = meta.map(|m| m.mtime_ns);
        if restore_name
            && let Some((stored, mtime)) = stored_name
            && !stored.is_empty()
        {
            let dir = match name.iter().rposition(|&b| b == b'/') {
                Some(slash) => &name[..=slash],
                None => &b""[..],
            };
            target = [dir, &stored[..]].concat();
            if mtime != 0 {
                stamp = Some(i64::from(mtime) * 1_000_000_000);
            }
        }
        if !force && ctx.stat(&target, false).await.is_ok() {
            ctx.error(&format!(
                "{} already exists; not overwritten",
                String::from_utf8_lossy(&target)
            ))
            .await;
            status = status.max(2);
            continue;
        }
        if meta.is_some_and(|m| m.kind != FileKind::File) && !force {
            if !quiet {
                ctx.error(&format!(
                    "{shown} is not a directory or a regular file - ignored"
                ))
                .await;
            }
            status = status.max(2);
            continue;
        }
        let options = OpenOptions {
            mode: meta.map(|m| m.mode & 0o777),
            ..OpenOptions::write()
        };
        let written = match ctx.open(&target, options).await {
            Ok(file) => file.write_all(&out).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            ctx.error(&format!(
                "{}: {}",
                String::from_utf8_lossy(&target),
                error_text(&e)
            ))
            .await;
            status = 1;
            continue;
        }
        if let Some(stamp) = stamp {
            let _ = ctx.set_times(&target, Some(stamp), Some(stamp), true).await;
        }
        if verbose {
            let (plain, packed) = if decompress {
                (out.len(), data.len())
            } else {
                (data.len(), out.len())
            };
            let ratio = if plain == 0 {
                0.0
            } else {
                100.0 - (packed as f64) * 100.0 / (plain as f64)
            };
            ctx.error_raw(
                format!(
                    "{shown}:\t{ratio:5.1}% -- {} {}\n",
                    if keep { "created" } else { "replaced with" },
                    String::from_utf8_lossy(&target)
                )
                .as_bytes(),
            )
            .await;
        }
        if !keep && let Err(e) = ctx.remove_file(&name).await {
            ctx.error(&format!("{shown}: {}", error_text(&e))).await;
            status = 1;
        }
    }
    status
}

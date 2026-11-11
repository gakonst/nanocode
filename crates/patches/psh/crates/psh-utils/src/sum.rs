//! The checksum utilities, each with its native output (D016: keyed on the name):
//!
//! - GNU `md5sum sha1sum sha224sum sha256sum sha384sum sha512sum [-bctwz] [--tag]
//!   [--quiet] [--status] [--strict] [--ignore-missing] [FILE]...`: `HASH  FILE`.
//! - BSD `md5 [-pqr] [-c HASH] [-s STRING] [FILE]...`: `MD5 (FILE) = HASH`.
//! - perl `shasum [-a ALG] [-bt] [-c] [--tag] ...`: `HASH  FILE`; `-c` takes the
//!   algorithm from each hash's length.
//!
//! `-c` verifies lines in either format (`HASH  FILE`, `HASH *FILE`, `ALG (FILE) =
//! HASH`). Written for psh after the GNU coreutils manual, FreeBSD's md5(1) and
//! shasum's documentation; the digests are ours (`sum_hash`).

use std::io;

use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::opts::{Parsed, Spec, both, long};
use crate::sum_hash::{Algo, Digest, hex};

pub const SPEC: Spec = Spec {
    name: "md5sum",
    opts: &[
        both('b', "binary"),
        both('t', "text"),
        both('c', "check"),
        long("tag"),
        both('z', "zero"),
        long("ignore-missing"),
        long("quiet"),
        long("status"),
        long("strict"),
        both('w', "warn"),
    ],
    posix_order: false,
};

pub const MD5_SPEC: Spec = Spec {
    name: "md5",
    opts: &[
        both('p', "passthrough"),
        both('q', "quiet"),
        both('r', "reverse"),
        both('c', "check").value(),
        both('s', "string").value(),
    ],
    posix_order: true,
};

pub const SHASUM_SPEC: Spec = Spec {
    name: "shasum",
    opts: &[
        both('a', "algorithm").value(),
        both('b', "binary"),
        both('t', "text"),
        both('c', "check"),
        long("tag"),
        long("ignore-missing"),
        both('q', "quiet"),
        both('s', "status"),
        long("strict"),
        both('w', "warn"),
    ],
    posix_order: false,
};

/// The algorithm a GNU-style name computes.
fn gnu_algo(name: &str) -> Algo {
    match name {
        "sha1sum" => Algo::Sha1,
        "sha224sum" => Algo::Sha224,
        "sha256sum" => Algo::Sha256,
        "sha384sum" => Algo::Sha384,
        "sha512sum" => Algo::Sha512,
        _ => Algo::Md5,
    }
}

/// The digest of an operand (`-` is stdin).
async fn digest_of(ctx: &mut UtilCtx<'_>, algo: Algo, name: &[u8]) -> io::Result<Vec<u8>> {
    let input = Input::open(ctx, name).await?;
    let mut d = Digest::new(algo);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = input.read(ctx, &mut buf).await?;
        if n == 0 {
            break;
        }
        d.update(&buf[..n]);
        ctx.check().await;
    }
    Ok(d.finish())
}

/// A file name as GNU's messages show it: in single quotes when it has characters a
/// shell would treat specially.
fn quotef(name: &[u8]) -> String {
    let s = String::from_utf8_lossy(name);
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "_-./+,:=@%^".contains(c) || !c.is_ascii());
    if plain {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `NAME: FILE: reason`, with GNU's quoting of the file.
async fn file_error(ctx: &mut UtilCtx<'_>, name: &[u8], e: &io::Error, quote: bool) {
    let shown = if quote {
        quotef(name)
    } else {
        String::from_utf8_lossy(name).into_owned()
    };
    ctx.error(&format!("{shown}: {}", psh_engine::host::error_text(e)))
        .await;
}

/// GNU escapes a name with a backslash or newline (and marks the line with a leading
/// backslash).
fn escape_name(name: &[u8]) -> Option<Vec<u8>> {
    if !name.iter().any(|&b| matches!(b, b'\\' | b'\n' | b'\r')) {
        return None;
    }
    let mut out = Vec::with_capacity(name.len() + 2);
    for &b in name {
        match b {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(b),
        }
    }
    Some(out)
}

fn unescape_name(name: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(name.len());
    let mut i = 0;
    while i < name.len() {
        if name[i] == b'\\' {
            match name.get(i + 1)? {
                b'\\' => out.push(b'\\'),
                b'n' => out.push(b'\n'),
                b'r' => out.push(b'\r'),
                _ => return None,
            }
            i += 2;
        } else {
            out.push(name[i]);
            i += 1;
        }
    }
    Some(out)
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let algo = gnu_algo(ctx.name());
    let check = p.has(&SPEC, "check");
    let tag = p.has(&SPEC, "tag");
    if check && tag {
        return common::usage(
            ctx,
            "the --tag option is meaningless when verifying checksums",
        )
        .await;
    }
    if !check {
        for opt in ["ignore-missing", "quiet", "status", "strict", "warn"] {
            if p.has(&SPEC, opt) {
                let msg = format!("the --{opt} option is meaningful only when verifying checksums");
                return common::usage(ctx, &msg).await;
            }
        }
    }
    let flags = CheckFlags::new(&p, &SPEC, "status");
    if check {
        let mut status = 0;
        for name in common::operands_or_stdin(&p) {
            let st = verify(ctx, Some(algo), &name, &flags, Dialect::Gnu).await;
            status = status.max(st);
        }
        return status;
    }
    let binary = p
        .iter(&SPEC)
        .filter(|(k, _)| k == "binary" || k == "text")
        .last()
        .is_some_and(|(k, _)| k == "binary");
    let zero = p.has(&SPEC, "zero");
    print_sums(
        ctx,
        algo,
        &common::operands_or_stdin(&p),
        binary,
        tag,
        zero,
        true,
    )
    .await
}

/// Prints `HASH  FILE` (or `--tag` lines) for each operand; GNU and shasum.
async fn print_sums(
    ctx: &mut UtilCtx<'_>,
    algo: Algo,
    names: &[Vec<u8>],
    binary: bool,
    tag: bool,
    zero: bool,
    gnu: bool,
) -> i32 {
    let mut status = 0;
    for name in names {
        let sum = match digest_of(ctx, algo, name).await {
            Ok(s) => s,
            Err(e) => {
                file_error(ctx, name, &e, gnu).await;
                status = 1;
                continue;
            }
        };
        let escaped = if zero || !gnu {
            None
        } else {
            escape_name(name)
        };
        let mut line = Vec::new();
        if escaped.is_some() {
            line.push(b'\\');
        }
        let shown = escaped.as_deref().unwrap_or(name);
        if tag {
            line.extend_from_slice(algo.tag().as_bytes());
            line.extend_from_slice(b" (");
            line.extend_from_slice(shown);
            line.extend_from_slice(b") = ");
            line.extend_from_slice(hex(&sum).as_bytes());
        } else {
            line.extend_from_slice(hex(&sum).as_bytes());
            line.extend_from_slice(if binary { b" *" } else { b"  " });
            line.extend_from_slice(shown);
        }
        line.push(if zero { 0 } else { b'\n' });
        if let Err(e) = ctx.write(&line).await {
            return common::write_failed(ctx, &e).await;
        }
    }
    status
}

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Gnu,
    Shasum,
}

struct CheckFlags {
    ignore_missing: bool,
    quiet: bool,
    status: bool,
    strict: bool,
    warn: bool,
}

impl CheckFlags {
    fn new(p: &Parsed, spec: &Spec, status_key: &str) -> CheckFlags {
        CheckFlags {
            ignore_missing: p.has(spec, "ignore-missing"),
            quiet: p.has(spec, "quiet"),
            status: p.has(spec, status_key),
            strict: p.has(spec, "strict"),
            warn: p.has(spec, "warn"),
        }
    }
}

/// One line of a checksum file: the expected digest, the file and whether its name was
/// escaped.
struct Entry {
    algo: Algo,
    sum: Vec<u8>,
    file: Vec<u8>,
    escaped: bool,
}

fn parse_hex(s: &[u8]) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    s.chunks(2)
        .map(|c| {
            let d = |b: u8| (b as char).to_digit(16);
            Some((d(c[0])? * 16 + d(c[1])?) as u8)
        })
        .collect()
}

/// The algorithm of a hex digest of this many digits, for shasum.
fn algo_of_len(digits: usize) -> Option<Algo> {
    Some(match digits {
        40 => Algo::Sha1,
        56 => Algo::Sha224,
        64 => Algo::Sha256,
        96 => Algo::Sha384,
        128 => Algo::Sha512,
        _ => return None,
    })
}

/// Parses a check line; `algo` is fixed (GNU names) or taken from the line (shasum).
fn parse_line(line: &[u8], algo: Option<Algo>) -> Option<Entry> {
    let mut line = line;
    while let [b' ' | b'\t', rest @ ..] = line {
        line = rest;
    }
    let escaped = line.first() == Some(&b'\\');
    if escaped {
        line = &line[1..];
    }
    let unescape = |name: &[u8]| -> Option<Vec<u8>> {
        if escaped {
            unescape_name(name)
        } else {
            Some(name.to_vec())
        }
    };
    // `ALG (FILE) = HASH`, also written `ALG(FILE)= HASH`.
    if let Some(open) = line.iter().position(|&b| b == b'(') {
        let name = line[..open].strip_suffix(b" ").unwrap_or(&line[..open]);
        let tagged = [
            Algo::Md5,
            Algo::Sha1,
            Algo::Sha224,
            Algo::Sha256,
            Algo::Sha384,
            Algo::Sha512,
        ]
        .into_iter()
        .find(|a| a.tag().as_bytes() == name);
        if let Some(a) = tagged
            && algo.is_none_or(|x| x == a)
            && let Some(close) = line.windows(2).rposition(|w| w == b")=" || w == b") ")
        {
            let mut rest = &line[close + 1..];
            rest = rest.strip_prefix(b" ").unwrap_or(rest);
            if let Some(r) = rest.strip_prefix(b"=") {
                let r = r.strip_prefix(b" ").unwrap_or(r);
                let sum = parse_hex(r)?;
                if sum.len() != a.len() {
                    return None;
                }
                return Some(Entry {
                    algo: a,
                    sum,
                    file: unescape(&line[open + 1..close])?,
                    escaped,
                });
            }
        }
    }
    let digits = line.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    let a = match algo {
        Some(a) => a,
        None => algo_of_len(digits)?,
    };
    if digits != a.len() * 2 {
        return None;
    }
    let sum = parse_hex(&line[..digits])?;
    let rest = &line[digits..];
    let name = match rest {
        [b' ', b' ' | b'*' | b'^' | b'U', name @ ..] if !name.is_empty() => name,
        [b' ', name @ ..] if !name.is_empty() => name,
        _ => return None,
    };
    Some(Entry {
        algo: a,
        sum,
        file: unescape(name)?,
        escaped,
    })
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Verifies the checksum file `list`: 0 when every listed file matched.
async fn verify(
    ctx: &mut UtilCtx<'_>,
    algo: Option<Algo>,
    list: &[u8],
    f: &CheckFlags,
    dialect: Dialect,
) -> i32 {
    let gnu = dialect == Dialect::Gnu;
    let shown_list = if list == b"-" {
        if gnu {
            quotef(b"standard input")
        } else {
            "standard input".to_string()
        }
    } else if gnu {
        quotef(list)
    } else {
        String::from_utf8_lossy(list).into_owned()
    };
    let mut text = Vec::new();
    let read = match Input::open(ctx, list).await {
        Ok(input) => input.read_to_end(ctx, &mut text).await,
        Err(e) => Err(e),
    };
    if let Err(e) = read {
        file_error(ctx, list, &e, gnu).await;
        return 1;
    }
    let (mut bad_lines, mut unreadable, mut mismatched, mut good, mut verified) = (0, 0, 0, 0, 0);
    let body = text.strip_suffix(b"\n").unwrap_or(&text);
    let lines = if text.is_empty() {
        Vec::new()
    } else {
        body.split(|&b| b == b'\n').collect()
    };
    for (n, raw) in lines.into_iter().enumerate() {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        let trimmed = line.trim_ascii_start();
        if trimmed.is_empty() || trimmed.starts_with(b"#") {
            continue;
        }
        let Some(entry) = parse_line(line, algo) else {
            bad_lines += 1;
            if f.warn {
                let alg = match (dialect, algo) {
                    (Dialect::Gnu, Some(a)) => a.tag(),
                    _ => "SHA",
                };
                ctx.error(&format!(
                    "{shown_list}: {}: improperly formatted {alg} checksum line",
                    n + 1
                ))
                .await;
            }
            continue;
        };
        good += 1;
        let mut shown = Vec::new();
        if entry.escaped {
            shown.push(b'\\');
            shown.extend_from_slice(&escape_name(&entry.file).unwrap_or(entry.file.clone()));
        } else {
            shown.extend_from_slice(&entry.file);
        }
        let result = match digest_of(ctx, entry.algo, &entry.file).await {
            Ok(sum) => Some(sum == entry.sum),
            Err(e) if f.ignore_missing && e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                if !f.status {
                    file_error(ctx, &entry.file, &e, gnu).await;
                }
                None
            }
        };
        let verdict: &[u8] = match result {
            Some(true) => {
                verified += 1;
                if f.quiet || f.status {
                    continue;
                }
                b": OK\n"
            }
            Some(false) => {
                verified += 1;
                mismatched += 1;
                b": FAILED\n"
            }
            None => {
                unreadable += 1;
                b": FAILED open or read\n"
            }
        };
        if f.status {
            continue;
        }
        shown.extend_from_slice(verdict);
        if let Err(e) = ctx.write(&shown).await {
            return common::write_failed(ctx, &e).await;
        }
        ctx.check().await;
    }
    if good == 0 {
        let what = if gnu { "checksum" } else { "SHA checksum" };
        ctx.error(&format!(
            "{shown_list}: no properly formatted {what} lines found"
        ))
        .await;
        return 1;
    }
    if !f.status {
        if bad_lines > 0 {
            let lines = plural(bad_lines, "line is", "lines are");
            ctx.error(&format!("WARNING: {lines} improperly formatted"))
                .await;
        }
        if unreadable > 0 {
            let files = plural(unreadable, "listed file", "listed files");
            ctx.error(&format!("WARNING: {files} could not be read"))
                .await;
        }
        if mismatched > 0 {
            let sums = plural(mismatched, "computed checksum", "computed checksums");
            ctx.error(&format!("WARNING: {sums} did NOT match")).await;
        }
    }
    if f.ignore_missing && verified == 0 && unreadable == 0 {
        if !f.status {
            ctx.error(&format!("{shown_list}: no file was verified"))
                .await;
        }
        return 1;
    }
    let failed = unreadable > 0 || mismatched > 0 || (f.strict && bad_lines > 0);
    i32::from(failed)
}

/// BSD `md5`.
pub async fn run_md5(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &MD5_SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let algo = Algo::Md5;
    let quiet = p.has(&MD5_SPEC, "quiet");
    let reverse = p.has(&MD5_SPEC, "reverse");
    let expect = p.last(&MD5_SPEC, "check").map(<[u8]>::to_ascii_lowercase);
    let mut out = Vec::new();
    // A mismatch with `-c` is 2, an unreadable file 1.
    let mut status = 0;
    let verdict = |sum: &str, out: &mut Vec<u8>, status: &mut i32| {
        if let Some(want) = &expect
            && want.as_slice() != sum.as_bytes()
        {
            if !quiet {
                out.extend_from_slice(b" [ Failed ]");
            }
            *status = 2;
        }
        out.push(b'\n');
    };
    if p.has(&MD5_SPEC, "passthrough") {
        let mut data = Vec::new();
        if let Err(e) = ctx.read_to_end(&mut data).await {
            ctx.error(&format!("stdin: {}", psh_engine::host::error_text(&e)))
                .await;
            return 1;
        }
        out.extend_from_slice(&data);
        let sum = hex(&crate::sum_hash::digest(algo, &data));
        out.extend_from_slice(sum.as_bytes());
        verdict(&sum, &mut out, &mut status);
    } else if p.has(&MD5_SPEC, "string") {
        for (k, v) in p.iter(&MD5_SPEC) {
            if k != "string" {
                continue;
            }
            let s = v.unwrap_or_default();
            let sum = hex(&crate::sum_hash::digest(algo, s));
            out.extend_from_slice(sum.as_bytes());
            if reverse && !quiet {
                out.push(b' ');
                out.extend_from_slice(s);
            }
            verdict(&sum, &mut out, &mut status);
        }
    } else if p.operands.is_empty() {
        match digest_of(ctx, algo, b"-").await {
            Ok(sum) => {
                let sum = hex(&sum);
                out.extend_from_slice(sum.as_bytes());
                verdict(&sum, &mut out, &mut status);
            }
            Err(e) => {
                common::file_error(ctx, b"stdin", &e).await;
                return 1;
            }
        }
    } else {
        for name in &p.operands {
            let sum = match digest_of(ctx, algo, name).await {
                Ok(s) => hex(&s),
                Err(e) => {
                    common::file_error(ctx, name, &e).await;
                    status = status.max(1);
                    continue;
                }
            };
            if quiet {
                out.extend_from_slice(sum.as_bytes());
            } else if reverse {
                out.extend_from_slice(sum.as_bytes());
                out.push(b' ');
                out.extend_from_slice(name);
            } else {
                out.extend_from_slice(b"MD5 (");
                out.extend_from_slice(name);
                out.extend_from_slice(b") = ");
                out.extend_from_slice(sum.as_bytes());
            }
            verdict(&sum, &mut out, &mut status);
            if let Err(e) = ctx.write(&std::mem::take(&mut out)).await {
                return common::write_failed(ctx, &e).await;
            }
        }
    }
    if let Err(e) = ctx.write(&out).await {
        return common::write_failed(ctx, &e).await;
    }
    status
}

/// perl's `shasum`.
pub async fn run_shasum(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SHASUM_SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let algo = match p.last(&SHASUM_SPEC, "algorithm") {
        None => Algo::Sha1,
        Some(a) => match a {
            b"1" => Algo::Sha1,
            b"224" => Algo::Sha224,
            b"256" => Algo::Sha256,
            b"384" => Algo::Sha384,
            b"512" => Algo::Sha512,
            b"512224" => Algo::Sha512_224,
            b"512256" => Algo::Sha512_256,
            _ => {
                ctx.error("Unrecognized algorithm").await;
                ctx.error_raw(b"Type shasum -h for help\n").await;
                return 1;
            }
        },
    };
    let names = common::operands_or_stdin(&p);
    if p.has(&SHASUM_SPEC, "check") {
        let flags = CheckFlags::new(&p, &SHASUM_SPEC, "status");
        let mut status = 0;
        for name in names {
            let st = verify(ctx, None, &name, &flags, Dialect::Shasum).await;
            status = status.max(st);
        }
        return status;
    }
    let binary = p
        .iter(&SHASUM_SPEC)
        .filter(|(k, _)| k == "binary" || k == "text")
        .last()
        .is_some_and(|(k, _)| k == "binary");
    let tag = p.has(&SHASUM_SPEC, "tag");
    print_sums(ctx, algo, &names, binary, tag, false, false).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let h = "b1946ac92492d2347c6235b4d2611184";
        let e = parse_line(format!("{h}  a").as_bytes(), Some(Algo::Md5)).unwrap();
        assert_eq!((e.file.as_slice(), e.escaped), (&b"a"[..], false));
        assert!(parse_line(format!("{h} *a b").as_bytes(), Some(Algo::Md5)).is_some());
        assert!(parse_line(format!("MD5 (a) = {h}").as_bytes(), Some(Algo::Md5)).is_some());
        assert!(parse_line(format!("MD5(a)= {h}").as_bytes(), Some(Algo::Md5)).is_some());
        assert!(parse_line(format!("SHA1 (a) = {h}").as_bytes(), Some(Algo::Md5)).is_none());
        assert!(parse_line(b"junk", Some(Algo::Md5)).is_none());
        let e = parse_line(format!("\\{h}  a\\nb").as_bytes(), Some(Algo::Md5)).unwrap();
        assert_eq!(e.file, b"a\nb");
        // shasum: the algorithm from the length.
        let sha256 = "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03";
        let e = parse_line(format!("{sha256}  a").as_bytes(), None).unwrap();
        assert_eq!(e.algo, Algo::Sha256);
    }

    #[test]
    fn quoting() {
        assert_eq!(quotef(b"a.txt"), "a.txt");
        assert_eq!(quotef(b"standard input"), "'standard input'");
        assert_eq!(quotef(b"it's"), "'it'\\''s'");
    }
}

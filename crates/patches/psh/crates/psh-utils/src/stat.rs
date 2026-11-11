//! `stat`: GNU's `-c/--format`, `--printf`, `-t` and default output, and BSD's `-f FORMAT`
//! with `-t TIMEFMT`, `-n`, `-q`; `-L` both. D016: `-f` with an argument containing `%`
//! is BSD's format; `-f` before plain file names is GNU's file-system mode, which falls
//! through to the real `stat`. `-t` is BSD's time format when a BSD format is given (or
//! its argument has a `%`), else GNU's terse output. Written for psh after the manuals;
//! the BSD formatter is psh's own (D1153).

use psh_engine::host::{ExtraMetadata, FileKind, Metadata, error_text};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::s;
use crate::modes::mode_string;
use crate::opts::{Spec, both, flag, long};
use crate::timefmt::{self, NS};

pub const SPEC: Spec = Spec {
    name: "stat",
    opts: &[
        both('L', "dereference"),
        both('c', "format").value(),
        long("printf").value(),
        flag('f').value(),
        long("file-system"),
        flag('t').value(),
        long("terse"),
        flag('n'),
        flag('q'),
    ],
    posix_order: false,
};

struct Info {
    name: Vec<u8>,
    m: Metadata,
    x: ExtraMetadata,
    target: Option<Vec<u8>>,
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let bsd = p
        .last(&SPEC, "f")
        .filter(|f| f.contains(&b'%'))
        .map(<[u8]>::to_vec);
    if p.has(&SPEC, "file-system") || (p.has(&SPEC, "f") && bsd.is_none()) {
        // GNU's file-system status needs statfs, which the host does not offer.
        if let Some(st) = ctx.run_program(argv.to_vec()).await {
            return st;
        }
        ctx.error("file system status (-f) is not supported (psh built-in stat)")
            .await;
        return 2;
    }
    let mut timefmt = None;
    let mut terse = p.has(&SPEC, "terse");
    if let Some(t) = p.last(&SPEC, "t").map(<[u8]>::to_vec) {
        if bsd.is_some() || t.contains(&b'%') {
            timefmt = Some(t);
        } else {
            // GNU's -t takes no value: what followed is a file.
            terse = true;
            let idx = p
                .opts
                .iter()
                .rposition(|f| SPEC.opts[f.index].key() == "t")
                .expect("present");
            p.opts.remove(idx);
            p.operands.insert(0, t);
        }
    }
    if p.operands.is_empty() {
        return common::usage(ctx, "missing operand").await;
    }
    let follow = p.has(&SPEC, "dereference");
    let gnu_fmt = p
        .opts
        .iter()
        .rev()
        .find_map(|f| match SPEC.opts[f.index].key().as_str() {
            "format" => Some((f.value.clone().unwrap_or_default(), true)),
            "printf" => Some((f.value.clone().unwrap_or_default(), false)),
            _ => None,
        });
    let quiet = p.has(&SPEC, "q");
    let no_newline = p.has(&SPEC, "n");
    let mut status = 0;
    for name in &p.operands {
        let (m, x) = match ctx.stat_extra(name, follow).await {
            Ok(v) => v,
            Err(e) => {
                if bsd.is_some() || timefmt.is_some() {
                    if !quiet {
                        ctx.error(&format!("{}: stat: {}", s(name), error_text(&e)))
                            .await;
                    }
                } else {
                    ctx.error(&format!("cannot stat '{}': {}", s(name), error_text(&e)))
                        .await;
                }
                status = 1;
                continue;
            }
        };
        let target = if m.kind == FileKind::Symlink {
            ctx.read_link(name).await.ok()
        } else {
            None
        };
        let info = Info {
            name: name.clone(),
            m,
            x,
            target,
        };
        let out = if let Some(f) = &bsd {
            match bsd_format(ctx, f, &info, timefmt.as_deref()) {
                Ok(mut o) => {
                    if !no_newline {
                        o.push(b'\n');
                    }
                    o
                }
                Err(bad) => {
                    ctx.error(&format!("{}: bad format", s(&bad))).await;
                    return 1;
                }
            }
        } else if let Some((f, newline)) = &gnu_fmt {
            let mut o = gnu_format(ctx, f, &info, !newline);
            if *newline {
                o.push(b'\n');
            }
            o
        } else if terse {
            gnu_format(
                ctx,
                b"%n %s %b %f %u %g %D %i %h %t %T %X %Y %Z %W %o\n",
                &info,
                false,
            )
        } else {
            // The name as GNU's default output quotes it: only when it must.
            let esc = |n: &[u8]| -> Vec<u8> {
                shell_quote(n)
                    .into_iter()
                    .flat_map(|b| if b == b'%' { vec![b'%', b'%'] } else { vec![b] })
                    .collect()
            };
            let mut f: Vec<u8> = b"  File: ".to_vec();
            f.extend(esc(name));
            if let Some(t) = &info.target {
                f.extend_from_slice(b" -> ");
                f.extend(esc(t));
            }
            f.extend_from_slice(b"\n  Size: %-10s\tBlocks: %-10b IO Block: %-6o %F\n");
            if matches!(m.kind, FileKind::CharDevice | FileKind::BlockDevice) {
                f.extend_from_slice(
                    b"Device: %Hd,%Ld\tInode: %-11i Links: %-5h Device type: %Hr,%Lr\n",
                );
            } else {
                f.extend_from_slice(b"Device: %Hd,%Ld\tInode: %-11i Links: %h\n");
            }
            f.extend_from_slice(
                b"Access: (%04a/%10.10A)  Uid: (%5u/%8U)   Gid: (%5g/%8G)\n\
                  Access: %x\nModify: %y\nChange: %z\n Birth: %w\n",
            );
            gnu_format(ctx, &f, &info, false)
        };
        if let Err(e) = ctx.write(&out).await {
            return common::write_failed(ctx, &e).await;
        }
    }
    status
}

/// A name quoted only when the shell would need it (`'a b'`), as GNU's default output.
fn shell_quote(n: &[u8]) -> Vec<u8> {
    let plain = !n.is_empty()
        && n.iter()
            .all(|&b| b.is_ascii_alphanumeric() || b"-_./+,:@%^=".contains(&b) || b >= 0x80);
    if plain {
        return n.to_vec();
    }
    let mut out = vec![b'\''];
    for &b in n {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

fn file_type(m: &Metadata) -> &'static str {
    match m.kind {
        FileKind::File if m.size == 0 => "regular empty file",
        FileKind::File => "regular file",
        FileKind::Dir => "directory",
        FileKind::Symlink => "symbolic link",
        FileKind::Fifo => "fifo",
        FileKind::Socket => "socket",
        FileKind::CharDevice => "character special file",
        FileKind::BlockDevice => "block special file",
        FileKind::Other => "weird file",
    }
}

fn type_bits(kind: FileKind) -> u32 {
    match kind {
        FileKind::Fifo => 0o010000,
        FileKind::CharDevice => 0o020000,
        FileKind::Dir => 0o040000,
        FileKind::BlockDevice => 0o060000,
        FileKind::File | FileKind::Other => 0o100000,
        FileKind::Symlink => 0o120000,
        FileKind::Socket => 0o140000,
    }
}

fn dev_parts(dev: u64) -> (u64, u64) {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        ((dev >> 24) & 0xff, dev & 0xff_ffff)
    } else {
        (
            ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0xfff),
            ((dev >> 12) & 0xffff_ff00) | (dev & 0xff),
        )
    }
}

fn human_time(ctx: &UtilCtx<'_>, ns: i64) -> Vec<u8> {
    timefmt::strftime(b"%Y-%m-%d %H:%M:%S.%N %z", &timefmt::local(ctx, ns))
}

/// `%.N` on an epoch time: seconds with N digits of the fraction (GNU).
fn epoch(ns: i64, prec: Option<usize>) -> String {
    let secs = ns.div_euclid(NS);
    match prec {
        Some(p) if p > 0 => {
            let frac = format!("{:09}", ns.rem_euclid(NS));
            let mut f = frac[..p.min(9)].to_string();
            while f.len() < p {
                f.push('0');
            }
            format!("{secs}.{f}")
        }
        _ => secs.to_string(),
    }
}

/// GNU's `printf`-like directives with flags (`-0# +'`), width and precision.
fn gnu_format(ctx: &UtilCtx<'_>, f: &[u8], info: &Info, escapes: bool) -> Vec<u8> {
    let (m, x) = (&info.m, &info.x);
    let mut out = Vec::new();
    let mut i = 0;
    while i < f.len() {
        let c = f[i];
        i += 1;
        if escapes && c == b'\\' && i < f.len() {
            let (b, used) = escape(&f[i..]);
            out.extend_from_slice(&b);
            i += used;
            continue;
        }
        if c != b'%' {
            out.push(c);
            continue;
        }
        let start = i;
        while i < f.len() && b"-0# +'".contains(&f[i]) {
            i += 1;
        }
        let flags = &f[start..i];
        let ws = i;
        while i < f.len() && f[i].is_ascii_digit() {
            i += 1;
        }
        let width: usize = std::str::from_utf8(&f[ws..i])
            .ok()
            .and_then(|w| w.parse().ok())
            .unwrap_or(0);
        let mut prec = None;
        if i < f.len() && f[i] == b'.' {
            i += 1;
            let ps = i;
            while i < f.len() && f[i].is_ascii_digit() {
                i += 1;
            }
            prec = Some(
                std::str::from_utf8(&f[ps..i])
                    .ok()
                    .and_then(|w| w.parse().ok())
                    .unwrap_or(0),
            );
        }
        let Some(&d) = f.get(i) else {
            out.push(b'%');
            out.extend_from_slice(&f[start..]);
            break;
        };
        i += 1;
        let mut d = d;
        let mut part = None;
        if (d == b'H' || d == b'L') && matches!(f.get(i), Some(b'd' | b'r')) {
            part = Some(d);
            d = f[i];
            i += 1;
        }
        let user = || {
            ctx.user_name(m.uid).map_or("UNKNOWN".into(), |n| {
                String::from_utf8_lossy(&n).into_owned()
            })
        };
        let group = || {
            ctx.group_name(m.gid).map_or("UNKNOWN".into(), |n| {
                String::from_utf8_lossy(&n).into_owned()
            })
        };
        enum V {
            Num(i128),
            Str(String),
        }
        let v = match d {
            b'%' => {
                out.push(b'%');
                continue;
            }
            b'a' => V::Str(format!("{:o}", m.mode & 0o7777)),
            b'A' => V::Str(mode_string(m.kind, m.mode)),
            b'b' => V::Num(x.blocks.into()),
            b'B' => V::Num(512),
            b'C' => V::Str("?".into()),
            b'd' => match part {
                Some(b'H') => V::Num(dev_parts(m.dev).0.into()),
                Some(_) => V::Num(dev_parts(m.dev).1.into()),
                None => V::Num(m.dev.into()),
            },
            b'D' => V::Str(format!("{:x}", m.dev)),
            b'f' => V::Str(format!("{:x}", type_bits(m.kind) | (m.mode & 0o7777))),
            b'F' => V::Str(file_type(m).into()),
            b'g' => V::Num(m.gid.into()),
            b'G' => V::Str(group()),
            b'h' => V::Num(x.nlink.into()),
            b'i' => V::Num(m.ino.into()),
            b'm' => V::Str("?".into()),
            b'n' => V::Str(String::from_utf8_lossy(&info.name).into_owned()),
            b'N' => {
                let mut t = format!("'{}'", s(&info.name));
                if let Some(target) = &info.target {
                    t.push_str(&format!(" -> '{}'", s(target)));
                }
                V::Str(t)
            }
            b'o' => V::Num(x.blksize.into()),
            b'r' => match part {
                Some(b'H') => V::Num(dev_parts(x.rdev).0.into()),
                Some(_) => V::Num(dev_parts(x.rdev).1.into()),
                None => V::Num(x.rdev.into()),
            },
            b'R' => V::Str(format!("{:x}", x.rdev)),
            b's' => V::Num(m.size.into()),
            b't' => V::Str(format!("{:x}", dev_parts(x.rdev).0)),
            b'T' => V::Str(format!("{:x}", dev_parts(x.rdev).1)),
            b'u' => V::Num(m.uid.into()),
            b'U' => V::Str(user()),
            b'w' => V::Str(match x.btime_ns {
                Some(b) => String::from_utf8_lossy(&human_time(ctx, b)).into_owned(),
                None => "-".into(),
            }),
            b'W' => V::Str(epoch(x.btime_ns.unwrap_or(0), prec.take())),
            b'x' => V::Str(String::from_utf8_lossy(&human_time(ctx, x.atime_ns)).into_owned()),
            b'X' => V::Str(epoch(x.atime_ns, prec.take())),
            b'y' => V::Str(String::from_utf8_lossy(&human_time(ctx, m.mtime_ns)).into_owned()),
            b'Y' => V::Str(epoch(m.mtime_ns, prec.take())),
            b'z' => V::Str(String::from_utf8_lossy(&human_time(ctx, x.ctime_ns)).into_owned()),
            b'Z' => V::Str(epoch(x.ctime_ns, prec.take())),
            other => {
                out.push(b'%');
                out.extend_from_slice(&f[start..i - 1]);
                out.push(other);
                continue;
            }
        };
        let left = flags.contains(&b'-');
        let zero = flags.contains(&b'0') && !left;
        let text = match v {
            V::Num(n) => {
                let mut t = n.to_string();
                if flags.contains(&b'+') && n >= 0 {
                    t.insert(0, '+');
                }
                if let Some(p) = prec
                    && t.len() < p
                {
                    t = format!("{}{t}", "0".repeat(p - t.len()));
                }
                t
            }
            V::Str(mut t) => {
                // `%04a`: octal modes pad with zeros; a precision cuts strings.
                if let Some(p) = prec
                    && t.chars().count() > p
                {
                    t = t.chars().take(p).collect();
                }
                t
            }
        };
        let len = text.chars().count();
        if len >= width {
            out.extend_from_slice(text.as_bytes());
        } else if left {
            out.extend_from_slice(text.as_bytes());
            out.extend(std::iter::repeat_n(b' ', width - len));
        } else {
            let pad = if zero && text.bytes().all(|b| b.is_ascii_digit()) {
                b'0'
            } else {
                b' '
            };
            out.extend(std::iter::repeat_n(pad, width - len));
            out.extend_from_slice(text.as_bytes());
        }
    }
    out
}

/// A backslash escape of `--printf`: the bytes and how many input bytes it used.
fn escape(b: &[u8]) -> (Vec<u8>, usize) {
    let simple = |c: u8| (vec![c], 1);
    match b[0] {
        b'n' => simple(b'\n'),
        b't' => simple(b'\t'),
        b'r' => simple(b'\r'),
        b'a' => simple(7),
        b'b' => simple(8),
        b'f' => simple(12),
        b'v' => simple(11),
        b'e' => simple(27),
        b'\\' => simple(b'\\'),
        b'"' => simple(b'"'),
        b'\'' => simple(b'\''),
        b'0'..=b'7' => {
            let n = b
                .iter()
                .take(3)
                .take_while(|c| (b'0'..=b'7').contains(c))
                .count();
            let v =
                u32::from_str_radix(std::str::from_utf8(&b[..n]).unwrap_or("0"), 8).unwrap_or(0);
            (vec![v as u8], n)
        }
        b'x' => {
            let n = b[1..]
                .iter()
                .take(2)
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            if n == 0 {
                return (b"\\x".to_vec(), 1);
            }
            let v = u8::from_str_radix(std::str::from_utf8(&b[1..1 + n]).unwrap_or("0"), 16)
                .unwrap_or(0);
            (vec![v], 1 + n)
        }
        c => (vec![b'\\', c], 1),
    }
}

/// The BSD formatter: `%[flags][width][.prec][fmt][sub]datum`, flags `#+-0 `, fmt `D O
/// U X F S`, sub `H M L`. `Err(directive)` for one it does not know (BSD: bad format).
fn bsd_format(
    ctx: &UtilCtx<'_>,
    f: &[u8],
    info: &Info,
    timefmt: Option<&[u8]>,
) -> Result<Vec<u8>, Vec<u8>> {
    let (m, x) = (&info.m, &info.x);
    let mut out = Vec::new();
    let mut i = 0;
    while i < f.len() {
        let c = f[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        let start = i - 1;
        let fl = i;
        while i < f.len() && b"#+-0 ".contains(&f[i]) {
            i += 1;
        }
        let flags = f[fl..i].to_vec();
        let ws = i;
        while i < f.len() && f[i].is_ascii_digit() {
            i += 1;
        }
        let width: usize = std::str::from_utf8(&f[ws..i])
            .ok()
            .and_then(|w| w.parse().ok())
            .unwrap_or(0);
        let mut prec = None;
        if f.get(i) == Some(&b'.') {
            i += 1;
            let ps = i;
            while i < f.len() && f[i].is_ascii_digit() {
                i += 1;
            }
            prec = std::str::from_utf8(&f[ps..i])
                .ok()
                .and_then(|w| w.parse::<usize>().ok());
        }
        let mut fmt = None;
        if let Some(&c) = f.get(i)
            && b"DOUXFS".contains(&c)
            && f.get(i + 1)
                .is_some_and(|n| b"HMLdipluagrmcBzbkfvNTYZ".contains(n))
        {
            fmt = Some(c);
            i += 1;
        }
        let mut sub = None;
        if let Some(&c) = f.get(i)
            && b"HML".contains(&c)
            && f.get(i + 1)
                .is_some_and(|n| b"dipluagrmcBzbkfvNTYZ".contains(n))
        {
            sub = Some(c);
            i += 1;
        }
        let Some(&d) = f.get(i) else {
            return Err(f[start..].to_vec());
        };
        i += 1;
        let bad = || f[start..i].to_vec();
        enum V {
            Num(u64),
            Str(Vec<u8>),
            Time(i64),
        }
        let v = match d {
            b'%' => V::Str(b"%".to_vec()),
            b'n' => V::Str(b"\n".to_vec()),
            b't' => V::Str(b"\t".to_vec()),
            b'd' => match sub {
                Some(b'H') => V::Num(dev_parts(m.dev).0),
                Some(b'L') => V::Num(dev_parts(m.dev).1),
                _ if fmt == Some(b'S') => V::Str(b"??".to_vec()),
                _ => V::Num(m.dev),
            },
            b'r' => match sub {
                Some(b'H') => V::Num(dev_parts(x.rdev).0),
                Some(b'L') => V::Num(dev_parts(x.rdev).1),
                _ if fmt == Some(b'S') => V::Str(b"??".to_vec()),
                _ => V::Num(x.rdev),
            },
            b'i' => V::Num(m.ino),
            b'l' => V::Num(x.nlink),
            b'u' if fmt == Some(b'S') => V::Str(
                ctx.user_name(m.uid)
                    .unwrap_or_else(|| m.uid.to_string().into_bytes()),
            ),
            b'u' => V::Num(m.uid.into()),
            b'g' if fmt == Some(b'S') => V::Str(
                ctx.group_name(m.gid)
                    .unwrap_or_else(|| m.gid.to_string().into_bytes()),
            ),
            b'g' => V::Num(m.gid.into()),
            b'p' => {
                let full = type_bits(m.kind) | (m.mode & 0o7777);
                if fmt == Some(b'S') {
                    let ms = mode_string(m.kind, m.mode).into_bytes();
                    V::Str(match sub {
                        Some(b'H') => ms[1..4].to_vec(),
                        Some(b'M') => ms[4..7].to_vec(),
                        Some(b'L') => ms[7..10].to_vec(),
                        _ => ms,
                    })
                } else {
                    V::Num(u64::from(match sub {
                        Some(b'H') => full >> 12,
                        Some(b'M') => (full >> 9) & 0o7,
                        Some(b'L') => full & 0o777,
                        _ => full,
                    }))
                }
            }
            b'a' => V::Time(x.atime_ns),
            b'm' => V::Time(m.mtime_ns),
            b'c' => V::Time(x.ctime_ns),
            b'B' => V::Time(x.btime_ns.unwrap_or(0)),
            b'z' => V::Num(m.size),
            b'b' => V::Num(x.blocks),
            b'k' => V::Num(x.blksize),
            b'f' | b'v' => V::Num(0),
            b'N' => V::Str(info.name.clone()),
            b'T' => V::Str(if sub == Some(b'H') {
                match m.kind {
                    FileKind::File | FileKind::Other => &b"Regular File"[..],
                    FileKind::Dir => b"Directory",
                    FileKind::Symlink => b"Symbolic Link",
                    FileKind::Fifo => b"Fifo File",
                    FileKind::Socket => b"Socket",
                    FileKind::CharDevice => b"Character Device",
                    FileKind::BlockDevice => b"Block Device",
                }
                .to_vec()
            } else {
                match m.kind {
                    FileKind::Dir => b"/".to_vec(),
                    FileKind::Symlink => b"@".to_vec(),
                    FileKind::Fifo => b"|".to_vec(),
                    FileKind::Socket => b"=".to_vec(),
                    FileKind::File if m.mode & 0o111 != 0 => b"*".to_vec(),
                    _ => Vec::new(),
                }
            }),
            b'Y' => V::Str(match &info.target {
                Some(t) if fmt == Some(b'S') => [&b" -> "[..], t].concat(),
                Some(t) => t.clone(),
                None => Vec::new(),
            }),
            b'Z' => {
                if matches!(m.kind, FileKind::CharDevice | FileKind::BlockDevice) {
                    let (a, b) = dev_parts(x.rdev);
                    V::Str(format!("{a},{b}").into_bytes())
                } else {
                    V::Num(m.size)
                }
            }
            _ => return Err(bad()),
        };
        let alt = flags.contains(&b'#');
        let text: Vec<u8> = match v {
            V::Str(t) => match prec {
                Some(p) if t.len() > p => t[..p].to_vec(),
                _ => t,
            },
            V::Num(n) => {
                let default = if d == b'p' { b'O' } else { b'D' };
                match fmt.unwrap_or(default) {
                    b'O' => format!("{}{n:o}", if alt && n != 0 { "0" } else { "" }).into_bytes(),
                    b'X' => format!("{}{n:x}", if alt { "0x" } else { "" }).into_bytes(),
                    b'S' => n.to_string().into_bytes(),
                    _ => {
                        let plus = if flags.contains(&b'+') { "+" } else { "" };
                        format!("{plus}{n}").into_bytes()
                    }
                }
            }
            V::Time(ns) => match fmt {
                Some(b'S') => {
                    let tf = timefmt.unwrap_or(b"%b %e %H:%M:%S %Y");
                    timefmt::strftime(tf, &timefmt::local(ctx, ns))
                }
                Some(b'F') => {
                    let p = prec.unwrap_or(9).min(9);
                    epoch(ns, Some(p)).into_bytes()
                }
                _ => ns.div_euclid(NS).to_string().into_bytes(),
            },
        };
        let len = text.len();
        if len >= width {
            out.extend_from_slice(&text);
        } else if flags.contains(&b'-') {
            out.extend_from_slice(&text);
            out.extend(std::iter::repeat_n(b' ', width - len));
        } else {
            let zero = flags.contains(&b'0') && text.iter().all(u8::is_ascii_digit);
            out.extend(std::iter::repeat_n(
                if zero { b'0' } else { b' ' },
                width - len,
            ));
            out.extend_from_slice(&text);
        }
    }
    Ok(out)
}

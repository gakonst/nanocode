//! `ls`: GNU's output (long format, columns with GNU's tab stops, `-m`, `-R`, sizes,
//! times, `--time-style`), sorted by byte order; BSD's `-G` (color: psh prints none, as
//! macOS does off a terminal) and `-T` (full times in the long format) per D1152.
//! Names are printed as they are (GNU's quoting is for terminals). Written for psh after
//! the manuals.

use std::io;

use psh_engine::host::{ExtraMetadata, FileKind, Metadata, error_text};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{human, join, s};
use crate::modes::mode_string;
use crate::opts::{Spec, both, flag, long};
use crate::timefmt::{self, NS};

pub const SPEC: Spec = Spec {
    name: "ls",
    opts: &[
        both('a', "all"),
        both('A', "almost-all"),
        flag('l'),
        both('h', "human-readable"),
        long("si"),
        flag('t'),
        flag('S'),
        flag('U'),
        flag('f'),
        flag('v'),
        flag('X'),
        both('r', "reverse"),
        both('d', "directory"),
        both('R', "recursive"),
        flag('1'),
        flag('C'),
        flag('x'),
        flag('m'),
        both('F', "classify").optional(),
        flag('p'),
        both('i', "inode"),
        both('s', "size"),
        both('L', "dereference"),
        both('H', "dereference-command-line"),
        both('n', "numeric-uid-gid"),
        flag('g'),
        flag('o'),
        flag('G'),
        flag('T'),
        flag('c'),
        flag('u'),
        both('k', "kibibytes"),
        both('B', "ignore-backups"),
        both('I', "ignore").value(),
        long("hide").value(),
        both('w', "width").value(),
        long("sort").value(),
        long("time").value(),
        long("time-style").value(),
        long("full-time"),
        long("color").optional(),
        long("group-directories-first"),
        both('N', "literal"),
        both('q', "hide-control-chars"),
    ],
    posix_order: false,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Time,
    Size,
    None,
    Version,
    Ext,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    Long,
    One,
    Columns,
    Across,
    Commas,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Mtime,
    Ctime,
    Atime,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hidden {
    No,
    Almost,
    All,
}

struct Opts {
    hidden: Hidden,
    layout: Layout,
    sort: Sort,
    reverse: bool,
    directory: bool,
    recursive: bool,
    classify: bool,
    slash: bool,
    inode: bool,
    size: bool,
    deref: bool,
    deref_args: bool,
    numeric: bool,
    owner: bool,
    group: bool,
    human: Option<u64>,
    time: Which,
    style: Style,
    ignore: Vec<Vec<u8>>,
    hide: Vec<Vec<u8>>,
    width: usize,
    dirs_first: bool,
}

#[derive(Clone)]
enum Style {
    Locale,
    BsdFull,
    /// (recent, old) formats.
    Format(Vec<u8>, Vec<u8>),
}

struct Entry {
    /// What is printed.
    name: Vec<u8>,
    /// What is stat'ed.
    path: Vec<u8>,
    meta: Option<(Metadata, ExtraMetadata)>,
    target: Option<Vec<u8>>,
    /// What a symlink points to, for -F and following.
    target_meta: Option<Metadata>,
}

impl Entry {
    fn kind(&self) -> Option<FileKind> {
        self.meta.as_ref().map(|(m, _)| m.kind)
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let tty = ctx.is_terminal(1);
    let env_width = ctx
        .var(b"COLUMNS")
        .and_then(|w| std::str::from_utf8(w).ok()?.parse::<usize>().ok())
        .filter(|&w| w > 0);
    let mut o = Opts {
        hidden: Hidden::No,
        layout: if tty { Layout::Columns } else { Layout::One },
        sort: Sort::Name,
        reverse: false,
        directory: false,
        recursive: false,
        classify: false,
        slash: false,
        inode: false,
        size: false,
        deref: false,
        deref_args: false,
        numeric: false,
        owner: true,
        group: true,
        human: None,
        time: Which::Mtime,
        style: Style::Locale,
        ignore: Vec::new(),
        hide: Vec::new(),
        width: env_width.unwrap_or(80),
        dirs_first: false,
    };
    let mut sort_time = false;
    for (k, v) in p.iter(&SPEC) {
        let v = v.unwrap_or_default();
        match k.as_str() {
            "all" => o.hidden = Hidden::All,
            "almost-all" => o.hidden = Hidden::Almost,
            "l" => o.layout = Layout::Long,
            "human-readable" => o.human = Some(1024),
            "si" => o.human = Some(1000),
            "t" => o.sort = Sort::Time,
            "S" => o.sort = Sort::Size,
            "U" => o.sort = Sort::None,
            "f" => {
                o.sort = Sort::None;
                o.hidden = Hidden::All;
            }
            "v" => o.sort = Sort::Version,
            "X" => o.sort = Sort::Ext,
            "reverse" => o.reverse = true,
            "directory" => o.directory = true,
            "recursive" => o.recursive = true,
            "1" => o.layout = Layout::One,
            "C" => o.layout = Layout::Columns,
            "x" => o.layout = Layout::Across,
            "m" => o.layout = Layout::Commas,
            "classify" => {
                o.classify = match v {
                    b"" | b"always" | b"yes" | b"force" => true,
                    b"never" | b"no" | b"none" => false,
                    b"auto" | b"tty" | b"if-tty" => tty,
                    w => return bad_arg(ctx, w, "--classify").await,
                }
            }
            "p" => o.slash = true,
            "inode" => o.inode = true,
            "size" => o.size = true,
            "dereference" => o.deref = true,
            "dereference-command-line" => o.deref_args = true,
            "numeric-uid-gid" => {
                o.numeric = true;
                o.layout = Layout::Long;
            }
            "g" => {
                o.owner = false;
                o.layout = Layout::Long;
            }
            "o" => {
                o.group = false;
                o.layout = Layout::Long;
            }
            // BSD: color (D1152). psh prints none, as macOS ls off a terminal.
            "G" => {}
            // BSD: full times (D1152).
            "T" => o.style = Style::BsdFull,
            "c" => {
                o.time = Which::Ctime;
                sort_time = true;
            }
            "u" => {
                o.time = Which::Atime;
                sort_time = true;
            }
            "kibibytes" => {}
            "ignore-backups" => {
                o.ignore.push(b"*~".to_vec());
                o.ignore.push(b".*~".to_vec());
            }
            "ignore" => o.ignore.push(v.to_vec()),
            "hide" => o.hide.push(v.to_vec()),
            "width" => match std::str::from_utf8(v)
                .ok()
                .and_then(|w| w.parse::<usize>().ok())
            {
                Some(0) => o.width = usize::MAX,
                Some(w) => o.width = w,
                None => {
                    let msg = format!("invalid line width: '{}'", s(v));
                    ctx.error(&msg).await;
                    return 2;
                }
            },
            "sort" => {
                o.sort = match v {
                    b"none" => Sort::None,
                    b"size" => Sort::Size,
                    b"time" => Sort::Time,
                    b"version" => Sort::Version,
                    b"extension" => Sort::Ext,
                    b"name" => Sort::Name,
                    w => return bad_arg(ctx, w, "--sort").await,
                }
            }
            "time" => {
                o.time = match v {
                    b"atime" | b"access" | b"use" => Which::Atime,
                    b"ctime" | b"status" => Which::Ctime,
                    b"mtime" | b"modification" => Which::Mtime,
                    // Birth times: the real ls.
                    w => return fall_through(ctx, argv, w, "--time").await,
                };
                sort_time = true;
            }
            "time-style" => match parse_style(v) {
                Some(st) => o.style = st,
                None => return bad_arg(ctx, v, "--time-style").await,
            },
            "full-time" => {
                o.layout = Layout::Long;
                o.style = parse_style(b"full-iso").expect("known style");
            }
            "color" => match v {
                b"" | b"never" | b"no" | b"none" | b"auto" | b"tty" | b"if-tty" => {}
                // Colors psh does not print: the real ls, or a clear failure.
                w => return fall_through(ctx, argv, w, "--color").await,
            },
            "group-directories-first" => o.dirs_first = true,
            _ => {}
        }
    }
    // -c/-u sort by that time unless long (GNU: then only with -t).
    if sort_time && o.layout != Layout::Long && o.sort == Sort::Name {
        o.sort = Sort::Time;
    }
    if o.hidden == Hidden::No && !o.hide.is_empty() {
        let hide = std::mem::take(&mut o.hide);
        o.ignore.extend(hide);
    }
    let operands = if p.operands.is_empty() {
        vec![b".".to_vec()]
    } else {
        p.operands.clone()
    };
    let explicit = !p.operands.is_empty();
    let mut status = 0;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for op in operands {
        let follow_arg = o.deref
            || o.deref_args
            || !(o.layout == Layout::Long || o.directory || o.classify || o.slash);
        let meta = match ctx.stat_extra(&op, o.deref || o.deref_args).await {
            Ok(m) => m,
            Err(e) => {
                let msg = format!("cannot access '{}': {}", s(&op), error_text(&e));
                ctx.error(&msg).await;
                status = 2;
                continue;
            }
        };
        let mut e = Entry {
            name: op.clone(),
            path: op.clone(),
            meta: Some(meta),
            target: None,
            target_meta: None,
        };
        if meta.0.kind == FileKind::Symlink {
            e.target = ctx.read_link(&op).await.ok();
            e.target_meta = ctx.stat(&op, true).await.ok();
        }
        // A symlink to a directory on the command line is listed as the directory
        // unless the long format, -d or -F asks for the link (GNU).
        let as_dir = !o.directory
            && match meta.0.kind {
                FileKind::Dir => true,
                FileKind::Symlink => {
                    follow_arg && e.target_meta.is_some_and(|m| m.kind == FileKind::Dir)
                }
                _ => false,
            };
        if as_dir {
            dirs.push(e);
        } else {
            files.push(e);
        }
    }
    sort(&mut files, &o);
    sort(&mut dirs, &o);
    let mut out = Out {
        buf: Vec::new(),
        failed: None,
    };
    let mut first = true;
    if !files.is_empty() {
        let now = now_ns(ctx);
        render(ctx, &mut out, &files, &o, false, now);
        first = false;
    }
    let headers = dirs.len() + files.len() > 1 || o.recursive || (explicit && status != 0);
    let mut queue: std::collections::VecDeque<Vec<u8>> = dirs.into_iter().map(|e| e.name).collect();
    while let Some(dir) = queue.pop_front() {
        ctx.check().await;
        if !first {
            out.buf.push(b'\n');
        }
        first = false;
        if headers {
            out.buf.extend_from_slice(&dir);
            out.buf.extend_from_slice(b":\n");
        }
        let entries = match list_dir(ctx, &dir, &o).await {
            Ok(e) => e,
            Err(e) => {
                flush(ctx, &mut out).await;
                let msg = format!("cannot open directory '{}': {}", s(&dir), error_text(&e));
                ctx.error(&msg).await;
                status = status.max(if explicit { 2 } else { 1 });
                continue;
            }
        };
        let now = now_ns(ctx);
        render(ctx, &mut out, &entries, &o, true, now);
        if o.recursive {
            let subs: Vec<Vec<u8>> = entries
                .iter()
                .filter(|e| e.kind() == Some(FileKind::Dir) && e.name != b"." && e.name != b"..")
                .map(|e| e.path.clone())
                .collect();
            for (i, sub) in subs.into_iter().enumerate() {
                queue.insert(i, sub);
            }
        }
        if out.buf.len() > 64 * 1024 {
            flush(ctx, &mut out).await;
        }
        if let Some(st) = out.failed {
            return st;
        }
    }
    flush(ctx, &mut out).await;
    out.failed.unwrap_or(status)
}

struct Out {
    buf: Vec<u8>,
    failed: Option<i32>,
}

async fn flush(ctx: &mut UtilCtx<'_>, out: &mut Out) {
    if out.failed.is_some() {
        return;
    }
    let data = std::mem::take(&mut out.buf);
    if let Err(e) = ctx.write(&data).await {
        out.failed = Some(common::write_failed(ctx, &e).await);
    }
}

fn now_ns(ctx: &UtilCtx<'_>) -> i64 {
    i64::try_from(ctx.now().as_nanos()).unwrap_or(i64::MAX)
}

async fn bad_arg(ctx: &mut UtilCtx<'_>, w: &[u8], opt: &str) -> i32 {
    let msg = format!("invalid argument '{}' for '{opt}'", s(w));
    ctx.error(&msg).await;
    2
}

async fn fall_through(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>], w: &[u8], opt: &str) -> i32 {
    if let Some(st) = ctx.run_program(argv.to_vec()).await {
        return st;
    }
    let msg = format!(
        "unsupported argument '{}' for '{opt}' (psh built-in ls)",
        s(w)
    );
    ctx.error(&msg).await;
    2
}

fn parse_style(v: &[u8]) -> Option<Style> {
    let v = v.strip_prefix(b"posix-").unwrap_or(v);
    Some(match v {
        b"full-iso" => Style::Format(
            b"%Y-%m-%d %H:%M:%S.%N %z".to_vec(),
            b"%Y-%m-%d %H:%M:%S.%N %z".to_vec(),
        ),
        b"long-iso" => Style::Format(b"%Y-%m-%d %H:%M".to_vec(), b"%Y-%m-%d %H:%M".to_vec()),
        b"iso" => Style::Format(b"%m-%d %H:%M".to_vec(), b"%Y-%m-%d ".to_vec()),
        b"locale" => Style::Locale,
        _ => {
            let f = v.strip_prefix(b"+")?;
            match f.iter().position(|&b| b == b'\n') {
                Some(i) => Style::Format(f[i + 1..].to_vec(), f[..i].to_vec()),
                None => Style::Format(f.to_vec(), f.to_vec()),
            }
        }
    })
}

/// GNU's fnmatch subset for `-I`/`--hide`: `*`, `?`, `[...]`, `\`.
pub fn glob(p: &[u8], n: &[u8]) -> bool {
    let (mut pi, mut ni) = (0, 0);
    let (mut star_p, mut star_n) = (usize::MAX, 0);
    while ni < n.len() {
        if pi < p.len() {
            match p[pi] {
                b'*' => {
                    star_p = pi;
                    star_n = ni;
                    pi += 1;
                    continue;
                }
                b'?' => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                b'[' => {
                    if let Some((matched, len)) = class(&p[pi..], n[ni]) {
                        if matched {
                            pi += len;
                            ni += 1;
                            continue;
                        }
                    } else if n[ni] == b'[' {
                        pi += 1;
                        ni += 1;
                        continue;
                    }
                }
                b'\\' if pi + 1 < p.len() => {
                    if p[pi + 1] == n[ni] {
                        pi += 2;
                        ni += 1;
                        continue;
                    }
                }
                c if c == n[ni] => {
                    pi += 1;
                    ni += 1;
                    continue;
                }
                _ => {}
            }
        }
        if star_p == usize::MAX {
            return false;
        }
        pi = star_p + 1;
        star_n += 1;
        ni = star_n;
    }
    p[pi..].iter().all(|&c| c == b'*')
}

fn class(p: &[u8], c: u8) -> Option<(bool, usize)> {
    let mut i = 1;
    let neg = matches!(p.get(i), Some(b'!' | b'^'));
    if neg {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == b']' && !first {
            return Some((matched != neg, i + 1));
        }
        first = false;
        let lo = p[i];
        if p.get(i + 1) == Some(&b'-') && p.get(i + 2).is_some_and(|&h| h != b']') {
            let hi = p[i + 2];
            if lo <= c && c <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if lo == c {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

async fn list_dir(ctx: &mut UtilCtx<'_>, dir: &[u8], o: &Opts) -> io::Result<Vec<Entry>> {
    let mut names: Vec<Vec<u8>> = ctx
        .read_dir(dir)
        .await?
        .into_iter()
        .map(|e| e.name)
        .collect();
    if o.hidden == Hidden::All {
        names.push(b".".to_vec());
        names.push(b"..".to_vec());
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if o.hidden == Hidden::No && name.first() == Some(&b'.') {
            continue;
        }
        if o.ignore.iter().any(|pat| glob(pat, &name)) {
            continue;
        }
        ctx.check().await;
        let path = join(dir, &name);
        let meta = ctx.stat_extra(&path, o.deref).await.ok();
        let meta = match meta {
            Some(m) => Some(m),
            // A dangling link with -L: the link itself.
            None if o.deref => ctx.stat_extra(&path, false).await.ok(),
            None => None,
        };
        let mut e = Entry {
            name,
            path,
            meta,
            target: None,
            target_meta: None,
        };
        if e.kind() == Some(FileKind::Symlink) {
            e.target = ctx.read_link(&e.path).await.ok();
            e.target_meta = ctx.stat(&e.path, true).await.ok();
        }
        out.push(e);
    }
    sort(&mut out, o);
    Ok(out)
}

fn time_of(m: &Metadata, x: &ExtraMetadata, w: Which) -> i64 {
    match w {
        Which::Mtime => m.mtime_ns,
        Which::Ctime => x.ctime_ns,
        Which::Atime => x.atime_ns,
    }
}

/// GNU's version order (`-v`): runs of digits compare as numbers.
fn version_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let na = a[si..i].iter().skip_while(|&&c| c == b'0').count();
            let nb = b[sj..j].iter().skip_while(|&&c| c == b'0').count();
            let ord = na.cmp(&nb).then_with(|| a[i - na..i].cmp(&b[j - nb..j]));
            if ord.is_ne() {
                return ord;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn sort(entries: &mut [Entry], o: &Opts) {
    use std::cmp::Ordering;
    let ext = |n: &[u8]| -> Vec<u8> {
        match n.iter().rposition(|&b| b == b'.') {
            Some(0) | None => Vec::new(),
            Some(i) => n[i..].to_vec(),
        }
    };
    let cmp = |a: &Entry, b: &Entry| -> Ordering {
        let by_name = || a.name.cmp(&b.name);
        let ord = match o.sort {
            Sort::None => Ordering::Equal,
            Sort::Name => by_name(),
            Sort::Version => version_cmp(&a.name, &b.name).then_with(by_name),
            Sort::Ext => ext(&a.name).cmp(&ext(&b.name)).then_with(by_name),
            Sort::Size => {
                let sa = a.meta.map_or(0, |(m, _)| m.size);
                let sb = b.meta.map_or(0, |(m, _)| m.size);
                sb.cmp(&sa).then_with(by_name)
            }
            Sort::Time => {
                let ta = a.meta.map_or(0, |(m, x)| time_of(&m, &x, o.time));
                let tb = b.meta.map_or(0, |(m, x)| time_of(&m, &x, o.time));
                tb.cmp(&ta).then_with(by_name)
            }
        };
        if o.reverse { ord.reverse() } else { ord }
    };
    if o.sort == Sort::None {
        if o.reverse {
            entries.reverse();
        }
    } else {
        entries.sort_by(cmp);
    }
    if o.dirs_first {
        let is_dir = |e: &Entry| {
            e.kind() == Some(FileKind::Dir)
                || (e.kind() == Some(FileKind::Symlink)
                    && e.target_meta.is_some_and(|m| m.kind == FileKind::Dir))
        };
        entries.sort_by_key(|e| !is_dir(e));
    }
}

fn indicator(e: &Entry, o: &Opts) -> &'static str {
    if !o.classify && !o.slash {
        return "";
    }
    let Some((m, _)) = &e.meta else {
        return "";
    };
    match m.kind {
        FileKind::Dir => "/",
        _ if o.slash && !o.classify => "",
        FileKind::Symlink if o.layout != Layout::Long => "@",
        FileKind::Symlink => "",
        FileKind::Fifo => "|",
        FileKind::Socket => "=",
        FileKind::File if m.mode & 0o111 != 0 => "*",
        _ => "",
    }
}

fn target_indicator(e: &Entry, o: &Opts) -> &'static str {
    if !o.classify {
        return "";
    }
    match e.target_meta {
        Some(m) if m.kind == FileKind::Dir => "/",
        Some(m) if m.kind == FileKind::Fifo => "|",
        Some(m) if m.kind == FileKind::Socket => "=",
        Some(m) if m.kind == FileKind::File && m.mode & 0o111 != 0 => "*",
        _ => "",
    }
}

fn blocks_k(x: &ExtraMetadata) -> u64 {
    x.blocks.div_ceil(2)
}

fn size_text(n: u64, o: &Opts) -> String {
    match o.human {
        Some(base) => human(n, base),
        None => n.to_string(),
    }
}

fn blocks_text(x: &ExtraMetadata, o: &Opts) -> String {
    match o.human {
        Some(base) => human(x.blocks * 512, base),
        None => blocks_k(x).to_string(),
    }
}

fn time_text(ctx: &UtilCtx<'_>, ns: i64, o: &Opts, now: i64) -> Vec<u8> {
    let t = timefmt::local(ctx, ns);
    // Recent: within the last six months (GNU's 31556952 / 2 seconds).
    let half_year = 15_778_476 * NS;
    let recent = ns <= now && ns > now - half_year;
    match &o.style {
        Style::Locale => {
            if recent {
                timefmt::strftime(b"%b %e %H:%M", &t)
            } else {
                timefmt::strftime(b"%b %e  %Y", &t)
            }
        }
        Style::BsdFull => timefmt::strftime(b"%b %e %H:%M:%S %Y", &t),
        Style::Format(r, old) => timefmt::strftime(if recent { r } else { old }, &t),
    }
}

fn pad_left(out: &mut Vec<u8>, text: &[u8], w: usize) {
    out.extend(std::iter::repeat_n(b' ', w.saturating_sub(text.len())));
    out.extend_from_slice(text);
}

fn pad_right(out: &mut Vec<u8>, text: &[u8], w: usize) {
    out.extend_from_slice(text);
    out.extend(std::iter::repeat_n(b' ', w.saturating_sub(text.len())));
}

fn display_width(name: &[u8]) -> usize {
    match std::str::from_utf8(name) {
        Ok(s) => s.chars().count(),
        Err(_) => name.len(),
    }
}

fn render(ctx: &UtilCtx<'_>, out: &mut Out, entries: &[Entry], o: &Opts, is_dir: bool, now: i64) {
    if (o.layout == Layout::Long || o.size) && is_dir {
        {
            let total: u64 = entries
                .iter()
                .filter_map(|e| e.meta)
                .map(|(_, x)| x.blocks)
                .sum();
            out.buf.extend_from_slice(b"total ");
            let text = match o.human {
                Some(base) => human(total * 512, base),
                None => total.div_ceil(2).to_string(),
            };
            out.buf.extend_from_slice(text.as_bytes());
            out.buf.push(b'\n');
        }
    }
    if o.layout == Layout::Long {
        long_format(ctx, out, entries, o, now);
        return;
    }
    // The names with their prefixes (inode, blocks) and indicators.
    let inode_w = entries
        .iter()
        .map(|e| e.meta.map_or(1, |(m, _)| m.ino.to_string().len()))
        .max()
        .unwrap_or(0);
    let blocks_w = entries
        .iter()
        .map(|e| e.meta.map_or(1, |(_, x)| blocks_text(&x, o).len()))
        .max()
        .unwrap_or(0);
    let cells: Vec<Vec<u8>> = entries
        .iter()
        .map(|e| {
            let mut c = Vec::new();
            if o.inode {
                let t = e.meta.map_or("?".into(), |(m, _)| m.ino.to_string());
                pad_left(
                    &mut c,
                    t.as_bytes(),
                    if o.layout == Layout::Commas {
                        0
                    } else {
                        inode_w
                    },
                );
                c.push(b' ');
            }
            if o.size {
                let t = e.meta.map_or("?".into(), |(_, x)| blocks_text(&x, o));
                pad_left(
                    &mut c,
                    t.as_bytes(),
                    if o.layout == Layout::Commas {
                        0
                    } else {
                        blocks_w
                    },
                );
                c.push(b' ');
            }
            c.extend_from_slice(&e.name);
            c.extend_from_slice(indicator(e, o).as_bytes());
            c
        })
        .collect();
    match o.layout {
        Layout::One | Layout::Long => {
            for c in cells {
                out.buf.extend_from_slice(&c);
                out.buf.push(b'\n');
            }
        }
        Layout::Commas => {
            let mut pos = 0;
            for (i, c) in cells.iter().enumerate() {
                let w = display_width(c);
                if i > 0 {
                    if pos + 2 + w < o.width {
                        out.buf.extend_from_slice(b", ");
                        pos += 2;
                    } else {
                        out.buf.extend_from_slice(b",\n");
                        pos = 0;
                    }
                }
                out.buf.extend_from_slice(c);
                pos += w;
            }
            if !cells.is_empty() {
                out.buf.push(b'\n');
            }
        }
        Layout::Columns | Layout::Across => columns(out, &cells, o),
    }
}

/// GNU's grid: as many columns as fit in the width (strictly less), two spaces apart at
/// least, padded with tabs to multiples of 8 where a tab reaches.
fn columns(out: &mut Out, cells: &[Vec<u8>], o: &Opts) {
    let n = cells.len();
    if n == 0 {
        return;
    }
    let widths: Vec<usize> = cells.iter().map(|c| display_width(c)).collect();
    let across = o.layout == Layout::Across;
    let mut best = (1, vec![widths.iter().copied().max().unwrap_or(0)]);
    for cols in (2..=n.min(o.width / 3 + 1)).rev() {
        let rows = n.div_ceil(cols);
        let cols = n.div_ceil(rows);
        let mut colw = vec![0usize; cols];
        for (i, w) in widths.iter().enumerate() {
            let c = if across { i % cols } else { i / rows };
            colw[c] = colw[c].max(*w);
        }
        let total: usize = colw.iter().sum::<usize>() + 2 * (cols - 1);
        if total < o.width {
            best = (cols, colw);
            break;
        }
    }
    let (cols, colw) = best;
    let rows = n.div_ceil(cols);
    for r in 0..rows {
        let mut pos = 0;
        let row: Vec<usize> = (0..cols)
            .map(|c| if across { r * cols + c } else { c * rows + r })
            .filter(|&i| i < n)
            .collect();
        for (k, &i) in row.iter().enumerate() {
            let c = if across { i % cols } else { i / rows };
            out.buf.extend_from_slice(&cells[i]);
            pos += widths[i];
            if k + 1 < row.len() {
                let target = colw[..=c].iter().sum::<usize>() + 2 * (c + 1);
                while pos < target {
                    if target / 8 > (pos + 1) / 8 {
                        out.buf.push(b'\t');
                        pos += 8 - pos % 8;
                    } else {
                        out.buf.push(b' ');
                        pos += 1;
                    }
                }
            }
        }
        out.buf.push(b'\n');
    }
}

fn long_format(ctx: &UtilCtx<'_>, out: &mut Out, entries: &[Entry], o: &Opts, now: i64) {
    struct Row {
        inode: String,
        blocks: String,
        mode: String,
        nlink: String,
        owner: Vec<u8>,
        group: Vec<u8>,
        size: String,
        time: Vec<u8>,
    }
    let rows: Vec<Option<Row>> = entries
        .iter()
        .map(|e| {
            let (m, x) = e.meta?;
            let owner = if o.numeric {
                m.uid.to_string().into_bytes()
            } else {
                ctx.user_name(m.uid)
                    .unwrap_or_else(|| m.uid.to_string().into_bytes())
            };
            let group = if o.numeric {
                m.gid.to_string().into_bytes()
            } else {
                ctx.group_name(m.gid)
                    .unwrap_or_else(|| m.gid.to_string().into_bytes())
            };
            let size = if matches!(m.kind, FileKind::CharDevice | FileKind::BlockDevice) {
                let (major, minor) = dev_parts(x.rdev);
                format!("{major}, {minor}")
            } else {
                size_text(m.size, o)
            };
            Some(Row {
                inode: m.ino.to_string(),
                blocks: blocks_text(&x, o),
                mode: mode_string(m.kind, m.mode),
                nlink: x.nlink.to_string(),
                owner,
                group,
                size,
                time: time_text(ctx, time_of(&m, &x, o.time), o, now),
            })
        })
        .collect();
    let w = |f: &dyn Fn(&Row) -> usize| rows.iter().flatten().map(f).max().unwrap_or(0);
    let (wi, wb, wn, wo, wg, ws) = (
        w(&|r| r.inode.len()),
        w(&|r| r.blocks.len()),
        w(&|r| r.nlink.len()),
        w(&|r| display_width(&r.owner)),
        w(&|r| display_width(&r.group)),
        w(&|r| r.size.len()),
    );
    for (e, row) in entries.iter().zip(&rows) {
        let b = &mut out.buf;
        let Some(r) = row else {
            // Could not stat it (a dangling entry): GNU's question marks.
            b.extend_from_slice(b"l????????? ? ? ? ? ? ");
            b.extend_from_slice(&e.name);
            b.push(b'\n');
            continue;
        };
        if o.inode {
            pad_left(b, r.inode.as_bytes(), wi);
            b.push(b' ');
        }
        if o.size {
            pad_left(b, r.blocks.as_bytes(), wb);
            b.push(b' ');
        }
        b.extend_from_slice(r.mode.as_bytes());
        b.push(b' ');
        pad_left(b, r.nlink.as_bytes(), wn);
        b.push(b' ');
        if o.owner {
            pad_right(b, &r.owner, wo);
            b.push(b' ');
        }
        if o.group {
            pad_right(b, &r.group, wg);
            b.push(b' ');
        }
        pad_left(b, r.size.as_bytes(), ws);
        b.push(b' ');
        b.extend_from_slice(&r.time);
        b.push(b' ');
        b.extend_from_slice(&e.name);
        if let Some(t) = &e.target {
            b.extend_from_slice(b" -> ");
            b.extend_from_slice(t);
            b.extend_from_slice(target_indicator(e, o).as_bytes());
        } else {
            b.extend_from_slice(indicator(e, o).as_bytes());
        }
        b.push(b'\n');
    }
}

/// Linux's and macOS's encodings of a device number.
fn dev_parts(rdev: u64) -> (u64, u64) {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        ((rdev >> 24) & 0xff, rdev & 0xff_ffff)
    } else {
        (
            ((rdev >> 32) & 0xffff_f000) | ((rdev >> 8) & 0xfff),
            ((rdev >> 12) & 0xffff_ff00) | (rdev & 0xff),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_and_versions() {
        assert!(glob(b"*.rs", b"main.rs") && !glob(b"*.rs", b"main.c"));
        assert!(glob(b"a?c", b"abc") && glob(b"[a-c]x", b"bx") && !glob(b"[!a-c]x", b"bx"));
        assert!(glob(b"*~", b"f~") && glob(b"*", b""));
        assert_eq!(version_cmp(b"f10", b"f9"), std::cmp::Ordering::Greater);
        assert_eq!(version_cmp(b"a1.2", b"a1.10"), std::cmp::Ordering::Less);
    }
}

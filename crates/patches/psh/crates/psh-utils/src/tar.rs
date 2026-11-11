//! tar (nanocode's addition), after GNU tar: create, extract and list, with -f -C -z -v
//! -O -k -m -p -P -h, --exclude, --strip-components and the old option style
//! (tar czf). Archives are written in GNU's format (long names as L entries, records
//! of 10240 bytes) and read in GNU's, ustar's and pax's. gzip is detected when
//! reading; bzip2, xz and zstd are answered as missing. The archive is held in memory.
//! Members are written in name order (GNU writes them as the directory lists them).

use psh_engine::host::{FileKind, Metadata, OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::codec::{self, GzipError};
use crate::common::Input;
use crate::fsutil::join;
use crate::timefmt;

const BLOCK: usize = 512;

#[derive(Default)]
struct Options {
    mode: Option<u8>,
    file: Option<Vec<u8>>,
    gzip: bool,
    auto: bool,
    verbose: u8,
    to_stdout: bool,
    keep_old: bool,
    touch: bool,
    same_permissions: bool,
    absolute: bool,
    dereference: bool,
    no_recursion: bool,
    numeric: bool,
    strip: usize,
    excludes: Vec<Vec<u8>>,
    /// Operands in order; a -C among them changes directory for what follows.
    items: Vec<Item>,
}

enum Item {
    Path(Vec<u8>),
    Chdir(Vec<u8>),
}

/// What tar lacks, by the option that asked for it.
struct Lacks(String);

fn parse(argv: &[Vec<u8>]) -> Result<Options, Result<Lacks, String>> {
    let mut o = Options::default();
    let mut args: Vec<Vec<u8>> = argv[1..].to_vec();
    // The old style: the first word is letters without a dash.
    if let Some(first) = args.first()
        && !first.is_empty()
        && first[0] != b'-'
    {
        let letters = args.remove(0);
        let mut expanded = Vec::new();
        let mut taken = 0;
        for &c in &letters {
            expanded.push(vec![b'-', c]);
            if matches!(c, b'f' | b'C' | b'b' | b'T' | b'X') {
                if taken >= args.len() {
                    return Err(Err(format!(
                        "Old option '{}' requires an argument.",
                        c as char
                    )));
                }
                expanded.push(args[taken].clone());
                taken += 1;
            }
        }
        expanded.extend(args.drain(taken..));
        args = expanded;
    }
    let lacks = |what: &str| Err(Ok(Lacks(what.to_string())));
    let mut i = 0;
    let mut only_operands = false;
    while i < args.len() {
        let arg = args[i].clone();
        i += 1;
        if only_operands || arg.len() < 2 || arg[0] != b'-' {
            o.items.push(Item::Path(arg));
            continue;
        }
        if arg == b"--" {
            only_operands = true;
            continue;
        }
        let mut value =
            |name: &str, attached: Option<&[u8]>| -> Result<Vec<u8>, Result<Lacks, String>> {
                if let Some(v) = attached {
                    return Ok(v.to_vec());
                }
                if i < args.len() {
                    i += 1;
                    return Ok(args[i - 1].clone());
                }
                Err(Err(format!("option requires an argument -- '{name}'")))
            };
        if arg.starts_with(b"--") {
            let text = String::from_utf8_lossy(&arg[2..]).into_owned();
            let (name, attached) = match text.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.as_bytes().to_vec())),
                None => (text, None),
            };
            match name.as_str() {
                "create" => o.mode = Some(b'c'),
                "extract" | "get" => o.mode = Some(b'x'),
                "list" => o.mode = Some(b't'),
                "file" => o.file = Some(value("file", attached.as_deref())?),
                "directory" => {
                    let dir = value("directory", attached.as_deref())?;
                    o.items.push(Item::Chdir(dir));
                }
                "gzip" | "gunzip" | "ungzip" => o.gzip = true,
                "auto-compress" => o.auto = true,
                "verbose" => o.verbose += 1,
                "to-stdout" => o.to_stdout = true,
                "keep-old-files" | "skip-old-files" => o.keep_old = true,
                "touch" => o.touch = true,
                "preserve-permissions" | "same-permissions" => o.same_permissions = true,
                "absolute-names" => o.absolute = true,
                "dereference" => o.dereference = true,
                "no-recursion" => o.no_recursion = true,
                "numeric-owner" => o.numeric = true,
                "exclude" => o.excludes.push(value("exclude", attached.as_deref())?),
                "strip-components" => {
                    let n = value("strip-components", attached.as_deref())?;
                    o.strip = String::from_utf8_lossy(&n)
                        .parse()
                        .map_err(|_| Err("Invalid number of elements".to_string()))?;
                }
                "no-same-owner"
                | "no-same-permissions"
                | "overwrite"
                | "wildcards"
                | "no-wildcards"
                | "recursion"
                | "force-local"
                | "no-overwrite-dir"
                | "unlink-first"
                | "no-acls"
                | "no-xattrs"
                | "no-selinux"
                | "ignore-failed-read"
                | "warning"
                | "format"
                | "blocking-factor"
                | "record-size"
                | "delay-directory-restore"
                | "seek"
                | "no-seek" => {}
                "bzip2" => return lacks("bzip2"),
                "xz" | "lzma" => return lacks("xz"),
                "zstd" => return lacks("zstd"),
                other => return lacks(&format!("the option --{other}")),
            }
            continue;
        }
        let mut k = 1;
        while k < arg.len() {
            let c = arg[k];
            k += 1;
            let attached = (k < arg.len()).then(|| arg[k..].to_vec());
            match c {
                b'c' | b'x' | b't' => o.mode = Some(c),
                b'z' => o.gzip = true,
                b'a' => o.auto = true,
                b'v' => o.verbose += 1,
                b'O' => o.to_stdout = true,
                b'k' => o.keep_old = true,
                b'm' => o.touch = true,
                b'p' => o.same_permissions = true,
                b'P' => o.absolute = true,
                b'h' => o.dereference = true,
                b'o' | b'U' | b'S' | b'w' | b'B' | b'i' => {}
                b'f' => {
                    o.file = Some(value("f", attached.as_deref())?);
                    break;
                }
                b'C' => {
                    let dir = value("C", attached.as_deref())?;
                    o.items.push(Item::Chdir(dir));
                    break;
                }
                b'b' => {
                    value("b", attached.as_deref())?;
                    break;
                }
                b'j' => return lacks("bzip2"),
                b'J' => return lacks("xz"),
                other => return lacks(&format!("the option -{}", other as char)),
            }
        }
    }
    Ok(o)
}

fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    if text.len() > digits {
        // GNU's base-256 form for numbers that do not fit.
        let n = field.len();
        for (i, slot) in field.iter_mut().enumerate() {
            let shift = 8 * (n - 1 - i);
            *slot = if shift >= 64 {
                0
            } else {
                (value >> shift) as u8
            };
        }
        field[0] |= 0x80;
        return;
    }
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

struct Entry {
    name: Vec<u8>,
    link: Vec<u8>,
    mode: u32,
    uid: u64,
    gid: u64,
    size: u64,
    mtime: i64,
    kind: u8,
    uname: Vec<u8>,
    gname: Vec<u8>,
}

fn raw_header(entry: &Entry, name: &[u8], link: &[u8]) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    h[..name.len().min(100)].copy_from_slice(&name[..name.len().min(100)]);
    octal(&mut h[100..108], u64::from(entry.mode & 0o7777));
    octal(&mut h[108..116], entry.uid);
    octal(&mut h[116..124], entry.gid);
    octal(&mut h[124..136], entry.size);
    octal(&mut h[136..148], entry.mtime.max(0) as u64);
    h[148..156].copy_from_slice(b"        ");
    h[156] = entry.kind;
    h[157..157 + link.len().min(100)].copy_from_slice(&link[..link.len().min(100)]);
    h[257..265].copy_from_slice(b"ustar  \0");
    h[265..265 + entry.uname.len().min(31)]
        .copy_from_slice(&entry.uname[..entry.uname.len().min(31)]);
    h[297..297 + entry.gname.len().min(31)]
        .copy_from_slice(&entry.gname[..entry.gname.len().min(31)]);
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    h[155] = b' ';
    h
}

fn pad(out: &mut Vec<u8>) {
    let rest = out.len() % BLOCK;
    if rest != 0 {
        out.resize(out.len() + BLOCK - rest, 0);
    }
}

fn write_header(out: &mut Vec<u8>, entry: &Entry) {
    // A name or link target over 100 bytes goes ahead in an entry of its own.
    for (kind, text) in [(b'K', &entry.link), (b'L', &entry.name)] {
        if text.len() > 100 {
            let long = Entry {
                name: b"././@LongLink".to_vec(),
                link: Vec::new(),
                mode: 0o644,
                uid: 0,
                gid: 0,
                size: text.len() as u64 + 1,
                mtime: 0,
                kind,
                uname: Vec::new(),
                gname: Vec::new(),
            };
            out.extend_from_slice(&raw_header(&long, &long.name, b""));
            out.extend_from_slice(text);
            out.push(0);
            pad(out);
        }
    }
    out.extend_from_slice(&raw_header(entry, &entry.name, &entry.link));
}

/// GNU's --exclude: the pattern matches the name, or the part of it that starts or
/// ends at a slash; a star also matches slashes.
fn excluded(patterns: &[Vec<u8>], name: &[u8]) -> bool {
    let name = name.strip_suffix(b"/").unwrap_or(name);
    let mut starts = vec![0];
    starts.extend(
        name.iter()
            .enumerate()
            .filter(|(_, b)| **b == b'/')
            .map(|(i, _)| i + 1),
    );
    let mut ends: Vec<usize> = name
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'/')
        .map(|(i, _)| i)
        .collect();
    ends.push(name.len());
    patterns.iter().any(|pattern| {
        let pattern = pattern.strip_suffix(b"/").unwrap_or(pattern);
        starts.iter().any(|&s| {
            ends.iter()
                .filter(|&&e| e > s)
                .any(|&e| crate::git::wildmatch(pattern, &name[s..e], false))
        })
    })
}

struct Tar<'c, 'v> {
    ctx: &'c mut UtilCtx<'v>,
    o: Options,
    out: Vec<u8>,
    status: i32,
    warned_slash: bool,
    /// Verbose lines go to stderr when the archive or the files go to stdout.
    lines_to_stderr: bool,
}

impl Tar<'_, '_> {
    async fn line(&mut self, text: &[u8]) {
        let mut line = text.to_vec();
        line.push(b'\n');
        if self.lines_to_stderr {
            self.ctx.error_raw(&line).await;
        } else {
            let _ = self.ctx.write(&line).await;
        }
    }

    async fn fail(&mut self, text: &str) {
        self.ctx.error(text).await;
        self.status = 2;
    }

    fn names(&self, meta: &Metadata) -> (Vec<u8>, Vec<u8>) {
        if self.o.numeric {
            return (Vec::new(), Vec::new());
        }
        (
            self.ctx.user_name(meta.uid).unwrap_or_default(),
            self.ctx.group_name(meta.gid).unwrap_or_default(),
        )
    }

    /// Adds a file, or a directory and what is in it, to the archive being written.
    async fn add(&mut self, disk: Vec<u8>, name: Vec<u8>) {
        let mut stack = vec![(disk, name)];
        while let Some((disk, name)) = stack.pop() {
            self.ctx.check().await;
            if excluded(&self.o.excludes, &name) {
                continue;
            }
            let meta = match self.ctx.stat(&disk, self.o.dereference).await {
                Ok(meta) => meta,
                Err(e) => {
                    let shown = String::from_utf8_lossy(&name).into_owned();
                    self.fail(&format!("{shown}: Cannot stat: {}", error_text(&e)))
                        .await;
                    continue;
                }
            };
            let (uname, gname) = self.names(&meta);
            let mut entry = Entry {
                name: name.clone(),
                link: Vec::new(),
                mode: meta.mode,
                uid: u64::from(meta.uid),
                gid: u64::from(meta.gid),
                size: 0,
                mtime: meta.mtime_ns.div_euclid(1_000_000_000),
                kind: b'0',
                uname,
                gname,
            };
            match meta.kind {
                FileKind::Dir => {
                    if !entry.name.ends_with(b"/") {
                        entry.name.push(b'/');
                    }
                    entry.kind = b'5';
                    write_header(&mut self.out, &entry);
                    if self.o.verbose > 0 {
                        let shown = entry.name.clone();
                        self.line(&shown).await;
                    }
                    if self.o.no_recursion {
                        continue;
                    }
                    match self.ctx.read_dir(&disk).await {
                        Ok(entries) => {
                            let mut names: Vec<Vec<u8>> =
                                entries.into_iter().map(|e| e.name).collect();
                            names.sort();
                            for child in names.into_iter().rev() {
                                stack.push((join(&disk, &child), join(&entry.name, &child)));
                            }
                        }
                        Err(e) => {
                            let shown = String::from_utf8_lossy(&name).into_owned();
                            self.fail(&format!("{shown}: Cannot open: {}", error_text(&e)))
                                .await;
                        }
                    }
                }
                FileKind::Symlink => {
                    entry.kind = b'2';
                    match self.ctx.read_link(&disk).await {
                        Ok(target) => entry.link = target,
                        Err(e) => {
                            let shown = String::from_utf8_lossy(&name).into_owned();
                            self.fail(&format!("{shown}: Cannot readlink: {}", error_text(&e)))
                                .await;
                            continue;
                        }
                    }
                    write_header(&mut self.out, &entry);
                    if self.o.verbose > 0 {
                        self.line(&name).await;
                    }
                }
                FileKind::File => {
                    let mut data = Vec::new();
                    let read = match Input::open(self.ctx, &disk).await {
                        Ok(input) => input.read_to_end(self.ctx, &mut data).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = read {
                        let shown = String::from_utf8_lossy(&name).into_owned();
                        self.fail(&format!("{shown}: Cannot open: {}", error_text(&e)))
                            .await;
                        continue;
                    }
                    entry.size = data.len() as u64;
                    write_header(&mut self.out, &entry);
                    self.out.extend_from_slice(&data);
                    pad(&mut self.out);
                    if self.o.verbose > 0 {
                        self.line(&name).await;
                    }
                }
                _ => {
                    let shown = String::from_utf8_lossy(&name).into_owned();
                    self.ctx
                        .error(&format!("{shown}: file is of an unsupported type, ignored"))
                        .await;
                }
            }
        }
    }

    async fn create(&mut self) -> i32 {
        let to_stdout = self.o.file.as_deref().is_none_or(|f| f == b"-");
        self.lines_to_stderr = to_stdout;
        let items = std::mem::take(&mut self.o.items);
        let mut dir: Vec<u8> = Vec::new();
        let mut any = false;
        for item in items {
            match item {
                Item::Chdir(to) => {
                    dir = if to.first() == Some(&b'/') || dir.is_empty() {
                        to
                    } else {
                        join(&dir, &to)
                    };
                }
                Item::Path(path) => {
                    any = true;
                    let disk = if path.first() == Some(&b'/') || dir.is_empty() {
                        path.clone()
                    } else {
                        join(&dir, &path)
                    };
                    let mut name = path.clone();
                    if !self.o.absolute && name.first() == Some(&b'/') {
                        if !self.warned_slash {
                            self.warned_slash = true;
                            self.ctx
                                .error("Removing leading `/' from member names")
                                .await;
                        }
                        while name.first() == Some(&b'/') {
                            name.remove(0);
                        }
                    }
                    // A name is kept from the last .. on, as GNU keeps it.
                    if !self.o.absolute {
                        let parts: Vec<&[u8]> = name.split(|&b| b == b'/').collect();
                        if let Some(last) = parts.iter().rposition(|p| *p == b"..") {
                            let prefix = parts[..=last].join(&b'/');
                            self.ctx
                                .error(&format!(
                                    "Removing leading `{}/' from member names",
                                    String::from_utf8_lossy(&prefix)
                                ))
                                .await;
                            name = parts[last + 1..].join(&b'/');
                        }
                    }
                    while name.len() > 1 && name.ends_with(b"/") {
                        name.pop();
                    }
                    if name.is_empty() {
                        name = b".".to_vec();
                    }
                    self.add(disk, name).await;
                }
            }
        }
        if !any {
            self.ctx
                .error("Cowardly refusing to create an empty archive")
                .await;
            self.ctx
                .error_raw(b"Try 'tar --help' or 'tar --usage' for more information.\n")
                .await;
            return 2;
        }
        // The end: two empty blocks, and the record filled.
        self.out.resize(self.out.len() + 2 * BLOCK, 0);
        let rest = self.out.len() % (20 * BLOCK);
        if rest != 0 {
            self.out.resize(self.out.len() + 20 * BLOCK - rest, 0);
        }
        let mut data = std::mem::take(&mut self.out);
        let name = self.o.file.clone().unwrap_or_default();
        let by_name = self.o.auto
            && (name.ends_with(b".gz") || name.ends_with(b".tgz") || name.ends_with(b".taz"));
        if self.o.gzip || by_name {
            data = codec::gzip(&data, 6, None, 0);
        }
        if to_stdout {
            if let Err(e) = self.ctx.write(&data).await {
                return crate::common::write_failed(self.ctx, &e).await;
            }
        } else {
            let written = match self.ctx.open(&name, OpenOptions::write()).await {
                Ok(file) => file.write_all(&data).await,
                Err(e) => Err(e),
            };
            if let Err(e) = written {
                let shown = String::from_utf8_lossy(&name).into_owned();
                self.ctx
                    .error(&format!("{shown}: Cannot open: {}", error_text(&e)))
                    .await;
                self.ctx
                    .error("Error is not recoverable: exiting now")
                    .await;
                return 2;
            }
        }
        if self.status != 0 {
            self.ctx
                .error("Exiting with failure status due to previous errors")
                .await;
        }
        self.status
    }
}

fn number(field: &[u8]) -> u64 {
    if field.first().is_some_and(|b| b & 0x80 != 0) {
        let mut value = 0u64;
        for (i, &b) in field.iter().enumerate() {
            let b = if i == 0 { b & 0x7f } else { b };
            value = value.wrapping_shl(8) | u64::from(b);
        }
        return value;
    }
    let mut value = 0u64;
    for &b in field {
        if (b'0'..=b'7').contains(&b) {
            value = value * 8 + u64::from(b - b'0');
        } else if value != 0 || b == 0 {
            if b == 0 || b == b' ' {
                if value != 0 {
                    break;
                }
            } else {
                break;
            }
        }
    }
    value
}

fn text(field: &[u8]) -> &[u8] {
    match field.iter().position(|&b| b == 0) {
        Some(nul) => &field[..nul],
        None => field,
    }
}

/// The members of an archive: each entry and where its data is.
fn members(data: &[u8]) -> Result<Vec<(Entry, std::ops::Range<usize>)>, &'static str> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut long_name: Option<Vec<u8>> = None;
    let mut long_link: Option<Vec<u8>> = None;
    let mut pax: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut any = false;
    while pos + BLOCK <= data.len() {
        let h = &data[pos..pos + BLOCK];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let stored = number(&h[148..156]);
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    32
                } else {
                    u64::from(b)
                }
            })
            .sum();
        if stored != sum {
            return Err(if any {
                "Skipping to next header"
            } else {
                "This does not look like a tar archive"
            });
        }
        any = true;
        let size = number(&h[124..136]) as usize;
        let body = pos + BLOCK..(pos + BLOCK + size).min(data.len());
        let next = pos + BLOCK + size.div_ceil(BLOCK) * BLOCK;
        let kind = h[156];
        match kind {
            b'L' => long_name = Some(text(&data[body.clone()]).to_vec()),
            b'K' => long_link = Some(text(&data[body.clone()]).to_vec()),
            b'x' => {
                let mut rest = &data[body.clone()];
                while let Some(space) = rest.iter().position(|&b| b == b' ') {
                    let Some(len) = std::str::from_utf8(&rest[..space])
                        .ok()
                        .and_then(|n| n.parse::<usize>().ok())
                    else {
                        break;
                    };
                    if len <= space + 1 || len > rest.len() {
                        break;
                    }
                    let record = &rest[space + 1..len - 1];
                    if let Some(eq) = record.iter().position(|&b| b == b'=') {
                        pax.push((record[..eq].to_vec(), record[eq + 1..].to_vec()));
                    }
                    rest = &rest[len..];
                }
            }
            b'g' => {}
            _ => {
                let mut name = text(&h[..100]).to_vec();
                if &h[257..263] == b"ustar\0" && h[345] != 0 {
                    let mut full = text(&h[345..500]).to_vec();
                    full.push(b'/');
                    full.extend_from_slice(&name);
                    name = full;
                }
                let mut entry = Entry {
                    name: long_name.take().unwrap_or(name),
                    link: long_link
                        .take()
                        .unwrap_or_else(|| text(&h[157..257]).to_vec()),
                    mode: number(&h[100..108]) as u32,
                    uid: number(&h[108..116]),
                    gid: number(&h[116..124]),
                    size: size as u64,
                    mtime: number(&h[136..148]) as i64,
                    kind: if kind == 0 || kind == b'7' {
                        b'0'
                    } else {
                        kind
                    },
                    uname: text(&h[265..297]).to_vec(),
                    gname: text(&h[297..329]).to_vec(),
                };
                for (key, value) in pax.drain(..) {
                    match key.as_slice() {
                        b"path" => entry.name = value,
                        b"linkpath" => entry.link = value,
                        b"mtime" => {
                            let whole = value.split(|&b| b == b'.').next().unwrap_or_default();
                            if let Some(t) =
                                std::str::from_utf8(whole).ok().and_then(|t| t.parse().ok())
                            {
                                entry.mtime = t;
                            }
                        }
                        b"uname" => entry.uname = value,
                        b"gname" => entry.gname = value,
                        _ => {}
                    }
                }
                // An old archive marks a directory by its trailing slash alone.
                if entry.kind == b'0' && entry.name.ends_with(b"/") {
                    entry.kind = b'5';
                }
                out.push((entry, body));
            }
        }
        pos = next;
    }
    Ok(out)
}

fn mode_text(entry: &Entry) -> String {
    let kind = match entry.kind {
        b'5' => 'd',
        b'2' => 'l',
        b'1' => 'h',
        b'3' => 'c',
        b'4' => 'b',
        b'6' => 'p',
        _ => '-',
    };
    let mut out = String::from(kind);
    for (shift, special, low, up) in [
        (6, 0o4000, 's', 'S'),
        (3, 0o2000, 's', 'S'),
        (0, 0o1000, 't', 'T'),
    ] {
        let bits = entry.mode >> shift;
        out.push(if bits & 4 != 0 { 'r' } else { '-' });
        out.push(if bits & 2 != 0 { 'w' } else { '-' });
        out.push(match (bits & 1 != 0, entry.mode & special != 0) {
            (true, true) => low,
            (false, true) => up,
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    out
}

/// A member's name as it lands: components stripped, no leading slash.
fn landed(o: &Options, name: &[u8]) -> Option<Vec<u8>> {
    let mut parts: Vec<&[u8]> = name
        .split(|&b| b == b'/')
        .filter(|p| !p.is_empty() && *p != b".")
        .collect();
    if parts.len() <= o.strip && o.strip > 0 {
        return None;
    }
    parts.drain(..o.strip.min(parts.len()));
    let mut out = if o.absolute && name.first() == Some(&b'/') && o.strip == 0 {
        b"/".to_vec()
    } else {
        Vec::new()
    };
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        return None;
    }
    Some(out)
}

impl Tar<'_, '_> {
    async fn mkdirs(&mut self, path: &[u8]) {
        let mut at = Vec::new();
        let absolute = path.first() == Some(&b'/');
        for part in path.split(|&b| b == b'/').filter(|p| !p.is_empty()) {
            if !at.is_empty() || absolute {
                at.push(b'/');
            }
            at.extend_from_slice(part);
            if self.ctx.stat(&at, true).await.is_err() {
                let _ = self.ctx.mkdir(&at, 0o777 & !self.ctx.umask()).await;
            }
        }
    }

    async fn read_or_list(&mut self) -> i32 {
        let extract = self.o.mode == Some(b'x');
        self.lines_to_stderr = extract && self.o.to_stdout;
        let name = self.o.file.clone().unwrap_or_else(|| b"-".to_vec());
        let shown = String::from_utf8_lossy(&name).into_owned();
        let mut data = Vec::new();
        let read = match Input::open(self.ctx, &name).await {
            Ok(input) => input.read_to_end(self.ctx, &mut data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            self.ctx
                .error(&format!("{shown}: Cannot open: {}", error_text(&e)))
                .await;
            self.ctx
                .error("Error is not recoverable: exiting now")
                .await;
            return 2;
        }
        if data.starts_with(&[0x1f, 0x8b]) {
            data = match codec::gunzip(&data) {
                Ok((plain, _)) => plain,
                Err(GzipError::NotGzip | GzipError::Corrupt(_)) => {
                    self.ctx
                        .error_raw(b"\ngzip: stdin: unexpected end of file\n")
                        .await;
                    self.ctx.error("Child returned status 1").await;
                    self.ctx
                        .error("Error is not recoverable: exiting now")
                        .await;
                    return 2;
                }
            };
        } else {
            let other = if data.starts_with(b"BZh") {
                Some("bzip2")
            } else if data.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
                Some("xz")
            } else if data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
                Some("zstd")
            } else {
                None
            };
            if let Some(what) = other {
                return missing(self.ctx, what).await;
            }
        }
        let list = match members(&data) {
            Ok(list) => list,
            Err(what) => {
                self.ctx.error(what).await;
                self.ctx
                    .error("Exiting with failure status due to previous errors")
                    .await;
                return 2;
            }
        };
        // The members asked for by name, and the directory to work in.
        let mut wanted: Vec<(Vec<u8>, bool)> = Vec::new();
        let mut dir: Vec<u8> = Vec::new();
        for item in std::mem::take(&mut self.o.items) {
            match item {
                Item::Chdir(to) => dir = to,
                Item::Path(p) => {
                    let p = p.strip_suffix(b"/").unwrap_or(&p).to_vec();
                    wanted.push((p, false));
                }
            }
        }
        let umask = self.ctx.umask();
        let mut dirs: Vec<(Vec<u8>, i64, u32)> = Vec::new();
        for (entry, body) in &list {
            self.ctx.check().await;
            let plain = entry.name.strip_suffix(b"/").unwrap_or(&entry.name);
            if !wanted.is_empty() {
                let mut hit = false;
                for (want, found) in wanted.iter_mut() {
                    if plain == want.as_slice()
                        || (plain.len() > want.len()
                            && plain.starts_with(want)
                            && plain[want.len()] == b'/')
                    {
                        *found = true;
                        hit = true;
                    }
                }
                if !hit {
                    continue;
                }
            }
            if excluded(&self.o.excludes, &entry.name) {
                continue;
            }
            if !extract {
                if self.o.verbose == 0 {
                    self.line(&entry.name).await;
                    continue;
                }
                let user = if entry.uname.is_empty() || self.o.numeric {
                    entry.uid.to_string()
                } else {
                    String::from_utf8_lossy(&entry.uname).into_owned()
                };
                let group = if entry.gname.is_empty() || self.o.numeric {
                    entry.gid.to_string()
                } else {
                    String::from_utf8_lossy(&entry.gname).into_owned()
                };
                let size = entry.size.to_string();
                let width = 19usize
                    .saturating_sub(user.len() + 1 + group.len() + 1)
                    .max(size.len());
                let tm = timefmt::local(self.ctx, entry.mtime * 1_000_000_000);
                let mut line = format!(
                    "{} {user}/{group} {size:>width$} {:04}-{:02}-{:02} {:02}:{:02} ",
                    mode_text(entry),
                    tm.year,
                    tm.mon,
                    tm.day,
                    tm.hour,
                    tm.min
                )
                .into_bytes();
                line.extend_from_slice(&entry.name);
                match entry.kind {
                    b'2' => {
                        line.extend_from_slice(b" -> ");
                        line.extend_from_slice(&entry.link);
                    }
                    b'1' => {
                        line.extend_from_slice(b" link to ");
                        line.extend_from_slice(&entry.link);
                    }
                    _ => {}
                }
                self.line(&line).await;
                continue;
            }
            if plain.split(|&b| b == b'/').any(|part| part == b"..") {
                let shown = String::from_utf8_lossy(&entry.name).into_owned();
                self.fail(&format!("{shown}: Member name contains '..'"))
                    .await;
                continue;
            }
            let Some(rel) = landed(&self.o, &entry.name) else {
                continue;
            };
            if !self.o.absolute && entry.name.first() == Some(&b'/') && !self.warned_slash {
                self.warned_slash = true;
                self.ctx
                    .error("Removing leading `/' from member names")
                    .await;
            }
            let path = if dir.is_empty() || rel.first() == Some(&b'/') {
                rel.clone()
            } else {
                join(&dir, &rel)
            };
            if self.o.verbose > 0 {
                let shown = entry.name.clone();
                self.line(&shown).await;
            }
            let shown = String::from_utf8_lossy(&rel).into_owned();
            let mode = if self.o.same_permissions {
                entry.mode & 0o7777
            } else {
                entry.mode & 0o777 & !umask
            };
            if entry.kind == b'5' {
                self.mkdirs(&path).await;
                let _ = self.ctx.set_mode(&path, mode).await;
                dirs.push((path, entry.mtime, mode));
                continue;
            }
            if self.o.to_stdout {
                if entry.kind == b'0'
                    && let Err(e) = self.ctx.write(&data[body.clone()]).await
                {
                    return crate::common::write_failed(self.ctx, &e).await;
                }
                continue;
            }
            if let Some(slash) = path.iter().rposition(|&b| b == b'/')
                && slash > 0
            {
                let parent = path[..slash].to_vec();
                self.mkdirs(&parent).await;
            }
            let existing = self.ctx.stat(&path, false).await.ok();
            if let Some(old) = &existing {
                if self.o.keep_old {
                    self.fail(&format!("{shown}: Cannot open: File exists"))
                        .await;
                    continue;
                }
                if old.kind != FileKind::Dir {
                    let _ = self.ctx.remove_file(&path).await;
                }
            }
            match entry.kind {
                b'0' => {
                    let options = OpenOptions {
                        mode: Some(mode),
                        ..OpenOptions::write()
                    };
                    let written = match self.ctx.open(&path, options).await {
                        Ok(file) => file.write_all(&data[body.clone()]).await,
                        Err(e) => Err(e),
                    };
                    if let Err(e) = written {
                        self.fail(&format!("{shown}: Cannot open: {}", error_text(&e)))
                            .await;
                        continue;
                    }
                    let _ = self.ctx.set_mode(&path, mode).await;
                    if !self.o.touch {
                        let t = entry.mtime * 1_000_000_000;
                        let _ = self.ctx.set_times(&path, Some(t), Some(t), true).await;
                    }
                }
                b'2' => {
                    if let Err(e) = self.ctx.symlink(&entry.link, &path).await {
                        self.fail(&format!(
                            "{shown}: Cannot create symlink to '{}': {}",
                            String::from_utf8_lossy(&entry.link),
                            error_text(&e)
                        ))
                        .await;
                    }
                }
                b'1' => {
                    let Some(source) = landed(&self.o, &entry.link) else {
                        continue;
                    };
                    let source = if dir.is_empty() {
                        source
                    } else {
                        join(&dir, &source)
                    };
                    if self.ctx.hard_link(&source, &path).await.is_err() {
                        let mut copy = Vec::new();
                        let read = match Input::open(self.ctx, &source).await {
                            Ok(input) => input.read_to_end(self.ctx, &mut copy).await,
                            Err(e) => Err(e),
                        };
                        let written = match read {
                            Ok(()) => match self.ctx.open(&path, OpenOptions::write()).await {
                                Ok(file) => file.write_all(&copy).await,
                                Err(e) => Err(e),
                            },
                            Err(e) => Err(e),
                        };
                        if let Err(e) = written {
                            self.fail(&format!(
                                "{shown}: Cannot hard link to '{}': {}",
                                String::from_utf8_lossy(&entry.link),
                                error_text(&e)
                            ))
                            .await;
                        }
                    }
                }
                _ => {
                    self.ctx
                        .error(&format!("{shown}: Unknown file type; file ignored"))
                        .await;
                }
            }
        }
        if !self.o.touch {
            for (path, mtime, _) in dirs.iter().rev() {
                let t = mtime * 1_000_000_000;
                let _ = self.ctx.set_times(path, Some(t), Some(t), true).await;
            }
        }
        for (want, found) in &wanted {
            if !found {
                let shown = String::from_utf8_lossy(want).into_owned();
                self.fail(&format!("{shown}: Not found in archive")).await;
            }
        }
        if self.status != 0 {
            self.ctx
                .error("Exiting with failure status due to previous errors")
                .await;
        }
        self.status
    }
}

/// The answer for something this tar lacks: the real program when there is one, else
/// the words a host with a machine looks for.
async fn missing(ctx: &mut UtilCtx<'_>, what: &str) -> i32 {
    ctx.error(&format!(
        "{what} (this simulated shell has no tar for it: it reads and writes plain and gzip archives)"
    ))
    .await;
    127
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let o = match parse(argv) {
        Ok(o) => o,
        Err(Ok(Lacks(what))) => {
            if let Some(status) = ctx.run_program(argv.to_vec()).await {
                return status;
            }
            return missing(ctx, &what).await;
        }
        Err(Err(usage)) => {
            ctx.error(&usage).await;
            ctx.error_raw(b"Try 'tar --help' or 'tar --usage' for more information.\n")
                .await;
            return 2;
        }
    };
    let Some(mode) = o.mode else {
        ctx.error("You must specify one of the '-Acdtrux', '--delete' or '--test-label' options")
            .await;
        ctx.error_raw(b"Try 'tar --help' or 'tar --usage' for more information.\n")
            .await;
        return 2;
    };
    let mut tar = Tar {
        ctx,
        o,
        out: Vec::new(),
        status: 0,
        warned_slash: false,
        lines_to_stderr: false,
    };
    if mode == b'c' {
        tar.create().await
    } else {
        tar.read_or_list().await
    }
}

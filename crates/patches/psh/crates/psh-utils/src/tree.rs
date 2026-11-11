//! tree (nanocode's addition), after tree 2.1: -a -d -f -i -F -L -I -P -s -h -p
//! --dirsfirst --noreport --charset. Names in byte order; lines drawn in UTF-8 where
//! the locale is, else in ASCII with spaces escaped, as tree prints to a pipe.

use psh_engine::host::{FileKind, Metadata};
use psh_engine::util::UtilCtx;

use crate::common;
use crate::fsutil::{human, join};
use crate::git::wildmatch;

struct Options {
    all: bool,
    dirs_only: bool,
    full: bool,
    no_indent: bool,
    classify: bool,
    level: Option<usize>,
    ignore: Vec<Vec<u8>>,
    only: Vec<Vec<u8>>,
    size: bool,
    human: bool,
    perms: bool,
    dirs_first: bool,
    utf8: bool,
}

struct Tree<'c, 'v> {
    ctx: &'c mut UtilCtx<'v>,
    o: Options,
    out: Vec<u8>,
    dirs: usize,
    files: usize,
}

fn mode_text(meta: &Metadata) -> String {
    let kind = match meta.kind {
        FileKind::Dir => 'd',
        FileKind::Symlink => 'l',
        _ => '-',
    };
    let mut text = String::from(kind);
    for shift in [6, 3, 0] {
        let bits = meta.mode >> shift;
        text.push(if bits & 4 != 0 { 'r' } else { '-' });
        text.push(if bits & 2 != 0 { 'w' } else { '-' });
        text.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    text
}

impl Tree<'_, '_> {
    fn name(&self, name: &[u8]) -> Vec<u8> {
        if self.o.utf8 {
            return name.to_vec();
        }
        let mut out = Vec::new();
        for &b in name {
            match b {
                b' ' => out.extend_from_slice(b"\\ "),
                0x21..=0x7e => out.push(b),
                _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            }
        }
        out
    }

    fn details(&self, meta: &Metadata) -> Vec<u8> {
        let mut parts = Vec::new();
        if self.o.perms {
            parts.push(mode_text(meta));
        }
        if self.o.human {
            parts.push(format!("{:>4}", human(meta.size, 1024)));
        } else if self.o.size {
            parts.push(format!("{:>11}", meta.size));
        }
        if parts.is_empty() {
            return Vec::new();
        }
        format!("[{}]  ", parts.join(" ")).into_bytes()
    }

    fn mark(&self, meta: &Metadata) -> &'static [u8] {
        if !self.o.classify {
            return b"";
        }
        match meta.kind {
            FileKind::Dir => b"/",
            FileKind::Fifo => b"|",
            FileKind::Socket => b"=",
            FileKind::File if meta.mode & 0o111 != 0 => b"*",
            _ => b"",
        }
    }

    async fn walk(&mut self, dir: &[u8], shown: &[u8], prefix: &[u8], depth: usize) {
        if self.o.level.is_some_and(|l| depth > l) {
            return;
        }
        let Ok(listed) = self.ctx.read_dir(dir).await else {
            return;
        };
        let mut entries: Vec<(Vec<u8>, Metadata, bool)> = Vec::new();
        for entry in listed {
            let name = entry.name;
            if !self.o.all && name.first() == Some(&b'.') {
                continue;
            }
            let path = join(dir, &name);
            let Ok(meta) = self.ctx.stat(&path, false).await else {
                continue;
            };
            let is_dir = meta.kind == FileKind::Dir;
            // A link to a directory counts as one and is not gone into.
            let to_dir = meta.kind == FileKind::Symlink
                && self
                    .ctx
                    .stat(&path, true)
                    .await
                    .is_ok_and(|m| m.kind == FileKind::Dir);
            if self.o.ignore.iter().any(|p| wildmatch(p, &name, false)) {
                continue;
            }
            if !is_dir
                && !self.o.only.is_empty()
                && !self.o.only.iter().any(|p| wildmatch(p, &name, false))
            {
                continue;
            }
            if self.o.dirs_only && !is_dir && !to_dir {
                continue;
            }
            entries.push((name, meta, is_dir || to_dir));
        }
        entries.sort_by(|a, b| {
            let first = if self.o.dirs_first {
                b.2.cmp(&a.2)
            } else {
                std::cmp::Ordering::Equal
            };
            first.then_with(|| a.0.cmp(&b.0))
        });
        let count = entries.len();
        for (n, (name, meta, counts_as_dir)) in entries.into_iter().enumerate() {
            self.ctx.check().await;
            let last = n + 1 == count;
            let path = join(dir, &name);
            let label = if self.o.full {
                join(shown, &name)
            } else {
                name.clone()
            };
            if !self.o.no_indent {
                self.out.extend_from_slice(prefix);
                let branch: &[u8] = match (self.o.utf8, last) {
                    (true, false) => "\u{251c}\u{2500}\u{2500} ".as_bytes(),
                    (true, true) => "\u{2514}\u{2500}\u{2500} ".as_bytes(),
                    (false, false) => b"|-- ",
                    (false, true) => b"`-- ",
                };
                self.out.extend_from_slice(branch);
            }
            let details = self.details(&meta);
            self.out.extend_from_slice(&details);
            let text = self.name(&label);
            self.out.extend_from_slice(&text);
            if meta.kind == FileKind::Symlink {
                let target = self.ctx.read_link(&path).await.unwrap_or_default();
                self.out.extend_from_slice(b" -> ");
                let text = self.name(&target);
                self.out.extend_from_slice(&text);
                if self.o.classify && counts_as_dir {
                    self.out.push(b'/');
                }
            } else {
                self.out.extend_from_slice(self.mark(&meta));
            }
            self.out.push(b'\n');
            if counts_as_dir {
                self.dirs += 1;
            } else {
                self.files += 1;
            }
            if meta.kind == FileKind::Dir {
                let mut deeper = prefix.to_vec();
                let pad: &[u8] = match (self.o.utf8, last) {
                    (_, true) => b"    ",
                    (true, false) => "\u{2502}\u{a0}\u{a0} ".as_bytes(),
                    (false, false) => b"|   ",
                };
                deeper.extend_from_slice(pad);
                Box::pin(self.walk(&path, &join(shown, &name), &deeper, depth + 1)).await;
            }
        }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let locale = [b"LC_ALL".as_slice(), b"LC_CTYPE", b"LANG"]
        .iter()
        .find_map(|name| ctx.var(name).filter(|v| !v.is_empty()))
        .map(|v| String::from_utf8_lossy(v).to_ascii_lowercase())
        .unwrap_or_default();
    let mut o = Options {
        all: false,
        dirs_only: false,
        full: false,
        no_indent: false,
        classify: false,
        level: None,
        ignore: Vec::new(),
        only: Vec::new(),
        size: false,
        human: false,
        perms: false,
        dirs_first: false,
        utf8: locale.contains("utf-8") || locale.contains("utf8"),
    };
    let mut report = true;
    let mut roots: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            roots.push(arg.clone());
            continue;
        }
        let text = String::from_utf8_lossy(arg).into_owned();
        let patterns = |i: &mut usize| -> Vec<Vec<u8>> {
            *i += 1;
            argv.get(*i - 1)
                .map(|v| v.split(|&b| b == b'|').map(<[u8]>::to_vec).collect())
                .unwrap_or_default()
        };
        match text.as_str() {
            "--noreport" => report = false,
            "--dirsfirst" => o.dirs_first = true,
            "--charset=ascii" | "--charset=ASCII" => o.utf8 = false,
            "--charset=utf-8" | "--charset=UTF-8" | "--charset=utf8" => o.utf8 = true,
            "-L" => {
                i += 1;
                let level = argv
                    .get(i - 1)
                    .and_then(|v| std::str::from_utf8(v).ok()?.parse::<usize>().ok())
                    .filter(|l| *l > 0);
                let Some(level) = level else {
                    ctx.error("Invalid level, must be greater than 0.").await;
                    return 1;
                };
                o.level = Some(level);
            }
            "-I" => o.ignore.extend(patterns(&mut i)),
            "-P" => o.only.extend(patterns(&mut i)),
            _ if !text.starts_with("--") => {
                for c in text[1..].chars() {
                    match c {
                        'a' => o.all = true,
                        'd' => o.dirs_only = true,
                        'f' => o.full = true,
                        'i' => o.no_indent = true,
                        'F' => o.classify = true,
                        's' => o.size = true,
                        'h' => o.human = true,
                        'p' => o.perms = true,
                        'n' | 'C' | 'A' | 'S' => {}
                        _ => {
                            if let Some(status) = ctx.run_program(argv.to_vec()).await {
                                return status;
                            }
                            ctx.error(&format!("unsupported option '{text}' (psh built-in tree)"))
                                .await;
                            return 2;
                        }
                    }
                }
            }
            _ => {
                if let Some(status) = ctx.run_program(argv.to_vec()).await {
                    return status;
                }
                ctx.error(&format!("unsupported option '{text}' (psh built-in tree)"))
                    .await;
                return 2;
            }
        }
    }
    if roots.is_empty() {
        roots.push(b".".to_vec());
    }
    let mut tree = Tree {
        ctx,
        o,
        out: Vec::new(),
        dirs: 0,
        files: 0,
    };
    let mut status = 0;
    for root in &roots {
        let meta = tree.ctx.stat(root, true).await;
        let is_dir = meta.as_ref().is_ok_and(|m| m.kind == FileKind::Dir);
        if let Ok(meta) = &meta {
            let details = tree.details(meta);
            tree.out.extend_from_slice(&details);
        }
        let text = tree.name(root);
        tree.out.extend_from_slice(&text);
        if !is_dir {
            tree.out.extend_from_slice(b"  [error opening dir]\n");
            if meta.is_ok() {
                tree.files += 1;
            } else {
                status = 2;
            }
            continue;
        }
        if tree.o.classify && !root.ends_with(b"/") {
            tree.out.push(b'/');
        }
        tree.out.push(b'\n');
        tree.dirs += 1;
        tree.walk(root, root, b"", 1).await;
    }
    if report {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut line = format!("\n{}", plural(tree.dirs, "directory", "directories"));
        if !tree.o.dirs_only {
            line.push_str(&format!(", {}", plural(tree.files, "file", "files")));
        }
        line.push('\n');
        tree.out.extend_from_slice(line.as_bytes());
    }
    let out = std::mem::take(&mut tree.out);
    match tree.ctx.write(&out).await {
        Ok(()) => status,
        Err(e) => common::write_failed(tree.ctx, &e).await,
    }
}

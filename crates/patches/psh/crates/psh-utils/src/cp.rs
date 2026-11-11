//! `cp [-R] [-a|-p] [-f|-n] [-L|-P|-H] [-l|-s] [-u] [-v] [-t DIR] [-T] [--parents] SRC...
//! DEST`: GNU semantics and messages; BSD's `-c` (clone) and `-X` are accepted as plain
//! copies. `cp -R src/ dst` copies the directory itself, as GNU does (D016); `src/.`
//! copies its contents in both dialects. `-i` falls through to the real `cp`.
//! Written for psh after the manuals.

use std::io;

use psh_engine::host::{FileKind, Metadata, OpenOptions, error_text};
use psh_engine::util::{LocalBoxFuture, UtilCtx};

use crate::common;
use crate::fsutil::{Need, base_name, is_below, join, resolve, s};
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "cp",
    opts: &[
        both('R', "recursive"),
        flag('r'),
        both('a', "archive"),
        flag('p'),
        long("preserve").optional(),
        long("no-preserve").value(),
        both('f', "force"),
        both('n', "no-clobber"),
        both('L', "dereference"),
        both('P', "no-dereference"),
        flag('H'),
        flag('d'),
        both('l', "link"),
        both('s', "symbolic-link"),
        both('u', "update").optional(),
        both('v', "verbose"),
        both('t', "target-directory").value(),
        both('T', "no-target-directory"),
        long("parents"),
        long("remove-destination"),
        both('x', "one-file-system"),
        flag('c'),
        flag('X'),
        both('b', "backup").optional(),
        both('S', "suffix").value(),
        long("strip-trailing-slashes"),
    ],
    posix_order: false,
};

/// Whether symlinks are followed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Deref {
    Always,
    Never,
    /// On the command line only (`-H`).
    Args,
}

#[derive(Clone)]
pub struct CopyOpts {
    pub recursive: bool,
    pub deref: Deref,
    pub mode: bool,
    pub times: bool,
    pub force: bool,
    pub no_clobber: bool,
    pub update: bool,
    pub link: bool,
    pub symlink: bool,
    pub verbose: bool,
    pub remove_first: bool,
    pub backup: Option<Vec<u8>>,
}

impl CopyOpts {
    /// What `mv` needs across file systems: everything as it is.
    pub fn for_move() -> CopyOpts {
        CopyOpts {
            recursive: true,
            deref: Deref::Never,
            mode: true,
            times: true,
            force: true,
            no_clobber: false,
            update: false,
            link: false,
            symlink: false,
            verbose: false,
            remove_first: false,
            backup: None,
        }
    }
}

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let mut o = CopyOpts {
        recursive: false,
        deref: Deref::Always,
        mode: false,
        times: false,
        force: false,
        no_clobber: false,
        update: false,
        link: false,
        symlink: false,
        verbose: false,
        remove_first: false,
        backup: None,
    };
    let mut deref_given = false;
    let mut backup = false;
    let mut suffix = b"~".to_vec();
    for (k, v) in p.iter(&SPEC) {
        match k.as_str() {
            "recursive" | "r" => o.recursive = true,
            "archive" => {
                o.recursive = true;
                o.deref = Deref::Never;
                deref_given = true;
                o.mode = true;
                o.times = true;
            }
            "p" => (o.mode, o.times) = (true, true),
            "preserve" => {
                let list = v.unwrap_or(b"mode,ownership,timestamps");
                for a in list.split(|&b| b == b',') {
                    match a {
                        b"mode" => o.mode = true,
                        b"timestamps" => o.times = true,
                        b"all" => (o.mode, o.times) = (true, true),
                        // Owners only change as root; links, context and xattrs are
                        // not kept (GNU too fails only when they were asked for
                        // explicitly).
                        b"ownership" | b"links" | b"context" | b"xattr" => {}
                        _ => return bad_attr(ctx, a, "--preserve").await,
                    }
                }
            }
            "no-preserve" => {
                for a in v.unwrap_or_default().split(|&b| b == b',') {
                    match a {
                        b"mode" => o.mode = false,
                        b"timestamps" => o.times = false,
                        b"all" => (o.mode, o.times) = (false, false),
                        b"ownership" | b"links" | b"context" | b"xattr" => {}
                        _ => return bad_attr(ctx, a, "--no-preserve").await,
                    }
                }
            }
            "force" => (o.force, o.no_clobber) = (true, false),
            "no-clobber" => (o.no_clobber, o.force) = (true, false),
            "dereference" => (o.deref, deref_given) = (Deref::Always, true),
            "no-dereference" | "d" => (o.deref, deref_given) = (Deref::Never, true),
            "H" => (o.deref, deref_given) = (Deref::Args, true),
            "link" => o.link = true,
            "symbolic-link" => o.symlink = true,
            "update" => match v {
                None | Some(b"older") => o.update = true,
                Some(b"all") => o.update = false,
                Some(b"none") => o.no_clobber = true,
                Some(w) => return bad_attr(ctx, w, "--update").await,
            },
            "verbose" => o.verbose = true,
            "remove-destination" => o.remove_first = true,
            "backup" => backup = !matches!(v, Some(b"none" | b"off")),
            "suffix" => {
                suffix = v.unwrap_or_default().to_vec();
                backup = true;
            }
            _ => {}
        }
    }
    if backup {
        if let Some(v) = p.last(&SPEC, "backup")
            && !matches!(v, b"simple" | b"never")
        {
            // Numbered backups: the real cp, or a clear failure.
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            let msg = format!("unsupported backup type '{}' (psh built-in cp)", s(v));
            ctx.error(&msg).await;
            return 2;
        }
        o.backup = Some(suffix);
    }
    if o.recursive && !deref_given {
        o.deref = Deref::Never;
    }
    let plan = match targets(ctx, &p, "cp").await {
        Ok(plan) => plan,
        Err(st) => return st,
    };
    let parents = p.has(&SPEC, "parents");
    let mut ok = true;
    for (src, dst) in plan.pairs(parents) {
        if parents && let Err(st) = make_parents(ctx, &plan, &src).await {
            ok &= st;
            continue;
        }
        ok &= copy(ctx, src, dst, &o, true).await;
    }
    if ok { 0 } else { 1 }
}

async fn bad_attr(ctx: &mut UtilCtx<'_>, a: &[u8], opt: &str) -> i32 {
    let msg = format!("invalid argument '{}' for '{opt}'", s(a));
    common::usage(ctx, &msg).await
}

/// Sources and where they go (shared with `mv` and `ln`).
pub struct Plan {
    pub sources: Vec<Vec<u8>>,
    /// The directory the sources go into, or the one destination.
    pub target: Vec<u8>,
    pub into_dir: bool,
}

impl Plan {
    pub fn pairs(&self, parents: bool) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.sources
            .iter()
            .map(|src| {
                let dst = if !self.into_dir {
                    self.target.clone()
                } else if parents {
                    join(&self.target, src)
                } else {
                    join(&self.target, base_name(src))
                };
                (src.clone(), dst)
            })
            .collect()
    }
}

/// The GNU forms: `SRC DEST`, `SRC... DIR`, `-t DIR SRC...`, `-T SRC DEST`.
pub async fn targets(
    ctx: &mut UtilCtx<'_>,
    p: &crate::opts::Parsed,
    name: &str,
) -> Result<Plan, i32> {
    let spec_t = p
        .opts
        .iter()
        .rev()
        .find(|f| f.value.is_some() && is_target_opt(name, f.index))
        .and_then(|f| f.value.clone());
    let no_target = p.opts.iter().any(|f| is_no_target_opt(name, f.index));
    let ops = &p.operands;
    if let Some(dir) = spec_t {
        if no_target {
            let msg = "cannot combine --target-directory (-t) and --no-target-directory (-T)";
            return Err(common::usage(ctx, msg).await);
        }
        if ops.is_empty() {
            return Err(common::usage(ctx, "missing file operand").await);
        }
        if !ctx
            .stat(&dir, true)
            .await
            .is_ok_and(|m| m.kind == FileKind::Dir)
        {
            let e = ctx
                .stat(&dir, true)
                .await
                .err()
                .unwrap_or_else(|| io::ErrorKind::NotADirectory.into());
            ctx.error(&format!(
                "target directory '{}': {}",
                s(&dir),
                error_text(&e)
            ))
            .await;
            return Err(1);
        }
        return Ok(Plan {
            sources: ops.clone(),
            target: dir,
            into_dir: true,
        });
    }
    match ops.len() {
        0 => return Err(common::usage(ctx, "missing file operand").await),
        1 => {
            let msg = format!("missing destination file operand after '{}'", s(&ops[0]));
            return Err(common::usage(ctx, &msg).await);
        }
        _ => {}
    }
    let (last, sources) = ops.split_last().expect("two operands");
    if no_target {
        if ops.len() > 2 {
            let msg = format!("extra operand '{}'", s(&ops[2]));
            return Err(common::usage(ctx, &msg).await);
        }
        return Ok(Plan {
            sources: sources.to_vec(),
            target: last.clone(),
            into_dir: false,
        });
    }
    let st = ctx.stat(last, true).await;
    let is_dir = st.as_ref().is_ok_and(|m| m.kind == FileKind::Dir);
    if ops.len() > 2 && !is_dir {
        let e = st
            .err()
            .unwrap_or_else(|| io::ErrorKind::NotADirectory.into());
        let msg = format!("target '{}': {}", s(last), error_text(&e));
        ctx.error(&msg).await;
        return Err(1);
    }
    Ok(Plan {
        sources: sources.to_vec(),
        target: last.clone(),
        into_dir: is_dir,
    })
}

fn is_target_opt(name: &str, index: usize) -> bool {
    spec_of(name).opts[index].long == Some("target-directory")
}

fn is_no_target_opt(name: &str, index: usize) -> bool {
    spec_of(name).opts[index].long == Some("no-target-directory")
}

fn spec_of(name: &str) -> &'static Spec {
    match name {
        "mv" => &crate::mv::SPEC,
        "ln" => &crate::ln::SPEC,
        _ => &SPEC,
    }
}

/// `--parents`: the directories of `src` under the target.
async fn make_parents(ctx: &mut UtilCtx<'_>, plan: &Plan, src: &[u8]) -> Result<(), bool> {
    let mut cur = plan.target.clone();
    let dir = crate::fsutil::dir_name(src);
    if dir == b"." {
        return Ok(());
    }
    for c in dir.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
        cur = join(&cur, c);
        match ctx.stat(&cur, true).await {
            Ok(m) if m.kind == FileKind::Dir => continue,
            Ok(_) => {
                let e = io::Error::from(io::ErrorKind::NotADirectory);
                fail(ctx, "cannot make directory", &cur, &e).await;
                return Err(false);
            }
            Err(_) => {}
        }
        let mode = 0o777 & !ctx.umask();
        if let Err(e) = ctx.mkdir(&cur, mode).await {
            fail(ctx, "cannot make directory", &cur, &e).await;
            return Err(false);
        }
    }
    Ok(())
}

async fn fail(ctx: &mut UtilCtx<'_>, what: &str, f: &[u8], e: &io::Error) {
    let msg = format!("{what} '{}': {}", s(f), error_text(e));
    ctx.error(&msg).await;
}

/// `/dev/fd/N`, `/dev/stdin` and friends: the shell's own descriptors.
fn is_dev_fd(p: &[u8]) -> bool {
    matches!(p, b"/dev/stdin" | b"/dev/stdout" | b"/dev/stderr")
        || p.strip_prefix(b"/dev/fd/")
            .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit))
}

fn same(a: &Metadata, b: &Metadata) -> bool {
    a.dev == b.dev && a.ino == b.ino && a.ino != 0
}

/// Copies `src` to `dst` (a tree when recursive): whether it all went.
pub fn copy<'a, 'v>(
    ctx: &'a mut UtilCtx<'v>,
    src: Vec<u8>,
    dst: Vec<u8>,
    o: &'a CopyOpts,
    top: bool,
) -> LocalBoxFuture<'a, bool> {
    Box::pin(async move {
        ctx.check().await;
        // The shell's descriptors (`cp <(cmd) >(cmd)`): streams, not files.
        if is_dev_fd(&src) || is_dev_fd(&dst) {
            let m = Metadata {
                kind: FileKind::File,
                size: 0,
                mode: 0o666,
                mtime_ns: 0,
                dev: 0,
                ino: 0,
                uid: 0,
                gid: 0,
            };
            if let Err((what, e)) = copy_file(ctx, &src, &dst, &m, o).await {
                fail(ctx, what, &src, &e).await;
                return false;
            }
            return verbose(ctx, o, &src, &dst).await;
        }
        let follow = match o.deref {
            Deref::Always => true,
            Deref::Never => false,
            Deref::Args => top,
        };
        let (sm, sx) = match ctx.stat_extra(&src, follow).await {
            Ok(m) => m,
            Err(e) => {
                fail(ctx, "cannot stat", &src, &e).await;
                return false;
            }
        };
        let dm = ctx.stat(&dst, false).await.ok();
        if let Some(d) = &dm
            && same(&sm, d)
        {
            let msg = format!("'{}' and '{}' are the same file", s(&src), s(&dst));
            ctx.error(&msg).await;
            return false;
        }
        if sm.kind == FileKind::Dir && !o.link && !o.symlink {
            if !o.recursive {
                let msg = format!("-r not specified; omitting directory '{}'", s(&src));
                ctx.error(&msg).await;
                return false;
            }
            return copy_dir(ctx, &src, &dst, &sm, dm.as_ref(), o).await;
        }
        if let Some(d) = &dm {
            if o.no_clobber {
                return true;
            }
            if o.update && d.mtime_ns >= sm.mtime_ns {
                return true;
            }
            let dfollow = ctx.stat(&dst, true).await.ok();
            if dfollow.is_some_and(|m| m.kind == FileKind::Dir) {
                let msg = format!(
                    "cannot overwrite directory '{}' with non-directory '{}'",
                    s(&dst),
                    s(&src)
                );
                ctx.error(&msg).await;
                return false;
            }
            if let Some(suffix) = &o.backup {
                let mut b = dst.clone();
                b.extend_from_slice(suffix);
                if let Err(e) = ctx.rename(&dst, &b).await {
                    fail(ctx, "cannot backup", &dst, &e).await;
                    return false;
                }
            } else if o.remove_first || o.link || o.symlink || sm.kind == FileKind::Symlink {
                // Links replace the name; a symlink is never written through.
                if let Err(e) = ctx.remove_file(&dst).await {
                    fail(ctx, "cannot remove", &dst, &e).await;
                    return false;
                }
            }
        }
        let r = if o.link {
            ctx.hard_link(&src, &dst)
                .await
                .map_err(|e| ("cannot create hard link", e))
        } else if o.symlink {
            ctx.symlink(&src, &dst)
                .await
                .map_err(|e| ("cannot create symbolic link", e))
        } else {
            match sm.kind {
                FileKind::Symlink => match ctx.read_link(&src).await {
                    Ok(t) => ctx
                        .symlink(&t, &dst)
                        .await
                        .map_err(|e| ("cannot create symbolic link", e)),
                    Err(e) => Err(("cannot read symbolic link", e)),
                },
                FileKind::File => copy_file(ctx, &src, &dst, &sm, o).await,
                _ => Err((
                    "cannot create special file",
                    io::Error::from(io::ErrorKind::Unsupported),
                )),
            }
        };
        if let Err((what, e)) = r {
            fail(ctx, what, &dst, &e).await;
            return false;
        }
        if sm.kind != FileKind::Symlink && !o.link && !o.symlink {
            preserve(ctx, &dst, &sm, sx.atime_ns, o).await;
        }
        verbose(ctx, o, &src, &dst).await
    })
}

async fn verbose(ctx: &mut UtilCtx<'_>, o: &CopyOpts, src: &[u8], dst: &[u8]) -> bool {
    if o.verbose {
        let line = format!("'{}' -> '{}'\n", s(src), s(dst));
        return ctx.write(line.as_bytes()).await.is_ok();
    }
    true
}

async fn preserve(ctx: &mut UtilCtx<'_>, dst: &[u8], sm: &Metadata, atime: i64, o: &CopyOpts) {
    if o.mode {
        let _ = ctx.set_mode(dst, sm.mode).await;
    }
    if o.times {
        let _ = ctx
            .set_times(dst, Some(atime), Some(sm.mtime_ns), true)
            .await;
    }
}

async fn copy_file(
    ctx: &mut UtilCtx<'_>,
    src: &[u8],
    dst: &[u8],
    sm: &Metadata,
    o: &CopyOpts,
) -> Result<(), (&'static str, io::Error)> {
    let input = ctx
        .open(src, OpenOptions::read())
        .await
        .map_err(|e| ("cannot open", e))?;
    let create = OpenOptions {
        write: true,
        create: true,
        truncate: true,
        mode: Some(sm.mode & 0o777 & !ctx.umask()),
        ..OpenOptions::default()
    };
    let out = match ctx.open(dst, create).await {
        Ok(f) => f,
        // `-f`: a destination that cannot be opened is removed and made anew.
        Err(e) if o.force && e.kind() == io::ErrorKind::PermissionDenied => {
            ctx.remove_file(dst)
                .await
                .map_err(|_| ("cannot create regular file", e))?;
            ctx.open(dst, create)
                .await
                .map_err(|e| ("cannot create regular file", e))?
        }
        Err(e) => return Err(("cannot create regular file", e)),
    };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = input
            .read(&mut buf)
            .await
            .map_err(|e| ("error reading", e))?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&buf[..n])
            .await
            .map_err(|e| ("error writing", e))?;
        ctx.check().await;
    }
}

async fn copy_dir(
    ctx: &mut UtilCtx<'_>,
    src: &[u8],
    dst: &[u8],
    sm: &Metadata,
    dm: Option<&Metadata>,
    o: &CopyOpts,
) -> bool {
    match dm {
        Some(d) if d.kind == FileKind::Dir => {}
        Some(d)
            if d.kind == FileKind::Symlink
                && ctx
                    .stat(dst, true)
                    .await
                    .is_ok_and(|m| m.kind == FileKind::Dir) => {}
        Some(_) => {
            let msg = format!(
                "cannot overwrite non-directory '{}' with directory '{}'",
                s(dst),
                s(src)
            );
            ctx.error(&msg).await;
            return false;
        }
        None => {
            let inside = matches!(
                (resolve(ctx, src, Need::All).await, resolve(ctx, dst, Need::None).await),
                (Ok(a), Ok(b)) if is_below(&b, &a)
            );
            // Writable for us while it is filled; its own mode at the end.
            let mode = (sm.mode & 0o777 & !ctx.umask()) | 0o700;
            if let Err(e) = ctx.mkdir(dst, mode).await {
                fail(ctx, "cannot create directory", dst, &e).await;
                return false;
            }
            // Into itself: GNU makes the directory, then refuses to fill it.
            if inside {
                let msg = format!(
                    "cannot copy a directory, '{}', into itself, '{}'",
                    s(src),
                    s(dst)
                );
                ctx.error(&msg).await;
                return false;
            }
            if !verbose(ctx, o, src, dst).await {
                return false;
            }
        }
    }
    let entries = match ctx.read_dir(src).await {
        Ok(mut e) => {
            e.sort_by(|a, b| a.name.cmp(&b.name));
            e
        }
        Err(e) => {
            fail(ctx, "cannot access", src, &e).await;
            return false;
        }
    };
    let mut ok = true;
    for e in entries {
        ok &= copy(ctx, join(src, &e.name), join(dst, &e.name), o, false).await;
    }
    let mode = if o.mode {
        sm.mode
    } else {
        sm.mode & 0o777 & !ctx.umask()
    };
    if dm.is_none() || o.mode {
        let _ = ctx.set_mode(dst, mode).await;
    }
    if o.times {
        let atime = ctx
            .stat_extra(src, false)
            .await
            .map_or(sm.mtime_ns, |(_, x)| x.atime_ns);
        let _ = ctx
            .set_times(dst, Some(atime), Some(sm.mtime_ns), true)
            .await;
    }
    ok
}

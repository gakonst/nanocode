//! `find` with GNU semantics and BSD's extra options (D016): start paths (`.` when
//! none), an expression of tests, actions and operators, `-exec`/`-execdir` (`;` and
//! `+`), `-delete`, `-prune`, `-quit`, `-printf`, `-L`/`-H` with loop detection.
//!
//! The walk is find's own (D1171), over the utility's asynchronous file system, so
//! actions run in walk order between the steps of the walk: `-exec` commands go through
//! the shell's dispatch (utilities, standalone builtins, programs) as children of find,
//! stopped with it. Depth first, each directory's entries sorted by name (GNU walks in
//! directory order, which is not stable across file systems). Written for psh after
//! GNU's and BSD's manuals.

mod expr;
mod printf;

use std::future::Future;
use std::pin::Pin;

use psh_engine::host::{FileKind, Metadata, error_text};
use psh_engine::util::{SpawnEnv, UtilCtx};

use crate::common;
use crate::fsutil;
use crate::opts::{Spec, flag};
use crate::timefmt::NS;

use expr::{Cmp, Expr, Field, Follow, Parsed, PermHow, Ref};

/// The leading options (the expression is parsed by [`expr::parse`]).
pub const SPEC: Spec = Spec {
    name: "find",
    opts: &[
        flag('H'),
        flag('L'),
        flag('P'),
        flag('E'),
        flag('s'),
        flag('x'),
        flag('d'),
    ],
    posix_order: true,
};

/// The size of a `-exec … +` command line (GNU's default for xargs too).
const BATCH_LIMIT: usize = 128 * 1024;

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let parsed = match expr::parse(&argv[1..], ctx.utf8()) {
        Ok(p) => p,
        Err(expr::Error::Usage(msg)) => {
            ctx.error(&msg).await;
            return 1;
        }
        Err(expr::Error::Unsupported(opt)) => {
            if let Some(st) = ctx.run_program(argv.to_vec()).await {
                return st;
            }
            ctx.error(&format!("unsupported option '{opt}' (psh built-in find)"))
                .await;
            return 2;
        }
    };
    for w in &parsed.warnings {
        ctx.error(w).await;
    }
    let now = i64::try_from(ctx.now().as_nanos()).unwrap_or(i64::MAX);
    let mut refs = Vec::new();
    for r in &parsed.refs {
        match resolve(ctx, r, parsed.global.follow != Follow::Never, now).await {
            Ok(v) => refs.push(v),
            Err(msg) => {
                ctx.error(&msg).await;
                return 1;
            }
        }
    }
    let mut run = Run {
        p: &parsed,
        refs,
        batches: parsed.execs.iter().map(|_| Batch::default()).collect(),
        status: 0,
        quit: false,
        prune: false,
        write_status: None,
        now,
        root_dev: 0,
        kind_only: parsed.global.follow == Follow::Never
            && !parsed.global.xdev
            && kind_only(&parsed.expr),
    };
    for (i, root) in parsed.paths.iter().enumerate() {
        if run.quit {
            break;
        }
        let Some(f) = run.file(ctx, root.clone(), 0, i).await else {
            continue;
        };
        run.root_dev = f.meta.dev;
        let mut stack = Vec::new();
        run.visit(ctx, f, &mut stack).await;
    }
    for i in 0..run.batches.len() {
        run.flush(ctx, i).await;
    }
    if let Err(e) = ctx.flush().await
        && run.write_status.is_none()
    {
        run.write_status = Some(common::write_failed(ctx, &e).await);
    }
    run.write_status.unwrap_or(run.status)
}

/// A resolved reference: a time, or a file's identity.
enum RefVal {
    Time(i64),
    File(u64, u64),
}

async fn resolve(ctx: &mut UtilCtx<'_>, r: &Ref, follow: bool, now: i64) -> Result<RefVal, String> {
    let missing = |ctx: &UtilCtx<'_>, p: &[u8], e: &std::io::Error| {
        format!("{}: {}", common::quote(ctx, p), error_text(e))
    };
    match r {
        Ref::File(p, field) => {
            let (m, x) = match ctx.stat_extra(p, follow).await {
                Ok(v) => v,
                Err(e) => return Err(missing(ctx, p, &e)),
            };
            let t = match field {
                Field::Modify => Some(m.mtime_ns),
                Field::Access => Some(x.atime_ns),
                Field::Change => Some(x.ctime_ns),
                Field::Birth => x.btime_ns,
            };
            t.map(RefVal::Time)
                .ok_or_else(|| format!("{}: cannot determine birth time", common::quote(ctx, p)))
        }
        Ref::Date(s) => crate::timefmt::parse_date(ctx, s, now)
            .map(RefVal::Time)
            .ok_or_else(|| {
                format!(
                    "I cannot figure out how to interpret {} as a date or time",
                    common::quote(ctx, s)
                )
            }),
        Ref::Same(p) => match ctx.stat(p, follow).await {
            Ok(m) => Ok(RefVal::File(m.dev, m.ino)),
            Err(e) => Err(missing(ctx, p, &e)),
        },
    }
}

/// One file of the walk.
struct File {
    /// As shown: the root as given, joined with `/`.
    path: Vec<u8>,
    depth: usize,
    /// The index of its start path.
    root: usize,
    meta: Metadata,
    /// `meta` is the link's target's (`-L`, `-H` for a root).
    followed: bool,
}

/// The files collected for one `-exec … +`.
#[derive(Default)]
struct Batch {
    files: Vec<Vec<u8>>,
    size: usize,
    /// `-execdir … +`: the directory they are in.
    dir: Option<Vec<u8>>,
}

struct Run<'p> {
    p: &'p Parsed,
    refs: Vec<RefVal>,
    batches: Vec<Batch>,
    status: i32,
    quit: bool,
    /// Set by `-prune` while the current file is evaluated.
    prune: bool,
    /// A failed write ended find with this status.
    write_status: Option<i32>,
    now: i64,
    root_dev: u64,
    /// Nothing the walk does asks more of a file than its kind: where reading the
    /// directory told it, the file is not asked (no `lstat` per file).
    kind_only: bool,
}

/// Whether evaluating `e` reads only a file's path and kind.
fn kind_only(e: &Expr) -> bool {
    match e {
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Comma(a, b) => kind_only(a) && kind_only(b),
        Expr::Not(a) => kind_only(a),
        Expr::Const(_)
        | Expr::Name(_)
        | Expr::Path(_)
        | Expr::Regex(_)
        | Expr::Type(_, false)
        | Expr::Depth(..)
        | Expr::Prune
        | Expr::Quit
        | Expr::Print(_) => true,
        _ => false,
    }
}

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

impl<'p> Run<'p> {
    async fn fail(&mut self, ctx: &mut UtilCtx<'_>, path: &[u8], e: &std::io::Error) {
        let msg = format!("{}: {}", common::quote(ctx, path), error_text(e));
        ctx.error(&msg).await;
        self.status = 1;
    }

    /// The file at `path`: `lstat`, or `stat` where links are followed (a dangling link
    /// stays a link).
    async fn file(
        &mut self,
        ctx: &mut UtilCtx<'_>,
        path: Vec<u8>,
        depth: usize,
        root: usize,
    ) -> Option<File> {
        let follow = match self.p.global.follow {
            Follow::Always => true,
            Follow::Roots => depth == 0,
            Follow::Never => false,
        };
        if follow && let Ok(meta) = ctx.stat(&path, true).await {
            return Some(File {
                path,
                depth,
                root,
                meta,
                followed: true,
            });
        }
        match ctx.stat(&path, false).await {
            Ok(meta) => Some(File {
                path,
                depth,
                root,
                meta,
                followed: false,
            }),
            Err(e) => {
                self.fail(ctx, &path, &e).await;
                None
            }
        }
    }

    fn visit<'a>(
        &'a mut self,
        ctx: &'a mut UtilCtx<'_>,
        f: File,
        stack: &'a mut Vec<(u64, u64, Vec<u8>)>,
    ) -> Boxed<'a, ()> {
        Box::pin(async move {
            ctx.check().await;
            if self.quit {
                return;
            }
            let g = &self.p.global;
            let is_dir = f.meta.kind == FileKind::Dir;
            if is_dir
                && f.followed
                && let Some((_, _, first)) = stack
                    .iter()
                    .find(|(d, i, _)| *d == f.meta.dev && *i == f.meta.ino)
            {
                let msg = format!(
                    "File system loop detected; {} is part of the same file system loop as {}.",
                    common::quote(ctx, &f.path),
                    common::quote(ctx, first)
                );
                ctx.error(&msg).await;
                self.status = 1;
                return;
            }
            let shown = f.depth >= g.min_depth;
            let mut descend = is_dir && g.max_depth.is_none_or(|m| f.depth < m);
            let expr = &self.p.expr;
            if !g.depth_first && shown {
                self.prune = false;
                self.eval_one(ctx, expr, &f).await;
                if self.prune {
                    descend = false;
                }
            }
            if descend && g.xdev && f.meta.dev != self.root_dev {
                descend = false;
            }
            if descend && !self.quit {
                match ctx.read_dir_kinds(&f.path).await {
                    Err(e) => self.fail(ctx, &f.path, &e).await,
                    Ok(mut names) => {
                        names.sort_by(|a, b| a.0.name.cmp(&b.0.name));
                        stack.push((f.meta.dev, f.meta.ino, f.path.clone()));
                        for (n, kind) in names {
                            if self.quit {
                                break;
                            }
                            let path = fsutil::join(&f.path, &n.name);
                            let child = match kind {
                                Some(kind) if self.kind_only => Some(File {
                                    path,
                                    depth: f.depth + 1,
                                    root: f.root,
                                    meta: Metadata {
                                        kind,
                                        size: 0,
                                        mode: 0,
                                        mtime_ns: 0,
                                        dev: 0,
                                        ino: 0,
                                        uid: 0,
                                        gid: 0,
                                    },
                                    followed: false,
                                }),
                                _ => self.file(ctx, path, f.depth + 1, f.root).await,
                            };
                            match child {
                                Some(child) if child.meta.kind == FileKind::Dir => {
                                    self.visit(ctx, child, stack).await
                                }
                                Some(child) => self.leaf(ctx, child).await,
                                None => {}
                            }
                        }
                        stack.pop();
                    }
                }
            }
            if g.depth_first && shown && !self.quit {
                self.eval_one(ctx, expr, &f).await;
            }
        })
    }

    /// [`Self::visit`] for a file that is not a directory: nothing below it, so no
    /// future on the heap for it.
    async fn leaf(&mut self, ctx: &mut UtilCtx<'_>, f: File) {
        ctx.check().await;
        if self.quit || f.depth < self.p.global.min_depth {
            return;
        }
        if !self.p.global.depth_first {
            self.prune = false;
        }
        let expr = &self.p.expr;
        self.eval_one(ctx, expr, &f).await;
    }

    /// [`Self::eval`] without a future on the heap for a single test.
    async fn eval_one(&mut self, ctx: &mut UtilCtx<'_>, e: &'p Expr, f: &File) -> bool {
        match e {
            Expr::And(..) | Expr::Or(..) | Expr::Comma(..) | Expr::Not(_) => {
                self.eval(ctx, e, f).await
            }
            _ => self.test(ctx, e, f).await,
        }
    }

    fn eval<'a>(
        &'a mut self,
        ctx: &'a mut UtilCtx<'_>,
        e: &'p Expr,
        f: &'a File,
    ) -> Boxed<'a, bool> {
        Box::pin(async move {
            match e {
                Expr::And(a, b) => {
                    self.eval_one(ctx, a, f).await && !self.quit && self.eval_one(ctx, b, f).await
                }
                Expr::Or(a, b) => {
                    self.eval_one(ctx, a, f).await || (!self.quit && self.eval_one(ctx, b, f).await)
                }
                Expr::Comma(a, b) => {
                    self.eval_one(ctx, a, f).await;
                    !self.quit && self.eval_one(ctx, b, f).await
                }
                Expr::Not(a) => !self.eval_one(ctx, a, f).await,
                _ => self.test(ctx, e, f).await,
            }
        })
    }

    /// The time a test compares, through links where the walk follows them.
    async fn time(&self, ctx: &UtilCtx<'_>, field: Field, f: &File) -> Option<i64> {
        if field == Field::Modify {
            return Some(f.meta.mtime_ns);
        }
        let (_, x) = ctx.stat_extra(&f.path, f.followed).await.ok()?;
        match field {
            Field::Access => Some(x.atime_ns),
            Field::Change => Some(x.ctime_ns),
            _ => x.btime_ns,
        }
    }

    async fn test(&mut self, ctx: &mut UtilCtx<'_>, e: &'p Expr, f: &File) -> bool {
        let m = &f.meta;
        match e {
            Expr::Const(b) => *b,
            Expr::Name(g) => g.matches(fsutil::base_name(&f.path)),
            Expr::Path(g) => g.matches(&f.path),
            Expr::Lname(g) => {
                if m.kind != FileKind::Symlink {
                    return false;
                }
                ctx.read_link(&f.path).await.is_ok_and(|t| g.matches(&t))
            }
            Expr::Regex(r) => r
                .find(&f.path, psh_regex::ExecOptions::default())
                .is_some_and(|(s, e)| s == 0 && e == f.path.len()),
            Expr::Type(letters, other) => {
                let kind = if !*other {
                    m.kind
                } else if f.followed {
                    match ctx.stat(&f.path, false).await {
                        Ok(l) => l.kind,
                        Err(_) => m.kind,
                    }
                } else if m.kind == FileKind::Symlink {
                    match ctx.stat(&f.path, true).await {
                        Ok(t) => t.kind,
                        Err(_) => FileKind::Symlink,
                    }
                } else {
                    m.kind
                };
                letters.contains(&printf::type_letter(kind))
            }
            Expr::Size(cmp, n, unit) => {
                cmp.test(i128::from(m.size.div_ceil(*unit)), i128::from(*n))
            }
            Expr::Age(field, cmp, n, unit) => {
                let Some(t) = self.time(ctx, *field, f).await else {
                    return false;
                };
                let age = i128::from(self.now) - i128::from(t);
                let span = i128::from(*unit) * i128::from(NS);
                let n = i128::from(*n);
                if *unit == 86400 {
                    // Whole days: the fraction is dropped.
                    cmp.test(age.div_euclid(span), n)
                } else {
                    // Minutes: `N` is the minute that is not over yet (GNU).
                    match cmp {
                        Cmp::Lt => age < n * span,
                        Cmp::Gt => age > n * span,
                        Cmp::Eq => age > (n - 1) * span && age <= n * span,
                    }
                }
            }
            Expr::Newer(field, i) => {
                let RefVal::Time(r) = self.refs[*i] else {
                    return false;
                };
                self.time(ctx, *field, f).await.is_some_and(|t| t > r)
            }
            Expr::Perm(how, want) => {
                let mode = m.mode & 0o7777;
                match how {
                    PermHow::Exact => mode == *want,
                    PermHow::All => mode & want == *want,
                    PermHow::Any => *want == 0 || mode & want != 0,
                }
            }
            Expr::Empty => match m.kind {
                FileKind::File => m.size == 0,
                FileKind::Dir => ctx.read_dir(&f.path).await.is_ok_and(|d| d.is_empty()),
                _ => false,
            },
            Expr::Links(cmp, n) => match ctx.stat_extra(&f.path, f.followed).await {
                Ok((_, x)) => cmp.test(i128::from(x.nlink), i128::from(*n)),
                Err(_) => false,
            },
            Expr::Inum(cmp, n) => cmp.test(i128::from(m.ino), i128::from(*n)),
            Expr::Uid(cmp, n) => cmp.test(i128::from(m.uid), i128::from(*n)),
            Expr::Gid(cmp, n) => cmp.test(i128::from(m.gid), i128::from(*n)),
            Expr::User(name) => ctx.user_name(m.uid).as_deref() == Some(name.as_slice()),
            Expr::Group(name) => ctx.group_name(m.gid).as_deref() == Some(name.as_slice()),
            Expr::NoUser => ctx.user_name(m.uid).is_none(),
            Expr::NoGroup => ctx.group_name(m.gid).is_none(),
            Expr::Access(a) => ctx.access(&f.path, *a).await.unwrap_or(false),
            Expr::SameFile(i) => {
                matches!(self.refs[*i], RefVal::File(d, n) if d == m.dev && n == m.ino)
            }
            Expr::Depth(cmp, n) => cmp.test(f.depth as i128, i128::from(*n)),
            Expr::Prune => {
                self.prune = true;
                true
            }
            Expr::Quit => {
                self.quit = true;
                true
            }
            Expr::Print(end) => {
                let mut line = f.path.clone();
                line.push(*end);
                self.write(ctx, &line).await;
                true
            }
            Expr::Printf(fmt) => {
                let out = self.printf(ctx, fmt, f).await;
                self.write(ctx, &out).await;
                true
            }
            Expr::Exec(i) => self.exec(ctx, *i, f).await,
            Expr::Delete => self.delete(ctx, f).await,
            Expr::And(..) | Expr::Or(..) | Expr::Not(..) | Expr::Comma(..) => {
                self.eval(ctx, e, f).await
            }
        }
    }

    async fn write(&mut self, ctx: &mut UtilCtx<'_>, data: &[u8]) {
        if let Err(e) = ctx.write(data).await {
            self.write_status = Some(common::write_failed(ctx, &e).await);
            self.quit = true;
        }
    }

    async fn printf(&self, ctx: &mut UtilCtx<'_>, fmt: &[u8], f: &File) -> Vec<u8> {
        let needs = printf::needs(fmt);
        let extra = if needs.extra {
            ctx.stat_extra(&f.path, f.followed)
                .await
                .ok()
                .map(|(_, x)| x)
        } else {
            None
        };
        let link = if needs.link && f.meta.kind == FileKind::Symlink {
            ctx.read_link(&f.path).await.ok()
        } else {
            None
        };
        let followed = if needs.follow && f.meta.kind == FileKind::Symlink {
            Some(match ctx.stat(&f.path, true).await {
                Ok(t) => printf::type_letter(t.kind),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => b'N',
                Err(_) => b'?',
            })
        } else {
            None
        };
        let facts = printf::Facts {
            path: &f.path,
            root: &self.p.paths[f.root],
            depth: f.depth,
            meta: &f.meta,
            extra,
            link,
            followed,
        };
        printf::format(ctx, fmt, &facts)
    }

    /// `-exec` and `-execdir`: `;` runs the command now and is true when it succeeds;
    /// `+` collects the file and is true.
    async fn exec(&mut self, ctx: &mut UtilCtx<'_>, i: usize, f: &File) -> bool {
        let x = &self.p.execs[i];
        let (dir, name) = if x.in_dir {
            let mut name = b"./".to_vec();
            name.extend_from_slice(fsutil::base_name(&f.path));
            (Some(ctx.path(&fsutil::dir_name(&f.path))), name)
        } else {
            (None, f.path.clone())
        };
        if !x.batch {
            let argv = x.argv.iter().map(|a| replace(a, &name)).collect();
            let env = SpawnEnv {
                cwd: dir,
                ..SpawnEnv::default()
            };
            let mut child = ctx.spawn(argv, env).await;
            let st = child.wait().await;
            if st == 126 || st == 127 {
                self.status = 1;
            }
            return st == 0;
        }
        let fixed: usize = x.argv.iter().map(|a| a.len() + 1).sum();
        let b = &self.batches[i];
        if !b.files.is_empty() && (b.dir != dir || fixed + b.size + name.len() + 1 > BATCH_LIMIT) {
            self.flush(ctx, i).await;
        }
        let b = &mut self.batches[i];
        b.size += name.len() + 1;
        b.files.push(name);
        b.dir = dir;
        true
    }

    /// Runs a `-exec … +` command with the files collected so far; a failure makes
    /// find's status 1.
    async fn flush(&mut self, ctx: &mut UtilCtx<'_>, i: usize) {
        let b = std::mem::take(&mut self.batches[i]);
        if b.files.is_empty() {
            return;
        }
        let argv = self.p.execs[i]
            .argv
            .iter()
            .cloned()
            .chain(b.files)
            .collect();
        let env = SpawnEnv {
            cwd: b.dir,
            ..SpawnEnv::default()
        };
        let mut child = ctx.spawn(argv, env).await;
        if child.wait().await != 0 {
            self.status = 1;
        }
    }

    async fn delete(&mut self, ctx: &mut UtilCtx<'_>, f: &File) -> bool {
        // GNU leaves the start directory `.` alone.
        if f.path == b"." {
            return true;
        }
        let r = if f.meta.kind == FileKind::Dir && !f.followed {
            ctx.remove_dir(&f.path).await
        } else {
            ctx.remove_file(&f.path).await
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                let msg = format!(
                    "cannot delete {}: {}",
                    common::quote(ctx, &f.path),
                    error_text(&e)
                );
                ctx.error(&msg).await;
                self.status = 1;
                false
            }
        }
    }
}

/// Every `{}` in an argument replaced by the file name.
fn replace(arg: &[u8], name: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(arg.len());
    let mut i = 0;
    while i < arg.len() {
        if arg[i..].starts_with(b"{}") {
            out.extend_from_slice(name);
            i += 2;
        } else {
            out.push(arg[i]);
            i += 1;
        }
    }
    out
}

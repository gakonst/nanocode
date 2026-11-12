//! The search (D1137): operands, GNU's recursion on the walker seam, one search per
//! file, and the exit status (0 a line was selected, 1 none, 2 an error, unless `-q`
//! selected one).

use std::cell::RefCell;
use std::io::{self, Read};

use ignore::WalkState;
use psh_engine::host::{FileKind, error_text};
use psh_engine::util::SyncCtx;

use super::pattern::{LineMatcher, Matchers, Pats};
use super::sink::{GrepSink, Out};
use super::{BinaryFiles, Config, Dirs, List};
use crate::matcher::{LineTerminator, Matcher};
use crate::search::walk::{WalkOptions, Walker};
use crate::searcher::{BinaryDetection, Searcher, SearcherBuilder};

pub(crate) fn run_sync(ctx: &mut SyncCtx, (cfg, m): (Config, Matchers)) -> i32 {
    let out = RefCell::new(Out::new(
        ctx.writer(1),
        ctx.writer(2),
        cfg.line_buffered || cfg.terminal,
    ));
    let mut run = Run {
        cfg: &cfg,
        spans: m.spans.as_ref(),
        out: &out,
        matched: false,
        error: false,
        stop: false,
        head: Vec::new(),
    };
    if cfg.nothing {
        let mut out = out.borrow_mut();
        if cfg.list == Some(List::NonMatching) {
            let ops = if cfg.operands.is_empty() {
                vec![cfg.label.clone()]
            } else {
                cfg.operands.clone()
            };
            for op in ops {
                let _ = out.write(&op);
                let _ = out.write(if cfg.null { b"\0" } else { b"\n" });
            }
        }
        let _ = out.flush();
        return 1;
    }
    let mut searcher = searcher(&cfg);
    match &m.lines {
        LineMatcher::Fast(rm) => run.all(ctx, rm, &mut searcher),
        LineMatcher::Each(p) => run.all(ctx, p, &mut searcher),
    }
    let (matched, error) = (run.matched, run.error);
    let mut out = out.borrow_mut();
    let _ = out.flush();
    match out.failed() {
        Some(io::ErrorKind::BrokenPipe) => return 141,
        Some(kind) => {
            let e: io::Error = kind.into();
            out.error(&format!("write error: {}", error_text(&e)));
            return 2;
        }
        None => {}
    }
    if matched && cfg.quiet {
        0
    } else if error {
        2
    } else if matched {
        0
    } else {
        1
    }
}

fn searcher(cfg: &Config) -> Searcher {
    let shows_lines = !cfg.quiet && !cfg.discard && cfg.list.is_none() && !cfg.count;
    let mut b = SearcherBuilder::new();
    b.line_terminator(LineTerminator::byte(if cfg.null_data {
        b'\0'
    } else {
        b'\n'
    }))
    .invert_match(cfg.invert)
    .line_number(cfg.line_number || cfg.context)
    .before_context(if shows_lines { cfg.before } else { 0 })
    .after_context(if shows_lines { cfg.after } else { 0 })
    .max_matches(cfg.max_count)
    .bom_sniffing(false)
    .binary_detection(if cfg.binary == BinaryFiles::Text || cfg.null_data {
        BinaryDetection::none()
    } else {
        BinaryDetection::convert(b'\0')
    });
    b.build()
}

/// The first read of each file (GNU's buffer size).
const HEAD: usize = 96 * 1024;

struct Run<'a> {
    cfg: &'a Config,
    spans: Option<&'a Pats>,
    out: &'a RefCell<Out>,
    matched: bool,
    error: bool,
    /// `-q` found a line, or stdout failed.
    stop: bool,
    head: Vec<u8>,
}

impl Run<'_> {
    fn all<M: Matcher>(&mut self, ctx: &mut SyncCtx, m: &M, s: &mut Searcher) {
        let cfg = self.cfg;
        let recursive = cfg.dirs == Dirs::Recurse;
        let implicit = cfg.operands.is_empty() && recursive;
        let operands = if !cfg.operands.is_empty() {
            cfg.operands.clone()
        } else if recursive {
            vec![b".".to_vec()]
        } else {
            vec![b"-".to_vec()]
        };
        let several = operands.len() > 1;
        for op in &operands {
            if self.stop {
                break;
            }
            if op == b"-" {
                let show = cfg.with_filename.unwrap_or(several);
                let mut rdr = FlushingStdin { ctx, out: self.out };
                self.search(m, s, &mut rdr, &cfg.label, show);
                continue;
            }
            self.operand(ctx, m, s, op, several, implicit);
        }
    }

    fn operand<M: Matcher>(
        &mut self,
        ctx: &SyncCtx,
        m: &M,
        s: &mut Searcher,
        op: &[u8],
        several: bool,
        implicit: bool,
    ) {
        let cfg = self.cfg;
        let meta = match ctx.stat(op, true) {
            Ok(meta) => meta,
            Err(e) => return self.file_error(op, &e),
        };
        match meta.kind {
            FileKind::Dir => match cfg.dirs {
                Dirs::Skip => {}
                Dirs::Read => {
                    let e = io::Error::new(io::ErrorKind::IsADirectory, "Is a directory");
                    self.file_error(op, &e);
                }
                Dirs::Recurse => {
                    if implicit || !excluded_dir(cfg, &suffixes(op)) {
                        self.walk(ctx, m, s, op, implicit);
                    }
                }
            },
            FileKind::CharDevice | FileKind::BlockDevice | FileKind::Fifo | FileKind::Socket
                if cfg.skip_devices => {}
            _ => {
                if included(cfg, &suffixes(op)) {
                    let show = cfg.with_filename.unwrap_or(several);
                    self.file(ctx, m, s, op, op, show);
                }
            }
        }
    }

    /// GNU's recursion: hidden files too, no ignore files, symbolic links followed with
    /// `-R` only; devices, FIFOs and sockets found on the way are skipped.
    fn walk<M: Matcher>(
        &mut self,
        ctx: &SyncCtx,
        m: &M,
        s: &mut Searcher,
        root: &[u8],
        implicit: bool,
    ) {
        let cfg = self.cfg;
        let opts = WalkOptions {
            hidden: false,
            ignore: false,
            git_ignore: false,
            git_exclude: false,
            git_global: false,
            parents: false,
            follow_links: cfg.deref,
            ..WalkOptions::default()
        };
        let walker = match Walker::new(ctx, opts) {
            Ok(w) => w,
            Err(e) => {
                self.error = true;
                self.out.borrow_mut().error(&e.to_string());
                return;
            }
        };
        let show = cfg.with_filename.unwrap_or(true);
        walker.run(&[root.to_vec()], &mut |r| {
            if self.stop {
                return WalkState::Quit;
            }
            let e = match r {
                Ok(e) => e,
                Err(err) => {
                    self.walk_error(&err, implicit);
                    return WalkState::Continue;
                }
            };
            if e.depth == 0 {
                return WalkState::Continue;
            }
            if e.is_dir() {
                return if excluded_dir(cfg, &[e.file_name()]) {
                    WalkState::Skip
                } else {
                    WalkState::Continue
                };
            }
            if e.is_file() && included(cfg, &[e.file_name()]) {
                let shown = display(&e.path, implicit);
                self.file(ctx, m, s, &e.abs, shown, show);
            }
            if self.stop {
                WalkState::Quit
            } else {
                WalkState::Continue
            }
        });
    }

    fn file<M: Matcher>(
        &mut self,
        ctx: &SyncCtx,
        m: &M,
        s: &mut Searcher,
        path: &[u8],
        shown: &[u8],
        show: bool,
    ) {
        match ctx.open(path) {
            Ok(mut f) => self.search(m, s, &mut f, shown, show),
            Err(e) => self.file_error(shown, &e),
        }
    }

    fn search<M: Matcher, R: Read>(
        &mut self,
        m: &M,
        s: &mut Searcher,
        mut rdr: R,
        name: &[u8],
        show: bool,
    ) {
        let cfg = self.cfg;
        // GNU decides on binary data per buffer it reads, the first one before any
        // output; the searcher's decoder hands its first bytes over in pieces, so the
        // first buffer is read here.
        let mut head = std::mem::take(&mut self.head);
        head.resize(HEAD, 0);
        let n = loop {
            match rdr.read(&mut head) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.head = head;
                    return self.file_error(name, &e);
                }
            }
        };
        let binary = cfg.binary != BinaryFiles::Text && !cfg.null_data && head[..n].contains(&0);
        let (count, suppressed) = if binary && cfg.binary == BinaryFiles::WithoutMatch {
            (0, false)
        } else {
            let mut sink = GrepSink::new(cfg, self.spans, self.out, name, show);
            sink.binary_seen = binary;
            let r = s.search_reader(m, io::Cursor::new(&head[..n]).chain(rdr), &mut sink);
            if let Err(e) = r {
                if self.out.borrow().failed().is_some() {
                    self.stop = true;
                    return;
                }
                self.file_error(name, &e);
            }
            if sink.binary_skip {
                (0, false)
            } else {
                (sink.count, sink.suppressed)
            }
        };
        self.head = head;
        if count > 0 {
            self.matched = true;
            if cfg.quiet {
                self.stop = true;
                return;
            }
        }
        let colors = cfg.colors.as_ref();
        let fname = colors.map_or("", |c| c.fname.as_str());
        let mut out = self.out.borrow_mut();
        let r = (|| -> io::Result<()> {
            match cfg.list {
                Some(List::Matching) if count > 0 => out.colored(colors, fname, name)?,
                Some(List::NonMatching) if count == 0 => out.colored(colors, fname, name)?,
                Some(_) => return Ok(()),
                None if cfg.count => {
                    if show {
                        out.colored(colors, fname, name)?;
                        if cfg.null {
                            out.write(b"\0")?;
                        } else {
                            out.colored(colors, colors.map_or("", |c| c.se.as_str()), b":")?;
                        }
                    }
                    out.write(count.to_string().as_bytes())?;
                    return out.write(b"\n");
                }
                None => return Ok(()),
            }
            out.write(if cfg.null { b"\0" } else { b"\n" })
        })();
        if r.is_err() {
            self.stop = true;
            return;
        }
        if suppressed {
            out.error(&format!(
                "{}: binary file matches",
                String::from_utf8_lossy(name)
            ));
        }
    }

    fn file_error(&mut self, name: &[u8], e: &io::Error) {
        self.error = true;
        if !self.cfg.no_messages {
            let msg = format!("{}: {}", String::from_utf8_lossy(name), error_text(e));
            self.out.borrow_mut().error(&msg);
        }
    }

    /// A walk error: `PATH: reason` (an error), or a loop (a warning).
    fn walk_error(&mut self, err: &ignore::Error, implicit: bool) {
        let shown = |p: &std::path::Path| {
            let b = crate::search::walk::os_bytes(p.as_os_str());
            String::from_utf8_lossy(display(&b, implicit)).into_owned()
        };
        let (path, io, looped) = dissect(err);
        if let Some(child) = looped {
            if !self.cfg.no_messages {
                let msg = format!("{}: warning: recursive directory loop", shown(child));
                self.out.borrow_mut().error(&msg);
            }
            return;
        }
        self.error = true;
        if self.cfg.no_messages {
            return;
        }
        let text = match io {
            Some(e) => error_text(inner_io(e)),
            None => err.to_string(),
        };
        let msg = match path {
            Some(p) => format!("{}: {text}", shown(p)),
            None => text,
        };
        self.out.borrow_mut().error(&msg);
    }
}

/// The path, I/O error and loop (its child) in a walk error.
fn dissect(
    e: &ignore::Error,
) -> (
    Option<&std::path::Path>,
    Option<&io::Error>,
    Option<&std::path::Path>,
) {
    match e {
        ignore::Error::WithPath { path, err } => {
            let (p, io, l) = dissect(err);
            (p.or(Some(path)), io, l)
        }
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            dissect(err)
        }
        ignore::Error::Loop { child, .. } => (None, None, Some(child)),
        ignore::Error::Io(io) => (None, Some(io), None),
        ignore::Error::Partial(errs) => errs.first().map_or((None, None, None), dissect),
        _ => (None, None, None),
    }
}

/// walkdir wraps the OS error (whose text GNU prints) in its own.
fn inner_io(e: &io::Error) -> &io::Error {
    e.get_ref()
        .and_then(|inner| inner.source())
        .and_then(|s| s.downcast_ref::<io::Error>())
        .unwrap_or(e)
}

/// A walked path as GNU prints it: without the `./` of an implicit `.`.
fn display(path: &[u8], implicit: bool) -> &[u8] {
    if implicit {
        path.strip_prefix(b"./").unwrap_or(path)
    } else {
        path
    }
}

/// The name suffixes `--exclude` matches a command-line file by: the whole name and
/// each part after a `/`.
fn suffixes(name: &[u8]) -> Vec<&[u8]> {
    let mut out = vec![name];
    for (i, &b) in name.iter().enumerate() {
        if b == b'/' && i + 1 < name.len() && name[i + 1] != b'/' {
            out.push(&name[i + 1..]);
        }
    }
    out
}

/// `--include`/`--exclude`: the last glob that matches decides; without one, a file is
/// searched unless the first glob is an `--include`.
fn included(cfg: &Config, names: &[&[u8]]) -> bool {
    for (include, g) in cfg.filters.iter().rev() {
        if names
            .iter()
            .any(|n| g.is_match(crate::search::walk::to_path(n)))
        {
            return *include;
        }
    }
    cfg.filters.first().is_none_or(|(include, _)| !include)
}

fn excluded_dir(cfg: &Config, names: &[&[u8]]) -> bool {
    cfg.exclude_dirs.iter().any(|g| {
        names
            .iter()
            .any(|n| g.is_match(crate::search::walk::to_path(n)))
    })
}

/// stdin for the searcher; stdout is flushed before each read (D1081), so a pipeline
/// reading grep's output does not wait for grep's input.
struct FlushingStdin<'a> {
    ctx: &'a mut SyncCtx,
    out: &'a RefCell<Out>,
}

impl Read for FlushingStdin<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let _ = self.out.borrow_mut().flush();
        self.ctx.read(buf)
    }
}

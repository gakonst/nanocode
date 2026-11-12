// Adapted from fd (https://github.com/sharkdp/fd, commit 193d0df), src/walk.rs (the
// walker setup, the filters of `spawn_senders`, the receiver's buffering and exit
// rules, `broken_symlink_from_err`, `search_str_for_entry`), src/filetypes.rs and
// src/filesystem.rs (`is_empty`), with the psh patches marked `psh:` (D1138):
// Copyright (c) 2017-present The fd developers. Licensed under the MIT License (see NOTICE).

// psh: the body of the blocking utility (D1130). The walk is psh's walker seam (sorted,
// on this thread) instead of `WalkParallel` with sender threads and a receiver thread;
// the filters run in the visitor and the receiver's logic follows each match.
// `--base-directory` is the walk's cwd (the process's never changes). With a command
// (`-x`, `-X`, `-l`) the results are collected for the front end, which runs them.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::search::ignore::{Error, WalkState};
use psh_engine::host::{DirEntry, FileKind, Metadata};
use psh_engine::util::SyncCtx;
use psh_pattern::bytes::RegexSet;

use super::cli::FileType;
use super::colors::Facts;
use super::filter::{OwnerFilter, SizeFilter, TimeFilter};
use super::output::{PrintConfig, Printed, print_entry, stripped_path};
use super::patterns::Patterns;
use crate::search::walk::{Entry, WalkHost, WalkOptions, Walker, join};

/// Maximum size of the output buffer before flushing results to the console
const MAX_BUFFER_LENGTH: usize = 1000;

/// fd's `Config`, as the body needs it.
pub struct Config {
    pub roots: Vec<Vec<u8>>,
    /// The walk's working directory: the shell's, or `--base-directory`'s (absolute).
    pub cwd: Vec<u8>,
    pub patterns: Patterns,
    /// For `--full-path`: what relative paths are joined to.
    pub full_path_base: Option<Vec<u8>>,
    pub walk: WalkOptions,
    pub min_depth: Option<usize>,
    pub prune: bool,
    pub quiet: bool,
    pub max_results: Option<usize>,
    pub file_types: Option<FileTypes>,
    pub extensions: Option<RegexSet>,
    pub size_constraints: Vec<SizeFilter>,
    pub time_constraints: Vec<TimeFilter>,
    pub owner_constraint: Option<OwnerFilter>,
    pub ignore_contain: Vec<Vec<u8>>,
    pub show_filesystem_errors: bool,
    pub print: PrintConfig,
    /// With a command: the (stripped) results go here instead of stdout.
    pub results: Option<Arc<Mutex<Vec<Vec<u8>>>>>,
}

/// The walk's host: the utility's, with the base directory as cwd.
struct Host<'a> {
    ctx: &'a SyncCtx,
    cwd: &'a [u8],
}

impl WalkHost for Host<'_> {
    fn cwd(&self) -> &[u8] {
        self.cwd
    }
    fn var(&self, name: &[u8]) -> Option<&[u8]> {
        self.ctx.var(name)
    }
    fn native_path(&self, abs: &[u8]) -> Option<io::Result<PathBuf>> {
        self.ctx.native_path(abs)
    }
    fn cancelled(&self) -> bool {
        self.ctx.cancelled()
    }
    fn stat(&self, abs: &[u8], follow: bool) -> io::Result<Metadata> {
        self.ctx.stat(abs, follow)
    }
    fn read_dir(&self, abs: &[u8]) -> io::Result<Vec<DirEntry>> {
        self.ctx.read_dir(abs)
    }
    fn read_file(&self, abs: &[u8]) -> io::Result<Vec<u8>> {
        WalkHost::read_file(self.ctx, abs)
    }
}

/// A result: the entry, or a broken symbolic link the walk reported as an error.
struct Found {
    path: Vec<u8>,
    abs: Vec<u8>,
    depth: Option<usize>,
    kind: Option<FileKind>,
}

/// Whether or not to show
#[derive(Default, Debug, PartialEq, Eq)]
pub struct FileTypes {
    pub files: bool,
    pub directories: bool,
    pub symlinks: bool,
    pub block_devices: bool,
    pub char_devices: bool,
    pub sockets: bool,
    pub pipes: bool,
    pub executables_only: bool,
    pub empty_only: bool,
}

impl FileTypes {
    pub fn from_types(types: &[FileType]) -> Self {
        let mut file_types = FileTypes::default();
        for value in types {
            match value {
                FileType::File => file_types.files = true,
                FileType::Directory => file_types.directories = true,
                FileType::Symlink => file_types.symlinks = true,
                FileType::Executable => {
                    file_types.executables_only = true;
                    file_types.files = true;
                }
                FileType::Empty => file_types.empty_only = true,
                FileType::BlockDevice => file_types.block_devices = true,
                FileType::CharDevice => file_types.char_devices = true,
                FileType::Socket => file_types.sockets = true,
                FileType::Pipe => file_types.pipes = true,
            }
        }

        // If only 'empty' was specified, search for both files and directories:
        if file_types.empty_only && !(file_types.files || file_types.directories) {
            file_types.files = true;
            file_types.directories = true;
        }

        file_types
    }
}

/// The receiver's state: fd's `ReceiverBuffer`.
struct Receiver<'c> {
    config: &'c Config,
    out: psh_engine::util::SyncWriter,
    err: psh_engine::util::SyncWriter,
    buf: Vec<u8>,
    streaming: bool,
    buffer: Vec<(Vec<u8>, bool, Option<super::colors::Style>)>,
    num_results: usize,
    /// The walk stopped: the exit status.
    done: Option<i32>,
}

impl Receiver<'_> {
    fn error(&mut self, msg: &str) {
        let line = format!("[fd error]: {}\n", super::sanitize_error(msg));
        let _ = self.err.write_all(line.as_bytes());
    }

    fn walk_error(&mut self, e: &Error) {
        if self.config.show_filesystem_errors {
            self.error(&e.to_string());
        }
    }

    /// A match: fd's `poll` for one entry.
    fn entry(&mut self, path: Vec<u8>, is_dir: bool, style: Option<super::colors::Style>) {
        if self.config.quiet {
            self.done = Some(0);
            return;
        }
        if let Some(results) = &self.config.results {
            let p = stripped_path(&path, &self.config.print).to_vec();
            results.lock().unwrap_or_else(|e| e.into_inner()).push(p);
        } else if self.streaming {
            self.print(&path, is_dir, style);
        } else {
            self.buffer.push((path, is_dir, style));
            if self.buffer.len() > MAX_BUFFER_LENGTH {
                self.stream();
            }
        }
        self.num_results += 1;
        if let Some(max_results) = self.config.max_results
            && self.num_results >= max_results
        {
            self.stop();
        }
    }

    fn print(&mut self, path: &[u8], is_dir: bool, style: Option<super::colors::Style>) {
        let e = Printed {
            path,
            is_dir,
            style,
        };
        print_entry(&mut self.buf, &e, &self.config.print);
        if self.buf.len() >= 8192 || self.config.print.interactive_terminal {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.done.is_some() || self.buf.is_empty() {
            return;
        }
        if let Err(e) = self.out.write_all(&self.buf) {
            if e.kind() != io::ErrorKind::BrokenPipe {
                self.error(&format!("Could not write to output: {e}"));
            }
            // Probably a broken pipe. Exit gracefully.
            self.done = Some(1);
        }
        self.buf.clear();
    }

    /// Switch ourselves into streaming mode.
    fn stream(&mut self) {
        self.streaming = true;
        let mut buffer = std::mem::take(&mut self.buffer);
        // fd sorts what it buffered by path (`Path`'s order: by components).
        buffer.sort_by(|a, b| components(&a.0).cmp(components(&b.0)));
        for (path, is_dir, style) in buffer {
            self.print(&path, is_dir, style);
        }
        self.flush();
    }

    /// Stop looping.
    fn stop(&mut self) {
        if !self.streaming {
            self.stream();
        }
        self.flush();
        if self.done.is_none() {
            self.done = Some(if self.config.quiet {
                i32::from(self.num_results == 0)
            } else {
                0
            });
        }
    }
}

/// `Path::components` on bytes, for sorting.
fn components(p: &[u8]) -> impl Iterator<Item = &[u8]> {
    p.split(|&b| super::output::is_separator(b))
        .enumerate()
        .filter(|(i, c)| !c.is_empty() && (*c != b"." || *i == 0))
        .map(|(_, c)| c)
}

pub fn run_sync(ctx: &mut SyncCtx, config: Box<Config>) -> i32 {
    let config = &*config;
    let out = ctx.writer(1);
    let err = ctx.writer(2);
    let mut rx = Receiver {
        config,
        out,
        err,
        buf: Vec::new(),
        streaming: false,
        buffer: Vec::new(),
        num_results: 0,
        done: None,
    };
    let host = Host {
        ctx,
        cwd: &config.cwd,
    };
    let walker = match Walker::new(&host, config.walk.clone()) {
        Ok(w) => w,
        Err(e) => {
            rx.error(&format!("Malformed exclude pattern: {e}"));
            return 1;
        }
    };
    let follow = config.walk.follow_links;
    // The errors of explicit ignore files come first: fd reported them before the walk
    // (the front end did).
    let mut started = false;
    walker.run(&config.roots, &mut |r| {
        if rx.done.is_some() {
            return WalkState::Quit;
        }
        if r.is_err() && !started {
            return WalkState::Continue;
        }
        started = true;
        let entry = match r {
            Ok(e) => {
                if let Some(err) = &e.err {
                    rx.walk_error(err);
                }
                // If the entry is a directory that contains a
                // "ignore contain" file", we want to skip this
                // directory.
                if e.is_dir()
                    && config
                        .ignore_contain
                        .iter()
                        .any(|ic| host.stat(&join(&e.abs, ic), true).is_ok())
                {
                    return WalkState::Skip;
                }
                if e.depth == 0 {
                    // Skip the root directory entry.
                    return WalkState::Continue;
                }
                found(e)
            }
            Err(err) => match broken_symlink_from_err(&host, err) {
                Ok(f) => f,
                Err(err) => {
                    rx.walk_error(&err);
                    return WalkState::Continue;
                }
            },
        };

        if let Some(min_depth) = config.min_depth
            && entry.depth.is_none_or(|d| d < min_depth)
        {
            return WalkState::Continue;
        }

        // Check the name first, since it doesn't require metadata
        let search_str = search_str_for_entry(&entry.path, config.full_path_base.as_deref());
        if !config.patterns.matches_path(&search_str) {
            return WalkState::Continue;
        }

        // Filter out unwanted extensions.
        if let Some(ref exts_regex) = config.extensions {
            match file_name(&entry.path) {
                Some(name) if exts_regex.is_match(name) => {}
                _ => return WalkState::Continue,
            }
        }

        // The entry's metadata, as `ignore`'s `DirEntry::metadata` (links followed only
        // with -L).
        let mut meta_cache: Option<Option<Metadata>> = None;
        let mut metadata = || -> Option<Metadata> {
            *meta_cache.get_or_insert_with(|| host.stat(&entry.abs, follow).ok())
        };

        // Filter out unwanted file types.
        if let Some(ref file_types) = config.file_types
            && should_ignore(file_types, &host, &entry, &mut metadata)
        {
            return WalkState::Continue;
        }

        if let Some(ref owner_constraint) = config.owner_constraint {
            match metadata() {
                Some(m) if owner_constraint.matches(&m) => {}
                _ => return WalkState::Continue,
            }
        }

        // Filter out unwanted sizes if it is a file and we have been given size constraints.
        if !config.size_constraints.is_empty() {
            // `Path::is_file` follows links.
            let is_file = host
                .stat(&entry.abs, true)
                .is_ok_and(|m| m.kind == FileKind::File);
            if !is_file {
                return WalkState::Continue;
            }
            match metadata() {
                Some(m) => {
                    if config
                        .size_constraints
                        .iter()
                        .any(|sc| !sc.is_within(m.size))
                    {
                        return WalkState::Continue;
                    }
                }
                None => return WalkState::Continue,
            }
        }

        // Filter out unwanted modification times
        if !config.time_constraints.is_empty() {
            let matched = metadata().is_some_and(|m| {
                config
                    .time_constraints
                    .iter()
                    .all(|tf| tf.applies_to(m.mtime_ns))
            });
            if !matched {
                return WalkState::Continue;
            }
        }

        let is_dir = entry.kind == Some(FileKind::Dir);
        let style = match &config.print.ls_colors {
            Some(c) if config.results.is_none() => {
                let name = last_component(&entry.path);
                let mode = if c.needs_mode() {
                    metadata().map(|m| m.mode)
                } else {
                    None
                };
                let target_exists = entry.kind == Some(FileKind::Symlink)
                    && c.needs_target()
                    && host.stat(&entry.abs, true).is_ok();
                let facts = Facts {
                    name,
                    kind: entry.kind,
                    mode,
                    target_exists,
                };
                c.style_for(&facts).copied()
            }
            _ => None,
        };
        rx.entry(entry.path, is_dir, style);
        if rx.done.is_some() {
            return WalkState::Quit;
        }

        // Apply pruning.
        if config.prune {
            return WalkState::Skip;
        }

        WalkState::Continue
    });
    if rx.done.is_none() {
        rx.stop();
    }
    rx.done.unwrap_or(0)
}

fn found(e: Entry) -> Found {
    Found {
        path: e.path,
        abs: e.abs,
        depth: Some(e.depth),
        kind: Some(e.kind),
    }
}

/// The name a path ends with (`Path::file_name`).
fn file_name(path: &[u8]) -> Option<&[u8]> {
    let name = last_component(path);
    (!name.is_empty() && name != b"." && name != b"..").then_some(name)
}

/// The last component, whatever it is.
fn last_component(path: &[u8]) -> &[u8] {
    let end = path.len()
        - path
            .iter()
            .rev()
            .take_while(|&&b| super::output::is_separator(b))
            .count();
    let p = &path[..end];
    let start = p
        .iter()
        .rposition(|&b| super::output::is_separator(b))
        .map_or(0, |i| i + 1);
    &p[start..]
}

fn should_ignore(
    t: &FileTypes,
    host: &Host<'_>,
    entry: &Found,
    metadata: &mut dyn FnMut() -> Option<Metadata>,
) -> bool {
    let Some(kind) = entry.kind else {
        return true;
    };
    let is = |k: FileKind| kind == k;
    (!t.files && is(FileKind::File))
        || (!t.directories && is(FileKind::Dir))
        || (!t.symlinks && is(FileKind::Symlink))
        || (!t.block_devices && is(FileKind::BlockDevice))
        || (!t.char_devices && is(FileKind::CharDevice))
        || (!t.sockets && is(FileKind::Socket))
        || (!t.pipes && is(FileKind::Fifo))
        // psh: "executable" by the permission bits (fd asks access(2) for the user).
        || (t.executables_only
            && !host
                .stat(&entry.abs, true)
                .is_ok_and(|m| m.mode & 0o111 != 0))
        || (t.empty_only && !is_empty(host, entry, metadata))
        || is(FileKind::Other)
}

fn is_empty(
    host: &Host<'_>,
    entry: &Found,
    metadata: &mut dyn FnMut() -> Option<Metadata>,
) -> bool {
    match entry.kind {
        Some(FileKind::Dir) => host.read_dir(&entry.abs).is_ok_and(|d| d.is_empty()),
        Some(FileKind::File) => metadata().is_some_and(|m| m.size == 0),
        _ => false,
    }
}

/// If `err` is for a broken symlink, create a broken symlink entry,
/// otherwise, return the original error.
fn broken_symlink_from_err(host: &Host<'_>, err: Error) -> Result<Found, Error> {
    let mut depth = None;
    let mut path: Option<&std::path::Path> = None;
    let mut current = &err;
    // Loop through the items in the chain, and extract the path and depth as we find them.
    // Then once we get to the (terminal) io error, check that it is for a broken symlink.
    loop {
        match current {
            Error::WithPath { err, path: p } => {
                path = Some(p);
                current = err;
            }
            Error::WithDepth { err, depth: d } => {
                depth = Some(*d);
                current = err;
            }
            Error::Io(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(path) = path {
                    let shown = crate::search::walk::os_bytes(path.as_os_str());
                    let abs = host.abs(&shown);
                    if host
                        .stat(&abs, false)
                        .is_ok_and(|m| m.kind == FileKind::Symlink)
                    {
                        return Ok(Found {
                            path: shown,
                            abs,
                            depth,
                            kind: Some(FileKind::Symlink),
                        });
                    }
                }
                return Err(err);
            }
            _ => return Err(err),
        }
    }
}

/// What the patterns match: the file name, or the absolute path with `--full-path`.
fn search_str_for_entry(entry_path: &[u8], full_path_base: Option<&[u8]>) -> Vec<u8> {
    if let Some(cwd) = full_path_base {
        // If full_path_base is some, that means that we need to return
        // the absolute path
        if crate::search::walk::is_absolute(entry_path) {
            return entry_path.to_vec();
        }
        let path = super::fmt::strip_current_dir(entry_path);
        join(cwd, path)
    } else {
        last_component(entry_path).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_str_for_entry_with_relative_path() {
        assert_eq!(
            search_str_for_entry(b"foo/bar", Some(b"/home/user")),
            b"/home/user/foo/bar"
        );
    }

    #[test]
    fn search_str_for_entry_strips_dot_prefix() {
        assert_eq!(
            search_str_for_entry(b"./foo/bar", Some(b"/home/user")),
            b"/home/user/foo/bar"
        );
    }

    #[test]
    fn search_str_for_entry_with_absolute_path() {
        assert_eq!(
            search_str_for_entry(b"/absolute/path", Some(b"/home/user")),
            b"/absolute/path"
        );
    }

    #[test]
    fn search_str_no_base_dir() {
        assert_eq!(search_str_for_entry(b"./foo/bar", None), b"bar");
        assert_eq!(search_str_for_entry(b"foo/bar", None), b"bar");
        assert_eq!(search_str_for_entry(b"foo", None), b"foo");
    }

    #[test]
    fn empty_only_implies_files_and_directories() {
        let ft = FileTypes::from_types(&[FileType::Empty]);
        assert!(ft.empty_only && ft.files && ft.directories);
        let ft = FileTypes::from_types(&[FileType::Empty, FileType::File]);
        assert!(ft.empty_only && ft.files && !ft.directories);
        let ft = FileTypes::from_types(&[FileType::Executable]);
        assert!(ft.executables_only && ft.files && !ft.directories);
    }

    #[test]
    fn path_order() {
        let mut v: Vec<&[u8]> = vec![b"./a.b", b"./a/b", b"./a", b"./-x"];
        v.sort_by(|a, b| components(a).cmp(components(b)));
        assert_eq!(v, vec![&b"./-x"[..], b"./a", b"./a/b", b"./a.b"]);
    }
}

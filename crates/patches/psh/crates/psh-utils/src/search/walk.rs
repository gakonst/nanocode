//! The walker seam (D1132): one interface, two walks with the same results.
//!
//! - **Native** (the file system is the process's own: the thread backend): `ignore`'s
//!   `Walk`, sorted by file name, over absolute OS roots, with `current_dir` set to the
//!   shell's cwd; paths are mapped back to the user's spelling for display.
//! - **Virtual** (wasm, sandbox hosts, or where the native walk would read the wrong
//!   global gitignore): [`super::vwalk`], a single-threaded walk over [`WalkHost`]
//!   that runs `ignore`'s matchers with the ignore chain of `ignore`'s `dir.rs`.
//!
//! Both visit in the same order: depth first, entries of a directory sorted by name
//! (ripgrep's `--sort path`), so output is deterministic. A visitor may skip a
//! directory or stop the walk; a cancelled command stops it too.

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use ignore::overrides::{Override, OverrideBuilder};
use ignore::types::Types;
use ignore::{Error, WalkBuilder, WalkState};
use psh_engine::host::{DirEntry, FileKind, Metadata};
use psh_engine::util::SyncCtx;

/// What a walk needs from its environment: the shell's cwd and environment and a
/// synchronous file system on absolute shell paths. [`SyncCtx`] is one; tests use
/// `std::fs` directly.
pub trait WalkHost {
    /// The shell's working directory (absolute).
    fn cwd(&self) -> &[u8];
    fn var(&self, name: &[u8]) -> Option<&[u8]>;
    /// The OS path for an absolute shell path when the file system is the process's
    /// own (then the native walker may run), else `None`.
    fn native_path(&self, abs: &[u8]) -> Option<io::Result<PathBuf>>;
    fn cancelled(&self) -> bool;
    fn stat(&self, abs: &[u8], follow: bool) -> io::Result<Metadata>;
    fn read_dir(&self, abs: &[u8]) -> io::Result<Vec<DirEntry>>;
    fn read_file(&self, abs: &[u8]) -> io::Result<Vec<u8>>;

    /// A shell path made absolute against the cwd.
    fn abs(&self, p: &[u8]) -> Vec<u8> {
        if is_absolute(p) {
            p.to_vec()
        } else {
            join(self.cwd(), p)
        }
    }
}

impl WalkHost for SyncCtx {
    fn cwd(&self) -> &[u8] {
        SyncCtx::cwd(self)
    }

    fn var(&self, name: &[u8]) -> Option<&[u8]> {
        SyncCtx::var(self, name)
    }

    fn native_path(&self, abs: &[u8]) -> Option<io::Result<PathBuf>> {
        SyncCtx::native_path(self, abs)
    }

    fn cancelled(&self) -> bool {
        SyncCtx::cancelled(self)
    }

    fn stat(&self, abs: &[u8], follow: bool) -> io::Result<Metadata> {
        SyncCtx::stat(self, abs, follow)
    }

    fn read_dir(&self, abs: &[u8]) -> io::Result<Vec<DirEntry>> {
        SyncCtx::read_dir(self, abs)
    }

    fn read_file(&self, abs: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        self.open(abs)?.read_to_end(&mut out)?;
        Ok(out)
    }
}

/// The walk's filters: `ignore`'s `WalkBuilder` settings, defaults as there.
#[derive(Clone, Debug)]
pub struct WalkOptions {
    /// Skip hidden files and directories.
    pub hidden: bool,
    /// Read `.ignore` files.
    pub ignore: bool,
    pub git_ignore: bool,
    /// Read `.git/info/exclude`.
    pub git_exclude: bool,
    /// Read the global gitignore (located from the shell's environment).
    pub git_global: bool,
    /// Ignore files of parent directories apply.
    pub parents: bool,
    /// git rules apply only inside a repository.
    pub require_git: bool,
    pub ignore_case_insensitive: bool,
    /// Ignore files read like `.ignore`, higher precedence first last (`.rgignore`).
    pub custom_ignore_filenames: Vec<String>,
    /// Explicit ignore files (`--ignore-file`), shell paths.
    pub ignore_files: Vec<Vec<u8>>,
    /// Override globs (`-g`; `true`: case-insensitive, `--iglob`), relative to
    /// [`WalkOptions::glob_root`].
    pub globs: Vec<(String, bool)>,
    /// What `globs` are relative to (a shell path); the cwd when `None`.
    pub glob_root: Option<Vec<u8>>,
    pub types: Option<Types>,
    pub max_depth: Option<usize>,
    pub min_depth: Option<usize>,
    pub follow_links: bool,
    pub same_file_system: bool,
    pub max_filesize: Option<u64>,
    /// Use the virtual walker even on the process's file system (tests).
    pub force_virtual: bool,
    /// Word I/O errors as `ignore`'s parallel walker does: `PATH: reason`, without
    /// walkdir's "IO error for operation on PATH: " (rg with more than one thread).
    pub parallel_errors: bool,
}

impl Default for WalkOptions {
    fn default() -> WalkOptions {
        WalkOptions {
            hidden: true,
            ignore: true,
            git_ignore: true,
            git_exclude: true,
            git_global: true,
            parents: true,
            require_git: true,
            ignore_case_insensitive: false,
            custom_ignore_filenames: Vec::new(),
            ignore_files: Vec::new(),
            globs: Vec::new(),
            glob_root: None,
            types: None,
            max_depth: None,
            min_depth: None,
            follow_links: false,
            same_file_system: false,
            max_filesize: None,
            force_virtual: false,
            parallel_errors: false,
        }
    }
}

/// One file or directory the walk found.
#[derive(Debug)]
pub struct Entry {
    /// The path as the user spelled the root, joined with `/`.
    pub path: Vec<u8>,
    /// The absolute shell path (for [`WalkHost`] and `SyncCtx` file operations).
    pub abs: Vec<u8>,
    /// The OS path, from the native walker.
    pub native: Option<PathBuf>,
    /// 0 for a root.
    pub depth: usize,
    /// The type as the walk sees it: a symbolic link unless links are followed.
    pub kind: FileKind,
    /// The path itself is a symbolic link.
    pub path_is_symlink: bool,
    /// Errors reading this directory's ignore files (the walk went on).
    pub err: Option<Error>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Dir
    }

    pub fn is_file(&self) -> bool {
        self.kind == FileKind::File
    }

    /// The file name (the root's own spelling for a root).
    pub fn file_name(&self) -> &[u8] {
        if self.depth == 0 {
            return &self.path;
        }
        let i = self
            .path
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(0, |i| i + 1);
        &self.path[i..]
    }
}

/// A configured walk.
pub struct Walker<'h> {
    host: &'h dyn WalkHost,
    opts: WalkOptions,
}

impl<'h> Walker<'h> {
    /// Checks the globs (an error is the user's, reported before any walking).
    pub fn new(host: &'h dyn WalkHost, opts: WalkOptions) -> Result<Walker<'h>, Error> {
        build_overrides(&to_path(&host.abs(b".")), &opts.globs)?;
        Ok(Walker { host, opts })
    }

    /// Whether this walk runs natively (`ignore::Walk`).
    pub fn is_native(&self) -> bool {
        let o = &self.opts;
        if o.force_virtual || !o.ignore_files.is_empty() {
            // Explicit ignore files match against the walk's own spelling of paths,
            // which is absolute natively: only the virtual walk gets them right.
            return false;
        }
        if self.host.native_path(self.host.cwd()).is_none() {
            return false;
        }
        // `ignore` locates the global gitignore from the process's environment.
        !o.git_global || process_env_matches(self.host)
    }

    /// Walks `roots` (shell paths, as the user spelled them) in order, calling `visit`
    /// for each entry and error. `visit` returns `Skip` to not descend into a
    /// directory and `Quit` to stop; the walk's result says whether it ran to the end.
    pub fn run(
        &self,
        roots: &[Vec<u8>],
        visit: &mut dyn FnMut(Result<Entry, Error>) -> WalkState,
    ) -> WalkState {
        if self.is_native() {
            self.run_native(roots, visit)
        } else {
            super::vwalk::run(self.host, &self.opts, roots, visit)
        }
    }

    fn run_native(
        &self,
        roots: &[Vec<u8>],
        visit: &mut dyn FnMut(Result<Entry, Error>) -> WalkState,
    ) -> WalkState {
        let host = self.host;
        let o = &self.opts;
        let native = |p: &[u8]| -> io::Result<PathBuf> {
            host.native_path(&host.abs(p))
                .unwrap_or_else(|| Err(io::ErrorKind::Unsupported.into()))
                .map(|p| normalize_native(&p))
        };
        let Ok(cwd) = native(b".") else {
            return super::vwalk::run(host, o, roots, visit);
        };
        // Each root: its OS path, its spelling and its absolute shell path.
        let mut mapped = Vec::new();
        for r in roots {
            match native(r) {
                Ok(n) => mapped.push((n, r.clone(), host.abs(r))),
                Err(e) => {
                    let err = Error::WithPath {
                        path: to_path(r),
                        err: Box::new(Error::WithDepth {
                            depth: 0,
                            err: Box::new(Error::Io(e)),
                        }),
                    };
                    if visit(Err(err)) == WalkState::Quit {
                        return WalkState::Quit;
                    }
                }
            }
        }
        let mut b = WalkBuilder::empty();
        for (n, _, _) in &mapped {
            b.add(n);
        }
        b.hidden(o.hidden)
            .ignore(o.ignore)
            .git_ignore(o.git_ignore)
            .git_exclude(o.git_exclude)
            .git_global(o.git_global)
            .parents(o.parents)
            .require_git(o.require_git)
            .ignore_case_insensitive(o.ignore_case_insensitive)
            .max_depth(o.max_depth)
            .follow_links(o.follow_links)
            .same_file_system(o.same_file_system)
            .max_filesize(o.max_filesize)
            .current_dir(cwd.clone())
            .sort_by_file_name(|a, b| a.cmp(b));
        for name in &o.custom_ignore_filenames {
            b.add_custom_ignore_filename(name);
        }
        let glob_root = match &o.glob_root {
            Some(r) => native(r).unwrap_or_else(|_| cwd.clone()),
            None => cwd.clone(),
        };
        // Checked in `new`.
        if let Ok(ov) = build_overrides(&glob_root, &o.globs) {
            b.overrides(ov);
        }
        if let Some(t) = &o.types {
            b.types(t.clone());
        }
        let map = |p: &Path, cur: usize| -> Option<(Vec<u8>, Vec<u8>)> {
            // Under the current root first (roots may nest), then any.
            let order = std::iter::once(cur).chain(0..mapped.len());
            for i in order {
                let Some((n, spelling, abs)) = mapped.get(i) else {
                    continue;
                };
                if let Ok(rel) = p.strip_prefix(n) {
                    let rel = path_bytes(rel);
                    if rel.is_empty() {
                        return Some((spelling.clone(), abs.clone()));
                    }
                    return Some((join(spelling, &rel), join(abs, &rel)));
                }
            }
            None
        };
        let mut cur = 0;
        let mut seen_root = false;
        // The directory the visitor skipped: what is under it is not visited.
        let mut skipped: Option<PathBuf> = None;
        for r in b.build() {
            if host.cancelled() {
                return WalkState::Quit;
            }
            let item = match r {
                Err(e) => {
                    // Its read error belongs to a skipped directory too.
                    if let (Some(s), Error::WithPath { path, .. }) = (&skipped, &e)
                        && path.starts_with(s)
                    {
                        continue;
                    }
                    Err(map_error(e, o.parallel_errors, &|p| {
                        map(p, cur).map(|(d, _)| to_path(&d))
                    }))
                }
                Ok(dent) => {
                    if let Some(s) = &skipped {
                        if dent.path().starts_with(s) {
                            continue;
                        }
                        skipped = None;
                    }
                    if dent.depth() == 0 {
                        // The next root with this path (the same root may be given
                        // twice, spelled differently).
                        let from = if seen_root { cur + 1 } else { 0 };
                        if let Some(i) = (from..mapped.len()).find(|&i| mapped[i].0 == dent.path())
                        {
                            cur = i;
                        }
                        seen_root = true;
                    }
                    // Shallower entries are walked but not visited: the minimum is
                    // applied here, not by `ignore`, whose `Walk` would then skip
                    // the root's own ignore files.
                    if o.min_depth.is_some_and(|m| dent.depth() < m) {
                        continue;
                    }
                    let Some((path, abs)) = map(dent.path(), cur) else {
                        continue;
                    };
                    let kind = dent.file_type().map_or(FileKind::Other, file_kind);
                    let err = dent.error().map(|e| {
                        map_error(e.clone(), o.parallel_errors, &|p| {
                            map(p, cur).map(|(d, _)| to_path(&d))
                        })
                    });
                    Ok(Entry {
                        path,
                        abs,
                        native: Some(dent.path().to_path_buf()),
                        depth: dent.depth(),
                        kind,
                        path_is_symlink: dent.path_is_symlink(),
                        err,
                    })
                }
            };
            let dir = item
                .as_ref()
                .ok()
                .filter(|e| e.is_dir())
                .and_then(|e| e.native.clone());
            match visit(item) {
                WalkState::Quit => return WalkState::Quit,
                WalkState::Skip => skipped = dir,
                WalkState::Continue => {}
            }
        }
        WalkState::Continue
    }
}

/// Whether the process environment names the same global gitignore as the shell's
/// (then `ignore`, which reads the process's, finds the right one).
fn process_env_matches(host: &dyn WalkHost) -> bool {
    const VARS: &[&str] = &[
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "GIT_CONFIG_GLOBAL",
        "GIT_CONFIG_SYSTEM",
    ];
    VARS.iter().all(|name| {
        let process = std::env::var_os(name).map(|v| os_bytes(&v));
        process.as_deref() == host.var(name.as_bytes())
    })
}

/// The override matcher for `globs`, relative to `root`.
pub fn build_overrides(root: &Path, globs: &[(String, bool)]) -> Result<Override, Error> {
    let mut b = OverrideBuilder::new(root);
    for (glob, icase) in globs {
        b.case_insensitive(*icase)?;
        b.add(glob)?;
    }
    b.build()
}

/// Rewrites the paths in a walk error (`map` gives the display path of an OS path).
/// `plain`: walkdir's wording of I/O errors is dropped ([`WalkOptions::parallel_errors`]).
fn map_error(e: Error, plain: bool, map: &dyn Fn(&Path) -> Option<PathBuf>) -> Error {
    let mp = |p: PathBuf| map(&p).unwrap_or(p);
    match e {
        Error::Partial(errs) => {
            Error::Partial(errs.into_iter().map(|e| map_error(e, plain, map)).collect())
        }
        Error::WithLineNumber { line, err } => Error::WithLineNumber {
            line,
            err: Box::new(map_error(*err, plain, map)),
        },
        Error::WithPath { path, err } => {
            let shown = mp(path.clone());
            let err = map_error(*err, plain, map);
            Error::WithPath {
                err: Box::new(reword_walkdir(err, &path, &shown, plain)),
                path: shown,
            }
        }
        Error::WithDepth { depth, err } => Error::WithDepth {
            depth,
            err: Box::new(map_error(*err, plain, map)),
        },
        Error::Loop { ancestor, child } => Error::Loop {
            ancestor: mp(ancestor),
            child: mp(child),
        },
        e => e,
    }
}

/// walkdir names the path in its I/O messages ("IO error for operation on PATH: ..."):
/// the display path replaces the OS one there too.
fn reword_walkdir(e: Error, os: &Path, shown: &Path, plain: bool) -> Error {
    match e {
        Error::WithDepth { depth, err } => Error::WithDepth {
            depth,
            err: Box::new(reword_walkdir(*err, os, shown, plain)),
        },
        Error::Io(io) if io.get_ref().is_some() => {
            let msg = io.to_string();
            let prefix = format!("IO error for operation on {}: ", os.display());
            match msg.strip_prefix(&prefix) {
                Some(rest) if plain => Error::Io(io::Error::new(io.kind(), rest.to_string())),
                Some(rest) => Error::Io(io::Error::new(
                    io.kind(),
                    format!("IO error for operation on {}: {rest}", shown.display()),
                )),
                None => Error::Io(io),
            }
        }
        e => e,
    }
}

pub(crate) fn file_kind(t: std::fs::FileType) -> FileKind {
    if t.is_dir() {
        return FileKind::Dir;
    }
    if t.is_file() {
        return FileKind::File;
    }
    if t.is_symlink() {
        return FileKind::Symlink;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if t.is_fifo() {
            return FileKind::Fifo;
        }
        if t.is_socket() {
            return FileKind::Socket;
        }
        if t.is_block_device() {
            return FileKind::BlockDevice;
        }
        if t.is_char_device() {
            return FileKind::CharDevice;
        }
    }
    FileKind::Other
}

/// An OS path without `.` components (so `ignore`'s prefix matching sees the same
/// relative paths as with a relative root) and without a trailing separator.
fn normalize_native(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        if c != std::path::Component::CurDir {
            out.push(c);
        }
    }
    out
}

pub(crate) fn is_absolute(p: &[u8]) -> bool {
    p.first() == Some(&b'/') || (p.len() >= 2 && p[0].is_ascii_alphabetic() && p[1] == b':')
}

/// `a` and `b` joined with one `/` (none added after a trailing `/`).
pub fn join(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = a.to_vec();
    if !out.is_empty() && out.last() != Some(&b'/') {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// A relative OS path as `/`-separated bytes.
fn path_bytes(p: &Path) -> Vec<u8> {
    let mut out = Vec::new();
    for c in p.components() {
        if !out.is_empty() {
            out.push(b'/');
        }
        out.extend_from_slice(&os_bytes(c.as_os_str()));
    }
    out
}

pub(crate) fn os_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        s.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// Shell path bytes as a `Path` (lossy where the platform's paths are not bytes).
pub fn to_path(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

// The ignore chain below (`Ignore`, `add_parents`, `add_child_path`, `matched`,
// `matched_ignore`, `create_gitignore`, `resolve_git_commondir`) is adapted from the
// `ignore` crate's `dir.rs`, and the traversal follows `ignore`'s `walk.rs` and
// walkdir's `lib.rs`, in ripgrep (https://github.com/BurntSushi/ripgrep, commit
// 91f5d22) and walkdir 2.5.0 (https://github.com/BurntSushi/walkdir):
// Copyright (c) 2015 Andrew Gallant. Licensed under the MIT License (see NOTICE).

//! The virtual walk (D1132): `ignore::Walk`'s traversal and ignore chain over a
//! [`WalkHost`] file system, so that wasm and sandbox hosts get ripgrep's filtering.
//! `ignore`'s matchers (`gitignore`, `overrides`, `types`) do the matching; only the
//! chain that feeds them, which in `ignore` reads `std::fs`, is re-implemented here.
//!
//! Paths are matched as `ignore` matches the paths of a walk rooted at the user's
//! spelling (`./src/a.rs`), with parent directories canonicalized (lexically here:
//! there is no `realpath` in [`WalkHost`]).

use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::Override;
use ignore::types::Types;
use ignore::{Error, Match, WalkState};
use psh_engine::host::FileKind;

use super::gitconfig;
use super::walk::{Entry, WalkHost, WalkOptions, build_overrides, join, to_path};

/// What every directory's matcher shares.
struct Shared {
    hidden: bool,
    ignore: bool,
    parents: bool,
    git_global: bool,
    git_ignore: bool,
    git_exclude: bool,
    require_git: bool,
    case_insensitive: bool,
    custom_names: Vec<String>,
    overrides: Override,
    types: Types,
    global: Gitignore,
    explicit: Vec<Gitignore>,
}

/// The rules of one directory (`IgnoreInner`).
struct Node {
    /// The directory as the walk spells it (absolute for absolute parents).
    dir: Vec<u8>,
    parent: Option<Rc<Node>>,
    is_absolute_parent: bool,
    has_absolute_parent_rules: bool,
    custom: Gitignore,
    ignore: Gitignore,
    git_ignore: Gitignore,
    git_exclude: Gitignore,
    has_git: bool,
}

#[derive(Clone)]
struct Ignore {
    node: Rc<Node>,
    /// The canonical path of the root this chain was built for (`add_parents`).
    absolute_base: Option<Rc<PathBuf>>,
}

/// Collects partial errors (`PartialErrorBuilder`).
#[derive(Default)]
struct Errs(Vec<Error>);

impl Errs {
    fn push(&mut self, e: Error) {
        self.0.push(e);
    }

    fn maybe_push(&mut self, e: Option<Error>) {
        if let Some(e) = e {
            self.0.push(e);
        }
    }

    fn into_option(mut self) -> Option<Error> {
        match self.0.len() {
            0 => None,
            1 => self.0.pop(),
            _ => Some(Error::Partial(self.0)),
        }
    }
}

fn empty(dir: &Path) -> Gitignore {
    GitignoreBuilder::new(dir)
        .build()
        .unwrap_or_else(|_| Gitignore::empty())
}

struct Chain<'h> {
    host: &'h dyn WalkHost,
    shared: Shared,
}

impl Chain<'_> {
    fn read(&self, p: &[u8]) -> io::Result<Vec<u8>> {
        self.host.read_file(&self.host.abs(p))
    }

    fn root(&self) -> Ignore {
        Ignore {
            node: Rc::new(Node {
                dir: Vec::new(),
                parent: None,
                is_absolute_parent: true,
                has_absolute_parent_rules: false,
                custom: Gitignore::empty(),
                ignore: Gitignore::empty(),
                git_ignore: Gitignore::empty(),
                git_exclude: Gitignore::empty(),
                has_git: false,
            }),
            absolute_base: None,
        }
    }

    /// A matcher from the ignore files `names` in `file_dir`, for paths below `dir`.
    /// I/O errors are ignored, as in `ignore`.
    fn create_gitignore(
        &self,
        dir: &[u8],
        file_dir: &[u8],
        names: &[&str],
    ) -> (Gitignore, Option<Error>) {
        let dir_path = to_path(dir);
        let mut b = GitignoreBuilder::new(&dir_path);
        let mut errs = Errs::default();
        let _ = b.case_insensitive(self.shared.case_insensitive);
        for name in names {
            let gipath = join(file_dir, name.as_bytes());
            if let Ok(data) = self.read(&gipath) {
                errs.maybe_push(add_lines(&mut b, &to_path(&gipath), &data));
            }
        }
        let gi = match b.build() {
            Ok(gi) => gi,
            Err(e) => {
                errs.push(e);
                empty(&dir_path)
            }
        };
        (gi, errs.into_option())
    }

    /// `git_dir` for `info/exclude`: `.git`, or for a worktree (`.git` is a file) the
    /// common directory it names.
    fn resolve_git_commondir(
        &self,
        dir: &[u8],
        git_type: Option<FileKind>,
    ) -> Result<Vec<u8>, Option<Error>> {
        let git_dir = join(dir, b".git");
        if git_type != Some(FileKind::File) {
            return Ok(git_dir);
        }
        let data = self
            .read(&git_dir)
            .map_err(|e| Some(with_path(Error::Io(e), &git_dir)))?;
        let line = first_line(&data).ok_or(None)?;
        let real = line.strip_prefix(b"gitdir: ").ok_or(None)?.to_vec();
        let data = self.read(&join(&real, b"commondir")).map_err(|_| None)?;
        let common = first_line(&data).ok_or(None)?;
        Ok(if common.starts_with(b".") {
            join(&real, common)
        } else {
            common.to_vec()
        })
    }

    fn add_child_path(&self, parent: &Ignore, dir: &[u8]) -> (Node, Option<Error>) {
        let s = &self.shared;
        let stat = |name: &[u8]| self.host.stat(&self.host.abs(&join(dir, name)), true);
        let check_vcs_dir = s.require_git && (s.git_ignore || s.git_exclude);
        let git_type = if check_vcs_dir || s.git_exclude {
            stat(b".git").ok().map(|m| m.kind)
        } else {
            None
        };
        let has_jj = check_vcs_dir && stat(b".jj").is_ok();
        let has_git = check_vcs_dir && (git_type.is_some() || has_jj);
        let mut errs = Errs::default();
        let dir_path = to_path(dir);
        let custom = if s.custom_names.is_empty() {
            Gitignore::empty()
        } else {
            let names: Vec<&str> = s.custom_names.iter().map(String::as_str).collect();
            let (m, e) = self.create_gitignore(dir, dir, &names);
            errs.maybe_push(e);
            m
        };
        let ignore = if !s.ignore {
            Gitignore::empty()
        } else {
            let (m, e) = self.create_gitignore(dir, dir, &[".ignore"]);
            errs.maybe_push(e);
            m
        };
        let git_ignore = if !s.git_ignore {
            Gitignore::empty()
        } else {
            let (m, e) = self.create_gitignore(dir, dir, &[".gitignore"]);
            errs.maybe_push(e);
            m
        };
        let git_exclude = if !s.git_exclude {
            Gitignore::empty()
        } else {
            match self.resolve_git_commondir(dir, git_type) {
                Ok(git_dir) => {
                    let (m, e) = self.create_gitignore(dir, &git_dir, &["info/exclude"]);
                    errs.maybe_push(e);
                    m
                }
                Err(e) => {
                    errs.maybe_push(e);
                    empty(&dir_path)
                }
            }
        };
        let node = Node {
            dir: dir.to_vec(),
            parent: Some(parent.node.clone()),
            is_absolute_parent: false,
            has_absolute_parent_rules: parent.node.has_absolute_parent_rules,
            custom,
            ignore,
            git_ignore,
            git_exclude,
            has_git,
        };
        (node, errs.into_option())
    }

    fn add_child(&self, parent: &Ignore, dir: &[u8]) -> (Ignore, Option<Error>) {
        let (node, err) = self.add_child_path(parent, dir);
        (
            Ignore {
                node: Rc::new(node),
                absolute_base: parent.absolute_base.clone(),
            },
            err,
        )
    }

    /// A child that reads no ignore files (`add_child_with_entries(dir, &[])`).
    fn add_empty_child(&self, parent: &Ignore, dir: &[u8]) -> Ignore {
        Ignore {
            node: Rc::new(Node {
                dir: dir.to_vec(),
                parent: Some(parent.node.clone()),
                is_absolute_parent: false,
                has_absolute_parent_rules: parent.node.has_absolute_parent_rules,
                custom: Gitignore::empty(),
                ignore: Gitignore::empty(),
                git_ignore: Gitignore::empty(),
                git_exclude: Gitignore::empty(),
                has_git: false,
            }),
            absolute_base: parent.absolute_base.clone(),
        }
    }

    /// The chain of the parent directories of `path` (a root that is a directory).
    fn add_parents(&self, root: &Ignore, path: &[u8]) -> (Ignore, Option<Error>) {
        let s = &self.shared;
        if !s.parents && !s.git_ignore && !s.git_exclude && !s.git_global {
            return (root.clone(), None);
        }
        let base = normalize(&self.host.abs(path));
        if self.host.stat(&base, true).is_err() {
            return (root.clone(), None);
        }
        let absolute_base = Rc::new(to_path(&base));
        let mut parents = Vec::new();
        let mut p = base.as_slice();
        while let Some(parent) = parent_of(p) {
            parents.push(parent);
            p = parent;
        }
        let mut errs = Errs::default();
        let mut ig = root.clone();
        for parent in parents.into_iter().rev() {
            let (mut node, err) = self.add_child_path(&ig, parent);
            errs.maybe_push(err);
            node.is_absolute_parent = true;
            node.has_git &= s.git_ignore;
            node.has_absolute_parent_rules |= !node.custom.is_empty()
                || !node.ignore.is_empty()
                || !node.git_ignore.is_empty()
                || !node.git_exclude.is_empty();
            ig = Ignore {
                node: Rc::new(node),
                absolute_base: Some(absolute_base.clone()),
            };
        }
        (ig, errs.into_option())
    }

    fn has_any_ignore_rules(&self) -> bool {
        let s = &self.shared;
        s.ignore
            || s.git_global
            || s.git_ignore
            || s.git_exclude
            || !s.custom_names.is_empty()
            || !s.explicit.is_empty()
    }

    /// `matched_dir_entry`: the rules, then hidden.
    fn matched_entry(&self, ig: &Ignore, path: &[u8], is_dir: bool) -> Match<()> {
        let m = self.matched(ig, &to_path(path), is_dir);
        if m.is_none() && self.shared.hidden && is_hidden(path) {
            return Match::Ignore(());
        }
        m
    }

    fn matched(&self, ig: &Ignore, path: &Path, is_dir: bool) -> Match<()> {
        let path = strip_prefix(Path::new("./"), path).unwrap_or(path);
        let s = &self.shared;
        if !s.overrides.is_empty() {
            let m = s.overrides.matched(path, is_dir).map(|_| ());
            if !m.is_none() {
                return m;
            }
        }
        let mut whitelisted = Match::None;
        if self.has_any_ignore_rules() {
            let m = self.matched_ignore(ig, path, is_dir);
            if m.is_ignore() {
                return m;
            } else if m.is_whitelist() {
                whitelisted = m;
            }
        }
        if !s.types.is_empty() {
            let m = s.types.matched(path, is_dir).map(|_| ());
            if m.is_ignore() {
                return m;
            } else if m.is_whitelist() {
                whitelisted = m;
            }
        }
        whitelisted
    }

    fn matched_ignore(&self, ig: &Ignore, path: &Path, is_dir: bool) -> Match<()> {
        let s = &self.shared;
        let parents = || Parents(Some(&ig.node));
        let (mut m_custom, mut m_ignore, mut m_gi, mut m_gi_exclude, mut m_explicit) = (
            Match::None,
            Match::None,
            Match::None,
            Match::None,
            Match::None,
        );
        let any_git = !s.require_git || parents().any(|n| n.has_git);
        let mut saw_git = false;
        let mut step = |n: &Node, path: &Path, saw_git: &mut bool| {
            if m_custom.is_none() {
                m_custom = n.custom.matched(path, is_dir).map(|_| ());
            }
            if m_ignore.is_none() {
                m_ignore = n.ignore.matched(path, is_dir).map(|_| ());
            }
            if any_git && !*saw_git && m_gi.is_none() {
                m_gi = n.git_ignore.matched(path, is_dir).map(|_| ());
            }
            if any_git && !*saw_git && m_gi_exclude.is_none() {
                m_gi_exclude = n.git_exclude.matched(path, is_dir).map(|_| ());
            }
            *saw_git = *saw_git || n.has_git;
        };
        for n in parents().take_while(|n| !n.is_absolute_parent) {
            step(n, path, &mut saw_git);
        }
        if s.parents
            && ig.node.has_absolute_parent_rules
            && let Some(abs_parent_path) = ig.absolute_base.as_deref()
        {
            // The candidate rewritten relative to the canonical root, as `ignore` does
            // (stripping the walk's own spelling of the root first).
            let last = parents().take_while(|n| !n.is_absolute_parent).last();
            let rel = match last {
                None => path.to_path_buf(),
                Some(n) => {
                    let dir = to_path(&n.dir);
                    if dir.as_path() == Path::new(".") {
                        path.to_path_buf()
                    } else {
                        let without_dot_slash = strip_if_is_prefix(Path::new("./"), &dir);
                        let relative_base = strip_if_is_prefix(without_dot_slash, path);
                        strip_if_is_prefix(Path::new("/"), relative_base).to_path_buf()
                    }
                }
            };
            let path = abs_parent_path.join(rel);
            for n in parents().skip_while(|n| !n.is_absolute_parent) {
                step(n, &path, &mut saw_git);
            }
        }
        for gi in s.explicit.iter().rev() {
            if !m_explicit.is_none() {
                break;
            }
            m_explicit = gi.matched(path, is_dir).map(|_| ());
        }
        let m_global = if any_git {
            s.global.matched(path, is_dir).map(|_| ())
        } else {
            Match::None
        };
        m_custom
            .or(m_ignore)
            .or(m_gi)
            .or(m_gi_exclude)
            .or(m_global)
            .or(m_explicit)
    }
}

struct Parents<'a>(Option<&'a Node>);

impl<'a> Iterator for Parents<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<&'a Node> {
        let n = self.0?;
        self.0 = n.parent.as_deref();
        Some(n)
    }
}

/// Adds the lines of an ignore file (`GitignoreBuilder::add`, on bytes already read).
fn add_lines(b: &mut GitignoreBuilder, path: &Path, data: &[u8]) -> Option<Error> {
    let mut errs = Errs::default();
    let mut rest = data;
    let mut lineno = 0u64;
    while !rest.is_empty() {
        lineno += 1;
        let (line, next) = match rest.iter().position(|&b| b == b'\n') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Ok(line) = std::str::from_utf8(line) else {
            let e = io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            );
            errs.push(tagged(Error::Io(e), path, lineno));
            break;
        };
        let line = if lineno == 1 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if let Err(e) = b.add_line(Some(path.to_path_buf()), line) {
            errs.push(tagged(e, path, lineno));
        }
    }
    errs.into_option()
}

fn tagged(e: Error, path: &Path, line: u64) -> Error {
    let e = Error::WithLineNumber {
        line,
        err: Box::new(e),
    };
    if path.as_os_str().is_empty() {
        return e;
    }
    Error::WithPath {
        path: path.to_path_buf(),
        err: Box::new(e),
    }
}

fn with_path(e: Error, path: &[u8]) -> Error {
    Error::WithPath {
        path: to_path(path),
        err: Box::new(e),
    }
}

fn with_depth(e: Error, depth: usize) -> Error {
    Error::WithDepth {
        depth,
        err: Box::new(e),
    }
}

/// The first line, as `BufRead::lines` gives it.
fn first_line(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }
    let line = data.split(|&b| b == b'\n').next().unwrap_or(data);
    Some(line.strip_suffix(b"\r").unwrap_or(line))
}

/// `ignore`'s `pathutil::strip_prefix`: bytes on unix, components elsewhere.
fn strip_prefix<'a>(prefix: &Path, path: &'a Path) -> Option<&'a Path> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let pre = prefix.as_os_str().as_bytes();
        let p = path.as_os_str().as_bytes();
        p.strip_prefix(pre)
            .map(|rest| Path::new(std::ffi::OsStr::from_bytes(rest)))
    }
    #[cfg(not(unix))]
    {
        path.strip_prefix(prefix).ok()
    }
}

fn strip_if_is_prefix<'a>(prefix: &Path, path: &'a Path) -> &'a Path {
    strip_prefix(prefix, path).unwrap_or(path)
}

/// The name starts with `.` (and is not `.` or `..`).
fn is_hidden(path: &[u8]) -> bool {
    let name = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
    name.starts_with(b".") && name != b"." && name != b".."
}

/// An absolute path with `.` and `..` resolved lexically and no trailing `/` (on
/// Windows `\` separates too, and becomes `/`).
fn normalize(p: &[u8]) -> Vec<u8> {
    let p: Vec<u8> = if cfg!(windows) {
        p.iter()
            .map(|&b| if b == b'\\' { b'/' } else { b })
            .collect()
    } else {
        p.to_vec()
    };
    let p = p.as_slice();
    let (prefix, rest) = if p.len() >= 2 && p[1] == b':' {
        (&p[..2], &p[2..])
    } else {
        (&p[..0], p)
    };
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in rest.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = prefix.to_vec();
    out.push(b'/');
    out.extend_from_slice(&parts.join(&b'/'));
    out
}

/// The parent of a normalized absolute path (`None` at the root).
fn parent_of(p: &[u8]) -> Option<&[u8]> {
    let i = p.iter().rposition(|&b| b == b'/')?;
    if i + 1 == p.len() {
        return None;
    }
    let root_len = if p.len() >= 2 && p[1] == b':' { 3 } else { 1 };
    Some(&p[..i.max(root_len - 1) + usize::from(i < root_len)])
}

/// The virtual walk: `ignore::Walk` with `sort_by_file_name` over `host`.
pub fn run(
    host: &dyn WalkHost,
    o: &WalkOptions,
    roots: &[Vec<u8>],
    visit: &mut dyn FnMut(Result<Entry, Error>) -> WalkState,
) -> WalkState {
    let cwd = to_path(host.cwd());
    let glob_root = o
        .glob_root
        .as_ref()
        .map_or(cwd.clone(), |r| to_path(&host.abs(r)));
    let overrides = build_overrides(&glob_root, &o.globs).unwrap_or_else(|_| Override::empty());
    let global = if o.git_global {
        global_gitignore(host, &cwd, o.ignore_case_insensitive)
    } else {
        Gitignore::empty()
    };
    let mut explicit = Vec::new();
    let mut explicit_errs = Vec::new();
    for f in &o.ignore_files {
        // Relative to nothing, as `WalkBuilder::add_ignore` builds them.
        let mut b = GitignoreBuilder::new("");
        let _ = b.case_insensitive(o.ignore_case_insensitive);
        match host.read_file(&host.abs(f)) {
            Ok(data) => {
                if let Some(e) = add_lines(&mut b, &to_path(f), &data) {
                    explicit_errs.push(e);
                }
            }
            Err(e) => explicit_errs.push(with_path(Error::Io(e), f)),
        }
        match b.build() {
            Ok(gi) => explicit.push(gi),
            Err(e) => explicit_errs.push(e),
        }
    }
    let chain = Chain {
        host,
        shared: Shared {
            hidden: o.hidden,
            ignore: o.ignore,
            parents: o.parents,
            git_global: o.git_global,
            git_ignore: o.git_ignore,
            git_exclude: o.git_exclude,
            require_git: o.require_git,
            case_insensitive: o.ignore_case_insensitive,
            custom_names: o.custom_ignore_filenames.clone(),
            overrides,
            types: o.types.clone().unwrap_or_else(Types::empty),
            global,
            explicit,
        },
    };
    let mut w = Walk {
        chain: &chain,
        o,
        visit,
        ancestors: Vec::new(),
        root_dev: 0,
    };
    for e in explicit_errs {
        if (w.visit)(Err(e)) == WalkState::Quit {
            return WalkState::Quit;
        }
    }
    let ig_root = chain.root();
    for r in roots {
        if w.root(&ig_root, r) == WalkState::Quit {
            return WalkState::Quit;
        }
    }
    WalkState::Continue
}

fn global_gitignore(host: &dyn WalkHost, cwd: &Path, case_insensitive: bool) -> Gitignore {
    let Some(path) = gitconfig::excludes_path(host) else {
        return Gitignore::empty();
    };
    let abs = host.abs(&path);
    if !host
        .stat(&abs, true)
        .is_ok_and(|m| m.kind == FileKind::File)
    {
        return Gitignore::empty();
    }
    let mut b = GitignoreBuilder::new(cwd);
    let _ = b.case_insensitive(case_insensitive);
    if let Ok(data) = host.read_file(&abs) {
        let _ = add_lines(&mut b, &to_path(&path), &data);
    }
    b.build().unwrap_or_else(|_| Gitignore::empty())
}

struct Walk<'a, 'h> {
    chain: &'a Chain<'h>,
    o: &'a WalkOptions,
    visit: &'a mut dyn FnMut(Result<Entry, Error>) -> WalkState,
    /// (dev, ino, display path) of the directories being walked, when following links.
    ancestors: Vec<(u64, u64, Vec<u8>)>,
    root_dev: u64,
}

impl Walk<'_, '_> {
    fn host(&self) -> &dyn WalkHost {
        self.chain.host
    }

    fn emit(&mut self, item: Result<Entry, Error>) -> WalkState {
        if self.host().cancelled() {
            return WalkState::Quit;
        }
        if let Ok(e) = &item
            && self.o.min_depth.is_some_and(|m| e.depth < m)
        {
            return WalkState::Continue;
        }
        (self.visit)(item)
    }

    /// A walkdir error: the path, also in the message, as walkdir words it.
    /// (`parallel_errors`: as `ignore`'s parallel walker words it, without the prefix.)
    fn io_err(&mut self, path: &[u8], depth: usize, e: io::Error) -> WalkState {
        let e = if self.o.parallel_errors {
            e
        } else {
            io::Error::new(
                e.kind(),
                format!("IO error for operation on {}: {e}", to_path(path).display()),
            )
        };
        self.emit(Err(with_path(with_depth(Error::Io(e), depth), path)))
    }

    fn root(&mut self, ig_root: &Ignore, r: &[u8]) -> WalkState {
        let host = self.chain.host;
        let abs = host.abs(r);
        let lmd = match host.stat(&abs, false) {
            Ok(m) => m,
            Err(e) => return self.io_err(r, 0, e),
        };
        let followed = host.stat(&abs, true);
        // walkdir always resolves a root link: a dangling one is an error.
        if lmd.kind == FileKind::Symlink
            && let Err(e) = followed
        {
            return self.io_err(r, 0, e);
        }
        let follow =
            self.o.follow_links || followed.as_ref().is_ok_and(|m| m.kind == FileKind::File);
        let md = if follow && lmd.kind == FileKind::Symlink {
            match followed {
                Ok(m) => m,
                Err(e) => return self.io_err(r, 0, e),
            }
        } else {
            lmd
        };
        let target = host.stat(&abs, true);
        self.root_dev = target.as_ref().map_or(md.dev, |m| m.dev);
        let target_is_dir = target.is_ok_and(|m| m.kind == FileKind::Dir);
        let is_dir_event =
            md.kind == FileKind::Dir || (md.kind == FileKind::Symlink && target_is_dir);
        let ig = if target_is_dir {
            let (ig, err) = self.chain.add_parents(ig_root, r);
            if let Some(err) = err
                && self.emit(Err(err)) == WalkState::Quit
            {
                return WalkState::Quit;
            }
            ig
        } else {
            ig_root.clone()
        };
        let entry = Entry {
            path: r.to_vec(),
            abs,
            native: None,
            depth: 0,
            kind: md.kind,
            path_is_symlink: lmd.kind == FileKind::Symlink,
            err: None,
        };
        self.ancestors.clear();
        if is_dir_event {
            self.dir(&ig, entry, md.dev, md.ino)
        } else {
            self.emit(Ok(entry))
        }
    }

    /// A directory: check it, visit it, then read it and visit its entries (a read
    /// error comes right after the directory, as in walkdir).
    fn dir(&mut self, ig: &Ignore, mut entry: Entry, dev: u64, ino: u64) -> WalkState {
        let host = self.chain.host;
        let depth = entry.depth;
        if depth > 0 && self.skip(ig, &entry) {
            return WalkState::Continue;
        }
        let at_max = self.o.max_depth == Some(depth);
        let child_ig = if at_max {
            self.chain.add_empty_child(ig, &entry.path)
        } else {
            let (child, err) = self.chain.add_child(ig, &entry.path);
            entry.err = err;
            child
        };
        let path = entry.path.clone();
        let abs = entry.abs.clone();
        match self.emit(Ok(entry)) {
            WalkState::Quit => return WalkState::Quit,
            WalkState::Skip => return WalkState::Continue,
            WalkState::Continue => {}
        }
        let other_fs = self.o.same_file_system && depth > 0 && dev != self.root_dev;
        if other_fs || self.o.max_depth.is_some_and(|m| depth >= m) {
            return WalkState::Continue;
        }
        let mut names = match host.read_dir(&abs) {
            Ok(list) => list,
            Err(e) => return self.io_err(&path, depth, e),
        };
        names.sort_by(|a, b| a.name.cmp(&b.name));
        if self.o.follow_links {
            self.ancestors.push((dev, ino, path.clone()));
        }
        for d in names {
            let cpath = join(&path, &d.name);
            let cabs = join(&abs, &d.name);
            if self.child(&child_ig, cpath, cabs, depth + 1) == WalkState::Quit {
                return WalkState::Quit;
            }
        }
        if self.o.follow_links {
            self.ancestors.pop();
        }
        WalkState::Continue
    }

    fn child(&mut self, ig: &Ignore, path: Vec<u8>, abs: Vec<u8>, depth: usize) -> WalkState {
        let host = self.chain.host;
        let lmd = match host.stat(&abs, false) {
            Ok(m) => m,
            Err(e) => return self.io_err(&path, depth, e),
        };
        let mut md = lmd;
        if self.o.follow_links && lmd.kind == FileKind::Symlink {
            md = match host.stat(&abs, true) {
                Ok(m) => m,
                Err(e) => return self.io_err(&path, depth, e),
            };
            // A host without file ids (0, 0) cannot tell loops apart.
            if md.kind == FileKind::Dir
                && (md.dev, md.ino) != (0, 0)
                && let Some((_, _, anc)) = self
                    .ancestors
                    .iter()
                    .rev()
                    .find(|(d, i, _)| *d == md.dev && *i == md.ino)
            {
                let e = Error::Loop {
                    ancestor: to_path(anc),
                    child: to_path(&path),
                };
                return self.emit(Err(with_depth(e, depth)));
            }
        }
        let entry = Entry {
            path,
            abs,
            native: None,
            depth,
            kind: md.kind,
            path_is_symlink: lmd.kind == FileKind::Symlink,
            err: None,
        };
        if md.kind == FileKind::Dir {
            return self.dir(ig, entry, md.dev, md.ino);
        }
        if self.skip(ig, &entry) {
            return WalkState::Continue;
        }
        if let Some(max) = self.o.max_filesize
            && md.size > max
        {
            return WalkState::Continue;
        }
        self.emit(Ok(entry))
    }

    fn skip(&self, ig: &Ignore, e: &Entry) -> bool {
        self.chain
            .matched_entry(ig, &e.path, e.is_dir())
            .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(normalize(b"/a/./b/../c/"), b"/a/c");
        assert_eq!(normalize(b"/"), b"/");
        assert_eq!(normalize(b"C:/x/../y"), b"C:/y");
        assert_eq!(parent_of(b"/a/b"), Some(&b"/a"[..]));
        assert_eq!(parent_of(b"/a"), Some(&b"/"[..]));
        assert_eq!(parent_of(b"/"), None);
        assert_eq!(parent_of(b"C:/a"), Some(&b"C:/"[..]));
        assert_eq!(parent_of(b"C:/"), None);
        assert!(is_hidden(b"./a/.b"));
        assert!(!is_hidden(b"a/.."));
        assert_eq!(first_line(b"gitdir: x\r\ny"), Some(&b"gitdir: x"[..]));
    }
}

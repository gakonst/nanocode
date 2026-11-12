//! The working tree against the index: which tracked files changed, and which files
//! are untracked (gitignore(5), through `search::ignore`'s matcher).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use crate::search::ignore::Match;
use crate::search::ignore::gitignore::{Gitignore, GitignoreBuilder};
use psh_engine::host::{FileKind, Metadata};

use super::diff::{Change, File};
use super::index::{CacheTree, Index, IndexEntry};
use super::repo::Repo;
use super::store::{Oid, join};
use super::wild::Pathspec;
use super::{Error, Res};
use crate::sum_hash::{Algo, Digest};

/// The name git gives a blob with this content.
pub fn blob_oid(data: &[u8]) -> Oid {
    let mut digest = Digest::new(Algo::Sha1);
    digest.update(format!("blob {}\0", data.len()).as_bytes());
    digest.update(data);
    Oid::from_bytes(&digest.finish())
}

pub enum State {
    Same,
    Gone,
    Changed(File),
}

pub struct Work<'r, 'a> {
    pub repo: &'r Repo<'a>,
    pub root: Vec<u8>,
    pub index: Index,
    filemode: bool,
    /// Line endings are converted on the way in (core.autocrlf, or attributes that
    /// name text or eol).
    converts: bool,
    /// Only whether a file changed is asked, not what it holds now: a file of another
    /// size than the index recorded is changed without being read, as git has it.
    pub quick: bool,
    /// What the scan for untracked files learned about the files it passed.
    seen: RefCell<HashMap<Vec<u8>, Metadata>>,
    scanned: Cell<bool>,
    /// The host has yet to answer something the scan asked: the scan went on, to ask
    /// for the rest in the same run, and the run is thrown away.
    blocked: Cell<bool>,
    /// What the command will have written by the time its changes are carried out: a
    /// path's mode and blob, or None where the file goes. A file named here is
    /// compared as that, not as the file system has it now.
    pub after: HashMap<Vec<u8>, Option<(u32, Oid)>>,
    /// The entries whose files had to be read to be compared though the index had
    /// their size and time: with what the file system says of one that holds what
    /// the entry does, None for one that changed.
    pub noted: RefCell<Vec<(Vec<u8>, Option<Metadata>)>>,
}

impl<'r, 'a> Work<'r, 'a> {
    pub fn new(repo: &'r Repo<'a>, root: &[u8]) -> Res<Work<'r, 'a>> {
        let index = Index::load(repo)?;
        let mut converts = matches!(
            repo.config
                .get("core.autocrlf")
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("true" | "input")
        );
        for path in [
            join(root, b".gitattributes"),
            join(&repo.common_dir, b"info/attributes"),
        ] {
            if let Some(text) = repo.fs.read_opt(&path)? {
                let text = String::from_utf8_lossy(&text);
                if text.contains("filter=") {
                    return Err(Error::Lacks(
                        "this repository's attributes name a filter".to_string(),
                        "git filters (LFS and the like)",
                    ));
                }
                converts |= text.contains("text") || text.contains("eol");
            }
        }
        Ok(Work {
            repo,
            root: root.to_vec(),
            index,
            filemode: repo.config.bool("core.filemode") != Some(false),
            converts,
            quick: false,
            seen: RefCell::default(),
            scanned: Cell::new(false),
            blocked: Cell::new(false),
            after: HashMap::new(),
            noted: RefCell::default(),
        })
    }

    /// An answer, or None with the run marked when the host has yet to give it.
    fn later<T>(&self, answer: std::io::Result<T>) -> Res<Option<T>> {
        match answer {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.blocked.set(true);
                Ok(None)
            }
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// The error of a run whose scan was not answered in full.
    pub fn answered(&self) -> Res<()> {
        if self.blocked.get() {
            return Err(Error::Io(std::io::ErrorKind::WouldBlock.into()));
        }
        Ok(())
    }

    /// The file in the worktree as a blob would hold it, when there is one.
    fn read(&self, path: &[u8], kind: FileKind, mode: u32, index_mode: u32) -> Res<File> {
        let full = join(&self.root, path);
        let (mode, data) = if kind == FileKind::Symlink {
            (0o120000, self.repo.fs.read_link(&full)?)
        } else {
            let mode = if !self.filemode && index_mode & 0o170000 == 0o100000 {
                index_mode
            } else if mode & 0o100 != 0 {
                0o100755
            } else {
                0o100644
            };
            (mode, self.repo.fs.read(&full)?)
        };
        Ok(File {
            mode,
            oid: blob_oid(&data),
            data: Some(Rc::new(data)),
        })
    }

    /// What the file system says of a path of the worktree, as the scan for untracked
    /// files saw it or as it is now.
    pub fn meta(&self, path: &[u8]) -> Res<Option<Metadata>> {
        if let Some(meta) = self.seen.borrow().get(path) {
            return Ok(Some(*meta));
        }
        Ok(self.repo.fs.stat_opt(&join(&self.root, path), false)?)
    }

    /// A file of the worktree byte for byte, when there is one.
    pub fn file_raw(&self, path: &[u8], index_mode: u32) -> Res<Option<(File, Metadata)>> {
        let Some(meta) = self.meta(path)? else {
            return Ok(None);
        };
        if !matches!(meta.kind, FileKind::File | FileKind::Symlink) {
            return Ok(None);
        }
        let file = self.read(path, meta.kind, meta.mode, index_mode)?;
        Ok(Some((file, meta)))
    }

    /// A file of the worktree as the index would hold it: its mode and content, and
    /// what the file system said of it before it was read.
    pub fn file(&self, path: &[u8], index_mode: u32) -> Res<Option<(File, Metadata)>> {
        let Some((file, meta)) = self.file_raw(path, index_mode)? else {
            return Ok(None);
        };
        if self.converts
            && meta.kind == FileKind::File
            && let Some(data) = &file.data
            && data.windows(2).any(|w| w == b"\r\n")
            && !super::patch::is_binary(data)
        {
            return Err(Error::Lacks(
                format!(
                    "'{}' has CRLF line endings in a repository that converts them",
                    String::from_utf8_lossy(path)
                ),
                "git that converts line endings on the way in",
            ));
        }
        Ok(Some((file, meta)))
    }

    /// Files written to the worktree would need more than their blobs' bytes: line
    /// endings, keywords or another encoding.
    pub fn smudges(&self) -> Res<()> {
        let config = &self.repo.config;
        let mut converts = config
            .get("core.autocrlf")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
            || config
                .get("core.eol")
                .is_some_and(|v| v.eq_ignore_ascii_case("crlf"));
        for path in [
            join(&self.root, b".gitattributes"),
            join(&self.repo.common_dir, b"info/attributes"),
        ] {
            if let Some(text) = self.repo.fs.read_opt(&path)? {
                let text = String::from_utf8_lossy(&text);
                converts |= text.split_whitespace().any(|word| {
                    matches!(word, "eol=crlf" | "ident" | "export-subst")
                        || word.starts_with("working-tree-encoding")
                });
            }
        }
        if converts {
            return Err(Error::Lacks(
                "this repository converts files on the way out".to_string(),
                "git that writes CRLF line endings, keywords or other encodings",
            ));
        }
        Ok(())
    }

    /// How a tracked file is, compared with its index entry.
    pub fn state(&self, entry: &IndexEntry) -> Res<State> {
        Ok(self.state_meta(entry)?.0)
    }

    /// The same, with what the file system said of the file before it was read.
    pub fn state_meta(&self, entry: &IndexEntry) -> Res<(State, Option<Metadata>)> {
        if let Some(after) = self.after.get(&entry.path) {
            let state = match *after {
                None => State::Gone,
                Some((mode, oid))
                    if oid == entry.oid && mode == entry.mode && !entry.intent_to_add =>
                {
                    State::Same
                }
                Some((mode, oid)) => State::Changed(File {
                    mode,
                    oid,
                    data: None,
                }),
            };
            return Ok((state, None));
        }
        let seen = self.seen.borrow().get(&entry.path).copied();
        let meta = match seen {
            Some(meta) => meta,
            // The scan passed every file of a directory that is there.
            None if self.scanned.get() && !self.blocked.get() => return Ok((State::Gone, None)),
            None => {
                let full = join(&self.root, &entry.path);
                match self.repo.fs.stat_opt(&full, false)? {
                    Some(meta) => meta,
                    None => return Ok((State::Gone, None)),
                }
            }
        };
        Ok((self.compare(entry, &meta)?, Some(meta)))
    }

    fn compare(&self, entry: &IndexEntry, meta: &Metadata) -> Res<State> {
        if entry.mode == 0o160000 {
            // A submodule: its own repository says what it is at.
            return Ok(if meta.kind == FileKind::Dir {
                State::Same
            } else {
                State::Gone
            });
        }
        if !matches!(meta.kind, FileKind::File | FileKind::Symlink) {
            return Ok(State::Gone);
        }
        let is_link = meta.kind == FileKind::Symlink;
        let mode_same = if is_link {
            entry.mode == 0o120000
        } else {
            entry.mode & 0o170000 == 0o100000
                && (!self.filemode || (entry.mode & 0o100 != 0) == (meta.mode & 0o100 != 0))
        };
        // Unchanged since the index recorded it, and recorded before the index was
        // written (git's racy check): the content is the entry's.
        let secs = meta.mtime_ns.div_euclid(1_000_000_000);
        let nanos = meta.mtime_ns.rem_euclid(1_000_000_000);
        // An entry of no size for a blob that has one was marked as changed.
        let marked = entry.size == 0 && entry.oid != blob_oid(b"");
        let recorded = secs == i64::from(entry.mtime.0)
            && (entry.mtime.1 == 0 || i64::from(entry.mtime.1) == nanos)
            && meta.size == u64::from(entry.size)
            && !marked
            && secs < self.index.written.div_euclid(1_000_000_000);
        if recorded && mode_same && !entry.intent_to_add {
            return Ok(State::Same);
        }
        if self.quick && !self.converts && meta.size != u64::from(entry.size) {
            return Ok(State::Changed(File {
                mode: if is_link {
                    0o120000
                } else if self.filemode && meta.mode & 0o100 != 0 {
                    0o100755
                } else if self.filemode {
                    0o100644
                } else {
                    entry.mode
                },
                oid: Oid::ZERO,
                data: None,
            }));
        }
        let mut file = self.read(&entry.path, meta.kind, meta.mode, entry.mode)?;
        if file.oid != entry.oid
            && self.converts
            && !is_link
            && let Some(data) = &file.data
            && data.windows(2).any(|w| w == b"\r\n")
            && !super::patch::is_binary(data)
        {
            let mut plain = Vec::with_capacity(data.len());
            for (i, &b) in data.iter().enumerate() {
                if b == b'\r' && data.get(i + 1) == Some(&b'\n') {
                    continue;
                }
                plain.push(b);
            }
            file.oid = blob_oid(&plain);
            file.data = Some(Rc::new(plain));
        }
        if file.oid == entry.oid && file.mode == entry.mode && !entry.intent_to_add {
            self.noted
                .borrow_mut()
                .push((entry.path.clone(), Some(*meta)));
            return Ok(State::Same);
        }
        if secs == i64::from(entry.mtime.0) && meta.size == u64::from(entry.size) {
            self.noted.borrow_mut().push((entry.path.clone(), None));
        }
        Ok(State::Changed(file))
    }

    /// The changes from the index to the worktree.
    pub fn unstaged(&self, spec: &Pathspec) -> Res<Vec<Change>> {
        let mut changes = Vec::new();
        let mut blocked = None;
        for entry in &self.index.entries {
            if entry.stage != 0 {
                return Err(super::diff::unmerged());
            }
            if entry.skip_worktree || entry.assume_valid || !spec.matches(&entry.path) {
                continue;
            }
            let old = File {
                mode: entry.mode,
                oid: entry.oid,
                data: None,
            };
            // A host that answers later is asked for everything before this run ends.
            let state = match self.state(entry) {
                Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    blocked = Some(e);
                    continue;
                }
                other => other?,
            };
            match state {
                State::Same => {}
                State::Gone if entry.intent_to_add => {}
                State::Gone => changes.push(Change {
                    path: entry.path.clone(),
                    from: None,
                    old: Some(old),
                    new: None,
                    score: 0,
                }),
                State::Changed(new) => changes.push(Change {
                    path: entry.path.clone(),
                    from: None,
                    old: if entry.intent_to_add { None } else { Some(old) },
                    new: Some(new),
                    score: 0,
                }),
            }
        }
        match blocked {
            Some(e) => Err(Error::Io(e)),
            None => Ok(changes),
        }
    }

    /// The changes from a tree (none: no commit yet) to the index.
    pub fn staged(&self, tree: Option<&Oid>, spec: &Pathspec) -> Res<Vec<Change>> {
        let entries: Vec<&IndexEntry> = self
            .index
            .entries
            .iter()
            .filter(|e| !e.intent_to_add)
            .collect();
        if entries
            .iter()
            .any(|e| e.stage != 0 && spec.matches(&e.path))
        {
            return Err(super::diff::unmerged());
        }
        let mut changes = Vec::new();
        self.staged_dir(
            tree,
            self.index.tree.as_ref(),
            &entries,
            0,
            spec,
            &mut changes,
        )?;
        Ok(changes)
    }

    /// One directory of the tree against the index entries under it (their paths
    /// share the first depth bytes). A directory the index last wrote as this very
    /// tree is not read.
    fn staged_dir(
        &self,
        tree: Option<&Oid>,
        cache: Option<&CacheTree>,
        entries: &[&IndexEntry],
        depth: usize,
        spec: &Pathspec,
        out: &mut Vec<Change>,
    ) -> Res<()> {
        if let (Some(tree), Some(cache)) = (tree, cache)
            && cache.count >= 0
            && cache.oid == *tree
            && cache.count as usize == entries.len()
        {
            return Ok(());
        }
        let base: &[u8] = entries.first().map_or(&[], |e| &e.path[..depth]);
        let listed = match tree {
            Some(tree) => self.repo.tree(tree)?,
            None => Rc::default(),
        };
        let change =
            |path: Vec<u8>, old: Option<File>, new: Option<File>, out: &mut Vec<Change>| {
                if spec.matches(&path) {
                    out.push(Change {
                        path,
                        from: None,
                        old,
                        new,
                        score: 0,
                    });
                }
            };
        let file = |mode: u32, oid: Oid| File {
            mode,
            oid,
            data: None,
        };
        let (mut i, mut j) = (0, 0);
        while i < listed.len() || j < entries.len() {
            // The index's next name at this level: a file, or a directory and the
            // entries under it.
            let group = entries.get(j).map(|first| {
                let rest = &first.path[depth..];
                match rest.iter().position(|&b| b == b'/') {
                    Some(slash) => {
                        let prefix = &first.path[..depth + slash + 1];
                        let len = entries[j..]
                            .iter()
                            .take_while(|e| e.path.starts_with(prefix))
                            .count();
                        (&rest[..slash], true, len)
                    }
                    None => (rest, false, 1),
                }
            });
            let order = match (listed.get(i), group) {
                (Some(t), Some((name, is_dir, _))) => {
                    let a = t.name.iter().copied().chain(t.is_tree().then_some(b'/'));
                    let b = name.iter().copied().chain(is_dir.then_some(b'/'));
                    a.cmp(b)
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Greater,
            };
            let under = |name: &[u8]| {
                let mut path = base.to_vec();
                path.extend_from_slice(name);
                path
            };
            match order {
                std::cmp::Ordering::Less => {
                    let t = &listed[i];
                    let path = under(&t.name);
                    if t.is_tree() {
                        if spec.may_match_under(&path) {
                            let mut files = Vec::new();
                            super::diff::flatten(self.repo, &t.oid, &path, spec, &mut files)?;
                            for (path, mode, oid) in files {
                                change(path, Some(file(mode, oid)), None, out);
                            }
                        }
                    } else {
                        change(path, Some(file(t.mode, t.oid)), None, out);
                    }
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    let (_, _, len) = group.expect("an index entry");
                    for e in &entries[j..j + len] {
                        change(e.path.clone(), None, Some(file(e.mode, e.oid)), out);
                    }
                    j += len;
                }
                std::cmp::Ordering::Equal => {
                    let t = &listed[i];
                    let (name, is_dir, len) = group.expect("an index entry");
                    if is_dir {
                        let child = cache.and_then(|c| c.children.iter().find(|c| c.name == name));
                        if spec.may_match_under(&under(name)) {
                            self.staged_dir(
                                Some(&t.oid),
                                child,
                                &entries[j..j + len],
                                depth + name.len() + 1,
                                spec,
                                out,
                            )?;
                        }
                    } else {
                        let e = entries[j];
                        if t.mode != e.mode || t.oid != e.oid {
                            change(
                                e.path.clone(),
                                Some(file(t.mode, t.oid)),
                                Some(file(e.mode, e.oid)),
                                out,
                            );
                        }
                    }
                    i += 1;
                    j += len;
                }
            }
        }
        Ok(())
    }

    /// The changes from a tree to the worktree, for the files the index tracks.
    pub fn against_tree(&self, tree: &Oid, spec: &Pathspec) -> Res<Vec<Change>> {
        let mut changes = self.staged(Some(tree), spec)?;
        // A file takes the worktree's content where that differs from the index.
        for unstaged in self.unstaged(spec)? {
            match changes.iter().position(|c| c.path == unstaged.path) {
                Some(at) => {
                    let same = match (&changes[at].old, &unstaged.new) {
                        (Some(a), Some(b)) => a == b,
                        (None, None) => true,
                        _ => false,
                    };
                    if same {
                        changes.remove(at);
                    } else {
                        changes[at].new = unstaged.new;
                    }
                }
                None => {
                    let old = self.repo.tree_entry(tree, &unstaged.path)?.map(|e| File {
                        mode: e.mode,
                        oid: e.oid,
                        data: None,
                    });
                    if old.is_some() || unstaged.new.is_some() {
                        changes.push(Change {
                            path: unstaged.path,
                            from: None,
                            old,
                            new: unstaged.new,
                            score: 0,
                        });
                    }
                }
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(changes)
    }

    /// The mode the index has a path under, when it tracks it.
    fn tracked_mode(&self, path: &[u8]) -> Option<u32> {
        self.index
            .entries
            .binary_search_by(|e| e.path.as_slice().cmp(path))
            .ok()
            .map(|at| self.index.entries[at].mode)
    }

    pub fn tracked_under(&self, dir: &[u8]) -> bool {
        let mut prefix = dir.to_vec();
        prefix.push(b'/');
        let at = self
            .index
            .entries
            .partition_point(|e| e.path.as_slice() < prefix.as_slice());
        self.index
            .entries
            .get(at)
            .is_some_and(|e| e.path.starts_with(&prefix))
    }
}

/// How git status shows untracked files.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Untracked {
    No,
    /// A directory with nothing tracked in it is one line.
    Normal,
    All,
}

struct Rules {
    /// The ignore files that apply, outermost first.
    stack: Vec<Gitignore>,
}

impl Rules {
    fn ignored(&self, path: &[u8], is_dir: bool) -> bool {
        let text = format!("/{}", String::from_utf8_lossy(path));
        for rules in self.stack.iter().rev() {
            match rules.matched(Path::new(&text), is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

fn rules(dir: &[u8], text: &[u8]) -> Gitignore {
    let root = format!("/{}", String::from_utf8_lossy(dir));
    let mut builder = GitignoreBuilder::new(Path::new(&root));
    for line in String::from_utf8_lossy(text).lines() {
        let _ = builder.add_line(line);
    }
    builder.build()
}

struct Scan<'w, 'r, 'a> {
    work: &'w Work<'r, 'a>,
    mode: Untracked,
    spec: &'w Pathspec,
    found: Vec<Vec<u8>>,
    rules: Rules,
}

impl Scan<'_, '_, '_> {
    /// Walks a directory that has tracked files (or is the root), and says whether
    /// something untracked is in it. With collect off it only says so; with hidden on
    /// (an ignored directory that has tracked files) nothing in it counts as
    /// untracked.
    fn walk(&mut self, dir: &[u8], collect: bool, hidden: bool) -> Res<bool> {
        let full = if dir.is_empty() {
            self.work.root.clone()
        } else {
            join(&self.work.root, dir)
        };
        let fs = self.work.repo.fs;
        let names = self.work.later(fs.list(&full))?.unwrap_or_default();
        let mut pushed = false;
        if !hidden
            && names.iter().any(|n| n == b".gitignore")
            && let Some(text) = self
                .work
                .later(fs.read_opt(&join(&full, b".gitignore")))?
                .flatten()
        {
            self.rules.stack.push(rules(dir, &text));
            pushed = true;
        }
        let mut any = false;
        for name in names {
            if name == b".git" {
                continue;
            }
            let path = if dir.is_empty() {
                name.clone()
            } else {
                join(dir, &name)
            };
            let Some(meta) = self
                .work
                .later(fs.stat_opt(&join(&full, &name), false))?
                .flatten()
            else {
                continue;
            };
            // Whether the paths asked for take this one as a whole, or name files
            // under it, which are then listed one by one.
            let whole = self.spec.matches(&path);
            let expands = self.mode == Untracked::All || !whole;
            if let Some(mode) = self.work.tracked_mode(&path) {
                self.work.seen.borrow_mut().insert(path.clone(), meta);
                // A directory where the index has a file: what is in it is untracked.
                if meta.kind == FileKind::Dir
                    && mode != 0o160000
                    && !hidden
                    && expands
                    && self.spec.may_match_under(&path)
                {
                    any |= self.walk(&path, collect, false)?;
                }
                continue;
            }
            if meta.kind == FileKind::Dir {
                if !self.spec.may_match_under(&path) {
                    continue;
                }
                let tracked = self.work.tracked_under(&path);
                let ignored = hidden || self.rules.ignored(&path, true);
                if ignored {
                    if tracked {
                        self.walk(&path, false, true)?;
                    }
                    continue;
                }
                let nested = self
                    .work
                    .later(fs.stat_opt(&join(&join(&full, &name), b".git"), false))?
                    .flatten()
                    .is_some();
                if tracked || (expands && !nested) {
                    any |= self.walk(&path, collect, false)?;
                } else if nested || self.walk(&path, false, false)? {
                    any = true;
                    if collect && whole {
                        let mut shown = path.clone();
                        shown.push(b'/');
                        self.found.push(shown);
                    }
                }
            } else {
                if hidden || self.rules.ignored(&path, false) || !self.spec.matches(&path) {
                    continue;
                }
                any = true;
                if collect {
                    self.found.push(path);
                } else {
                    break;
                }
            }
        }
        if pushed {
            self.rules.stack.pop();
        }
        Ok(any)
    }
}

/// The untracked files (directories end in a slash), by path.
pub fn untracked(
    work: &Work<'_, '_>,
    mode: Untracked,
    spec: &Pathspec,
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Res<Vec<Vec<u8>>> {
    if mode == Untracked::No {
        return Ok(Vec::new());
    }
    let fs = work.repo.fs;
    let mut stack = Vec::new();
    let global = match work.repo.config.get("core.excludesfile") {
        Some(path) => Some(match path.strip_prefix("~/") {
            Some(rest) => join(&env(b"HOME").unwrap_or_default(), rest.as_bytes()),
            None => path.as_bytes().to_vec(),
        }),
        None => match (env(b"XDG_CONFIG_HOME"), env(b"HOME")) {
            (Some(xdg), _) if !xdg.is_empty() => Some(join(&xdg, b"git/ignore")),
            (_, Some(home)) => Some(join(&home, b".config/git/ignore")),
            _ => None,
        },
    };
    if let Some(path) = global
        && path.first() == Some(&b'/')
        && let Some(text) = fs.read_opt(&path)?
    {
        stack.push(rules(b"", &text));
    }
    if let Some(text) = fs.read_opt(&join(&work.repo.common_dir, b"info/exclude"))? {
        stack.push(rules(b"", &text));
    }
    let mut scan = Scan {
        work,
        mode,
        spec,
        found: Vec::new(),
        rules: Rules { stack },
    };
    scan.walk(b"", true, false)?;
    // Every tracked file that is there was passed, when nothing limited the walk.
    work.scanned
        .set(spec.literal().is_some_and(|paths| paths.is_empty()));
    scan.found.sort();
    Ok(scan.found)
}

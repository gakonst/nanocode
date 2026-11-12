use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, FileType};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::gitignore::{Gitignore, GitignoreBuilder};
use super::overrides::Override;
use super::types::Types;
use super::{Error, Match, PartialErrors, file_name, strip_prefix};
use crate::search::gitconfig;

pub struct WalkBuilder {
    roots: Vec<PathBuf>,
    current_dir: PathBuf,
    shared: Shared,
    max_depth: Option<usize>,
    max_filesize: Option<u64>,
    follow_links: bool,
    same_file_system: bool,
}

struct Shared {
    hidden: bool,
    ignore: bool,
    parents: bool,
    git_global: bool,
    git_ignore: bool,
    git_exclude: bool,
    require_git: bool,
    case_insensitive: bool,
    custom_names: Vec<OsString>,
    overrides: Override,
    types: Types,
    global: Gitignore,
}

impl WalkBuilder {
    pub fn new(current_dir: PathBuf) -> WalkBuilder {
        WalkBuilder {
            roots: Vec::new(),
            current_dir,
            shared: Shared {
                hidden: true,
                ignore: true,
                parents: true,
                git_global: true,
                git_ignore: true,
                git_exclude: true,
                require_git: true,
                case_insensitive: false,
                custom_names: Vec::new(),
                overrides: Override::empty(),
                types: Types::empty(),
                global: Gitignore::empty(),
            },
            max_depth: None,
            max_filesize: None,
            follow_links: false,
            same_file_system: false,
        }
    }

    pub fn add(&mut self, path: &Path) -> &mut WalkBuilder {
        self.roots.push(path.to_path_buf());
        self
    }

    pub fn hidden(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.hidden = yes;
        self
    }

    pub fn ignore(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.ignore = yes;
        self
    }

    pub fn git_ignore(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.git_ignore = yes;
        self
    }

    pub fn git_exclude(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.git_exclude = yes;
        self
    }

    pub fn git_global(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.git_global = yes;
        self
    }

    pub fn parents(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.parents = yes;
        self
    }

    pub fn require_git(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.require_git = yes;
        self
    }

    pub fn ignore_case_insensitive(&mut self, yes: bool) -> &mut WalkBuilder {
        self.shared.case_insensitive = yes;
        self
    }

    pub fn max_depth(&mut self, depth: Option<usize>) -> &mut WalkBuilder {
        self.max_depth = depth;
        self
    }

    pub fn follow_links(&mut self, yes: bool) -> &mut WalkBuilder {
        self.follow_links = yes;
        self
    }

    pub fn same_file_system(&mut self, yes: bool) -> &mut WalkBuilder {
        self.same_file_system = yes;
        self
    }

    pub fn max_filesize(&mut self, size: Option<u64>) -> &mut WalkBuilder {
        self.max_filesize = size;
        self
    }

    pub fn add_custom_ignore_filename(&mut self, name: &str) -> &mut WalkBuilder {
        self.shared.custom_names.push(OsString::from(name));
        self
    }

    pub fn overrides(&mut self, overrides: Override) -> &mut WalkBuilder {
        self.shared.overrides = overrides;
        self
    }

    pub fn types(&mut self, types: Types) -> &mut WalkBuilder {
        self.shared.types = types;
        self
    }

    pub fn build(self) -> Walk {
        let follow_links = self.follow_links;
        let roots: Vec<(PathBuf, bool)> = self
            .roots
            .into_iter()
            .map(|root| {
                let follow = follow_links || root.is_file();
                (root, follow)
            })
            .collect();
        let mut shared = self.shared;
        if shared.git_global {
            shared.global = global_gitignore(&self.current_dir, shared.case_insensitive);
        }
        let root_ignore = Ignore {
            node: Rc::new(Node::root()),
            absolute_base: None,
        };
        Walk {
            roots: roots.into_iter(),
            chain: Chain {
                shared,
                previous: root_ignore.clone(),
            },
            root_ignore,
            start: None,
            frames: Vec::new(),
            pushed_last: false,
            follow_links: false,
            root_device: None,
            max_depth: self.max_depth,
            max_filesize: self.max_filesize,
            same_file_system: self.same_file_system,
        }
    }
}

fn global_gitignore(current_dir: &Path, case_insensitive: bool) -> Gitignore {
    let Some(path) = gitconfig::native_excludes_path() else {
        return Gitignore::empty();
    };
    if !path.is_file() {
        return Gitignore::empty();
    }
    let mut builder = GitignoreBuilder::new(current_dir);
    builder.case_insensitive(case_insensitive);
    let _ = builder.add(&path);
    builder.build()
}

pub struct DirEntry {
    path: PathBuf,
    file_type: FileType,
    follow_link: bool,
    depth: usize,
    err: Option<Error>,
}

impl DirEntry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn file_type(&self) -> FileType {
        self.file_type
    }

    pub fn path_is_symlink(&self) -> bool {
        self.file_type.is_symlink() || self.follow_link
    }

    pub fn error(&self) -> Option<&Error> {
        self.err.as_ref()
    }

    fn metadata(&self) -> io::Result<fs::Metadata> {
        if self.follow_link {
            fs::metadata(&self.path)
        } else {
            fs::symlink_metadata(&self.path)
        }
    }

    fn file_name(&self) -> &OsStr {
        self.path.file_name().unwrap_or(self.path.as_os_str())
    }
}

#[derive(Debug)]
struct IoErrorWithPath {
    path: Option<PathBuf>,
    err: io::Error,
}

impl fmt::Display for IoErrorWithPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            None => self.err.fmt(f),
            Some(path) => write!(
                f,
                "IO error for operation on {}: {}",
                path.display(),
                self.err
            ),
        }
    }
}

impl std::error::Error for IoErrorWithPath {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.err)
    }
}

enum WalkError {
    Io {
        depth: usize,
        path: Option<PathBuf>,
        err: io::Error,
    },
    Loop {
        depth: usize,
        ancestor: PathBuf,
        child: PathBuf,
    },
}

impl WalkError {
    fn into_error(self) -> Error {
        match self {
            WalkError::Loop {
                depth,
                ancestor,
                child,
            } => Error::WithDepth {
                depth,
                err: Box::new(Error::Loop { ancestor, child }),
            },
            WalkError::Io { depth, path, err } => {
                let kind = err.kind();
                let inner = IoErrorWithPath {
                    path: path.clone(),
                    err,
                };
                let error = Error::WithDepth {
                    depth,
                    err: Box::new(Error::Io(io::Error::new(kind, inner))),
                };
                match path {
                    Some(path) => Error::WithPath {
                        path,
                        err: Box::new(error),
                    },
                    None => error,
                }
            }
        }
    }
}

struct Node {
    dir: PathBuf,
    parent: Option<Rc<Node>>,
    is_absolute_parent: bool,
    has_absolute_parent_rules: bool,
    custom: Gitignore,
    ignore: Gitignore,
    git_ignore: Gitignore,
    git_exclude: Gitignore,
    has_git: bool,
}

impl Node {
    fn root() -> Node {
        Node {
            dir: PathBuf::new(),
            parent: None,
            is_absolute_parent: true,
            has_absolute_parent_rules: false,
            custom: Gitignore::empty(),
            ignore: Gitignore::empty(),
            git_ignore: Gitignore::empty(),
            git_exclude: Gitignore::empty(),
            has_git: false,
        }
    }

    fn has_rules(&self) -> bool {
        !self.custom.is_empty()
            || !self.ignore.is_empty()
            || !self.git_ignore.is_empty()
            || !self.git_exclude.is_empty()
    }
}

#[derive(Clone)]
struct Ignore {
    node: Rc<Node>,
    absolute_base: Option<Rc<PathBuf>>,
}

impl Ignore {
    fn nodes(&self) -> Nodes<'_> {
        Nodes(Some(&self.node))
    }

    fn empty_child(&self, dir: &Path) -> Ignore {
        Ignore {
            node: Rc::new(Node {
                dir: dir.to_path_buf(),
                parent: Some(self.node.clone()),
                is_absolute_parent: false,
                has_absolute_parent_rules: self.node.has_absolute_parent_rules,
                ..Node::root()
            }),
            absolute_base: self.absolute_base.clone(),
        }
    }
}

struct Nodes<'a>(Option<&'a Node>);

impl<'a> Iterator for Nodes<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<&'a Node> {
        let node = self.0?;
        self.0 = node.parent.as_deref();
        Some(node)
    }
}

struct Chain {
    shared: Shared,
    previous: Ignore,
}

impl Chain {
    fn add_parents(&mut self, root: &Ignore, path: &Path) -> (Ignore, Option<Error>) {
        let shared = &self.shared;
        if !shared.parents && !shared.git_ignore && !shared.git_exclude && !shared.git_global {
            self.previous = root.clone();
            return (root.clone(), None);
        }
        let Ok(base) = path.canonicalize() else {
            self.previous = root.clone();
            return (root.clone(), None);
        };
        let base = Rc::new(base);
        let parents: Vec<&Path> = base.ancestors().skip(1).collect();
        let mut errors = PartialErrors::default();
        let mut chain = root.clone();
        for parent in parents.into_iter().rev() {
            let mut previous =
                std::iter::successors(Some(&self.previous.node), |node| node.parent.as_ref());
            if let Some(prebuilt) =
                previous.find(|node| node.is_absolute_parent && node.dir == parent)
            {
                chain = Ignore {
                    node: prebuilt.clone(),
                    absolute_base: Some(base.clone()),
                };
                continue;
            }
            let (mut node, error) = self.add_child_path(&chain, parent);
            errors.maybe_push(error);
            node.is_absolute_parent = true;
            node.has_git &= self.shared.git_ignore;
            node.has_absolute_parent_rules |= node.has_rules();
            chain = Ignore {
                node: Rc::new(node),
                absolute_base: Some(base.clone()),
            };
        }
        self.previous = chain.clone();
        (chain, errors.into_error())
    }

    fn add_child(&self, parent: &Ignore, dir: &Path) -> (Ignore, Option<Error>) {
        let (node, error) = self.add_child_path(parent, dir);
        let child = Ignore {
            node: Rc::new(node),
            absolute_base: parent.absolute_base.clone(),
        };
        (child, error)
    }

    fn add_child_path(&self, parent: &Ignore, dir: &Path) -> (Node, Option<Error>) {
        let shared = &self.shared;
        let check_vcs_dir = shared.require_git && (shared.git_ignore || shared.git_exclude);
        let git_type = if check_vcs_dir || shared.git_exclude {
            dir.join(".git")
                .metadata()
                .ok()
                .map(|metadata| metadata.file_type())
        } else {
            None
        };
        let has_git = check_vcs_dir && (git_type.is_some() || dir.join(".jj").exists());
        let mut errors = PartialErrors::default();
        let mut read = |file_dir: &Path, names: &[&OsStr]| {
            let (matcher, error) = create_gitignore(dir, file_dir, names, shared.case_insensitive);
            errors.maybe_push(error);
            matcher
        };
        let custom = if shared.custom_names.is_empty() {
            Gitignore::empty()
        } else {
            let names: Vec<&OsStr> = shared
                .custom_names
                .iter()
                .map(OsString::as_os_str)
                .collect();
            read(dir, &names)
        };
        let ignore = if shared.ignore {
            read(dir, &[OsStr::new(".ignore")])
        } else {
            Gitignore::empty()
        };
        let git_ignore = if shared.git_ignore {
            read(dir, &[OsStr::new(".gitignore")])
        } else {
            Gitignore::empty()
        };
        let git_exclude = match git_type {
            Some(file_type) if shared.git_exclude => match resolve_git_commondir(dir, file_type) {
                Ok(git_dir) => read(&git_dir, &[OsStr::new("info/exclude")]),
                Err(error) => {
                    errors.maybe_push(error);
                    Gitignore::empty()
                }
            },
            _ => Gitignore::empty(),
        };
        let node = Node {
            dir: dir.to_path_buf(),
            parent: Some(parent.node.clone()),
            is_absolute_parent: false,
            has_absolute_parent_rules: parent.node.has_absolute_parent_rules,
            custom,
            ignore,
            git_ignore,
            git_exclude,
            has_git,
        };
        (node, errors.into_error())
    }

    fn has_any_ignore_rules(&self) -> bool {
        let shared = &self.shared;
        shared.ignore
            || shared.git_global
            || shared.git_ignore
            || shared.git_exclude
            || !shared.custom_names.is_empty()
    }

    fn matched_entry(&self, chain: &Ignore, entry: &DirEntry) -> Match<()> {
        let found = self.matched(chain, entry.path(), entry.file_type.is_dir());
        if found.is_none() && self.shared.hidden && is_hidden(entry.path()) {
            return Match::Ignore(());
        }
        found
    }

    fn matched(&self, chain: &Ignore, path: &Path, is_dir: bool) -> Match<()> {
        let path = strip_prefix(Path::new("./"), path).unwrap_or(path);
        let shared = &self.shared;
        if !shared.overrides.is_empty() {
            let found = shared.overrides.matched(path, is_dir);
            if !found.is_none() {
                return found;
            }
        }
        let mut whitelisted = Match::None;
        if self.has_any_ignore_rules() {
            let found = self.matched_ignore(chain, path, is_dir);
            if found.is_ignore() {
                return found;
            }
            if found.is_whitelist() {
                whitelisted = found;
            }
        }
        if !shared.types.is_empty() {
            let found = shared.types.matched(path, is_dir);
            if found.is_ignore() {
                return found;
            }
            if found.is_whitelist() {
                whitelisted = found;
            }
        }
        whitelisted
    }

    fn matched_ignore(&self, chain: &Ignore, path: &Path, is_dir: bool) -> Match<()> {
        let shared = &self.shared;
        let any_git = !shared.require_git || chain.nodes().any(|node| node.has_git);
        let mut found = Found {
            custom: Match::None,
            ignore: Match::None,
            git_ignore: Match::None,
            git_exclude: Match::None,
            any_git,
            saw_git: false,
        };
        for node in chain.nodes().take_while(|node| !node.is_absolute_parent) {
            found.step(node, path, is_dir);
        }
        if shared.parents
            && chain.node.has_absolute_parent_rules
            && let Some(base) = chain.absolute_base.as_deref()
        {
            let relative = match chain
                .nodes()
                .take_while(|node| !node.is_absolute_parent)
                .last()
            {
                None => path,
                Some(node) if node.dir.as_path() == Path::new(".") => path,
                Some(node) => {
                    let without_dot_slash = strip_if_prefix(Path::new("./"), &node.dir);
                    let relative_base = strip_if_prefix(without_dot_slash, path);
                    strip_if_prefix(Path::new("/"), relative_base)
                }
            };
            let absolute = base.join(relative);
            for node in chain.nodes().skip_while(|node| !node.is_absolute_parent) {
                found.step(node, &absolute, is_dir);
            }
        }
        let global = if any_git {
            shared.global.matched(path, is_dir)
        } else {
            Match::None
        };
        found
            .custom
            .or(found.ignore)
            .or(found.git_ignore)
            .or(found.git_exclude)
            .or(global)
    }
}

struct Found {
    custom: Match<()>,
    ignore: Match<()>,
    git_ignore: Match<()>,
    git_exclude: Match<()>,
    any_git: bool,
    saw_git: bool,
}

impl Found {
    fn step(&mut self, node: &Node, path: &Path, is_dir: bool) {
        if self.custom.is_none() {
            self.custom = node.custom.matched(path, is_dir);
        }
        if self.ignore.is_none() {
            self.ignore = node.ignore.matched(path, is_dir);
        }
        if self.any_git && !self.saw_git && self.git_ignore.is_none() {
            self.git_ignore = node.git_ignore.matched(path, is_dir);
        }
        if self.any_git && !self.saw_git && self.git_exclude.is_none() {
            self.git_exclude = node.git_exclude.matched(path, is_dir);
        }
        self.saw_git = self.saw_git || node.has_git;
    }
}

fn strip_if_prefix<'a>(prefix: &Path, path: &'a Path) -> &'a Path {
    strip_prefix(prefix, path).unwrap_or(path)
}

fn is_hidden(path: &Path) -> bool {
    file_name(path).is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
}

fn create_gitignore(
    dir: &Path,
    file_dir: &Path,
    names: &[&OsStr],
    case_insensitive: bool,
) -> (Gitignore, Option<Error>) {
    let mut builder = GitignoreBuilder::new(dir);
    builder.case_insensitive(case_insensitive);
    let mut errors = PartialErrors::default();
    for name in names {
        let path = file_dir.join(name);
        if path.exists() {
            errors.maybe_push_ignore_io(builder.add(&path));
        }
    }
    (builder.build(), errors.into_error())
}

fn resolve_git_commondir(dir: &Path, git_type: FileType) -> Result<PathBuf, Option<Error>> {
    let git_dir = dir.join(".git");
    if !git_type.is_file() {
        return Ok(git_dir);
    }
    let io_error = |err: io::Error, path: &Path| Some(Error::Io(err).with_path(path));
    let file = File::open(&git_dir).map_err(|err| io_error(err, &git_dir))?;
    let line = match BufReader::new(file).lines().next() {
        Some(Ok(line)) => line,
        Some(Err(err)) => return Err(io_error(err, &git_dir)),
        None => return Err(None),
    };
    let Some(real_git_dir) = line.strip_prefix("gitdir: ") else {
        return Err(None);
    };
    let real_git_dir = PathBuf::from(real_git_dir);
    let commondir_file = real_git_dir.join("commondir");
    let Ok(file) = File::open(&commondir_file) else {
        return Err(None);
    };
    let commondir = match BufReader::new(file).lines().next() {
        Some(Ok(line)) => line,
        Some(Err(err)) => return Err(io_error(err, &commondir_file)),
        None => return Err(None),
    };
    if commondir.starts_with('.') {
        Ok(real_git_dir.join(commondir))
    } else {
        Ok(PathBuf::from(commondir))
    }
}

struct Frame {
    entries: std::vec::IntoIter<Result<DirEntry, WalkError>>,
    chain: Ignore,
    ancestor: Option<PathBuf>,
}

pub struct Walk {
    roots: std::vec::IntoIter<(PathBuf, bool)>,
    chain: Chain,
    root_ignore: Ignore,
    start: Option<(PathBuf, Ignore)>,
    frames: Vec<Frame>,
    pushed_last: bool,
    follow_links: bool,
    root_device: Option<u64>,
    max_depth: Option<usize>,
    max_filesize: Option<u64>,
    same_file_system: bool,
}

impl Iterator for Walk {
    type Item = Result<DirEntry, Error>;

    fn next(&mut self) -> Option<Result<DirEntry, Error>> {
        self.pushed_last = false;
        loop {
            if let Some((root, chain)) = self.start.take() {
                if let Some(item) = self.start_root(root, &chain) {
                    return Some(item);
                }
                continue;
            }
            if let Some(frame) = self.frames.last_mut() {
                match frame.entries.next() {
                    None => {
                        self.frames.pop();
                    }
                    Some(Err(error)) => return Some(Err(error.into_error())),
                    Some(Ok(entry)) => {
                        let chain = frame.chain.clone();
                        if let Some(item) = self.handle(entry, &chain) {
                            return Some(item);
                        }
                    }
                }
                continue;
            }
            let (root, follow) = self.roots.next()?;
            self.follow_links = follow;
            self.root_device = None;
            if !root.is_dir() {
                self.chain.previous = self.root_ignore.clone();
                self.start = Some((root, self.root_ignore.clone()));
                continue;
            }
            let root_ignore = self.root_ignore.clone();
            let (chain, error) = self.chain.add_parents(&root_ignore, &root);
            self.start = Some((root, chain));
            if let Some(error) = error {
                return Some(Err(error));
            }
        }
    }
}

impl Walk {
    pub fn skip_current_dir(&mut self) {
        if self.pushed_last {
            self.frames.pop();
            self.pushed_last = false;
        }
    }

    fn start_root(&mut self, root: PathBuf, chain: &Ignore) -> Option<Result<DirEntry, Error>> {
        if self.same_file_system {
            match device_number(&root) {
                Ok(device) => self.root_device = Some(device),
                Err(err) => return Some(Err(io_error(0, Some(root), err))),
            }
        }
        let entry = match entry_from_path(0, root, false) {
            Ok(entry) => entry,
            Err(error) => return Some(Err(error.into_error())),
        };
        self.handle(entry, chain)
    }

    fn handle(&mut self, entry: DirEntry, chain: &Ignore) -> Option<Result<DirEntry, Error>> {
        let mut entry = entry;
        let depth = entry.depth;
        if self.follow_links && entry.file_type.is_symlink() {
            entry = match self.follow(entry) {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error.into_error())),
            };
        }
        let file_type = entry.file_type;
        let mut descend = false;
        if !file_type.is_symlink() && file_type.is_dir() {
            if self.same_file_system && depth > 0 {
                let root_device = self.root_device.expect("the root's device was read");
                match device_number(&entry.path) {
                    Ok(device) => descend = device == root_device,
                    Err(err) => return Some(Err(io_error(depth, Some(entry.path), err))),
                }
            } else {
                descend = true;
            }
        } else if depth == 0 && file_type.is_symlink() {
            match fs::metadata(&entry.path) {
                Ok(metadata) => descend = metadata.is_dir(),
                Err(err) => return Some(Err(io_error(depth, Some(entry.path), err))),
            }
        }
        let is_dir_event = file_type.is_dir()
            || (file_type.is_symlink()
                && depth == 0
                && fs::metadata(&entry.path).is_ok_and(|metadata| metadata.is_dir()));
        if self.skip_entry(&entry, chain) {
            if is_dir_event && !descend {
                self.frames.pop();
            }
            return None;
        }
        if !is_dir_event {
            return Some(Ok(entry));
        }
        let (child, error) = if self.max_depth == Some(depth) {
            (chain.empty_child(&entry.path), None)
        } else {
            self.chain.add_child(chain, &entry.path)
        };
        entry.err = error;
        if descend && self.max_depth.is_none_or(|max| depth < max) {
            let entries = read_sorted(&entry.path, depth);
            let ancestor = self.follow_links.then(|| entry.path.clone());
            self.frames.push(Frame {
                entries: entries.into_iter(),
                chain: child,
                ancestor,
            });
            self.pushed_last = true;
        }
        Some(Ok(entry))
    }

    fn follow(&self, entry: DirEntry) -> Result<DirEntry, WalkError> {
        let depth = entry.depth;
        let entry = entry_from_path(depth, entry.path, true)?;
        if entry.file_type.is_dir() {
            self.check_loop(depth, &entry.path)?;
        }
        Ok(entry)
    }

    fn check_loop(&self, depth: usize, child: &Path) -> Result<(), WalkError> {
        let identity = |path: &Path| {
            file_identity(path).map_err(|err| WalkError::Io {
                depth,
                path: None,
                err,
            })
        };
        let child_identity = identity(child)?;
        for ancestor in self
            .frames
            .iter()
            .rev()
            .filter_map(|frame| frame.ancestor.as_ref())
        {
            if identity(ancestor)? == child_identity {
                return Err(WalkError::Loop {
                    depth,
                    ancestor: ancestor.clone(),
                    child: child.to_path_buf(),
                });
            }
        }
        Ok(())
    }

    fn skip_entry(&self, entry: &DirEntry, chain: &Ignore) -> bool {
        if entry.depth == 0 {
            return false;
        }
        if self.chain.matched_entry(chain, entry).is_ignore() {
            return true;
        }
        if let Some(max) = self.max_filesize
            && !entry.file_type.is_dir()
        {
            return entry.metadata().is_ok_and(|metadata| metadata.len() > max);
        }
        false
    }
}

fn io_error(depth: usize, path: Option<PathBuf>, err: io::Error) -> Error {
    WalkError::Io { depth, path, err }.into_error()
}

fn entry_from_path(depth: usize, path: PathBuf, follow: bool) -> Result<DirEntry, WalkError> {
    let metadata = if follow {
        fs::metadata(&path)
    } else {
        fs::symlink_metadata(&path)
    };
    match metadata {
        Ok(metadata) => Ok(DirEntry {
            path,
            file_type: metadata.file_type(),
            follow_link: follow,
            depth,
            err: None,
        }),
        Err(err) => Err(WalkError::Io {
            depth,
            path: Some(path),
            err,
        }),
    }
}

fn read_sorted(dir: &Path, depth: usize) -> Vec<Result<DirEntry, WalkError>> {
    let reader = match fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(err) => {
            return vec![Err(WalkError::Io {
                depth,
                path: Some(dir.to_path_buf()),
                err,
            })];
        }
    };
    let mut entries: Vec<Result<DirEntry, WalkError>> = reader
        .map(|item| {
            let child = depth + 1;
            let item = item.map_err(|err| WalkError::Io {
                depth: child,
                path: None,
                err,
            })?;
            let path = item.path();
            match item.file_type() {
                Ok(file_type) => Ok(DirEntry {
                    path,
                    file_type,
                    follow_link: false,
                    depth: child,
                    err: None,
                }),
                Err(err) => Err(WalkError::Io {
                    depth: child,
                    path: Some(path),
                    err,
                }),
            }
        })
        .collect();
    entries.sort_by(|a, b| match (a, b) {
        (Ok(a), Ok(b)) => a.file_name().cmp(b.file_name()),
        (Err(_), Err(_)) => std::cmp::Ordering::Equal,
        (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
        (Err(_), Ok(_)) => std::cmp::Ordering::Less,
    });
    entries
}

#[cfg(unix)]
fn device_number(path: &Path) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).map(|metadata| metadata.dev())
}

#[cfg(not(unix))]
fn device_number(_: &Path) -> io::Result<u64> {
    Err(io::Error::other(
        "walkdir: same_file_system option not supported on this platform",
    ))
}

#[cfg(unix)]
fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = File::open(path)?.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    File::open(path)?;
    Err(io::ErrorKind::Unsupported.into())
}

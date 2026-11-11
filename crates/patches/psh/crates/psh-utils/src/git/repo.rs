//! A repository: where it is (a .git directory, a .git file of a linked worktree,
//! GIT_DIR), its configuration, its references and its objects by kind.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::io;
use std::rc::Rc;

use super::store::{DELTA_BASE_CACHE, Data, Fs, Kind, Odb, Oid, corrupt, join};
use super::{Error, Res};

/// Configuration: the files git reads, later ones winning, and -c from the command
/// line last. Keys are section.subsection.name with the section and name in lower case.
#[derive(Default)]
pub struct Config {
    pub entries: Vec<(String, String)>,
}

impl Config {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)?.to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" | "1" | "" => Some(true),
            "false" | "no" | "off" | "0" => Some(false),
            _ => None,
        }
    }

    pub fn parse(&mut self, text: &[u8]) {
        let text = String::from_utf8_lossy(text);
        let mut section = String::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let Some(end) = rest.rfind(']') else { continue };
                let head = rest[..end].trim();
                section = match head.split_once(char::is_whitespace) {
                    Some((name, sub)) => {
                        let sub = sub.trim().trim_matches('"').replace("\\\"", "\"");
                        format!(
                            "{}.{}",
                            name.to_ascii_lowercase(),
                            sub.replace("\\\\", "\\")
                        )
                    }
                    None => match head.split_once('.') {
                        Some((name, sub)) => {
                            format!("{}.{}", name.to_ascii_lowercase(), sub.to_ascii_lowercase())
                        }
                        None => head.to_ascii_lowercase(),
                    },
                };
                continue;
            }
            let (name, value) = match line.split_once('=') {
                Some((name, value)) => (name.trim(), value.trim().to_string()),
                None => (line, "true".to_string()),
            };
            let mut raw = value;
            while raw.ends_with('\\') {
                raw.pop();
                match lines.next() {
                    Some(next) => raw.push_str(next.trim_end()),
                    None => break,
                }
            }
            let mut value = String::new();
            let mut quoted = false;
            let mut chars = raw.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => quoted = !quoted,
                    '\\' => match chars.next() {
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some(other) => value.push(other),
                        None => {}
                    },
                    '#' | ';' if !quoted => break,
                    _ => value.push(c),
                }
            }
            let value = if raw.contains('"') {
                value
            } else {
                value.trim_end().to_string()
            };
            self.entries
                .push((format!("{section}.{}", name.to_ascii_lowercase()), value));
        }
    }
}

pub enum RefValue {
    Direct(Oid),
    Symbolic(String),
}

struct Packed {
    name: String,
    oid: Oid,
    peeled: Option<Oid>,
}

pub struct Commit {
    pub id: Oid,
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Vec<u8>,
    pub committer: Vec<u8>,
    /// Where the message starts in data.
    pub body: usize,
    pub data: Data,
}

/// A name, an address, a time and a zone: one author or committer line.
pub struct Ident<'a> {
    pub name: &'a [u8],
    pub email: &'a [u8],
    pub time: i64,
    /// The zone as written (-0700 is -700).
    pub zone: i32,
}

impl Ident<'_> {
    pub fn parse(line: &[u8]) -> Ident<'_> {
        let open = line.iter().rposition(|&b| b == b'<');
        let close = line.iter().rposition(|&b| b == b'>');
        let (Some(open), Some(close)) = (open, close) else {
            return Ident {
                name: line,
                email: b"",
                time: 0,
                zone: 0,
            };
        };
        let mut name = &line[..open];
        while name.last() == Some(&b' ') {
            name = &name[..name.len() - 1];
        }
        let rest = String::from_utf8_lossy(&line[(close + 1).min(line.len())..]).into_owned();
        let mut words = rest.split_whitespace();
        let time = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
        let zone = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
        Ident {
            name,
            email: &line[open + 1..close.max(open + 1)],
            time,
            zone,
        }
    }
}

impl Commit {
    pub fn parse(id: Oid, data: Data) -> io::Result<Commit> {
        let mut tree = None;
        let mut parents = Vec::new();
        let mut author = Vec::new();
        let mut committer = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            let end = data[pos..]
                .iter()
                .position(|&b| b == b'\n')
                .map_or(data.len(), |n| pos + n);
            let line = &data[pos..end];
            pos = (end + 1).min(data.len());
            if line.is_empty() {
                break;
            }
            if let Some(hex) = line.strip_prefix(b"tree ") {
                tree = Oid::from_hex(hex);
            } else if let Some(hex) = line.strip_prefix(b"parent ") {
                parents.extend(Oid::from_hex(hex));
            } else if let Some(who) = line.strip_prefix(b"author ") {
                author = who.to_vec();
            } else if let Some(who) = line.strip_prefix(b"committer ") {
                committer = who.to_vec();
            }
            if end == data.len() {
                pos = data.len();
            }
        }
        Ok(Commit {
            id,
            tree: tree.ok_or_else(|| corrupt("a commit has no tree"))?,
            parents,
            author,
            committer,
            body: pos,
            data,
        })
    }

    pub fn message(&self) -> &[u8] {
        &self.data[self.body..]
    }

    pub fn time(&self) -> i64 {
        Ident::parse(&self.committer).time
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub mode: u32,
    pub name: Vec<u8>,
    pub oid: Oid,
}

impl Entry {
    pub fn is_tree(&self) -> bool {
        self.mode == 0o40000
    }
}

pub fn parse_tree(data: &[u8]) -> io::Result<Vec<Entry>> {
    let bad = || corrupt("a tree object is malformed");
    let mut entries = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let space = data[pos..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(bad)?;
        let mut mode = 0u32;
        for &b in &data[pos..pos + space] {
            mode = mode * 8 + u32::from(b.wrapping_sub(b'0') & 7);
        }
        pos += space + 1;
        let nul = data[pos..].iter().position(|&b| b == 0).ok_or_else(bad)?;
        let name = data[pos..pos + nul].to_vec();
        pos += nul + 1;
        let oid = data.get(pos..pos + 20).ok_or_else(bad)?;
        entries.push(Entry {
            mode,
            name,
            oid: Oid::from_bytes(oid),
        });
        pos += 20;
    }
    Ok(entries)
}

pub struct Repo<'a> {
    pub fs: &'a Fs<'a>,
    /// The repository's own directory (of a linked worktree: its directory under
    /// worktrees/).
    pub git_dir: Vec<u8>,
    /// What every worktree shares: objects, refs, configuration.
    pub common_dir: Vec<u8>,
    /// The working tree, when there is one.
    pub work_tree: Option<Vec<u8>>,
    pub config: Config,
    pub odb: Odb<'a>,
    packed: OnceCell<Vec<Packed>>,
    commits: RefCell<HashMap<Oid, Rc<Commit>>>,
    trees: RefCell<HashMap<Oid, Rc<Vec<Entry>>>>,
    abbrev: OnceCell<usize>,
    /// The commits a shallow clone ends at: their parents are not here.
    shallow: OnceCell<Vec<Oid>>,
}

fn parent(path: &[u8]) -> Option<&[u8]> {
    if path.len() <= 1 {
        return None;
    }
    let cut = path.iter().rposition(|&b| b == b'/')?;
    Some(if cut == 0 { &path[..1] } else { &path[..cut] })
}

/// A path without its dot and dot-dot parts (lexically).
pub fn normalize(path: &[u8]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path.split(|&b| b == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let mut out = Vec::new();
    for part in parts {
        out.push(b'/');
        out.extend_from_slice(part);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let Some((last, rest)) = s.split_last() {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

impl<'a> Repo<'a> {
    /// The repository cwd is in, as git finds it.
    pub fn discover(
        fs: &'a Fs<'a>,
        cwd: &[u8],
        env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
        overrides: &[(String, String)],
    ) -> Res<Repo<'a>> {
        let cwd = normalize(cwd);
        let (git_dir, work_tree) = match env(b"GIT_DIR") {
            Some(dir) => {
                let dir = if dir.first() == Some(&b'/') {
                    normalize(&dir)
                } else {
                    normalize(&join(&cwd, &dir))
                };
                if !Self::is_git_dir(fs, &dir)? {
                    return Err(Error::Fatal(format!(
                        "not a git repository: '{}'",
                        String::from_utf8_lossy(&dir)
                    )));
                }
                let tree = env(b"GIT_WORK_TREE").map(|t| {
                    if t.first() == Some(&b'/') {
                        normalize(&t)
                    } else {
                        normalize(&join(&cwd, &t))
                    }
                });
                (dir, Some(tree.unwrap_or_else(|| cwd.clone())))
            }
            None => {
                let mut dir: &[u8] = &cwd;
                loop {
                    let dot = join(dir, b".git");
                    if let Some(meta) = fs.stat_opt(&dot, true)? {
                        if meta.kind == psh_engine::host::FileKind::Dir {
                            if Self::is_git_dir(fs, &dot)? {
                                break (dot, Some(dir.to_vec()));
                            }
                        } else {
                            let text = fs.read(&dot)?;
                            if let Some(target) = trim(&text).strip_prefix(b"gitdir: ") {
                                let target = if target.first() == Some(&b'/') {
                                    normalize(target)
                                } else {
                                    normalize(&join(dir, target))
                                };
                                break (target, Some(dir.to_vec()));
                            }
                            return Err(Error::Fatal(format!(
                                "invalid gitfile format: {}",
                                String::from_utf8_lossy(&dot)
                            )));
                        }
                    }
                    if Self::is_git_dir(fs, dir)? {
                        break (dir.to_vec(), None);
                    }
                    match parent(dir) {
                        Some(up) => dir = up,
                        None => {
                            return Err(Error::Fatal(
                                "not a git repository (or any of the parent directories): .git"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        };
        let common_dir = match fs.read_opt(&join(&git_dir, b"commondir"))? {
            Some(text) => {
                let rel = trim(&text);
                if rel.first() == Some(&b'/') {
                    normalize(rel)
                } else {
                    normalize(&join(&git_dir, rel))
                }
            }
            None => git_dir.clone(),
        };
        let mut config = Config::default();
        let global = match env(b"GIT_CONFIG_GLOBAL") {
            Some(path) => vec![path],
            None => {
                let mut paths = Vec::new();
                let home = env(b"HOME");
                match (env(b"XDG_CONFIG_HOME"), &home) {
                    (Some(xdg), _) if !xdg.is_empty() => paths.push(join(&xdg, b"git/config")),
                    (_, Some(home)) => paths.push(join(home, b".config/git/config")),
                    _ => {}
                }
                if let Some(home) = &home {
                    paths.push(join(home, b".gitconfig"));
                }
                paths
            }
        };
        for path in global {
            if path.first() == Some(&b'/')
                && let Some(text) = fs.read_opt(&path)?
            {
                config.parse(&text);
            }
        }
        if let Some(text) = fs.read_opt(&join(&common_dir, b"config"))? {
            config.parse(&text);
        }
        if config.bool("extensions.worktreeconfig") == Some(true)
            && let Some(text) = fs.read_opt(&join(&git_dir, b"config.worktree"))?
        {
            config.parse(&text);
        }
        config.entries.extend(overrides.iter().cloned());
        if let Some(format) = config.get("extensions.objectformat")
            && format != "sha1"
        {
            return Err(Error::Lacks(
                format!("this repository's objects are named by {format}"),
                "git for object formats other than SHA-1",
            ));
        }
        if let Some(storage) = config.get("extensions.refstorage")
            && storage != "files"
        {
            return Err(Error::Lacks(
                format!("this repository keeps its references as {storage}"),
                "git for reference stores other than files",
            ));
        }
        let mut work_tree = work_tree;
        if config.bool("core.bare") == Some(true) {
            work_tree = None;
        }
        if let Some(tree) = config.get("core.worktree")
            && env(b"GIT_DIR").is_none()
        {
            let tree = tree.as_bytes();
            work_tree = Some(if tree.first() == Some(&b'/') {
                normalize(tree)
            } else {
                normalize(&join(&git_dir, tree))
            });
        }
        let limit = config
            .get("core.deltabasecachelimit")
            .and_then(parse_size)
            .unwrap_or(DELTA_BASE_CACHE);
        let odb = Odb::open(fs, join(&common_dir, b"objects"), limit)?;
        Ok(Repo {
            fs,
            git_dir,
            common_dir,
            work_tree,
            config,
            odb,
            packed: OnceCell::new(),
            commits: RefCell::default(),
            trees: RefCell::default(),
            abbrev: OnceCell::new(),
            shallow: OnceCell::new(),
        })
    }

    fn is_git_dir(fs: &Fs<'_>, dir: &[u8]) -> io::Result<bool> {
        if fs.stat_opt(&join(dir, b"HEAD"), false)?.is_none() {
            return Ok(false);
        }
        if fs.stat_opt(&join(dir, b"commondir"), true)?.is_some() {
            return Ok(true);
        }
        Ok(fs.stat_opt(&join(dir, b"objects"), true)?.is_some()
            && fs.stat_opt(&join(dir, b"refs"), true)?.is_some())
    }

    fn ref_path(&self, name: &str) -> Vec<u8> {
        let shared = (name == "refs" || name.starts_with("refs/"))
            && !name.starts_with("refs/bisect/")
            && !name.starts_with("refs/worktree/")
            && !name.starts_with("refs/rewritten/");
        join(
            if shared {
                &self.common_dir
            } else {
                &self.git_dir
            },
            name.as_bytes(),
        )
    }

    fn packed(&self) -> io::Result<&[Packed]> {
        if let Some(packed) = self.packed.get() {
            return Ok(packed);
        }
        let mut refs: Vec<Packed> = Vec::new();
        if let Some(text) = self.fs.read_opt(&join(&self.common_dir, b"packed-refs"))? {
            for line in text.split(|&b| b == b'\n') {
                if line.is_empty() || line[0] == b'#' {
                    continue;
                }
                if line[0] == b'^' {
                    if let (Some(last), Some(oid)) = (refs.last_mut(), Oid::from_hex(&line[1..])) {
                        last.peeled = Some(oid);
                    }
                    continue;
                }
                if line.len() > 41
                    && let Some(oid) = Oid::from_hex(&line[..40])
                {
                    refs.push(Packed {
                        name: String::from_utf8_lossy(&line[41..]).into_owned(),
                        oid,
                        peeled: None,
                    });
                }
            }
        }
        refs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(self.packed.get_or_init(|| refs))
    }

    /// What a reference file or a packed entry says, unresolved.
    pub fn read_ref(&self, name: &str) -> io::Result<Option<RefValue>> {
        if let Some(text) = self.fs.read_opt(&self.ref_path(name))? {
            let text = trim(&text);
            if let Some(target) = text.strip_prefix(b"ref:") {
                let target = String::from_utf8_lossy(target).trim().to_string();
                return Ok(Some(RefValue::Symbolic(target)));
            }
            if let Some(oid) = text.get(..40).and_then(Oid::from_hex) {
                return Ok(Some(RefValue::Direct(oid)));
            }
            return Ok(None);
        }
        if name.starts_with("refs/") {
            let packed = self.packed()?;
            if let Ok(i) = packed.binary_search_by(|p| p.name.as_str().cmp(name)) {
                return Ok(Some(RefValue::Direct(packed[i].oid)));
            }
        }
        Ok(None)
    }

    /// The object a reference names, through symbolic references.
    pub fn resolve_ref(&self, name: &str) -> io::Result<Option<Oid>> {
        let mut name = name.to_string();
        for _ in 0..5 {
            match self.read_ref(&name)? {
                Some(RefValue::Direct(oid)) => return Ok(Some(oid)),
                Some(RefValue::Symbolic(target)) => name = target,
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    /// The references whose names start with prefix, by name.
    pub fn refs(&self, prefix: &str) -> io::Result<Vec<(String, Oid)>> {
        let mut found: Vec<(String, Oid)> = Vec::new();
        let dir = prefix.trim_end_matches('/');
        let mut stack = vec![dir.to_string()];
        while let Some(dir) = stack.pop() {
            let path = self.ref_path(&dir);
            for name in self.fs.list(&path)? {
                let full = format!("{dir}/{}", String::from_utf8_lossy(&name));
                let meta = self.fs.stat(&join(&path, &name), true);
                match meta {
                    Ok(m) if m.kind == psh_engine::host::FileKind::Dir => stack.push(full),
                    Ok(_) => {
                        if let Some(oid) = self.resolve_ref(&full)? {
                            found.push((full, oid));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Err(e),
                    Err(_) => {}
                }
            }
        }
        for p in self.packed()? {
            if p.name.starts_with(prefix) && !found.iter().any(|(n, _)| *n == p.name) {
                found.push((p.name.clone(), p.oid));
            }
        }
        found.retain(|(name, _)| name.starts_with(prefix));
        found.sort();
        Ok(found)
    }

    /// The branch HEAD is on (refs/heads/NAME), or None when it is detached.
    pub fn head_ref(&self) -> io::Result<Option<String>> {
        match self.read_ref("HEAD")? {
            Some(RefValue::Symbolic(target)) => Ok(Some(target)),
            _ => Ok(None),
        }
    }

    pub fn head(&self) -> io::Result<Option<Oid>> {
        self.resolve_ref("HEAD")
    }

    pub fn object(&self, oid: &Oid) -> Res<(Kind, Data)> {
        match self.odb.read(oid)? {
            Some(found) => Ok(found),
            None => Err(Error::Fatal(format!("bad object {}", oid.hex()))),
        }
    }

    pub fn commit(&self, oid: &Oid) -> Res<Rc<Commit>> {
        if let Some(commit) = self.commits.borrow().get(oid) {
            return Ok(commit.clone());
        }
        let (kind, data) = self.object(oid)?;
        if kind != Kind::Commit {
            return Err(Error::Fatal(format!(
                "object {} is a {}, not a commit",
                oid.hex(),
                kind.name()
            )));
        }
        let mut commit = Commit::parse(*oid, data)?;
        if self.shallow()?.binary_search(oid).is_ok() {
            commit.parents.clear();
        }
        let commit = Rc::new(commit);
        self.commits.borrow_mut().insert(*oid, commit.clone());
        Ok(commit)
    }

    fn shallow(&self) -> io::Result<&[Oid]> {
        if let Some(shallow) = self.shallow.get() {
            return Ok(shallow);
        }
        let mut oids = Vec::new();
        if let Some(text) = self.fs.read_opt(&join(&self.common_dir, b"shallow"))? {
            oids.extend(text.split(|&b| b == b'\n').filter_map(Oid::from_hex));
        }
        oids.sort();
        Ok(self.shallow.get_or_init(|| oids))
    }

    pub fn tree(&self, oid: &Oid) -> Res<Rc<Vec<Entry>>> {
        if let Some(tree) = self.trees.borrow().get(oid) {
            return Ok(tree.clone());
        }
        let (kind, data) = self.object(oid)?;
        if kind != Kind::Tree {
            return Err(Error::Fatal(format!(
                "object {} is a {}, not a tree",
                oid.hex(),
                kind.name()
            )));
        }
        let tree = Rc::new(parse_tree(&data)?);
        self.trees.borrow_mut().insert(*oid, tree.clone());
        Ok(tree)
    }

    pub fn blob(&self, oid: &Oid) -> Res<Data> {
        Ok(self.object(oid)?.1)
    }

    /// Follows tags to what they name.
    pub fn peel(&self, oid: &Oid) -> Res<(Oid, Kind)> {
        let mut oid = *oid;
        loop {
            let (kind, data) = self.object(&oid)?;
            if kind != Kind::Tag {
                return Ok((oid, kind));
            }
            oid = tag_target(&data).ok_or_else(|| corrupt("a tag names no object"))?;
        }
    }

    /// The entry a path names in a tree.
    pub fn tree_entry(&self, tree: &Oid, path: &[u8]) -> Res<Option<Entry>> {
        let mut at = *tree;
        let mut parts = path
            .split(|&b| b == b'/')
            .filter(|p| !p.is_empty())
            .peekable();
        while let Some(part) = parts.next() {
            let entries = self.tree(&at)?;
            let Some(entry) = entries.iter().find(|e| e.name == part) else {
                return Ok(None);
            };
            if parts.peek().is_none() {
                return Ok(Some(entry.clone()));
            }
            if !entry.is_tree() {
                return Ok(None);
            }
            at = entry.oid;
        }
        Ok(Some(Entry {
            mode: 0o40000,
            name: Vec::new(),
            oid: *tree,
        }))
    }

    /// The digits git abbreviates a name to before it checks that it is unique:
    /// core.abbrev, else from the number of packed objects, at least seven.
    pub fn abbrev_len(&self) -> Res<usize> {
        if let Some(len) = self.abbrev.get() {
            return Ok(*len);
        }
        let len = match self.config.get("core.abbrev") {
            Some("no" | "off" | "false") => 40,
            Some(n) if n.parse::<usize>().is_ok() => n.parse::<usize>().unwrap_or(7).clamp(4, 40),
            _ => {
                let count = self.odb.approximate_count()?;
                let bits = (u64::BITS - count.leading_zeros()).max(1) as usize;
                bits.div_ceil(2).max(7)
            }
        };
        Ok(*self.abbrev.get_or_init(|| len))
    }

    pub fn short(&self, oid: &Oid) -> Res<String> {
        Ok(self.odb.abbrev(oid, self.abbrev_len()?)?)
    }

    pub fn short_at(&self, oid: &Oid, min: usize) -> Res<String> {
        Ok(self.odb.abbrev(oid, min)?)
    }
}

pub fn tag_target(data: &[u8]) -> Option<Oid> {
    let line = data.split(|&b| b == b'\n').next()?;
    Oid::from_hex(line.strip_prefix(b"object ")?)
}

fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    let (digits, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s, ""),
    };
    let n: usize = digits.parse().ok()?;
    let mult = match unit.to_ascii_lowercase().as_str() {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        _ => return None,
    };
    n.checked_mul(mult)
}

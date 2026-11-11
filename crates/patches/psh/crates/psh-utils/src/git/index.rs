//! The index (gitformat-index(5)): versions 2 to 4 read, version 2 or 3 written.

use psh_engine::host::Metadata;

use super::repo::Repo;
use super::store::{Oid, corrupt, join};
use super::{Error, Res};
use crate::sum_hash::{Algo, Digest};

#[derive(Clone)]
pub struct IndexEntry {
    pub path: Vec<u8>,
    pub oid: Oid,
    pub mode: u32,
    pub stage: u8,
    pub size: u32,
    pub mtime: (u32, u32),
    pub ctime: (u32, u32),
    pub dev: u32,
    pub ino: u32,
    pub uid: u32,
    pub gid: u32,
    pub assume_valid: bool,
    pub skip_worktree: bool,
    pub intent_to_add: bool,
    /// The file is written when the command's changes are carried out, and what the
    /// file system then says of it is recorded.
    pub fresh: bool,
}

impl IndexEntry {
    /// An entry with nothing recorded of the file: it is compared by content.
    pub fn new(path: Vec<u8>, mode: u32, oid: Oid) -> IndexEntry {
        IndexEntry {
            path,
            oid,
            mode,
            stage: 0,
            size: 0,
            mtime: (0, 0),
            ctime: (0, 0),
            dev: 0,
            ino: 0,
            uid: 0,
            gid: 0,
            assume_valid: false,
            skip_worktree: false,
            intent_to_add: false,
            fresh: false,
        }
    }

    /// Records what the file system says of the file. ctime is the status change when
    /// the host tells it, else the modification time.
    pub fn record(&mut self, meta: &Metadata, ctime_ns: Option<i64>) {
        let split = |ns: i64| {
            (
                ns.div_euclid(1_000_000_000) as u32,
                ns.rem_euclid(1_000_000_000) as u32,
            )
        };
        self.mtime = split(meta.mtime_ns);
        self.ctime = split(ctime_ns.unwrap_or(meta.mtime_ns));
        self.dev = meta.dev as u32;
        self.ino = meta.ino as u32;
        self.uid = meta.uid;
        self.gid = meta.gid;
        self.size = meta.size as u32;
    }
}

/// The tree a directory of the index was last written as (the TREE extension): valid
/// while count is not negative, and then count is how many entries are under it.
#[derive(Clone)]
pub struct CacheTree {
    pub name: Vec<u8>,
    pub count: i64,
    pub oid: Oid,
    pub children: Vec<CacheTree>,
}

impl CacheTree {
    /// Marks the trees on the way to path as no longer written.
    fn invalidate(&mut self, path: &[u8]) {
        self.count = -1;
        if let Some(slash) = path.iter().position(|&b| b == b'/')
            && let Some(child) = self.children.iter_mut().find(|c| c.name == path[..slash])
        {
            child.invalidate(&path[slash + 1..]);
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.name);
        out.push(0);
        out.extend_from_slice(format!("{} {}\n", self.count, self.children.len()).as_bytes());
        if self.count >= 0 {
            out.extend_from_slice(&self.oid.0);
        }
        // git keeps the subtrees by the length of their names, then by name.
        let mut children: Vec<&CacheTree> = self.children.iter().collect();
        children.sort_by(|a, b| (a.name.len(), &a.name).cmp(&(b.name.len(), &b.name)));
        for child in children {
            child.write(out);
        }
    }

    /// Drops the trees of directories the index no longer has, and those whose count
    /// is not what the index holds. entries are the ones under this tree, and their
    /// paths share the first depth bytes.
    fn settle(&mut self, entries: &[IndexEntry], depth: usize) {
        if self.count >= 0 && self.count as usize != entries.len() {
            self.count = -1;
        }
        if entries.iter().any(|e| e.intent_to_add) {
            self.count = -1;
        }
        self.children.retain_mut(|child| {
            let mut prefix = entries
                .first()
                .map_or(Vec::new(), |e| e.path[..depth].to_vec());
            prefix.extend_from_slice(&child.name);
            prefix.push(b'/');
            let start = entries.partition_point(|e| e.path.as_slice() < prefix.as_slice());
            let len = entries[start..]
                .iter()
                .take_while(|e| e.path.starts_with(&prefix))
                .count();
            if len == 0 {
                return false;
            }
            child.settle(&entries[start..start + len], prefix.len());
            true
        });
    }
}

fn cache_tree(data: &[u8], pos: &mut usize) -> Option<CacheTree> {
    let nul = data[*pos..].iter().position(|&b| b == 0)?;
    let name = data[*pos..*pos + nul].to_vec();
    *pos += nul + 1;
    let end = data[*pos..].iter().position(|&b| b == b'\n')?;
    let line = std::str::from_utf8(&data[*pos..*pos + end]).ok()?;
    *pos += end + 1;
    let (count, subtrees) = line.split_once(' ')?;
    let count: i64 = count.parse().ok()?;
    let subtrees: usize = subtrees.parse().ok()?;
    let mut oid = Oid::ZERO;
    if count >= 0 {
        oid = Oid::from_bytes(data.get(*pos..*pos + 20)?);
        *pos += 20;
    }
    let mut children = Vec::with_capacity(subtrees);
    for _ in 0..subtrees {
        children.push(cache_tree(data, pos)?);
    }
    Some(CacheTree {
        name,
        count,
        oid,
        children,
    })
}

#[derive(Default, Clone)]
pub struct Index {
    pub entries: Vec<IndexEntry>,
    pub tree: Option<CacheTree>,
    /// The index file's own modification time, for entries written in the same second.
    pub written: i64,
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

impl Index {
    /// The repository's index; empty when it has none yet.
    pub fn load(repo: &Repo<'_>) -> Res<Index> {
        let path = join(&repo.git_dir, b"index");
        let Some(data) = repo.fs.read_opt(&path)? else {
            return Ok(Index::default());
        };
        let written = repo.fs.stat(&path, true)?.mtime_ns;
        let mut index = Index::parse(&data)?;
        index.written = written;
        Ok(index)
    }

    pub fn parse(data: &[u8]) -> Res<Index> {
        let bad = || Error::Io(corrupt("the index file is malformed"));
        if data.len() < 12 + 20 || &data[..4] != b"DIRC" {
            return Err(bad());
        }
        let version = be32(&data[4..]);
        if !(2..=4).contains(&version) {
            return Err(Error::Lacks(
                format!("the index file is version {version}"),
                "git for that index version",
            ));
        }
        let count = be32(&data[8..]) as usize;
        let end = data.len() - 20;
        let mut entries: Vec<IndexEntry> = Vec::with_capacity(count);
        let mut pos = 12;
        for _ in 0..count {
            let fixed = data.get(pos..pos + 62).ok_or_else(bad)?;
            let flags = u16::from_be_bytes([fixed[60], fixed[61]]);
            let mut at = pos + 62;
            let mut extended = 0u16;
            if flags & 0x4000 != 0 {
                let more = data.get(at..at + 2).ok_or_else(bad)?;
                extended = u16::from_be_bytes([more[0], more[1]]);
                at += 2;
            }
            let path = if version == 4 {
                let mut c = *data.get(at).ok_or_else(bad)?;
                at += 1;
                let mut strip = usize::from(c & 0x7f);
                while c & 0x80 != 0 {
                    c = *data.get(at).ok_or_else(bad)?;
                    at += 1;
                    strip = ((strip + 1) << 7) | usize::from(c & 0x7f);
                }
                let nul = data[at..end].iter().position(|&b| b == 0).ok_or_else(bad)?;
                let previous: &[u8] = entries.last().map_or(&[], |e| &e.path);
                let keep = previous.len().checked_sub(strip).ok_or_else(bad)?;
                let mut path = previous[..keep].to_vec();
                path.extend_from_slice(&data[at..at + nul]);
                pos = at + nul + 1;
                path
            } else {
                let nul = data[at..end].iter().position(|&b| b == 0).ok_or_else(bad)?;
                let path = data[at..at + nul].to_vec();
                let len = at + nul - pos;
                pos += (len + 8) & !7;
                path
            };
            entries.push(IndexEntry {
                path,
                oid: Oid::from_bytes(&fixed[40..60]),
                mode: be32(&fixed[24..]),
                stage: ((flags >> 12) & 3) as u8,
                size: be32(&fixed[36..]),
                mtime: (be32(&fixed[8..]), be32(&fixed[12..])),
                ctime: (be32(&fixed[0..]), be32(&fixed[4..])),
                dev: be32(&fixed[16..]),
                ino: be32(&fixed[20..]),
                uid: be32(&fixed[28..]),
                gid: be32(&fixed[32..]),
                assume_valid: flags & 0x8000 != 0,
                skip_worktree: extended & 0x4000 != 0,
                intent_to_add: extended & 0x2000 != 0,
                fresh: false,
            });
        }
        let mut tree = None;
        while pos + 8 <= end {
            let name = &data[pos..pos + 4];
            let size = be32(&data[pos + 4..]) as usize;
            if name == b"TREE"
                && let Some(body) = data.get(pos + 8..pos + 8 + size)
            {
                tree = cache_tree(body, &mut 0);
            }
            if name == b"link" || name == b"sdir" {
                return Err(Error::Lacks(
                    "the index is split or sparse".to_string(),
                    "git for split and sparse indexes",
                ));
            }
            pos += 8 + size;
        }
        Ok(Index {
            entries,
            tree,
            written: 0,
        })
    }

    /// Where path is among the entries, or where it would be.
    pub fn find(&self, path: &[u8]) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|e| e.path.as_slice().cmp(path))
    }

    pub fn get(&self, path: &[u8]) -> Option<&IndexEntry> {
        self.find(path).ok().map(|at| &self.entries[at])
    }

    /// The entries under a directory (its path without a trailing slash).
    fn under(&self, dir: &[u8]) -> std::ops::Range<usize> {
        let mut prefix = dir.to_vec();
        prefix.push(b'/');
        let start = self
            .entries
            .partition_point(|e| e.path.as_slice() < prefix.as_slice());
        let len = self.entries[start..]
            .iter()
            .take_while(|e| e.path.starts_with(&prefix))
            .count();
        start..start + len
    }

    fn touched(&mut self, path: &[u8]) {
        if let Some(tree) = &mut self.tree {
            tree.invalidate(path);
        }
    }

    /// Puts an entry in, in place of the one of its path and of whatever a file and a
    /// directory of one name would both need.
    pub fn set(&mut self, entry: IndexEntry) {
        self.touched(&entry.path);
        let under = self.under(&entry.path);
        self.entries.drain(under);
        let mut end = entry.path.len();
        while let Some(slash) = entry.path[..end].iter().rposition(|&b| b == b'/') {
            if let Ok(at) = self.find(&entry.path[..slash]) {
                self.entries.remove(at);
            }
            end = slash;
        }
        match self.find(&entry.path) {
            Ok(at) => self.entries[at] = entry,
            Err(at) => self.entries.insert(at, entry),
        }
    }

    pub fn remove(&mut self, path: &[u8]) -> Option<IndexEntry> {
        let at = self.find(path).ok()?;
        self.touched(path);
        Some(self.entries.remove(at))
    }

    /// The file as git writes it: version 2, or 3 when an entry needs the flags only
    /// that has, with the trees written so far.
    pub fn serialize(&mut self) -> Vec<u8> {
        if let Some(mut tree) = self.tree.take() {
            tree.settle(&self.entries, 0);
            self.tree = Some(tree);
        }
        let extended = |e: &IndexEntry| e.skip_worktree || e.intent_to_add;
        let version: u32 = if self.entries.iter().any(extended) {
            3
        } else {
            2
        };
        let mut out = Vec::with_capacity(self.entries.len() * 96 + 64);
        out.extend_from_slice(b"DIRC");
        out.extend_from_slice(&version.to_be_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        for e in &self.entries {
            let start = out.len();
            for field in [
                e.ctime.0, e.ctime.1, e.mtime.0, e.mtime.1, e.dev, e.ino, e.mode, e.uid, e.gid,
                e.size,
            ] {
                out.extend_from_slice(&field.to_be_bytes());
            }
            out.extend_from_slice(&e.oid.0);
            let mut flags = e.path.len().min(0xfff) as u16 | u16::from(e.stage) << 12;
            if e.assume_valid {
                flags |= 0x8000;
            }
            if extended(e) {
                flags |= 0x4000;
            }
            out.extend_from_slice(&flags.to_be_bytes());
            if extended(e) {
                let more = u16::from(e.skip_worktree) << 14 | u16::from(e.intent_to_add) << 13;
                out.extend_from_slice(&more.to_be_bytes());
            }
            out.extend_from_slice(&e.path);
            let len = out.len() - start;
            out.resize(start + ((len + 8) & !7), 0);
        }
        if let Some(tree) = &self.tree {
            let mut body = Vec::new();
            tree.write(&mut body);
            out.extend_from_slice(b"TREE");
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&body);
        }
        let mut digest = Digest::new(Algo::Sha1);
        digest.update(&out);
        out.extend_from_slice(&digest.finish());
        out
    }
}

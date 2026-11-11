//! The index (gitformat-index(5)), versions 2 to 4.

use super::repo::Repo;
use super::store::{Oid, corrupt, join};
use super::{Error, Res};

pub struct IndexEntry {
    pub path: Vec<u8>,
    pub oid: Oid,
    pub mode: u32,
    pub stage: u8,
    pub size: u32,
    pub mtime: (u32, u32),
    pub assume_valid: bool,
    pub skip_worktree: bool,
    pub intent_to_add: bool,
}

/// The tree a directory of the index was last written as (the TREE extension): valid
/// while count is not negative, and then count is how many entries are under it.
pub struct CacheTree {
    pub name: Vec<u8>,
    pub count: i64,
    pub oid: Oid,
    pub children: Vec<CacheTree>,
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

#[derive(Default)]
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
        let written = repo.fs.stat(&path, true).map_or(0, |m| m.mtime_ns);
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
                assume_valid: flags & 0x8000 != 0,
                skip_worktree: extended & 0x4000 != 0,
                intent_to_add: extended & 0x2000 != 0,
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
}

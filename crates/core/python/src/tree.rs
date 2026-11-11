use std::{cell::RefCell, collections::BTreeMap, io};

use crate::{Files, Kind, Remove, Stat, Write};

pub enum Entry {
    File {
        data: Vec<u8>,
        mode: u32,
        mtime_ns: i64,
    },
    Dir {
        mode: u32,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    pub removed: Vec<Vec<u8>>,
    pub made: Vec<(Vec<u8>, u32)>,
    pub written: Vec<Vec<u8>>,
}

enum Node {
    File {
        data: Vec<u8>,
        mode: u32,
        mtime_ns: i64,
        changed: bool,
    },
    Dir {
        mode: u32,
    },
}

impl Node {
    const fn kind(&self) -> Kind {
        match self {
            Self::File { .. } => Kind::File,
            Self::Dir { .. } => Kind::Dir,
        }
    }
}

pub struct Tree {
    roots: Vec<Vec<u8>>,
    first: RefCell<BTreeMap<Vec<u8>, Kind>>,
    nodes: RefCell<BTreeMap<Vec<u8>, Node>>,
}

fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/",
        Some(cut) => &path[..cut],
    }
}

fn name(path: &[u8]) -> &[u8] {
    let cut = path.iter().rposition(|&byte| byte == b'/');
    &path[cut.map_or(0, |cut| cut + 1)..]
}

fn under(path: &[u8], dir: &[u8]) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/") || dir == b"/")
}

fn missing() -> io::Error {
    io::ErrorKind::NotFound.into()
}

impl Tree {
    pub const fn new(roots: Vec<Vec<u8>>) -> Self {
        Self {
            roots,
            first: RefCell::new(BTreeMap::new()),
            nodes: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn put(&self, path: Vec<u8>, entry: Entry) {
        let node = match entry {
            Entry::File {
                data,
                mode,
                mtime_ns,
            } => Node::File {
                data,
                mode,
                mtime_ns,
                changed: false,
            },
            Entry::Dir { mode } => Node::Dir { mode },
        };
        self.first.borrow_mut().insert(path.clone(), node.kind());
        self.nodes.borrow_mut().insert(path, node);
    }

    pub fn delta(&self) -> Delta {
        let first = self.first.borrow();
        let nodes = self.nodes.borrow();
        let mut delta = Delta::default();
        for (path, kind) in first.iter().rev() {
            if nodes.get(path).map(Node::kind) != Some(*kind) {
                delta.removed.push(path.clone());
            }
        }
        for (path, node) in nodes.iter() {
            match node {
                Node::Dir { mode } if first.get(path) != Some(&Kind::Dir) => {
                    delta.made.push((path.clone(), *mode));
                }
                Node::File { changed: true, .. } => delta.written.push(path.clone()),
                _ => {}
            }
        }
        delta
    }

    pub fn take(&self, path: &[u8]) -> Option<Vec<u8>> {
        match self.nodes.borrow_mut().get_mut(path) {
            Some(Node::File { data, .. }) => Some(std::mem::take(data)),
            _ => None,
        }
    }

    fn within(&self, path: &[u8]) -> io::Result<()> {
        if self.roots.iter().any(|root| under(path, root)) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "outside the directories this shell copied in for Python",
            ))
        }
    }
}

impl Files for Tree {
    fn stat(&self, path: &[u8]) -> io::Result<Stat> {
        self.within(path)?;
        match self.nodes.borrow().get(path) {
            Some(Node::File {
                data,
                mode,
                mtime_ns,
                ..
            }) => Ok(Stat {
                kind: Kind::File,
                size: data.len() as u64,
                mode: *mode,
                mtime_ns: *mtime_ns,
            }),
            Some(Node::Dir { mode }) => Ok(Stat {
                kind: Kind::Dir,
                size: 0,
                mode: *mode,
                mtime_ns: 0,
            }),
            None => Err(missing()),
        }
    }

    fn read(&self, path: &[u8], _limit: u64) -> io::Result<Vec<u8>> {
        self.within(path)?;
        match self.nodes.borrow().get(path) {
            Some(Node::File { data, .. }) => Ok(data.clone()),
            Some(Node::Dir { .. }) => Err(io::ErrorKind::IsADirectory.into()),
            None => Err(missing()),
        }
    }

    fn write(&self, path: &[u8], more: &[u8], write: Write) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        if !matches!(nodes.get(parent(path)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        match (nodes.get_mut(path), write) {
            (Some(Node::Dir { .. }), _) => Err(io::ErrorKind::IsADirectory.into()),
            (Some(Node::File { .. }), Write::New) => Err(io::ErrorKind::AlreadyExists.into()),
            (Some(Node::File { data, changed, .. }), Write::Append) => {
                data.extend_from_slice(more);
                *changed = true;
                Ok(())
            }
            (Some(Node::File { data, changed, .. }), Write::Replace) => {
                *data = more.to_vec();
                *changed = true;
                Ok(())
            }
            (None, _) => {
                let node = Node::File {
                    data: more.to_vec(),
                    mode: 0o644,
                    mtime_ns: 0,
                    changed: true,
                };
                nodes.insert(path.to_vec(), node);
                Ok(())
            }
        }
    }

    fn list(&self, path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        self.within(path)?;
        let nodes = self.nodes.borrow();
        match nodes.get(path) {
            Some(Node::Dir { .. }) => Ok(nodes
                .keys()
                .filter(|other| other.len() > path.len() && parent(other) == path)
                .map(|other| name(other).to_vec())
                .collect()),
            Some(Node::File { .. }) => Err(io::ErrorKind::NotADirectory.into()),
            None => Err(missing()),
        }
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        if nodes.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if !matches!(nodes.get(parent(path)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        nodes.insert(path.to_vec(), Node::Dir { mode });
        Ok(())
    }

    fn remove(&self, path: &[u8], remove: Remove) -> io::Result<()> {
        self.within(path)?;
        let mut nodes = self.nodes.borrow_mut();
        match (nodes.get(path), remove) {
            (None, _) => return Err(missing()),
            (Some(Node::Dir { .. }), Remove::File) => {
                return Err(io::ErrorKind::IsADirectory.into());
            }
            (Some(Node::File { .. }), Remove::Dir) => {
                return Err(io::ErrorKind::NotADirectory.into());
            }
            (Some(Node::Dir { .. }), Remove::Dir) => {
                if nodes
                    .keys()
                    .any(|other| other.len() > path.len() && parent(other) == path)
                {
                    return Err(io::ErrorKind::DirectoryNotEmpty.into());
                }
            }
            (Some(Node::File { .. }), Remove::File) => {}
        }
        nodes.remove(path);
        Ok(())
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        self.within(from)?;
        self.within(to)?;
        let mut nodes = self.nodes.borrow_mut();
        if !nodes.contains_key(from) {
            return Err(missing());
        }
        if !matches!(nodes.get(parent(to)), Some(Node::Dir { .. })) {
            return Err(missing());
        }
        let moved: Vec<Vec<u8>> = nodes
            .keys()
            .filter(|path| {
                path.strip_prefix(from)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
            })
            .cloned()
            .collect();
        for path in moved {
            let Some(mut node) = nodes.remove(&path) else {
                continue;
            };
            if let Node::File { changed, .. } = &mut node {
                *changed = true;
            }
            let mut placed = to.to_vec();
            placed.extend_from_slice(&path[from.len()..]);
            nodes.insert(placed, node);
        }
        Ok(())
    }
}

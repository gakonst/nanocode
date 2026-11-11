use crate::{
    fs::{FileSystem, Kind, Remove, Reply, Stat, Write, normalize},
    machine::Machine,
};
use bytes::{Bytes, BytesMut};
use std::{cell::RefCell, collections::BTreeMap, io, rc::Rc};

const SYMLINK_HOPS: u32 = 40;
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;
const LINK_MODE: u32 = 0o777;

enum Node {
    File {
        data: Bytes,
        mode: u32,
        mtime_ns: i64,
    },
    Dir {
        children: BTreeMap<Vec<u8>, Self>,
        mode: u32,
        mtime_ns: i64,
    },
    Link {
        target: Vec<u8>,
        mtime_ns: i64,
    },
}

impl Node {
    const fn stat(&self) -> Stat {
        match self {
            Self::File {
                data,
                mode,
                mtime_ns,
            } => Stat {
                kind: Kind::File,
                size: data.len() as u64,
                mode: *mode,
                mtime_ns: *mtime_ns,
            },
            Self::Dir { mode, mtime_ns, .. } => Stat {
                kind: Kind::Dir,
                size: 0,
                mode: *mode,
                mtime_ns: *mtime_ns,
            },
            Self::Link { target, mtime_ns } => Stat {
                kind: Kind::Symlink,
                size: target.len() as u64,
                mode: LINK_MODE,
                mtime_ns: *mtime_ns,
            },
        }
    }
}

pub struct MemFs {
    root: RefCell<Node>,
    machine: Rc<dyn Machine>,
}

fn parts(path: &[u8]) -> Vec<&[u8]> {
    path.split(|&byte| byte == b'/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn missing<T>() -> io::Result<T> {
    Err(io::ErrorKind::NotFound.into())
}

impl MemFs {
    pub fn new(machine: Rc<dyn Machine>) -> Self {
        let mtime_ns = stamp(&*machine);
        Self {
            root: RefCell::new(Node::Dir {
                children: BTreeMap::new(),
                mode: DIR_MODE,
                mtime_ns,
            }),
            machine,
        }
    }

    pub fn insert(&self, path: &[u8], data: Bytes) -> io::Result<()> {
        let path = normalize(path);
        let names = parts(&path);
        let Some((name, dirs)) = names.split_last() else {
            return Err(io::ErrorKind::IsADirectory.into());
        };
        let mtime_ns = stamp(&*self.machine);
        let mut root = self.root.borrow_mut();
        let mut node = &mut *root;
        for dir in dirs {
            let Node::Dir { children, .. } = node else {
                return Err(io::ErrorKind::NotADirectory.into());
            };
            node = children.entry(dir.to_vec()).or_insert_with(|| Node::Dir {
                children: BTreeMap::new(),
                mode: DIR_MODE,
                mtime_ns,
            });
        }
        let Node::Dir { children, .. } = node else {
            return Err(io::ErrorKind::NotADirectory.into());
        };
        children.insert(
            name.to_vec(),
            Node::File {
                data,
                mode: FILE_MODE,
                mtime_ns,
            },
        );
        Ok(())
    }

    pub fn get(&self, path: &[u8]) -> io::Result<Bytes> {
        let path = self.resolve(&normalize(path), true)?;
        match find(&self.root.borrow(), &parts(&path)) {
            Some(Node::File { data, .. }) => Ok(data.clone()),
            Some(Node::Dir { .. }) => Err(io::ErrorKind::IsADirectory.into()),
            Some(Node::Link { .. }) | None => missing(),
        }
    }

    fn resolve(&self, path: &[u8], follow: bool) -> io::Result<Vec<u8>> {
        let root = self.root.borrow();
        let mut rest: Vec<Vec<u8>> = parts(path).into_iter().rev().map(<[u8]>::to_vec).collect();
        let mut found: Vec<Vec<u8>> = Vec::new();
        let mut hops = 0;
        while let Some(part) = rest.pop() {
            found.push(part);
            let names: Vec<&[u8]> = found.iter().map(Vec::as_slice).collect();
            let Some(Node::Link { target, .. }) = find(&root, &names) else {
                continue;
            };
            if rest.is_empty() && !follow {
                break;
            }
            hops += 1;
            if hops > SYMLINK_HOPS {
                return Err(io::Error::other("Too many levels of symbolic links"));
            }
            found.pop();
            if target.starts_with(b"/") {
                found.clear();
            }
            for part in target.split(|&byte| byte == b'/').rev() {
                match part {
                    b"" | b"." => {}
                    _ => rest.push(part.to_vec()),
                }
            }
            let mut pending = Vec::new();
            while let Some(part) = rest.pop() {
                if part == b".." {
                    if pending.pop().is_none() {
                        found.pop();
                    }
                } else {
                    pending.push(part);
                }
            }
            rest.extend(pending.into_iter().rev());
        }
        let mut out = Vec::new();
        for part in &found {
            out.push(b'/');
            out.extend_from_slice(part);
        }
        if out.is_empty() {
            out.push(b'/');
        }
        Ok(out)
    }

    fn within<T>(
        &self,
        path: &[u8],
        then: impl FnOnce(&mut BTreeMap<Vec<u8>, Node>, &[u8], i64) -> io::Result<T>,
    ) -> io::Result<T> {
        let path = self.resolve(path, false)?;
        let names = parts(&path);
        let Some((name, dirs)) = names.split_last() else {
            return Err(io::ErrorKind::InvalidInput.into());
        };
        let dir = self.resolve(&join_parts(dirs), true)?;
        let now = stamp(&*self.machine);
        let mut root = self.root.borrow_mut();
        match find_mut(&mut root, &parts(&dir)) {
            Some(Node::Dir {
                children, mtime_ns, ..
            }) => {
                let result = then(children, name, now);
                if result.is_ok() {
                    *mtime_ns = now;
                }
                result
            }
            Some(_) => Err(io::ErrorKind::NotADirectory.into()),
            None => missing(),
        }
    }
}

fn stamp(machine: &dyn Machine) -> i64 {
    i64::try_from(machine.now().as_nanos()).unwrap_or(i64::MAX)
}

fn join_parts(names: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for name in names {
        out.push(b'/');
        out.extend_from_slice(name);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

fn find<'a>(root: &'a Node, names: &[&[u8]]) -> Option<&'a Node> {
    let mut node = root;
    for name in names {
        let Node::Dir { children, .. } = node else {
            return None;
        };
        node = children.get(*name)?;
    }
    Some(node)
}

fn find_mut<'a>(root: &'a mut Node, names: &[&[u8]]) -> Option<&'a mut Node> {
    let mut node = root;
    for name in names {
        let Node::Dir { children, .. } = node else {
            return None;
        };
        node = children.get_mut(*name)?;
    }
    Some(node)
}

impl FileSystem for MemFs {
    fn stat(&self, path: &[u8], follow: bool) -> Reply<Stat> {
        Reply::Now(self.resolve(path, follow).and_then(|path| {
            find(&self.root.borrow(), &parts(&path)).map_or_else(missing, |node| Ok(node.stat()))
        }))
    }

    fn read(&self, path: &[u8], limit: u64) -> Reply<Bytes> {
        Reply::Now(self.get(path).and_then(|data| {
            if data.len() as u64 > limit {
                Err(io::ErrorKind::FileTooLarge.into())
            } else {
                Ok(data)
            }
        }))
    }

    fn write(&self, path: &[u8], data: &[u8], how: Write) -> Reply<()> {
        Reply::Now(self.resolve(path, true).and_then(|path| {
            self.within(&path, |children, name, now| {
                match children.get_mut(name) {
                    Some(Node::File {
                        data: held,
                        mtime_ns,
                        ..
                    }) => {
                        match how {
                            Write::New => return Err(io::ErrorKind::AlreadyExists.into()),
                            Write::Replace => *held = Bytes::copy_from_slice(data),
                            Write::Append => {
                                let mut grown = std::mem::take(held)
                                    .try_into_mut()
                                    .unwrap_or_else(|shared| BytesMut::from(&shared[..]));
                                grown.extend_from_slice(data);
                                *held = grown.freeze();
                            }
                        }
                        *mtime_ns = now;
                    }
                    Some(Node::Dir { .. }) => return Err(io::ErrorKind::IsADirectory.into()),
                    Some(Node::Link { .. }) => return missing(),
                    None => {
                        children.insert(
                            name.to_vec(),
                            Node::File {
                                data: Bytes::copy_from_slice(data),
                                mode: FILE_MODE,
                                mtime_ns: now,
                            },
                        );
                    }
                }
                Ok(())
            })
        }))
    }

    fn list(&self, path: &[u8]) -> Reply<Vec<Vec<u8>>> {
        Reply::Now(self.resolve(path, true).and_then(|path| {
            match find(&self.root.borrow(), &parts(&path)) {
                Some(Node::Dir { children, .. }) => Ok(children.keys().cloned().collect()),
                Some(_) => Err(io::ErrorKind::NotADirectory.into()),
                None => missing(),
            }
        }))
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> Reply<()> {
        Reply::Now(self.within(path, |children, name, now| {
            if children.contains_key(name) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            children.insert(
                name.to_vec(),
                Node::Dir {
                    children: BTreeMap::new(),
                    mode,
                    mtime_ns: now,
                },
            );
            Ok(())
        }))
    }

    fn remove(&self, path: &[u8], what: Remove) -> Reply<()> {
        Reply::Now(self.within(path, |children, name, _now| {
            match (children.get(name), what) {
                (None, _) => return missing(),
                (Some(Node::Dir { .. }), Remove::File) => {
                    return Err(io::ErrorKind::IsADirectory.into());
                }
                (Some(Node::Dir { children, .. }), Remove::Dir) if !children.is_empty() => {
                    return Err(io::ErrorKind::DirectoryNotEmpty.into());
                }
                (Some(Node::File { .. } | Node::Link { .. }), Remove::Dir) => {
                    return Err(io::ErrorKind::NotADirectory.into());
                }
                _ => {}
            }
            children.remove(name);
            Ok(())
        }))
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> Reply<()> {
        if to.len() > from.len() && to.starts_with(from) && to[from.len()] == b'/' {
            return Reply::Now(Err(io::ErrorKind::InvalidInput.into()));
        }
        let moved = self.within(from, |children, name, _now| {
            children.remove(name).map_or_else(missing, Ok)
        });
        let moved = match moved {
            Ok(node) => node,
            Err(error) => return Reply::Now(Err(error)),
        };
        let mut moved = Some(moved);
        let placed = self.within(to, |children, name, _now| {
            match (children.get(name), moved.as_ref()) {
                (Some(Node::Dir { children, .. }), Some(Node::Dir { .. }))
                    if !children.is_empty() =>
                {
                    return Err(io::ErrorKind::DirectoryNotEmpty.into());
                }
                (Some(Node::Dir { .. }), Some(Node::File { .. } | Node::Link { .. })) => {
                    return Err(io::ErrorKind::IsADirectory.into());
                }
                (Some(Node::File { .. } | Node::Link { .. }), Some(Node::Dir { .. })) => {
                    return Err(io::ErrorKind::NotADirectory.into());
                }
                _ => {}
            }
            if let Some(node) = moved.take() {
                children.insert(name.to_vec(), node);
            }
            Ok(())
        });
        if let (Err(_), Some(node)) = (&placed, moved) {
            let back = self.within(from, |children, name, _now| {
                children.insert(name.to_vec(), node);
                Ok(())
            });
            if let Err(error) = back {
                return Reply::Now(Err(error));
            }
        }
        Reply::Now(placed)
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> Reply<()> {
        Reply::Now(self.within(link, |children, name, now| {
            if children.contains_key(name) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            children.insert(
                name.to_vec(),
                Node::Link {
                    target: target.to_vec(),
                    mtime_ns: now,
                },
            );
            Ok(())
        }))
    }

    fn read_link(&self, path: &[u8]) -> Reply<Vec<u8>> {
        Reply::Now(self.resolve(path, false).and_then(|path| {
            match find(&self.root.borrow(), &parts(&path)) {
                Some(Node::Link { target, .. }) => Ok(target.clone()),
                Some(_) => Err(io::ErrorKind::InvalidInput.into()),
                None => missing(),
            }
        }))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> Reply<()> {
        Reply::Now(self.resolve(path, true).and_then(|path| {
            match find_mut(&mut self.root.borrow_mut(), &parts(&path)) {
                Some(Node::File { mode: held, .. } | Node::Dir { mode: held, .. }) => {
                    *held = mode;
                    Ok(())
                }
                Some(Node::Link { .. }) | None => missing(),
            }
        }))
    }

    fn set_mtime(&self, path: &[u8], mtime_ns: i64) -> Reply<()> {
        Reply::Now(self.resolve(path, true).and_then(|path| {
            match find_mut(&mut self.root.borrow_mut(), &parts(&path)) {
                Some(
                    Node::File { mtime_ns: held, .. }
                    | Node::Dir { mtime_ns: held, .. }
                    | Node::Link { mtime_ns: held, .. },
                ) => {
                    *held = mtime_ns;
                    Ok(())
                }
                None => missing(),
            }
        }))
    }
}

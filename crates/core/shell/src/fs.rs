use nanocode_bytes::Bytes;
use std::{future::Future, io, pin::Pin};

pub enum Reply<T> {
    Now(io::Result<T>),
    Later(Pin<Box<dyn Future<Output = io::Result<T>>>>),
}

impl<T: 'static> Reply<T> {
    pub async fn get(self) -> io::Result<T> {
        match self {
            Self::Now(result) => result,
            Self::Later(pending) => pending.await,
        }
    }

    pub fn map<U: 'static>(
        self,
        then: impl FnOnce(io::Result<T>) -> io::Result<U> + 'static,
    ) -> Reply<U> {
        match self {
            Self::Now(result) => Reply::Now(then(result)),
            Self::Later(pending) => Reply::Later(Box::pin(async move { then(pending.await) })),
        }
    }

    pub fn boxed(self) -> Pin<Box<dyn Future<Output = io::Result<T>>>> {
        match self {
            Self::Now(result) => Box::pin(std::future::ready(result)),
            Self::Later(pending) => pending,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    pub mode: u32,
    pub mtime_ns: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Replace,
    Append,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remove {
    File,
    Dir,
}

pub trait FileSystem {
    fn stat(&self, path: &[u8], follow: bool) -> Reply<Stat>;
    fn read(&self, path: &[u8], limit: u64) -> Reply<Bytes>;
    fn write(&self, path: &[u8], data: &[u8], how: Write) -> Reply<()>;
    fn list(&self, path: &[u8]) -> Reply<Vec<Vec<u8>>>;
    fn mkdir(&self, path: &[u8], mode: u32) -> Reply<()>;
    fn remove(&self, path: &[u8], what: Remove) -> Reply<()>;
    fn rename(&self, from: &[u8], to: &[u8]) -> Reply<()>;

    fn symlink(&self, target: &[u8], link: &[u8]) -> Reply<()> {
        let _ = (target, link);
        Reply::Now(Err(io::ErrorKind::Unsupported.into()))
    }

    fn read_link(&self, path: &[u8]) -> Reply<Vec<u8>> {
        let _ = path;
        Reply::Now(Err(io::ErrorKind::Unsupported.into()))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> Reply<()> {
        let _ = (path, mode);
        Reply::Now(Err(io::ErrorKind::Unsupported.into()))
    }

    fn set_mtime(&self, path: &[u8], mtime_ns: i64) -> Reply<()> {
        let _ = (path, mtime_ns);
        Reply::Now(Err(io::ErrorKind::Unsupported.into()))
    }
}

pub fn normalize(path: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(path.len() + 1);
    for part in path.split(|&byte| byte == b'/') {
        match part {
            b"" | b"." => {}
            b".." => {
                let cut = out.iter().rposition(|&byte| byte == b'/').unwrap_or(0);
                out.truncate(cut);
            }
            _ => {
                out.push(b'/');
                out.extend_from_slice(part);
            }
        }
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

pub fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&byte| byte == b'/') {
        Some(0) | None => b"/",
        Some(cut) => &path[..cut],
    }
}

pub fn join(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = Vec::with_capacity(dir.len() + 1 + name.len());
    path.extend_from_slice(dir);
    if !dir.ends_with(b"/") {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}

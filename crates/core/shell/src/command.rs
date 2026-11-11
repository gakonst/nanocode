use crate::{
    file::Capture,
    fs::{FileSystem, Remove, Reply, Stat, Write, normalize},
    host::Vfs,
    shell::Limits,
};
use bytes::Bytes;
use psh_engine::{ControlHandle, HostFile};
use std::{
    any::Any,
    future::{Future, poll_fn},
    io,
    pin::Pin,
    rc::Rc,
    sync::Arc,
};

pub trait Commands {
    fn has(&self, name: &[u8]) -> bool;
    fn run(&self, call: Call) -> Pin<Box<dyn Future<Output = i32>>>;
}

pub struct NoCommands;

impl Commands for NoCommands {
    fn has(&self, _name: &[u8]) -> bool {
        false
    }

    fn run(&self, _call: Call) -> Pin<Box<dyn Future<Output = i32>>> {
        Box::pin(std::future::ready(127))
    }
}

pub struct Call {
    pub argv: Vec<Vec<u8>>,
    pub env: Vec<(Vec<u8>, Vec<u8>)>,
    pub cwd: Vec<u8>,
    pub stdin: Stream,
    pub stdout: Stream,
    pub stderr: Stream,
    pub shell: Session,
}

#[derive(Clone)]
pub struct Session(pub(crate) Rc<Vfs>);

impl Session {
    pub fn files(&self) -> Rc<dyn FileSystem> {
        Rc::new(Tracked(self.0.clone()))
    }

    pub fn limits(&self) -> Limits {
        self.0.limits
    }

    pub fn lacks(&self, name: &[u8]) {
        self.0.lacks(name);
    }

    pub fn stop(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let control = self.0.control.get().cloned();
        Arc::new(move || control.as_ref().is_some_and(ControlHandle::cancelled))
    }
}

struct Tracked(Rc<Vfs>);

impl FileSystem for Tracked {
    fn stat(&self, path: &[u8], follow: bool) -> Reply<Stat> {
        self.0.stat(&normalize(path), follow)
    }

    fn read(&self, path: &[u8], _limit: u64) -> Reply<Bytes> {
        self.0.read(&normalize(path))
    }

    fn write(&self, path: &[u8], data: &[u8], how: Write) -> Reply<()> {
        self.0.write(&normalize(path), data, how)
    }

    fn list(&self, path: &[u8]) -> Reply<Vec<Vec<u8>>> {
        self.0.list(&normalize(path))
    }

    fn mkdir(&self, path: &[u8], mode: u32) -> Reply<()> {
        self.0.mkdir(normalize(path), mode)
    }

    fn remove(&self, path: &[u8], what: Remove) -> Reply<()> {
        self.0.remove(normalize(path), what)
    }

    fn rename(&self, from: &[u8], to: &[u8]) -> Reply<()> {
        self.0.rename(normalize(from), normalize(to))
    }

    fn symlink(&self, target: &[u8], link: &[u8]) -> Reply<()> {
        self.0.symlink(target, normalize(link))
    }

    fn read_link(&self, path: &[u8]) -> Reply<Vec<u8>> {
        self.0.read_link(&normalize(path))
    }

    fn set_mode(&self, path: &[u8], mode: u32) -> Reply<()> {
        self.0.set_mode(&normalize(path), mode)
    }

    fn set_mtime(&self, path: &[u8], mtime_ns: i64) -> Reply<()> {
        self.0.set_mtime(&normalize(path), mtime_ns)
    }
}

#[derive(Clone)]
pub struct Stream(pub(crate) Rc<dyn HostFile>);

impl Stream {
    pub fn captured(&self) -> bool {
        let file: &dyn Any = self.0.as_ref();
        file.is::<Capture>()
    }

    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        poll_fn(|cx| self.0.poll_read(cx, buf)).await
    }

    pub async fn read_to_end(&self, out: &mut Vec<u8>) -> io::Result<()> {
        let mut chunk = vec![0; crate::file::PIPE_BYTES];
        loop {
            let read = self.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            out.extend_from_slice(&chunk[..read]);
        }
    }

    pub async fn write_all(&self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let wrote = poll_fn(|cx| self.0.poll_write(cx, data)).await?;
            if wrote == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            data = &data[wrote..];
        }
        Ok(())
    }
}

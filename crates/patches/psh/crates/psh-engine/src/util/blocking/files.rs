//! A blocking utility's file system as a value it can keep (nanocode's addition, for
//! `python3`): what [`SyncCtx`] sees, owned, so that an interpreter the body starts can
//! call back into it, and with the operations that change files. Paths are the shell's,
//! absolute (see [`SyncCtx::path`]).
//!
//! The backends are [`SyncCtx`]'s: `std::fs` on the OS paths on a worker thread, Host
//! futures polled once inline.

use std::io::{self, Read, Write};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{Io, SyncCtx, native_read_dir, native_stat, poll_once, would_block};
use crate::host::{DirEntry, Host, HostFuture, Metadata, NativePathFn, OpenOptions};

/// [`SyncCtx::files`].
#[derive(Clone)]
pub struct SyncFiles {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    Native(NativePathFn),
    Host(Rc<dyn Host>),
}

impl SyncCtx {
    /// The file system this utility runs on, to keep past the borrow of the context.
    pub fn files(&self) -> SyncFiles {
        SyncFiles {
            backend: match &self.io {
                Io::Thread { native, .. } => Backend::Native(*native),
                Io::Inline { host, .. } => Backend::Host(host.clone()),
            },
        }
    }
}

fn block_on_once<T>(mut fut: HostFuture<io::Result<T>>) -> io::Result<T> {
    poll_once(|cx| fut.as_mut().poll(cx)).unwrap_or_else(|| Err(would_block()))
}

impl SyncCtx {
    /// Whether the stop was requested ([`SyncCtx::cancelled`]), as a value another
    /// thread can keep and ask: the thread that keeps `python3`'s interpreter does.
    pub fn canceller(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        match &self.io {
            Io::Thread { pipe, .. } => {
                let pipe = pipe.clone();
                Arc::new(move || pipe.cancelled.load(Ordering::SeqCst))
            }
            Io::Inline { .. } => Arc::new(|| false),
        }
    }
}

fn too_large() -> io::Error {
    io::ErrorKind::FileTooLarge.into()
}

impl SyncFiles {
    /// The process's own file system through `native`: what [`SyncCtx::files`] gives on
    /// a worker thread, for a thread the caller keeps.
    pub fn on(native: NativePathFn) -> SyncFiles {
        SyncFiles {
            backend: Backend::Native(native),
        }
    }

    /// The OS path function, when these are the process's own files.
    pub fn native(&self) -> Option<NativePathFn> {
        match &self.backend {
            Backend::Native(native) => Some(*native),
            Backend::Host(_) => None,
        }
    }

    /// `stat` (`follow`) or `lstat`.
    pub fn stat(&self, path: &[u8], follow: bool) -> io::Result<Metadata> {
        match &self.backend {
            Backend::Native(native) => native_stat(&native(path)?, follow),
            Backend::Host(host) => block_on_once(host.stat(path, follow)),
        }
    }

    /// The entries of a directory, without `.` and `..`, in no particular order.
    pub fn read_dir(&self, path: &[u8]) -> io::Result<Vec<DirEntry>> {
        match &self.backend {
            Backend::Native(native) => native_read_dir(&native(path)?),
            Backend::Host(host) => block_on_once(host.read_dir(path)),
        }
    }

    /// A file's bytes; one of more than `limit` bytes is `FileTooLarge`.
    pub fn read(&self, path: &[u8], limit: u64) -> io::Result<Vec<u8>> {
        let mut data = Vec::new();
        match &self.backend {
            Backend::Native(native) => {
                let file = std::fs::File::open(native(path)?)?;
                file.take(limit.saturating_add(1)).read_to_end(&mut data)?;
            }
            Backend::Host(host) => {
                let file = block_on_once(host.open(path, OpenOptions::read()))?;
                let mut buf = vec![0u8; super::BUFFER];
                loop {
                    let n = poll_once(|cx| file.poll_read(cx, &mut buf))
                        .unwrap_or_else(|| Err(would_block()))?;
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if data.len() as u64 > limit {
                        break;
                    }
                }
            }
        }
        if data.len() as u64 > limit {
            return Err(too_large());
        }
        Ok(data)
    }

    /// Writes `data` to a file opened with `opts` (create, truncate, append or
    /// `create_new`, as the caller chose).
    pub fn write(&self, path: &[u8], data: &[u8], opts: OpenOptions) -> io::Result<()> {
        match &self.backend {
            Backend::Native(native) => {
                let mut open = std::fs::OpenOptions::new();
                open.write(opts.write)
                    .append(opts.append)
                    .create(opts.create)
                    .truncate(opts.truncate)
                    .create_new(opts.create_new);
                #[cfg(unix)]
                if let Some(mode) = opts.mode {
                    use std::os::unix::fs::OpenOptionsExt;
                    open.mode(mode);
                }
                open.open(native(path)?)?.write_all(data)
            }
            Backend::Host(host) => {
                let file = block_on_once(host.open(path, opts))?;
                let mut rest = data;
                while !rest.is_empty() {
                    let n = poll_once(|cx| file.poll_write(cx, rest))
                        .unwrap_or_else(|| Err(would_block()))?;
                    if n == 0 {
                        return Err(io::ErrorKind::WriteZero.into());
                    }
                    rest = &rest[n..];
                }
                Ok(())
            }
        }
    }

    /// Creates one directory with the permission bits `mode`.
    pub fn mkdir(&self, path: &[u8], mode: u32) -> io::Result<()> {
        match &self.backend {
            Backend::Native(native) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new().mode(mode).create(native(path)?)
                }
                #[cfg(not(unix))]
                {
                    let _ = mode;
                    std::fs::create_dir(native(path)?)
                }
            }
            Backend::Host(host) => block_on_once(host.mkdir(path, mode)),
        }
    }

    /// Removes a name that is not a directory.
    pub fn remove_file(&self, path: &[u8]) -> io::Result<()> {
        match &self.backend {
            Backend::Native(native) => std::fs::remove_file(native(path)?),
            Backend::Host(host) => block_on_once(host.remove_file(path)),
        }
    }

    /// Removes an empty directory.
    pub fn remove_dir(&self, path: &[u8]) -> io::Result<()> {
        match &self.backend {
            Backend::Native(native) => std::fs::remove_dir(native(path)?),
            Backend::Host(host) => block_on_once(host.remove_dir(path)),
        }
    }

    /// `rename(2)`.
    pub fn rename(&self, from: &[u8], to: &[u8]) -> io::Result<()> {
        match &self.backend {
            Backend::Native(native) => std::fs::rename(native(from)?, native(to)?),
            Backend::Host(host) => block_on_once(host.rename(from, to)),
        }
    }
}

//! Open files of the Unix host: regular files, devices, pipes, FIFOs, sockets and
//! terminals as pollable [`HostFile`]s that never block the engine thread on something
//! another part of the engine has to do first (see [`super::ready`]).

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::task::{Context, Poll};

use psh_engine::HostFile;

use super::ready;

/// An open descriptor of the host.
pub struct UnixFile {
    file: File,
    /// A pipe, FIFO, socket or character device: I/O on it may wait. Regular files,
    /// directories and block devices never wait for another process.
    stream: bool,
    /// `O_NONBLOCK` is set: a private pipe end of our own, never shared.
    nonblocking: bool,
    terminal: bool,
    write_waits: bool,
    #[cfg(target_os = "macos")]
    pipe: bool,
}

impl UnixFile {
    /// Wraps an open descriptor, shared or not; its flags are left alone.
    pub fn new(fd: impl Into<OwnedFd>) -> UnixFile {
        use std::os::unix::fs::FileTypeExt;
        let file = File::from(fd.into());
        let meta = file.metadata().ok();
        let kind = meta.as_ref().map(|m| m.file_type());
        let stream = kind.is_none_or(|ft| !(ft.is_file() || ft.is_dir() || ft.is_block_device()));
        let terminal = stream && {
            use std::io::IsTerminal;
            file.is_terminal()
        };
        let device = kind.is_some_and(|ft| ft.is_char_device());
        UnixFile {
            file,
            stream,
            nonblocking: false,
            terminal,
            write_waits: stream && (terminal || !device),
            #[cfg(target_os = "macos")]
            pipe: meta.as_ref().is_some_and(|m| {
                use std::os::unix::fs::MetadataExt;
                m.file_type().is_fifo() && m.dev() == 0
            }),
        }
    }

    /// A pipe end only we hold: made non-blocking.
    pub(crate) fn private(fd: OwnedFd) -> io::Result<UnixFile> {
        ready::set_nonblocking(fd.as_raw_fd())?;
        Ok(UnixFile {
            file: File::from(fd),
            stream: true,
            nonblocking: true,
            terminal: false,
            write_waits: true,
            #[cfg(target_os = "macos")]
            pipe: true,
        })
    }

    /// A duplicate of the process's descriptor `n` (0, 1, 2 for the shell's stdio).
    pub fn stdio(n: RawFd) -> io::Result<UnixFile> {
        // SAFETY: fcntl(F_DUPFD_CLOEXEC) only reads the descriptor table; a failure
        // (n not open) is reported, a success returns a new descriptor we own.
        let fd = unsafe { libc::fcntl(n, libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by fcntl and is owned by nobody else.
        let owned = unsafe { <OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        Ok(UnixFile::new(owned))
    }

    pub fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    #[cfg(target_os = "macos")]
    fn room(&self) -> usize {
        match self.file.metadata() {
            Ok(m) if self.pipe => ready::PIPE_ROOM
                .saturating_sub(m.len() as usize)
                .max(ready::WRITE_CHUNK),
            _ => ready::WRITE_CHUNK,
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn room(&self) -> usize {
        ready::WRITE_CHUNK
    }

    fn wait(&self, cx: &mut Context<'_>, events: i16) -> Poll<io::Result<usize>> {
        match ready::watch(self.raw_fd(), events, cx.waker()) {
            Ok(()) => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl Drop for UnixFile {
    fn drop(&mut self) {
        if self.stream {
            ready::forget(self.raw_fd());
        }
    }
}

impl HostFile for UnixFile {
    fn kind(&self) -> Option<psh_engine::host::FileKind> {
        use psh_engine::host::FileKind;
        use std::os::unix::fs::FileTypeExt;
        let t = self.file.metadata().ok()?.file_type();
        Some(if t.is_file() {
            FileKind::File
        } else if t.is_dir() {
            FileKind::Dir
        } else if t.is_fifo() {
            FileKind::Fifo
        } else if t.is_socket() {
            FileKind::Socket
        } else if t.is_char_device() {
            FileKind::CharDevice
        } else if t.is_block_device() {
            FileKind::BlockDevice
        } else {
            FileKind::Other
        })
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        loop {
            if self.stream && !self.nonblocking {
                match ready::now(self.raw_fd(), ready::READ) {
                    Ok(true) => {}
                    Ok(false) => return self.wait(cx, ready::READ),
                    Err(e) => return Poll::Ready(Err(e)),
                }
            }
            match (&self.file).read(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return self.wait(cx, ready::READ);
                }
                r => return Poll::Ready(r),
            }
        }
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut buf = buf;
        loop {
            if self.write_waits && !self.nonblocking {
                match ready::now(self.raw_fd(), ready::WRITE) {
                    Ok(true) => {}
                    Ok(false) => return self.wait(cx, ready::WRITE),
                    Err(e) => return Poll::Ready(Err(e)),
                }
                // A blocking write larger than what the pipe has room for would wait.
                buf = &buf[..buf.len().min(self.room())];
            }
            match (&self.file).write(buf) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return self.wait(cx, ready::WRITE);
                }
                r => return Poll::Ready(r),
            }
        }
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }

    fn seek(&self, pos: io::SeekFrom) -> Option<io::Result<u64>> {
        use std::io::Seek;
        (!self.stream).then(|| (&self.file).seek(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;

    #[test]
    fn shared_pipe_reads_are_pending_until_data() {
        let (r, mut w) = io::pipe().unwrap();
        let f = UnixFile::new(OwnedFd::from(r));
        assert!(f.stream && !f.nonblocking);
        let mut cx = Context::from_waker(Waker::noop());
        let mut buf = [0u8; 8];
        assert!(f.poll_read(&mut cx, &mut buf).is_pending());
        w.write_all(b"hi").unwrap();
        assert!(matches!(f.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(2))));
        drop(w);
        assert!(matches!(f.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(0))));
    }

    #[test]
    fn private_end_writes_until_full_then_pends() {
        let (r, w) = io::pipe().unwrap();
        let f = UnixFile::private(OwnedFd::from(w)).unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        let chunk = [b'x'; 4096];
        let mut total = 0;
        loop {
            match f.poll_write(&mut cx, &chunk) {
                Poll::Ready(Ok(n)) => total += n,
                Poll::Pending => break,
                Poll::Ready(Err(e)) => panic!("{e}"),
            }
            assert!(total < 16 << 20, "never filled");
        }
        assert!(total >= 4096);
        drop(r);
        assert!(matches!(
            f.poll_write(&mut cx, &chunk),
            Poll::Ready(Err(e)) if e.kind() == io::ErrorKind::BrokenPipe
        ));
    }

    #[test]
    fn regular_files_are_not_streams() {
        let f = UnixFile::new(OwnedFd::from(File::open("/dev/null").unwrap()));
        assert!(f.stream, "a character device may wait");
        let dir = std::env::temp_dir();
        let path = dir.join(format!("psh-os-file-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let f = UnixFile::new(OwnedFd::from(File::open(&path).unwrap()));
        assert!(!f.stream);
        std::fs::remove_file(&path).unwrap();
    }
}

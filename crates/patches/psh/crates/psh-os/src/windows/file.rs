//! Open files of the Windows host as pollable [`HostFile`]s that never block the engine
//! thread on something another part of the engine has to do first.
//!
//! - Regular files, `NUL` and consoles: synchronous I/O (a disk never waits for another
//!   process; console input is read synchronously, as bash's `read` waits for a line).
//! - Shared pipes (the shell's own stdio, [`Host::pipe`](psh_engine::Host::pipe) ends):
//!   handles others hold too. A thread blocked in `ReadFile` on one would take bytes
//!   meant for a child (`read line; cat`), so reads take only what `PeekNamedPipe` says
//!   is there, and otherwise wait for [`super::poller`]. Writes are synchronous: their
//!   reader is another process (vprocs talk through engine pipes), so a full pipe only
//!   applies backpressure (D1077).
//! - Pump pipes (`ChildFd::Pipe`): a fresh named pipe whose other end only the child
//!   holds. Our end is overlapped and owned by a helper thread that moves bytes between
//!   it and a bounded buffer; dropping our end stops the thread at once (a cancel event,
//!   `CancelIoEx`). Anonymous pipes cannot do overlapped I/O, which is why these are
//!   named (as Rust's std does for `Command` pipes).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

use psh_engine::HostFile;
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Globalization::{
    CP_ACP, CP_UTF8, GetACP, MultiByteToWideChar, WideCharToMultiByte,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES,
    FILE_TYPE_CHAR, FILE_TYPE_PIPE, FILE_WRITE_ATTRIBUTES, GetFileType, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Console::GetConsoleMode;
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    PeekNamedPipe,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};

use super::{owned, poller};

/// What a pump buffers at most, and the size of a pump pipe's buffer.
const PUMP_CAPACITY: usize = 64 * 1024;
/// Pump threads only move bytes.
const PUMP_STACK: usize = 64 << 10;

/// How the output of native programs is decoded where the engine reads it (D1078).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decode {
    /// Bytes as they come. The default.
    #[default]
    Off,
    /// Output that is not valid UTF-8 is taken to be in the ANSI code page (`GetACP`)
    /// and converted to UTF-8. Nothing changes when the code page is UTF-8 already.
    Ansi,
}

/// An open file, pipe end or console of the host.
pub struct WinFile {
    kind: Kind,
}

enum Kind {
    /// Regular files, `NUL`, consoles: synchronous.
    Sync(File),
    /// A pipe others hold too.
    Shared(File),
    /// Our end of a pump pipe, run by a helper thread.
    Pump(Arc<Pump>),
    /// `/dev/zero` (zeros; writes vanish) or `/dev/urandom`, `/dev/random` (the
    /// system's random bytes), which Windows has no files for (D1075).
    Dev(Dev),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dev {
    Zero,
    Random,
    /// A directory opened for reading: reads fail with "Is a directory".
    Dir,
}

impl WinFile {
    /// Wraps an open handle, looking at what it is.
    pub fn new(handle: impl Into<OwnedHandle>) -> WinFile {
        let file = File::from(handle.into());
        // SAFETY: a valid open handle.
        let kind = match unsafe { GetFileType(file.as_raw_handle() as HANDLE) } {
            FILE_TYPE_PIPE => Kind::Shared(file),
            _ => Kind::Sync(file),
        };
        WinFile { kind }
    }

    /// A duplicate of the process's standard handle `n` (0, 1, 2).
    pub fn stdio(n: u32) -> io::Result<WinFile> {
        use std::os::windows::io::AsHandle;
        let h = match n {
            0 => io::stdin().as_handle().try_clone_to_owned()?,
            1 => io::stdout().as_handle().try_clone_to_owned()?,
            _ => io::stderr().as_handle().try_clone_to_owned()?,
        };
        Ok(WinFile::new(h))
    }

    /// One of the emulated devices.
    pub(crate) fn dev(d: Dev) -> WinFile {
        WinFile { kind: Kind::Dev(d) }
    }

    /// The handle to give a child (duplicated by the spawner), or `None` for a pump
    /// end, which only its thread holds, and for an emulated device.
    pub(crate) fn raw_handle(&self) -> Option<RawHandle> {
        match &self.kind {
            Kind::Sync(f) | Kind::Shared(f) => Some(f.as_raw_handle()),
            Kind::Pump(_) | Kind::Dev(_) => None,
        }
    }
}

impl Drop for WinFile {
    fn drop(&mut self) {
        match &self.kind {
            Kind::Shared(f) => poller::forget(f.as_raw_handle() as HANDLE),
            Kind::Pump(p) => p.close(),
            Kind::Sync(_) | Kind::Dev(_) => {}
        }
    }
}

fn is_broken(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error().map(|c| c as u32),
        Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA)
    )
}

impl HostFile for WinFile {
    fn kind(&self) -> Option<psh_engine::host::FileKind> {
        use psh_engine::host::FileKind;
        Some(match &self.kind {
            Kind::Sync(f) => match f.metadata() {
                Ok(m) if m.is_file() => FileKind::File,
                Ok(m) if m.is_dir() => FileKind::Dir,
                _ => FileKind::CharDevice,
            },
            Kind::Shared(_) | Kind::Pump(_) => FileKind::Fifo,
            Kind::Dev(Dev::Dir) => FileKind::Dir,
            Kind::Dev(_) => FileKind::CharDevice,
        })
    }

    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        match &self.kind {
            Kind::Sync(f) => loop {
                match (&*f).read(buf) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    // A console or pipe whose writer is gone: end of file.
                    Err(e) if is_broken(&e) => return Poll::Ready(Ok(0)),
                    r => return Poll::Ready(r),
                }
            },
            Kind::Shared(f) => {
                let h = f.as_raw_handle() as HANDLE;
                match available(h) {
                    Ok(0) => {
                        poller::watch(h, cx.waker());
                        Poll::Pending
                    }
                    Ok(n) => {
                        let n = n.min(buf.len());
                        match (&*f).read(&mut buf[..n]) {
                            Err(e) if is_broken(&e) => Poll::Ready(Ok(0)),
                            r => Poll::Ready(r),
                        }
                    }
                    // The writers are gone: end of file once the data was read.
                    Err(e) if is_broken(&e) => Poll::Ready(Ok(0)),
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Kind::Pump(p) => p.poll_read(cx, buf),
            Kind::Dev(Dev::Zero) => {
                buf.fill(0);
                Poll::Ready(Ok(buf.len()))
            }
            Kind::Dev(Dev::Random) => Poll::Ready(random_bytes(buf).map(|()| buf.len())),
            Kind::Dev(Dev::Dir) => Poll::Ready(Err(io::ErrorKind::IsADirectory.into())),
        }
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &self.kind {
            Kind::Sync(f) | Kind::Shared(f) => loop {
                match (&*f).write(buf) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if is_broken(&e) => {
                        return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
                    }
                    r => return Poll::Ready(r),
                }
            },
            Kind::Pump(p) => p.poll_write(cx, buf),
            Kind::Dev(Dev::Dir) => Poll::Ready(Err(io::ErrorKind::IsADirectory.into())),
            Kind::Dev(_) => Poll::Ready(Ok(buf.len())),
        }
    }

    fn seek(&self, pos: io::SeekFrom) -> Option<io::Result<u64>> {
        use std::io::Seek;
        match &self.kind {
            Kind::Sync(f) => Some((&*f).seek(pos)),
            _ => None,
        }
    }

    fn is_terminal(&self) -> bool {
        match &self.kind {
            Kind::Sync(f) => {
                let h = f.as_raw_handle() as HANDLE;
                let mut mode = 0;
                // SAFETY: a valid handle and a writable mode.
                unsafe { GetFileType(h) == FILE_TYPE_CHAR && GetConsoleMode(h, &raw mut mode) != 0 }
            }
            _ => false,
        }
    }
}

/// The system's cryptographic random bytes.
fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    for chunk in buf.chunks_mut(u32::MAX as usize) {
        // SAFETY: `chunk` is writable for its length; no algorithm handle with the
        // system-preferred flag.
        let st = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if st < 0 {
            return Err(io::Error::other(format!("BCryptGenRandom: {st:#x}")));
        }
    }
    Ok(())
}

/// Bytes waiting in a pipe; `Err` with a broken-pipe code when its writers are gone and
/// it is empty.
fn available(h: HANDLE) -> io::Result<usize> {
    let mut avail = 0u32;
    // SAFETY: a valid pipe handle; only the count is asked for.
    let ok = unsafe {
        PeekNamedPipe(
            h,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &raw mut avail,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(avail as usize)
}

/// Whether a shared pipe has data or hung up, for the poller.
pub(crate) fn readable_now(h: HANDLE) -> bool {
    !matches!(available(h), Ok(0))
}

// --- pump pipes

/// The engine's end of a pump pipe: a buffer between the engine and the thread that
/// owns the OS handle.
pub(crate) struct Pump {
    state: Mutex<PumpState>,
    /// The thread waits here for room (reader) or data (writer).
    changed: Condvar,
    /// Set when the engine's end is dropped; the thread waits on it with its I/O.
    cancel: OwnedHandle,
    /// The child's output, read by the engine (else the engine writes).
    reading: bool,
}

struct PumpState {
    buf: VecDeque<u8>,
    /// Reader: the child's end closed. Writer: the engine's end was dropped and the
    /// thread should finish what is buffered and close.
    eof: bool,
    /// Reader: a read failed. Writer: the child's end is gone (broken pipe).
    error: Option<io::ErrorKind>,
    /// The engine's end was dropped.
    closed: bool,
    waker: Option<Waker>,
}

impl Pump {
    fn poll_read(&self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut st = lock(&self.state);
        if !st.buf.is_empty() {
            let n = buf.len().min(st.buf.len());
            for (d, s) in buf.iter_mut().zip(st.buf.drain(..n)) {
                *d = s;
            }
            self.changed.notify_all();
            return Poll::Ready(Ok(n));
        }
        if let Some(k) = st.error.take() {
            return Poll::Ready(Err(k.into()));
        }
        if st.eof {
            return Poll::Ready(Ok(0));
        }
        set_waker(&mut st.waker, cx.waker());
        Poll::Pending
    }

    fn poll_write(&self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let mut st = lock(&self.state);
        if st.error.is_some() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let room = PUMP_CAPACITY.saturating_sub(st.buf.len());
        if room == 0 {
            set_waker(&mut st.waker, cx.waker());
            return Poll::Pending;
        }
        let n = room.min(buf.len());
        st.buf.extend(&buf[..n]);
        self.changed.notify_all();
        Poll::Ready(Ok(n))
    }

    /// The engine's end is dropped. A reader stops at once (the child then gets a
    /// broken pipe). A writer hands over everything it buffered, then closes, so the
    /// child sees end of file after the last byte: it is never cancelled, since a chunk
    /// may be in flight when the buffer is empty (cancelling it lost data). A child that
    /// stops reading ends it when the child ends (broken pipe).
    fn close(&self) {
        let mut st = lock(&self.state);
        st.closed = true;
        st.eof = true;
        st.waker = None;
        drop(st);
        self.changed.notify_all();
        if self.reading {
            // SAFETY: our event handle.
            unsafe { SetEvent(self.cancel.as_raw_handle() as HANDLE) };
        }
    }
}

fn set_waker(slot: &mut Option<Waker>, w: &Waker) {
    if !slot.as_ref().is_some_and(|old| old.will_wake(w)) {
        *slot = Some(w.clone());
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static PIPE_SEQ: AtomicUsize = AtomicUsize::new(0);

/// A new pump pipe: (the engine's end, the child's end). The child's end is
/// inheritable; the engine's end is a [`WinFile`] whose thread owns the OS handle.
/// `input`: the child reads from it.
pub(crate) fn pump_pipe(input: bool, decode: Decode) -> io::Result<(WinFile, OwnedHandle)> {
    let (ours, theirs) = named_pipe(input)?;
    // SAFETY: anonymous manual-reset event, initially unset.
    let cancel = owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })?;
    let pump = Arc::new(Pump {
        state: Mutex::new(PumpState {
            buf: VecDeque::new(),
            eof: false,
            error: None,
            closed: false,
            waker: None,
        }),
        changed: Condvar::new(),
        cancel,
        reading: !input,
    });
    let p = pump.clone();
    let decoder = (decode == Decode::Ansi && !input)
        .then(Decoder::new)
        .flatten();
    std::thread::Builder::new()
        .name("psh-pump".into())
        .stack_size(PUMP_STACK)
        .spawn(move || {
            if p.reading {
                read_loop(&p, ours, decoder);
            } else {
                write_loop(&p, ours);
            }
        })?;
    Ok((
        WinFile {
            kind: Kind::Pump(pump),
        },
        theirs,
    ))
}

/// A one-instance named pipe with a random name: our overlapped server end and the
/// child's synchronous, inheritable client end.
fn named_pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut tries = 0;
    loop {
        let name = format!(
            r"\\.\pipe\psh-{}-{}-{:x}",
            std::process::id(),
            PIPE_SEQ.fetch_add(1, Ordering::Relaxed),
            random()
        );
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let access = if child_reads {
            PIPE_ACCESS_OUTBOUND
        } else {
            PIPE_ACCESS_INBOUND
        };
        // SAFETY: a NUL-terminated name; null security attributes (not inheritable).
        let server = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                access | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                PUMP_CAPACITY as u32,
                PUMP_CAPACITY as u32,
                0,
                std::ptr::null(),
            )
        };
        let server = match owned(server) {
            Ok(s) => s,
            // ERROR_ACCESS_DENIED: the name exists (FIRST_PIPE_INSTANCE); another name.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && tries < 10 => {
                tries += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        // The attribute rights let the child query the pipe (PeekNamedPipe, quotas).
        let client_access = if child_reads {
            GENERIC_READ | FILE_WRITE_ATTRIBUTES
        } else {
            GENERIC_WRITE | FILE_READ_ATTRIBUTES
        };
        // SAFETY: a NUL-terminated name and valid attributes.
        let client = unsafe {
            CreateFileW(
                wide.as_ptr(),
                client_access,
                0,
                &raw const sa,
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        return Ok((server, owned(client)?));
    }
}

/// Bits for pipe names that other processes cannot guess easily.
fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(PIPE_SEQ.load(Ordering::Relaxed));
    h.finish()
}

/// One overlapped operation on `h`, waiting for it or the cancel event: the bytes
/// moved, `Ok(None)` when cancelled.
fn overlapped_io(
    h: HANDLE,
    cancel: HANDLE,
    event: HANDLE,
    write: bool,
    buf: *mut u8,
    len: u32,
) -> io::Result<Option<u32>> {
    // SAFETY: OVERLAPPED is plain data; zeroed is its documented initial state.
    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event;
    let mut done = 0u32;
    // SAFETY: `buf` is valid for `len` bytes and, like `ov`, outlives the operation:
    // this function waits for it to finish or be cancelled before returning.
    let ok = unsafe {
        if write {
            WriteFile(h, buf, len, &raw mut done, &raw mut ov)
        } else {
            ReadFile(h, buf, len, &raw mut done, &raw mut ov)
        }
    };
    if ok == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(e);
        }
        let handles = [event, cancel];
        // SAFETY: two valid handles.
        let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if r != WAIT_OBJECT_0 {
            // Cancelled (or the wait failed): stop the operation and wait until it no
            // longer touches `buf` and `ov`.
            // SAFETY: `ov` is the pending operation's.
            unsafe { CancelIoEx(h, &raw const ov) };
            // SAFETY: as above; TRUE waits for the cancellation to complete.
            unsafe { GetOverlappedResult(h, &raw const ov, &raw mut done, 1) };
            return Ok(None);
        }
    }
    // SAFETY: the operation completed; `done` is writable.
    if unsafe { GetOverlappedResult(h, &raw const ov, &raw mut done, 0) } == 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
            return Ok(None);
        }
        return Err(e);
    }
    Ok(Some(done))
}

fn read_loop(p: &Pump, h: OwnedHandle, mut decoder: Option<Decoder>) {
    // SAFETY: anonymous auto-reset event.
    let Ok(event) = owned(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) }) else {
        finish(p, Some(io::ErrorKind::Other));
        return;
    };
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        {
            let mut st = lock(&p.state);
            while st.buf.len() >= PUMP_CAPACITY && !st.closed {
                st = p.changed.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.closed {
                return;
            }
        }
        let r = overlapped_io(
            h.as_raw_handle() as HANDLE,
            p.cancel.as_raw_handle() as HANDLE,
            event.as_raw_handle() as HANDLE,
            false,
            buf.as_mut_ptr(),
            buf.len() as u32,
        );
        match r {
            Ok(None) => return,
            Ok(Some(0)) => {}
            Ok(Some(n)) => {
                let data = &buf[..n as usize];
                let mut st = lock(&p.state);
                match &mut decoder {
                    Some(d) => d.push(data, &mut st.buf),
                    None => st.buf.extend(data),
                }
                if let Some(w) = st.waker.take() {
                    drop(st);
                    w.wake();
                }
            }
            Err(e)
                if matches!(
                    e.raw_os_error().map(|c| c as u32),
                    Some(ERROR_BROKEN_PIPE | ERROR_HANDLE_EOF | ERROR_NO_DATA)
                ) =>
            {
                if let Some(d) = &mut decoder {
                    d.finish(&mut lock(&p.state).buf);
                }
                finish(p, None);
                return;
            }
            Err(e) => {
                finish(p, Some(e.kind()));
                return;
            }
        }
    }
}

/// The reader reached the end (or failed): wake the engine.
fn finish(p: &Pump, error: Option<io::ErrorKind>) {
    let mut st = lock(&p.state);
    st.eof = true;
    st.error = error;
    let w = st.waker.take();
    drop(st);
    if let Some(w) = w {
        w.wake();
    }
}

fn write_loop(p: &Pump, h: OwnedHandle) {
    // SAFETY: anonymous auto-reset event.
    let Ok(event) = owned(unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) }) else {
        lock(&p.state).error = Some(io::ErrorKind::Other);
        return;
    };
    let mut chunk = Vec::with_capacity(16 * 1024);
    loop {
        {
            let mut st = lock(&p.state);
            while st.buf.is_empty() && !st.eof {
                st = p.changed.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.buf.is_empty() {
                // Closed and drained: dropping the handle gives the child end of file.
                return;
            }
            let n = st.buf.len().min(16 * 1024);
            chunk.clear();
            chunk.extend(st.buf.drain(..n));
            let w = st.waker.take();
            drop(st);
            if let Some(w) = w {
                w.wake();
            }
        }
        let mut off = 0;
        while off < chunk.len() {
            let r = overlapped_io(
                h.as_raw_handle() as HANDLE,
                p.cancel.as_raw_handle() as HANDLE,
                event.as_raw_handle() as HANDLE,
                true,
                chunk[off..].as_mut_ptr(),
                (chunk.len() - off) as u32,
            );
            match r {
                Ok(Some(n)) => off += n as usize,
                Ok(None) => return,
                Err(_) => {
                    // The child closed its end: the engine sees a broken pipe.
                    let mut st = lock(&p.state);
                    st.error = Some(io::ErrorKind::BrokenPipe);
                    st.buf.clear();
                    let w = st.waker.take();
                    drop(st);
                    if let Some(w) = w {
                        w.wake();
                    }
                    return;
                }
            }
        }
    }
}

// --- decoding

/// Turns output that is not UTF-8 into UTF-8 from the ANSI code page, chunk by chunk.
/// An incomplete UTF-8 sequence at the end of a chunk waits for the next one.
struct Decoder {
    carry: Vec<u8>,
}

impl Decoder {
    /// `None` when the ANSI code page is UTF-8 already.
    fn new() -> Option<Decoder> {
        // SAFETY: no arguments.
        (unsafe { GetACP() } != CP_UTF8).then(|| Decoder { carry: Vec::new() })
    }

    fn push(&mut self, data: &[u8], out: &mut VecDeque<u8>) {
        self.carry.extend_from_slice(data);
        match std::str::from_utf8(&self.carry) {
            Ok(_) => {
                out.extend(self.carry.drain(..));
            }
            Err(e) if e.error_len().is_none() => {
                // Valid up to an incomplete sequence at the end.
                let n = e.valid_up_to();
                out.extend(self.carry.drain(..n));
            }
            Err(_) => {
                out.extend(ansi_to_utf8(&self.carry));
                self.carry.clear();
            }
        }
    }

    fn finish(&mut self, out: &mut VecDeque<u8>) {
        if !self.carry.is_empty() {
            out.extend(ansi_to_utf8(&self.carry));
            self.carry.clear();
        }
    }
}

fn ansi_to_utf8(b: &[u8]) -> Vec<u8> {
    if b.is_empty() || b.len() > i32::MAX as usize / 2 {
        return b.to_vec();
    }
    // SAFETY: a size query with valid input.
    let n = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            b.as_ptr(),
            b.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if n <= 0 {
        return b.to_vec();
    }
    let mut w = vec![0u16; n as usize];
    // SAFETY: `w` holds `n` units.
    unsafe { MultiByteToWideChar(CP_ACP, 0, b.as_ptr(), b.len() as i32, w.as_mut_ptr(), n) };
    // SAFETY: a size query with valid input.
    let m = unsafe {
        WideCharToMultiByte(
            CP_UTF8,
            0,
            w.as_ptr(),
            n,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    if m <= 0 {
        return b.to_vec();
    }
    let mut out = vec![0u8; m as usize];
    // SAFETY: `out` holds `m` bytes.
    unsafe {
        WideCharToMultiByte(
            CP_UTF8,
            0,
            w.as_ptr(),
            n,
            out.as_mut_ptr(),
            m,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    out
}

/// An anonymous pipe for [`Host::pipe`](psh_engine::Host::pipe): both ends shared
/// files (the engine may read or write them, and pass them to children).
pub(crate) fn os_pipe() -> io::Result<(WinFile, WinFile)> {
    let (r, w) = io::pipe()?;
    // SAFETY: the handles were just created and are owned by these values alone.
    let (r, w) = unsafe {
        use std::os::windows::io::IntoRawHandle;
        (
            OwnedHandle::from_raw_handle(r.into_raw_handle()),
            OwnedHandle::from_raw_handle(w.into_raw_handle()),
        )
    };
    Ok((WinFile::new(r), WinFile::new(w)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::task::Waker;

    fn block_on<T>(mut f: impl FnMut(&mut Context<'_>) -> Poll<T>) -> T {
        let mut cx = Context::from_waker(Waker::noop());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Poll::Ready(v) = f(&mut cx) {
                return v;
            }
            assert!(std::time::Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn pump_moves_bytes_both_ways_and_ends() {
        // Engine writes into an input pump; the "child" end reads.
        let (ours, theirs) = pump_pipe(true, Decode::Off).unwrap();
        let data = vec![b'x'; 200_000];
        let reader = std::thread::spawn(move || {
            let mut f = File::from(theirs);
            let mut got = Vec::new();
            f.read_to_end(&mut got).unwrap();
            got.len()
        });
        let mut off = 0;
        while off < data.len() {
            off += block_on(|cx| ours.poll_write(cx, &data[off..])).unwrap();
        }
        drop(ours);
        assert_eq!(reader.join().unwrap(), data.len());

        // The "child" writes into an output pump; the engine reads to end of file.
        let (ours, theirs) = pump_pipe(false, Decode::Off).unwrap();
        let writer = std::thread::spawn(move || {
            let mut f = File::from(theirs);
            f.write_all(&vec![b'y'; 100_000]).unwrap();
        });
        let mut total = 0;
        let mut buf = [0u8; 4096];
        loop {
            let n = block_on(|cx| ours.poll_read(cx, &mut buf)).unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        writer.join().unwrap();
        assert_eq!(total, 100_000);
    }

    /// Sizes around every buffer boundary (the 16 KiB chunks, the 64 KiB pump and pipe
    /// buffers), each 100 times, through an input pump closed right after the last
    /// write: every byte must arrive, in order. The reading "child" starts late, so
    /// writes are in flight when the engine's end is dropped.
    #[test]
    fn input_pumps_deliver_every_byte_when_closed_mid_write() {
        let sizes = [
            1, 4095, 4096, 4097, 16383, 16384, 16385, 65535, 65536, 65537, 131072, 208890,
        ];
        for round in 0..100 {
            for &size in &sizes {
                let (ours, theirs) = pump_pipe(true, Decode::Off).unwrap();
                let data: Vec<u8> = (0..size).map(|i| (i * 7 + round) as u8).collect();
                let want = data.clone();
                let reader = std::thread::spawn(move || {
                    if round % 3 == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    let mut f = File::from(theirs);
                    let mut got = Vec::new();
                    f.read_to_end(&mut got).unwrap();
                    got
                });
                let mut off = 0;
                while off < data.len() {
                    off += block_on(|cx| ours.poll_write(cx, &data[off..])).unwrap();
                }
                drop(ours);
                let got = reader.join().unwrap();
                assert!(
                    got == want,
                    "round {round}, size {size}: {} of {} bytes",
                    got.len(),
                    want.len()
                );
            }
        }
    }

    /// The same for output pumps: the "child" writes and closes, the engine reads to the
    /// end.
    #[test]
    fn output_pumps_deliver_every_byte() {
        let sizes = [1, 4096, 16385, 65536, 65537, 208890];
        for round in 0..100 {
            for &size in &sizes {
                let (ours, theirs) = pump_pipe(false, Decode::Off).unwrap();
                let data: Vec<u8> = (0..size).map(|i| (i * 13 + round) as u8).collect();
                let want = data.clone();
                let writer = std::thread::spawn(move || {
                    let mut f = File::from(theirs);
                    f.write_all(&data).unwrap();
                });
                let mut got = Vec::new();
                let mut buf = [0u8; 7000];
                loop {
                    let n = block_on(|cx| ours.poll_read(cx, &mut buf)).unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                writer.join().unwrap();
                assert!(
                    got == want,
                    "round {round}, size {size}: {} bytes",
                    got.len()
                );
            }
        }
    }

    #[test]
    fn dropping_a_pump_breaks_the_childs_pipe() {
        let (ours, theirs) = pump_pipe(false, Decode::Off).unwrap();
        drop(ours);
        let mut f = File::from(theirs);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match f.write_all(&[b'z'; 1024]) {
                Ok(()) => assert!(std::time::Instant::now() < deadline, "never broke"),
                Err(e) => {
                    assert!(is_broken(&e), "{e}");
                    break;
                }
            }
        }
    }

    #[test]
    fn shared_pipe_reads_only_what_is_there() {
        let (r, w) = os_pipe().unwrap();
        let mut cx = Context::from_waker(Waker::noop());
        let mut buf = [0u8; 8];
        assert!(r.poll_read(&mut cx, &mut buf).is_pending());
        assert!(matches!(w.poll_write(&mut cx, b"hi"), Poll::Ready(Ok(2))));
        assert!(matches!(r.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(2))));
        drop(w);
        assert!(matches!(r.poll_read(&mut cx, &mut buf), Poll::Ready(Ok(0))));
    }

    #[test]
    fn emulated_devices() {
        let mut cx = Context::from_waker(Waker::noop());
        let z = WinFile::dev(Dev::Zero);
        let mut buf = [7u8; 16];
        assert!(matches!(
            z.poll_read(&mut cx, &mut buf),
            Poll::Ready(Ok(16))
        ));
        assert_eq!(buf, [0; 16]);
        assert!(matches!(z.poll_write(&mut cx, b"gone"), Poll::Ready(Ok(4))));
        let r = WinFile::dev(Dev::Random);
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        assert!(matches!(r.poll_read(&mut cx, &mut a), Poll::Ready(Ok(32))));
        assert!(matches!(r.poll_read(&mut cx, &mut b), Poll::Ready(Ok(32))));
        assert_ne!(a, b);
    }

    #[test]
    fn decoder_keeps_utf8_and_converts_the_rest() {
        let Some(mut d) = Decoder::new() else {
            return;
        };
        let mut out = VecDeque::new();
        d.push("é".as_bytes(), &mut out);
        assert_eq!(out.drain(..).collect::<Vec<_>>(), "é".as_bytes());
        // A split UTF-8 sequence waits for its second half.
        d.push(&"é".as_bytes()[..1], &mut out);
        assert!(out.is_empty());
        d.push(&"é".as_bytes()[1..], &mut out);
        assert_eq!(out.drain(..).collect::<Vec<_>>(), "é".as_bytes());
        // Not UTF-8: converted from the ANSI code page (0xE9 is é in 1252).
        d.push(&[b'a', 0xE9, b'b'], &mut out);
        let s = String::from_utf8(out.drain(..).collect()).unwrap();
        assert!(
            s.starts_with('a') && s.ends_with('b') && s.chars().count() == 3,
            "{s}"
        );
    }
}

use crate::libc;
use std::{
    io,
    mem::MaybeUninit,
    net::SocketAddr,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    time::Duration,
};

use super::checked;

pub const WAKE_TOKEN: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub token: u64,
    pub readable: bool,
    pub writable: bool,
    pub read_closed: bool,
    pub write_closed: bool,
    pub error: bool,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn timeout_ms(timeout: Option<Duration>) -> libc::c_int {
    timeout.map_or(-1, |timeout| {
        let rounded = timeout
            .as_nanos()
            .div_ceil(Duration::from_millis(1).as_nanos());
        libc::c_int::try_from(rounded).unwrap_or(libc::c_int::MAX)
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod system {
    use super::{
        AsFd, AsRawFd, BorrowedFd, Duration, Event, FromRawFd, MaybeUninit, OwnedFd, WAKE_TOKEN,
        checked, io, timeout_ms,
    };
    use crate::libc;

    pub struct Poller {
        queue: OwnedFd,
        wake: OwnedFd,
    }

    pub struct Events {
        raw: Vec<libc::epoll_event>,
        filled: usize,
    }

    impl Events {
        #[must_use]
        pub fn with_capacity(capacity: usize) -> Self {
            assert!(capacity > 0, "an event buffer holds at least one event");
            Self {
                raw: vec![libc::epoll_event { events: 0, u64: 0 }; capacity],
                filled: 0,
            }
        }

        pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
            self.raw[..self.filled].iter().map(|raw| {
                let flags = raw.events.cast_signed();
                Event {
                    token: raw.u64,
                    readable: flags & (libc::EPOLLIN | libc::EPOLLPRI) != 0,
                    writable: flags & libc::EPOLLOUT != 0,
                    read_closed: flags & libc::EPOLLHUP != 0
                        || (flags & libc::EPOLLIN != 0 && flags & libc::EPOLLRDHUP != 0),
                    write_closed: flags & libc::EPOLLHUP != 0
                        || (flags & libc::EPOLLOUT != 0 && flags & libc::EPOLLERR != 0)
                        || flags == libc::EPOLLERR,
                    error: flags & libc::EPOLLERR != 0,
                }
            })
        }
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            let queue = checked(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
            let queue = unsafe { OwnedFd::from_raw_fd(queue) };
            let wake =
                checked(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
            let wake = unsafe { OwnedFd::from_raw_fd(wake) };
            let poller = Self { queue, wake };
            poller.control(
                libc::EPOLL_CTL_ADD,
                poller.wake.as_fd(),
                WAKE_TOKEN,
                libc::EPOLLIN | libc::EPOLLET,
            )?;
            Ok(poller)
        }

        fn control(
            &self,
            operation: libc::c_int,
            fd: BorrowedFd<'_>,
            token: u64,
            flags: libc::c_int,
        ) -> io::Result<()> {
            let mut event = libc::epoll_event {
                events: flags.cast_unsigned(),
                u64: token,
            };
            checked(unsafe {
                libc::epoll_ctl(
                    self.queue.as_raw_fd(),
                    operation,
                    fd.as_raw_fd(),
                    &raw mut event,
                )
            })
            .map(drop)
        }

        pub fn register(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
            assert_ne!(token, WAKE_TOKEN, "the wake token is the poller's own");
            let flags = libc::EPOLLIN | libc::EPOLLOUT | libc::EPOLLRDHUP | libc::EPOLLET;
            self.control(libc::EPOLL_CTL_ADD, fd, token, flags)
        }

        pub fn deregister(&self, fd: BorrowedFd<'_>) -> io::Result<()> {
            self.control(libc::EPOLL_CTL_DEL, fd, 0, 0)
        }

        pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
            events.filled = 0;
            let capacity = libc::c_int::try_from(events.raw.len()).unwrap_or(libc::c_int::MAX);
            let filled = unsafe {
                libc::epoll_wait(
                    self.queue.as_raw_fd(),
                    events.raw.as_mut_ptr(),
                    capacity,
                    timeout_ms(timeout),
                )
            };
            if filled < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(());
                }
                return Err(error);
            }
            events.filled = usize::try_from(filled).expect("epoll_wait returns a count");
            assert!(
                events.filled <= events.raw.len(),
                "epoll_wait fills at most the buffer"
            );
            if events.raw[..events.filled]
                .iter()
                .any(|raw| raw.u64 == WAKE_TOKEN)
            {
                let mut drained = MaybeUninit::<u64>::uninit();
                unsafe {
                    libc::read(
                        self.wake.as_raw_fd(),
                        drained.as_mut_ptr().cast(),
                        size_of::<u64>(),
                    )
                };
            }
            Ok(())
        }

        pub fn wake(&self) -> io::Result<()> {
            let one = 1_u64;
            let written = unsafe {
                libc::write(
                    self.wake.as_raw_fd(),
                    (&raw const one).cast(),
                    size_of::<u64>(),
                )
            };
            if written < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(());
                }
                return Err(error);
            }
            Ok(())
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
mod system {
    use super::{
        AsRawFd, BorrowedFd, Duration, Event, FromRawFd, OwnedFd, WAKE_TOKEN, checked, io,
    };
    use crate::libc;

    pub struct Poller {
        queue: OwnedFd,
    }

    pub struct Events {
        raw: Vec<libc::kevent>,
        filled: usize,
    }

    unsafe impl Send for Events {}

    const fn change(ident: libc::uintptr_t, filter: i16, flags: u16, token: u64) -> libc::kevent {
        libc::kevent {
            ident,
            filter,
            flags,
            fflags: 0,
            data: 0,
            udata: token as usize as *mut libc::c_void,
        }
    }

    impl Events {
        #[must_use]
        pub fn with_capacity(capacity: usize) -> Self {
            assert!(capacity > 0, "an event buffer holds at least one event");
            Self {
                raw: vec![change(0, 0, 0, 0); capacity],
                filled: 0,
            }
        }

        pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
            self.raw[..self.filled].iter().map(|raw| {
                let eof = raw.flags & libc::EV_EOF != 0;
                let error = raw.flags & libc::EV_ERROR != 0 || (eof && raw.fflags != 0);
                Event {
                    token: raw.udata as usize as u64,
                    readable: raw.filter == libc::EVFILT_READ,
                    writable: raw.filter == libc::EVFILT_WRITE,
                    read_closed: raw.filter == libc::EVFILT_READ && eof,
                    write_closed: raw.filter == libc::EVFILT_WRITE && eof,
                    error,
                }
            })
        }
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            let queue = checked(unsafe { libc::kqueue() })?;
            let queue = unsafe { OwnedFd::from_raw_fd(queue) };
            checked(unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
            let poller = Self { queue };
            poller.apply(&mut [change(
                0,
                libc::EVFILT_USER,
                libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT,
                WAKE_TOKEN,
            )])?;
            Ok(poller)
        }

        fn apply(&self, changes: &mut [libc::kevent]) -> io::Result<()> {
            let count = libc::c_int::try_from(changes.len()).expect("a change list is short");
            checked(unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    changes.as_ptr(),
                    count,
                    changes.as_mut_ptr(),
                    count,
                    std::ptr::null(),
                )
            })?;
            for answer in changes.iter() {
                let code = i32::try_from(answer.data).unwrap_or(0);
                if answer.flags & libc::EV_ERROR != 0
                    && code != 0
                    && code != libc::ENOENT
                    && code != libc::EPIPE
                {
                    return Err(io::Error::from_raw_os_error(code));
                }
            }
            Ok(())
        }

        pub fn register(&self, fd: BorrowedFd<'_>, token: u64) -> io::Result<()> {
            assert_ne!(token, WAKE_TOKEN, "the wake token is the poller's own");
            let ident =
                libc::uintptr_t::try_from(fd.as_raw_fd()).expect("a descriptor is not negative");
            let flags = libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT;
            self.apply(&mut [
                change(ident, libc::EVFILT_READ, flags, token),
                change(ident, libc::EVFILT_WRITE, flags, token),
            ])
        }

        pub fn deregister(&self, fd: BorrowedFd<'_>) -> io::Result<()> {
            let ident =
                libc::uintptr_t::try_from(fd.as_raw_fd()).expect("a descriptor is not negative");
            let flags = libc::EV_DELETE | libc::EV_RECEIPT;
            self.apply(&mut [
                change(ident, libc::EVFILT_READ, flags, 0),
                change(ident, libc::EVFILT_WRITE, flags, 0),
            ])
        }

        pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
            events.filled = 0;
            let spec = timeout.map(|timeout| libc::timespec {
                tv_sec: libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX),
                tv_nsec: libc::c_long::from(timeout.subsec_nanos()),
            });
            let spec_ptr = spec.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
            let capacity = libc::c_int::try_from(events.raw.len()).unwrap_or(libc::c_int::MAX);
            let filled = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.raw.as_mut_ptr(),
                    capacity,
                    spec_ptr,
                )
            };
            if filled < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(());
                }
                return Err(error);
            }
            events.filled = usize::try_from(filled).expect("kevent returns a count");
            assert!(
                events.filled <= events.raw.len(),
                "kevent fills at most the buffer"
            );
            Ok(())
        }

        pub fn wake(&self) -> io::Result<()> {
            let mut trigger = change(
                0,
                libc::EVFILT_USER,
                libc::EV_ADD | libc::EV_RECEIPT,
                WAKE_TOKEN,
            );
            trigger.fflags = libc::NOTE_TRIGGER;
            self.apply(&mut [trigger])
        }
    }
}

pub use system::{Events, Poller};

fn socket_address(address: &SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    let mut storage = unsafe { MaybeUninit::<libc::sockaddr_storage>::zeroed().assume_init() };
    let length = match address {
        SocketAddr::V4(v4) => {
            let raw = libc::sockaddr_in {
                #[cfg(target_vendor = "apple")]
                sin_len: u8::try_from(size_of::<libc::sockaddr_in>())
                    .expect("sockaddr_in is short"),
                sin_family: libc::sa_family_t::try_from(libc::AF_INET).expect("AF_INET fits"),
                sin_port: v4.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(v4.ip().octets()),
                },
                sin_zero: [0; 8],
            };
            unsafe { std::ptr::write((&raw mut storage).cast::<libc::sockaddr_in>(), raw) };
            size_of::<libc::sockaddr_in>()
        }
        SocketAddr::V6(v6) => {
            let raw = libc::sockaddr_in6 {
                #[cfg(target_vendor = "apple")]
                sin6_len: u8::try_from(size_of::<libc::sockaddr_in6>())
                    .expect("sockaddr_in6 is short"),
                sin6_family: libc::sa_family_t::try_from(libc::AF_INET6).expect("AF_INET6 fits"),
                sin6_port: v6.port().to_be(),
                sin6_flowinfo: v6.flowinfo(),
                sin6_addr: libc::in6_addr {
                    s6_addr: v6.ip().octets(),
                },
                sin6_scope_id: v6.scope_id(),
            };
            unsafe { std::ptr::write((&raw mut storage).cast::<libc::sockaddr_in6>(), raw) };
            size_of::<libc::sockaddr_in6>()
        }
    };
    (
        storage,
        libc::socklen_t::try_from(length).expect("a socket address is short"),
    )
}

pub enum Connecting {
    Connected(OwnedFd),
    InProgress(OwnedFd),
}

pub fn connect_nonblocking(address: &SocketAddr) -> io::Result<Connecting> {
    let family = match address {
        SocketAddr::V4(_) => libc::AF_INET,
        SocketAddr::V6(_) => libc::AF_INET6,
    };
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let socket = {
        let raw = checked(unsafe {
            libc::socket(
                family,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        })?;
        unsafe { OwnedFd::from_raw_fd(raw) }
    };
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let socket = {
        let raw = checked(unsafe { libc::socket(family, libc::SOCK_STREAM, 0) })?;
        let socket = unsafe { OwnedFd::from_raw_fd(raw) };
        super::set_close_on_exec(&socket, true)?;
        super::set_nonblocking(&socket, true)?;
        let one: libc::c_int = 1;
        checked(unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_NOSIGPIPE,
                (&raw const one).cast(),
                libc::socklen_t::try_from(size_of::<libc::c_int>()).expect("an int is short"),
            )
        })?;
        socket
    };
    let (storage, length) = socket_address(address);
    let connected =
        unsafe { libc::connect(socket.as_raw_fd(), (&raw const storage).cast(), length) };
    if connected == 0 {
        return Ok(Connecting::Connected(socket));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EINPROGRESS) {
        return Ok(Connecting::InProgress(socket));
    }
    Err(error)
}

pub fn socket_error(fd: impl AsFd) -> io::Result<Option<io::Error>> {
    let mut code: libc::c_int = 0;
    let mut length = libc::socklen_t::try_from(size_of::<libc::c_int>()).expect("an int is short");
    checked(unsafe {
        libc::getsockopt(
            fd.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            (&raw mut code).cast(),
            &raw mut length,
        )
    })?;
    Ok((code != 0).then(|| io::Error::from_raw_os_error(code)))
}

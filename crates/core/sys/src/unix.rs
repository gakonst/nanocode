use std::{
    io,
    mem::MaybeUninit,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::process::CommandExt,
    },
    process::Command,
};

#[cfg(target_os = "linux")]
const PSEUDOTERMINAL_MULTIPLEXER: &std::ffi::CStr = c"/dev/ptmx";
#[cfg(target_vendor = "apple")]
const PSEUDOTERMINAL_NAME_BYTES: usize = 128;
#[cfg(target_os = "linux")]
const PSEUDOTERMINAL_DIRECTORY: &str = "/dev/pts/";

fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(result)
}

fn raw(fd: &impl AsFd) -> RawFd {
    fd.as_fd().as_raw_fd()
}

pub fn read(fd: impl AsFd, buffer: &mut [u8]) -> io::Result<usize> {
    let read = unsafe { libc::read(raw(&fd), buffer.as_mut_ptr().cast(), buffer.len()) };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    let read = usize::try_from(read).expect("a successful read returns a count");
    assert!(read <= buffer.len(), "a read returns at most the buffer's length");
    Ok(read)
}

pub fn set_close_on_exec(fd: impl AsFd, close: bool) -> io::Result<()> {
    let flags = if close { libc::FD_CLOEXEC } else { 0 };
    checked(unsafe { libc::fcntl(raw(&fd), libc::F_SETFD, flags) }).map(drop)
}

pub fn status_flags(fd: impl AsFd) -> io::Result<libc::c_int> {
    checked(unsafe { libc::fcntl(raw(&fd), libc::F_GETFL) })
}

pub fn set_status_flags(fd: impl AsFd, flags: libc::c_int) -> io::Result<()> {
    checked(unsafe { libc::fcntl(raw(&fd), libc::F_SETFL, flags) }).map(drop)
}

pub fn set_nonblocking(fd: impl AsFd, nonblocking: bool) -> io::Result<()> {
    let mut value = libc::c_int::from(nonblocking);
    checked(unsafe { libc::ioctl(raw(&fd), libc::FIONBIO, &raw mut value) }).map(drop)
}

pub fn duplicate_close_on_exec(fd: impl AsFd, lowest: RawFd) -> io::Result<OwnedFd> {
    let duplicate = checked(unsafe { libc::fcntl(raw(&fd), libc::F_DUPFD_CLOEXEC, lowest) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

pub fn redirect_input(fd: impl AsFd) -> io::Result<()> {
    checked(unsafe { libc::dup2(raw(&fd), libc::STDIN_FILENO) }).map(drop)
}

pub fn inherited_descriptor(descriptor: RawFd) -> io::Result<OwnedFd> {
    checked(unsafe { libc::fcntl(descriptor, libc::F_GETFD) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

pub fn reap_children() {
    loop {
        let reaped = unsafe { libc::waitpid(-1, std::ptr::null_mut(), 0) };
        if reaped == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
            return;
        }
    }
}

pub fn readable_now(fd: BorrowedFd<'_>) -> io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = checked(unsafe { libc::poll(&raw mut descriptor, 1, 0) })?;
    Ok(ready > 0)
}

#[derive(Clone)]
pub struct Termios(libc::termios);

impl Termios {
    pub const fn make_raw(&mut self) {
        self.0.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        self.0.c_oflag &= !libc::OPOST;
        self.0.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        self.0.c_cflag &= !(libc::CSIZE | libc::PARENB);
        self.0.c_cflag |= libc::CS8;
        self.0.c_cc[libc::VMIN] = 1;
        self.0.c_cc[libc::VTIME] = 0;
    }

    #[must_use]
    pub const fn end_of_file(&self) -> u8 {
        self.0.c_cc[libc::VEOF]
    }
}

pub fn terminal_attributes(fd: impl AsFd) -> io::Result<Termios> {
    let mut attributes = MaybeUninit::<libc::termios>::uninit();
    checked(unsafe { libc::tcgetattr(raw(&fd), attributes.as_mut_ptr()) })?;
    Ok(Termios(unsafe { attributes.assume_init() }))
}

pub fn set_terminal_attributes(fd: impl AsFd, attributes: &Termios) -> io::Result<()> {
    checked(unsafe { libc::tcsetattr(raw(&fd), libc::TCSANOW, &raw const attributes.0) }).map(drop)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub rows: u16,
    pub columns: u16,
}

pub fn window_size(fd: impl AsFd) -> io::Result<WindowSize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    checked(unsafe { libc::ioctl(raw(&fd), libc::TIOCGWINSZ, &raw mut size) })?;
    Ok(WindowSize {
        rows: size.ws_row,
        columns: size.ws_col,
    })
}

pub fn set_window_size(fd: impl AsFd, size: WindowSize) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: size.rows,
        ws_col: size.columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    checked(unsafe { libc::ioctl(raw(&fd), libc::TIOCSWINSZ, &raw const size) }).map(drop)
}

#[cfg(target_os = "linux")]
pub fn open_pseudoterminal() -> io::Result<(OwnedFd, OwnedFd)> {
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    let controller = match checked(unsafe { libc::open(PSEUDOTERMINAL_MULTIPLEXER.as_ptr(), flags) }) {
        Ok(controller) => unsafe { OwnedFd::from_raw_fd(controller) },
        Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
            return Err(io::Error::from_raw_os_error(libc::EAGAIN));
        }
        Err(error) => return Err(error),
    };
    let mut unlock: libc::c_int = 0;
    checked(unsafe { libc::ioctl(raw(&controller), libc::TIOCSPTLCK, &raw mut unlock) })?;
    let mut number: libc::c_uint = 0;
    checked(unsafe { libc::ioctl(raw(&controller), libc::TIOCGPTN, &raw mut number) })?;
    let path = std::ffi::CString::new(format!("{PSEUDOTERMINAL_DIRECTORY}{number}"))
        .expect("a terminal path has no zero byte");
    let terminal = checked(unsafe { libc::open(path.as_ptr(), flags) })?;
    Ok((controller, unsafe { OwnedFd::from_raw_fd(terminal) }))
}

#[cfg(target_vendor = "apple")]
pub fn open_pseudoterminal() -> io::Result<(OwnedFd, OwnedFd)> {
    let controller = checked(unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) })?;
    let controller = unsafe { OwnedFd::from_raw_fd(controller) };
    set_close_on_exec(&controller, true)?;
    checked(unsafe { libc::grantpt(raw(&controller)) })?;
    checked(unsafe { libc::unlockpt(raw(&controller)) })?;
    let mut name = [0_u8; PSEUDOTERMINAL_NAME_BYTES];
    checked(unsafe { libc::ioctl(raw(&controller), libc::TIOCPTYGNAME.into(), name.as_mut_ptr()) })?;
    let path = std::ffi::CStr::from_bytes_until_nul(&name).map_err(io::Error::other)?;
    let terminal = checked(unsafe {
        libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC)
    })?;
    Ok((controller, unsafe { OwnedFd::from_raw_fd(terminal) }))
}

pub fn lead_session(command: &mut Command, default_signals: &'static [libc::c_int]) {
    let lead = move || -> io::Result<()> {
        for &signal in default_signals {
            if unsafe { libc::signal(signal, libc::SIG_DFL) } == libc::SIG_ERR {
                return Err(io::Error::last_os_error());
            }
        }
        checked(unsafe { libc::setsid() })?;
        checked(unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) })?;
        Ok(())
    };
    unsafe { command.pre_exec(lead) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limit {
    pub current: Option<u64>,
    pub maximum: Option<u64>,
}

fn limit_value(value: libc::rlim_t) -> Option<u64> {
    (value != libc::RLIM_INFINITY).then_some(value)
}

#[must_use]
pub fn open_file_limit() -> Limit {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let result = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
    assert_eq!(result, 0, "getrlimit of RLIMIT_NOFILE does not fail");
    Limit {
        current: limit_value(limit.rlim_cur),
        maximum: limit_value(limit.rlim_max),
    }
}

pub fn set_open_file_limit(limit: Limit) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: limit.current.unwrap_or(libc::RLIM_INFINITY),
        rlim_max: limit.maximum.unwrap_or(libc::RLIM_INFINITY),
    };
    checked(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) }).map(drop)
}

#[cfg(target_os = "linux")]
fn control_space() -> usize {
    let descriptor_bytes = u32::try_from(size_of::<RawFd>()).expect("a descriptor fits");
    usize::try_from(unsafe { libc::CMSG_SPACE(descriptor_bytes) }).expect("the space fits")
}

#[cfg(target_os = "linux")]
pub fn send_descriptor(socket: impl AsFd, descriptor: BorrowedFd<'_>) -> io::Result<()> {
    let space = control_space();
    let mut control = vec![0_u64; space.div_ceil(size_of::<u64>())];
    let mut byte = [0_u8];
    let mut slice = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut slice;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space;
    let header = unsafe { libc::CMSG_FIRSTHDR(&raw const message) };
    assert!(!header.is_null(), "the control buffer holds one header");
    let descriptor_bytes = u32::try_from(size_of::<RawFd>()).expect("a descriptor fits");
    unsafe {
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(descriptor_bytes) as usize;
        libc::CMSG_DATA(header).cast::<RawFd>().write_unaligned(descriptor.as_raw_fd());
    }
    let sent = unsafe { libc::sendmsg(raw(&socket), &raw const message, 0) };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn receive_descriptor(socket: impl AsFd) -> io::Result<Option<OwnedFd>> {
    let space = control_space();
    let mut control = vec![0_u64; space.div_ceil(size_of::<u64>())];
    let mut byte = [0_u8];
    let mut slice = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut slice;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = space;
    let received = unsafe { libc::recvmsg(raw(&socket), &raw mut message, libc::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut found = None;
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&raw const message) };
    while !header.is_null() {
        let (level, kind, length) = unsafe { ((*header).cmsg_level, (*header).cmsg_type, (*header).cmsg_len) };
        let data_offset = unsafe { libc::CMSG_DATA(header).offset_from(header.cast::<u8>()) };
        let data_bytes = length
            .checked_sub(usize::try_from(data_offset).expect("data follows its header"))
            .expect("a control message is at least its header");
        if level == libc::SOL_SOCKET && kind == libc::SCM_RIGHTS {
            let count = data_bytes / size_of::<RawFd>();
            for index in 0..count {
                let received = unsafe { libc::CMSG_DATA(header).cast::<RawFd>().add(index).read_unaligned() };
                let owned = unsafe { OwnedFd::from_raw_fd(received) };
                if found.is_none() {
                    found = Some(owned);
                }
            }
        }
        header = unsafe { libc::CMSG_NXTHDR(&raw const message, header) };
    }
    Ok(found)
}

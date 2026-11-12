use std::{
    fs::File,
    io::{self, IsTerminal},
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
    process::Command,
    sync::LazyLock,
};

use nanocode_sync::Slot;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr},
};

const DECIMAL_RADIX: u32 = 10;
const NOW: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 0,
};

static ORIGINAL_MODE: LazyLock<Slot<Termios>> = LazyLock::new(Slot::default);

pub enum Tty {
    Standard(io::Stdin),
    Device(File),
}

impl AsRawFd for Tty {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Self::Standard(stdin) => stdin.as_raw_fd(),
            Self::Device(file) => file.as_raw_fd(),
        }
    }
}

impl AsFd for Tty {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Standard(stdin) => stdin.as_fd(),
            Self::Device(file) => file.as_fd(),
        }
    }
}

pub fn tty() -> io::Result<Tty> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(Tty::Standard(stdin));
    }
    File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(Tty::Device)
}

#[must_use]
pub fn raw_mode_enabled() -> bool {
    !ORIGINAL_MODE.is_empty()
}

pub fn enable_raw_mode() -> io::Result<()> {
    if raw_mode_enabled() {
        return Ok(());
    }
    let tty = tty()?;
    let original = tcgetattr(&tty)?;
    let mut raw = original.clone();
    raw.make_raw();
    tcsetattr(&tty, OptionalActions::Now, &raw)?;
    assert!(
        ORIGINAL_MODE.put(original).is_none(),
        "raw mode is entered once before it is left"
    );
    Ok(())
}

pub fn disable_raw_mode() -> io::Result<()> {
    let Some(original) = ORIGINAL_MODE.take() else {
        return Ok(());
    };
    let restored = tty().and_then(|tty| Ok(tcsetattr(&tty, OptionalActions::Now, &original)?));
    if restored.is_err() {
        ORIGINAL_MODE.put(original);
    }
    restored
}

pub fn size() -> io::Result<(u16, u16)> {
    let window = match File::open("/dev/tty") {
        Ok(device) => tcgetwinsize(&device),
        Err(_) => tcgetwinsize(io::stdout()),
    };
    if let Ok(window) = window {
        return Ok((window.ws_col, window.ws_row));
    }
    match (tput("cols"), tput("lines")) {
        (Some(columns), Some(rows)) => Ok((columns, rows)),
        _ => Err(io::Error::last_os_error()),
    }
}

fn tput(capability: &str) -> Option<u16> {
    let output = Command::new("tput").arg(capability).output().ok()?;
    let mut value = 0_u16;
    for digit in output
        .stdout
        .into_iter()
        .filter_map(|byte| char::from(byte).to_digit(DECIMAL_RADIX))
    {
        value = value
            .checked_mul(u16::try_from(DECIMAL_RADIX).expect("the radix fits in u16"))?
            .checked_add(u16::try_from(digit).expect("a decimal digit fits in u16"))?;
    }
    (value > 0).then_some(value)
}

pub(crate) fn readable_now(fd: BorrowedFd<'_>) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(&fd, PollFlags::IN)];
    loop {
        match poll(&mut descriptors, Some(&NOW)) {
            Ok(ready) => return Ok(ready > 0),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

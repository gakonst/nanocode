#[cfg(unix)]
pub fn stdout_columns() -> Option<usize> {
    use nanocode_sys::libc;
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one winsize into the valid, writable pointer.
    let status = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &raw mut size) };
    (status == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

#[cfg(windows)]
pub fn stdout_columns() -> Option<usize> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    };
    // SAFETY: GetStdHandle takes no pointers.
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: CONSOLE_SCREEN_BUFFER_INFO is plain data, valid when zeroed.
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: the handle was checked and info is a valid, writable buffer of its type.
    if unsafe { GetConsoleScreenBufferInfo(handle, &raw mut info) } == 0 {
        return None;
    }
    let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
    usize::try_from(width).ok().filter(|&width| width > 0)
}

#[cfg(not(any(unix, windows)))]
pub fn stdout_columns() -> Option<usize> {
    None
}

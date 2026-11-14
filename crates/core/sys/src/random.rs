#[cfg(unix)]
use crate::libc;
use std::io;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    let mut filled = 0;
    while filled < bytes.len() {
        let rest = &mut bytes[filled..];
        let read = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        let read = usize::try_from(read).expect("a successful getrandom returns a count");
        assert!(read > 0, "getrandom fills at least one byte of a nonempty buffer");
        assert!(read <= rest.len(), "getrandom fills at most the buffer");
        filled += read;
    }
    Ok(())
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android")),
    not(all(target_vendor = "apple", not(target_os = "macos")))
))]
const GETENTROPY_BYTES_MAX: usize = 256;

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android")),
    not(all(target_vendor = "apple", not(target_os = "macos")))
))]
pub fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    for chunk in bytes.chunks_mut(GETENTROPY_BYTES_MAX) {
        if unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    let status = unsafe { libc::CCRandomGenerateBytes(bytes.as_mut_ptr().cast(), bytes.len()) };
    if status != libc::kCCSuccess {
        return Err(io::Error::other(format!(
            "CCRandomGenerateBytes failed with status {status}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
#[cfg_attr(
    target_arch = "x86",
    link(name = "bcryptprimitives", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "bcryptprimitives", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

#[cfg(windows)]
pub fn fill_random(bytes: &mut [u8]) -> io::Result<()> {
    if unsafe { ProcessPrng(bytes.as_mut_ptr(), bytes.len()) } == 0 {
        return Err(io::Error::other("ProcessPrng failed"));
    }
    Ok(())
}

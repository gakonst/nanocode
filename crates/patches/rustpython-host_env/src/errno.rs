// spell-checker:disable

/// Return the platform `strerror(errno)` message as an owned `String`.
/// Returns `None` when the runtime gives no description for `errno`.
#[cfg(any(unix, windows))]
#[must_use]
pub fn strerror_string(errno: i32) -> Option<String> {
    let ptr = unsafe { libc::strerror(errno) };
    if ptr.is_null() {
        return None;
    }
    let s = unsafe { core::ffi::CStr::from_ptr(ptr) }.to_string_lossy();
    Some(s.into_owned())
}

#[cfg(any(unix, windows, target_os = "wasi"))]
pub mod errors {
    pub use libc::*;
    #[cfg(windows)]
    pub use windows_sys::Win32::{
        Foundation::*,
        Networking::WinSock::{
            WSABASEERR, WSADESCRIPTION_LEN, WSAEACCES, WSAEADDRINUSE, WSAEADDRNOTAVAIL,
            WSAEAFNOSUPPORT, WSAEALREADY, WSAEBADF, WSAECANCELLED, WSAECONNABORTED,
            WSAECONNREFUSED, WSAECONNRESET, WSAEDESTADDRREQ, WSAEDISCON, WSAEDQUOT, WSAEFAULT,
            WSAEHOSTDOWN, WSAEHOSTUNREACH, WSAEINPROGRESS, WSAEINTR, WSAEINVAL,
            WSAEINVALIDPROCTABLE, WSAEINVALIDPROVIDER, WSAEISCONN, WSAELOOP, WSAEMFILE,
            WSAEMSGSIZE, WSAENAMETOOLONG, WSAENETDOWN, WSAENETRESET, WSAENETUNREACH, WSAENOBUFS,
            WSAENOMORE, WSAENOPROTOOPT, WSAENOTCONN, WSAENOTEMPTY, WSAENOTSOCK, WSAEOPNOTSUPP,
            WSAEPFNOSUPPORT, WSAEPROCLIM, WSAEPROTONOSUPPORT, WSAEPROTOTYPE,
            WSAEPROVIDERFAILEDINIT, WSAEREFUSED, WSAEREMOTE, WSAESHUTDOWN, WSAESOCKTNOSUPPORT,
            WSAESTALE, WSAETIMEDOUT, WSAETOOMANYREFS, WSAEUSERS, WSAEWOULDBLOCK, WSAID_ACCEPTEX,
            WSAID_CONNECTEX, WSAID_DISCONNECTEX, WSAID_GETACCEPTEXSOCKADDRS, WSAID_TRANSMITFILE,
            WSAID_TRANSMITPACKETS, WSAID_WSAPOLL, WSAID_WSARECVMSG, WSANO_DATA, WSANO_RECOVERY,
            WSANOTINITIALISED, WSAPROTOCOL_LEN, WSASERVICE_NOT_FOUND, WSASYS_STATUS_LEN,
            WSASYSCALLFAILURE, WSASYSNOTREADY, WSATRY_AGAIN, WSATYPE_NOT_FOUND, WSAVERNOTSUPPORTED,
        },
    };
    #[cfg(windows)]
    mod wsa {
        use windows_sys::Win32::Networking::WinSock;
        pub const EADDRINUSE: i32 = WinSock::WSAEADDRINUSE as i32;
        pub const EADDRNOTAVAIL: i32 = WinSock::WSAEADDRNOTAVAIL as i32;
        pub const EAFNOSUPPORT: i32 = WinSock::WSAEAFNOSUPPORT as i32;
        pub const EALREADY: i32 = WinSock::WSAEALREADY as i32;
        pub const ECONNABORTED: i32 = WinSock::WSAECONNABORTED as i32;
        pub const ECONNREFUSED: i32 = WinSock::WSAECONNREFUSED as i32;
        pub const ECONNRESET: i32 = WinSock::WSAECONNRESET as i32;
        pub const EDESTADDRREQ: i32 = WinSock::WSAEDESTADDRREQ as i32;
        pub const EDQUOT: i32 = WinSock::WSAEDQUOT as i32;
        pub const EHOSTDOWN: i32 = WinSock::WSAEHOSTDOWN as i32;
        pub const EHOSTUNREACH: i32 = WinSock::WSAEHOSTUNREACH as i32;
        pub const EINPROGRESS: i32 = WinSock::WSAEINPROGRESS as i32;
        pub const EISCONN: i32 = WinSock::WSAEISCONN as i32;
        pub const ELOOP: i32 = WinSock::WSAELOOP as i32;
        pub const EMSGSIZE: i32 = WinSock::WSAEMSGSIZE as i32;
        pub const ENETDOWN: i32 = WinSock::WSAENETDOWN as i32;
        pub const ENETRESET: i32 = WinSock::WSAENETRESET as i32;
        pub const ENETUNREACH: i32 = WinSock::WSAENETUNREACH as i32;
        pub const ENOBUFS: i32 = WinSock::WSAENOBUFS as i32;
        pub const ENOPROTOOPT: i32 = WinSock::WSAENOPROTOOPT as i32;
        pub const ENOTCONN: i32 = WinSock::WSAENOTCONN as i32;
        pub const ENOTSOCK: i32 = WinSock::WSAENOTSOCK as i32;
        pub const EOPNOTSUPP: i32 = WinSock::WSAEOPNOTSUPP as i32;
        pub const EPFNOSUPPORT: i32 = WinSock::WSAEPFNOSUPPORT as i32;
        pub const EPROTONOSUPPORT: i32 = WinSock::WSAEPROTONOSUPPORT as i32;
        pub const EPROTOTYPE: i32 = WinSock::WSAEPROTOTYPE as i32;
        pub const EREMOTE: i32 = WinSock::WSAEREMOTE as i32;
        pub const ESHUTDOWN: i32 = WinSock::WSAESHUTDOWN as i32;
        pub const ESOCKTNOSUPPORT: i32 = WinSock::WSAESOCKTNOSUPPORT as i32;
        pub const ESTALE: i32 = WinSock::WSAESTALE as i32;
        pub const ETIMEDOUT: i32 = WinSock::WSAETIMEDOUT as i32;
        pub const ETOOMANYREFS: i32 = WinSock::WSAETOOMANYREFS as i32;
        pub const EUSERS: i32 = WinSock::WSAEUSERS as i32;
        pub const EWOULDBLOCK: i32 = WinSock::WSAEWOULDBLOCK as i32;
    }
    #[cfg(windows)]
    pub use wsa::*;
}

#![cfg(unix)]

pub mod bubblewrap;
#[cfg(target_os = "linux")]
pub mod confine;
/// Landlock and seccomp are Linux's: elsewhere confinement is refused.
#[cfg(not(target_os = "linux"))]
#[path = "confine_other.rs"]
pub mod confine;
pub mod egress;

pub const SYSTEM: [&str; 7] = [
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc/ssl",
    "/etc/ca-certificates",
];

pub const PROXY_VARIABLES: [&str; 6] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
];

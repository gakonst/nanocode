#[allow(unused_imports)]
use core::{mem, ptr};
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"),
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64"),
    all(target_os = "android", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "ios", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod unix;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"),
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64"),
    all(target_os = "android", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "ios", target_arch = "aarch64")
))]
pub use unix::*;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"),
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"),
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64")
))]
pub use linux::*;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_gnu;
#[cfg(any(
    all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "gnu", target_arch = "aarch64")
))]
pub use linux_gnu::*;
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_gnu_x86_64;
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "x86_64"))]
pub use linux_gnu_x86_64::*;
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_gnu_aarch64;
#[cfg(all(target_os = "linux", target_env = "gnu", target_arch = "aarch64"))]
pub use linux_gnu_aarch64::*;
#[cfg(any(
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_musl;
#[cfg(any(
    all(target_os = "linux", target_env = "musl", target_arch = "x86_64"),
    all(target_os = "linux", target_env = "musl", target_arch = "aarch64")
))]
pub use linux_musl::*;
#[cfg(all(target_os = "linux", target_env = "musl", target_arch = "x86_64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_musl_x86_64;
#[cfg(all(target_os = "linux", target_env = "musl", target_arch = "x86_64"))]
pub use linux_musl_x86_64::*;
#[cfg(all(target_os = "linux", target_env = "musl", target_arch = "aarch64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod linux_musl_aarch64;
#[cfg(all(target_os = "linux", target_env = "musl", target_arch = "aarch64"))]
pub use linux_musl_aarch64::*;
#[cfg(all(target_os = "android", target_arch = "aarch64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod android_aarch64;
#[cfg(all(target_os = "android", target_arch = "aarch64"))]
pub use android_aarch64::*;
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "ios", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod apple;
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "ios", target_arch = "aarch64")
))]
pub use apple::*;
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod macos;
#[cfg(any(
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
pub use macos::*;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod macos_x86_64;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
pub use macos_x86_64::*;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod macos_aarch64;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use macos_aarch64::*;
#[cfg(all(target_os = "ios", target_arch = "aarch64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod ios_aarch64;
#[cfg(all(target_os = "ios", target_arch = "aarch64"))]
pub use ios_aarch64::*;
#[cfg(any(
    all(target_os = "windows", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "windows", target_env = "msvc", target_arch = "x86_64"),
    all(target_os = "windows", target_env = "msvc", target_arch = "aarch64")
))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod windows;
#[cfg(any(
    all(target_os = "windows", target_env = "gnu", target_arch = "x86_64"),
    all(target_os = "windows", target_env = "msvc", target_arch = "x86_64"),
    all(target_os = "windows", target_env = "msvc", target_arch = "aarch64")
))]
pub use windows::*;
#[cfg(all(target_os = "windows", target_env = "gnu", target_arch = "x86_64"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod windows_gnu_x86_64;
#[cfg(all(target_os = "windows", target_env = "gnu", target_arch = "x86_64"))]
pub use windows_gnu_x86_64::*;
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_unsafe,
    clippy::all,
    clippy::missing_const_for_fn,
    clippy::use_self,
    clippy::missing_safety_doc
)]
mod wasm32;
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub use wasm32::*;

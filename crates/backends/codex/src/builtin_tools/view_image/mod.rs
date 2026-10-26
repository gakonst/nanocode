pub(crate) mod host;
#[cfg(not(target_family = "wasm"))]
mod native;

pub(crate) use host::HostViewImageHandler;
#[cfg(not(target_family = "wasm"))]
pub(crate) use native::ViewImageHandler;

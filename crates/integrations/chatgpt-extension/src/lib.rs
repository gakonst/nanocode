#![cfg(unix)]

mod bridge;
mod browsers;
mod host;
mod native;
mod proxy;
mod relay;
mod steps;

pub use bridge::{Bridge, Guard, Launch, Launcher, Registration, Upstream, host_path};
pub use browsers::{
    browser_dirs, browser_for_bundle, chatgpt_installed, config_base, default_browser,
    default_from_handlers, extension_enabled, extension_on, profiles,
};
pub use host::{NativeHost, extension_origin, manifest_path};
pub use native::{MAX_NATIVE, MAX_RESPONSE, read_body, standalone_reply, write_body};
pub use proxy::Connection;
pub use steps::{GROUP_TITLE_MAX, Reference, Snapshot, Tabs, group_title, snapshot};

pub const EXTENSION_ID: &str = "hehggadaopoacecdllhhajmbjkdcmajg";
pub const EDGE_EXTENSION_ID: &str = "odlomjlbamekndcpllcnffbgeohgkmjh";
pub const HOST_NAME: &str = "com.openai.codexextension";
pub const ORIGIN: &str = "chrome-extension://hehggadaopoacecdllhhajmbjkdcmajg/";
pub const EDGE_ORIGIN: &str = "chrome-extension://odlomjlbamekndcpllcnffbgeohgkmjh/";

pub type Error = Box<dyn std::error::Error + Send + Sync>;

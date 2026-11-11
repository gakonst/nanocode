#![cfg(not(target_family = "wasm"))]

mod backend;
pub mod driver;
#[cfg(target_os = "macos")]
pub mod embedded;
mod engine;
mod keys;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod mcp;
mod policy;
mod state;
mod tool;
#[cfg(target_os = "windows")]
mod windows;

pub use backend::{App, Backend, Button, Capabilities, Direction, Pointer, Press, Read, Target};
pub use engine::{Desktop, Engine, Step, find};
pub use keys::{Chord, Key, Modifiers};
pub use mcp::Mcp;
pub use nanocode_computer_repl::*;
pub use policy::{AppIdentity, Leases, Policy, SENSITIVE, app_key, sensitive};
pub use state::{Node, Selection, render, text_range};
pub use tool::{ACTIONS, Computer, Limits, Native, Provider, definition, step_line};

#[cfg(target_os = "macos")]
fn platform(native: Native) -> Result<Box<dyn Desktop>, String> {
    Ok(Box::new(native.engine(macos::Ax::default())))
}

#[cfg(target_os = "linux")]
fn platform(native: Native) -> Result<Box<dyn Desktop>, String> {
    Ok(Box::new(native.engine(linux::AtSpi::new())))
}

#[cfg(target_os = "windows")]
fn platform(native: Native) -> Result<Box<dyn Desktop>, String> {
    Ok(Box::new(native.engine(windows::Uia)))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn platform(_: Native) -> Result<Box<dyn Desktop>, String> {
    Err("Computer use has no engine for this platform, so nothing ran.".into())
}

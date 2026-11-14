use nanocode_executor::process::Command;
use serde::{Deserialize, Serialize};
use std::{
    io::{self, PipeWriter},
    os::{
        fd::{OwnedFd, RawFd},
        unix::net::UnixStream,
    },
    path::{Path, PathBuf},
};

pub const ARGUMENT: &str = "tool-sandbox";

pub const CHANNEL: &str = "tool-channel";

pub const CHANNEL_DESCRIPTOR: RawFd = 3;

pub struct Channel {
    pub host: UnixStream,
    pub program: OwnedFd,
    pub input: PipeWriter,
}

pub fn channel() -> io::Result<Channel> {
    Err(unsupported())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub read: Vec<PathBuf>,
    pub write: PathBuf,
    pub port: u16,
}

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Landlock confinement needs Linux",
    )
}

pub fn check() -> io::Result<()> {
    Err(unsupported())
}

impl Policy {
    pub fn command(&self, _launcher: &Path, _program: &Path) -> io::Result<Command> {
        Err(unsupported())
    }

    pub fn command_with_channel(&self, _launcher: &Path, _program: &Path) -> io::Result<Command> {
        Err(unsupported())
    }
}

pub const fn launch_if_requested() {}

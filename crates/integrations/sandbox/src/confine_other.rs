use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::process::Command;

pub const ARGUMENT: &str = "tool-sandbox";

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
}

pub const fn launch_if_requested() {}

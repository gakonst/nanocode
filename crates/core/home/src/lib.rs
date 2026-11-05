mod fs_util;
mod instructions;
#[cfg(unix)]
pub mod private;
mod project;
#[cfg(unix)]
pub mod release;
mod scoped;
#[cfg(unix)]
pub mod service;
mod session_environment;
#[cfg(not(target_family = "wasm"))]
pub mod shell;
mod skills;

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

use serde::Serialize;

pub use instructions::{
    GlobalInstructions, InstructionKind, InstructionSource, MAX_INSTRUCTION_FILE_BYTES,
    PROJECT_DOC_SEPARATOR, combine_instructions, global_instructions,
};
pub use project::{InstructionFile, ProjectHome, ProjectInstructions};
pub use scoped::{PROJECT_INSTRUCTION_FILES, instruction_candidates, instruction_scopes};
pub use session_environment::SessionEnvironment;
pub use skills::{SkillEntry, SkillRoot, SkillSet, resolve_skills};

pub const NANOCODE_HOME_ENV: &str = "NANOCODE_HOME";

pub const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

#[derive(Debug, thiserror::Error)]
pub enum HomeError {
    #[error("home directory is unavailable; set HOME (or {0})")]
    Unavailable(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,

    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,

    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub level: Level,
    pub path: PathBuf,
    pub message: String,
}

impl Diagnostic {
    pub(crate) fn info(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            level: Level::Info,
            path: path.into(),
            message: message.into(),
        }
    }
    pub(crate) fn warning(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self {
            level: Level::Warning,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Info => "info",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}: {}", self.path.display(), self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentHome {
    nanocode_home: PathBuf,
    claude_home: PathBuf,
    user_home: Option<PathBuf>,
}

impl AgentHome {
    pub fn new(nanocode_home: impl Into<PathBuf>, claude_home: impl Into<PathBuf>) -> Self {
        Self {
            nanocode_home: nanocode_home.into(),
            claude_home: claude_home.into(),
            user_home: None,
        }
    }

    #[must_use]
    pub fn with_user_home(mut self, user_home: Option<PathBuf>) -> Self {
        self.user_home = user_home;
        self
    }

    pub fn from_env() -> Result<Self, HomeError> {
        Self::from_vars(|name| std::env::var_os(name))
    }

    pub fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, HomeError> {
        let get = |name: &str| {
            var(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let user_home = get("HOME").or_else(|| get("USERPROFILE"));
        let nanocode_home = match get(NANOCODE_HOME_ENV) {
            Some(path) => path,
            None => user_home
                .as_ref()
                .ok_or(HomeError::Unavailable(NANOCODE_HOME_ENV))?
                .join(".nanocode"),
        };
        let claude_home = match get(CLAUDE_CONFIG_DIR_ENV) {
            Some(path) => path,
            None => user_home
                .as_ref()
                .ok_or(HomeError::Unavailable(CLAUDE_CONFIG_DIR_ENV))?
                .join(".claude"),
        };
        Ok(Self {
            nanocode_home,
            claude_home,
            user_home,
        })
    }

    pub fn nanocode_home(&self) -> &Path {
        &self.nanocode_home
    }

    pub fn claude_home(&self) -> &Path {
        &self.claude_home
    }

    pub fn user_home(&self) -> Option<&Path> {
        self.user_home.as_deref()
    }

    pub fn project(&self, workspace: impl AsRef<Path>) -> ProjectHome {
        ProjectHome::discover(workspace)
    }
}

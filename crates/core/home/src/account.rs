//! The OS user a program runs as, and where its per-user files go: macOS's
//! `~/Library` folders, or the XDG directories elsewhere.

use nix::unistd::{User, getuid};
use std::{env, path::PathBuf};

/// Why the user could not be told.
#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("cannot determine your OS user name")]
    Name,
    #[error("cannot determine your home directory")]
    Home,
}

/// The OS user: its name, home and login shell. The name is the password
/// database's, else `$USER`; the home is `$HOME`, else the database's, and must
/// be absolute.
#[derive(Clone, Debug)]
pub struct Account {
    pub name: String,
    pub home: PathBuf,
    pub shell: Option<PathBuf>,
}

impl Account {
    pub fn current() -> Result<Self, AccountError> {
        let user = User::from_uid(getuid()).ok().flatten();
        let name = user
            .as_ref()
            .map(|user| user.name.clone())
            .or_else(|| env::var("USER").ok())
            .filter(|name| !name.is_empty())
            .ok_or(AccountError::Name)?;
        let home = env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| user.as_ref().map(|user| user.dir.clone()))
            .filter(|home| home.is_absolute())
            .ok_or(AccountError::Home)?;
        let shell = user
            .map(|user| user.shell)
            .filter(|shell| shell.is_absolute());
        Ok(Self { name, home, shell })
    }

    /// A program's configuration directory: `~/Library/Application
    /// Support/<mac>` on macOS, else `${XDG_CONFIG_HOME:-~/.config}/<name>`.
    pub fn config_dir(&self, mac: &str, name: &str) -> PathBuf {
        self.user_path(
            "Library/Application Support",
            mac,
            "XDG_CONFIG_HOME",
            ".config",
            name,
        )
    }

    /// A program's log file: `~/Library/Logs/<name>` on macOS, else
    /// `${XDG_STATE_HOME:-~/.local/state}/<name>`.
    pub fn log_path(&self, name: &str) -> PathBuf {
        self.user_path("Library/Logs", name, "XDG_STATE_HOME", ".local/state", name)
    }

    fn user_path(
        &self,
        library: &str,
        mac: &str,
        base: &str,
        fallback: &str,
        name: &str,
    ) -> PathBuf {
        if cfg!(target_os = "macos") {
            return self.home.join(library).join(mac);
        }
        env::var_os(base)
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .unwrap_or_else(|| self.home.join(fallback))
            .join(name)
    }
}

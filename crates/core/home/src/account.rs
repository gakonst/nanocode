use std::{env, path::PathBuf};

#[derive(Debug)]
pub enum AccountError {
    Name,
    Home,
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name => formatter.write_str("cannot determine your OS user name"),
            Self::Home => formatter.write_str("cannot determine your home directory"),
        }
    }
}

impl std::error::Error for AccountError {}

#[derive(Clone, Debug)]
pub struct Account {
    pub name: String,
    pub home: PathBuf,
    pub shell: Option<PathBuf>,
}

impl Account {
    pub fn current() -> Result<Self, AccountError> {
        let user = nanocode_platform::unix::user_entry(nanocode_platform::unix::user_id())
            .ok()
            .flatten();
        let name = user
            .as_ref()
            .map(|user| user.name.clone())
            .or_else(|| env::var("USER").ok())
            .filter(|name| !name.is_empty())
            .ok_or(AccountError::Name)?;
        let home = env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(|| user.as_ref().map(|user| user.home.clone()))
            .filter(|home| home.is_absolute())
            .ok_or(AccountError::Home)?;
        let shell = user
            .map(|user| user.shell)
            .filter(|shell| shell.is_absolute());
        Ok(Self { name, home, shell })
    }

    pub fn config_dir(&self, mac: &str, name: &str) -> PathBuf {
        self.user_path(
            "Library/Application Support",
            mac,
            "XDG_CONFIG_HOME",
            ".config",
            name,
        )
    }

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

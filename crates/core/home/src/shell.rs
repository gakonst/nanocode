use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    program: PathBuf,
    arguments: Vec<String>,
}

static CHOSEN: std::sync::OnceLock<Shell> = std::sync::OnceLock::new();

pub fn choose(shell: Shell) -> Result<(), Shell> {
    CHOSEN.set(shell)
}

pub fn chosen() -> Option<Shell> {
    CHOSEN.get().cloned()
}

impl Shell {
    #[must_use]
    pub const fn psh(program: PathBuf, arguments: Vec<String>) -> Self {
        Self { program, arguments }
    }

    pub fn path(&self) -> &Path {
        &self.program
    }

    pub fn warm_args(&self, login: bool) -> Vec<String> {
        let mut args = self.arguments.clone();
        args.push("--warm".to_owned());
        if login {
            args.push("-l".to_owned());
        }
        args
    }

    pub fn args(&self, script: &str, login: bool) -> Vec<String> {
        self.arguments
            .iter()
            .cloned()
            .chain([
                if login { "-lc" } else { "-c" }.to_owned(),
                script.to_owned(),
            ])
            .collect()
    }
}

pub const fn default_shell_name() -> &'static str {
    "bash"
}

#[cfg(unix)]
fn user_shell_path() -> Option<PathBuf> {
    static SHELL: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    SHELL
        .get_or_init(|| {
            nanocode_platform::unix::user_entry(nanocode_platform::unix::user_id())
                .ok()
                .flatten()
                .map(|user| user.shell)
        })
        .clone()
}

#[cfg(unix)]
pub fn login_shell() -> Option<PathBuf> {
    let runs_profiles = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, "zsh" | "bash" | "sh"))
            && path.is_file()
    };
    let fallbacks: &[&str] = if cfg!(target_os = "macos") {
        &["/bin/zsh", "/bin/bash", "/bin/sh"]
    } else {
        &["/bin/bash", "/usr/bin/bash", "/bin/zsh", "/bin/sh"]
    };
    user_shell_path()
        .filter(|path| runs_profiles(path))
        .or_else(|| {
            fallbacks
                .iter()
                .map(PathBuf::from)
                .find(|path| path.is_file())
        })
}

#[cfg(unix)]
pub fn os_version() -> Option<&'static str> {
    static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    VERSION
        .get_or_init(|| {
            let system = nanocode_platform::unix::system_release().ok()?;
            Some(format!(
                "{} {}",
                system.name.to_string_lossy(),
                system.release.to_string_lossy()
            ))
        })
        .as_deref()
}

#[cfg(not(unix))]
pub const fn os_version() -> Option<&'static str> {
    None
}

#[cfg(target_os = "macos")]
pub fn warm_user_shell() {
    let _ = std::thread::Builder::new()
        .name("nanocode-user-shell".to_owned())
        .spawn(|| drop(user_shell_path()));
}

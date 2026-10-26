pub(crate) fn environment_section(workspace: &str) -> String {
    let mut lines = vec![
        "# Environment".to_owned(),
        "You have been invoked in the following environment: ".to_owned(),
    ];
    if !workspace.is_empty() {
        lines.push(format!("Primary working directory: {workspace}"));
        lines.push(format!(
            "Is a git repository: {}",
            is_git_repository(workspace)
        ));
    }
    lines.extend(host_lines());
    lines.push(format!("Today's date: {}", current_date()));
    lines.join("\n")
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeEnvironment {
    working_directory: Option<String>,
    git_repository: Option<bool>,
    platform: Option<String>,
    shell: Option<String>,
    os_version: Option<String>,
    date: Option<String>,
    project_instructions: Option<String>,
}

impl ClaudeEnvironment {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn working_directory(mut self, directory: impl Into<String>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    #[must_use]
    pub const fn git_repository(mut self, repository: bool) -> Self {
        self.git_repository = Some(repository);
        self
    }

    #[must_use]
    pub fn platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    #[must_use]
    pub fn shell(mut self, shell: impl Into<String>) -> Self {
        self.shell = Some(shell.into());
        self
    }

    #[must_use]
    pub fn os_version(mut self, version: impl Into<String>) -> Self {
        self.os_version = Some(version.into());
        self
    }

    #[must_use]
    pub fn date(mut self, date: impl Into<String>) -> Self {
        self.date = Some(date.into());
        self
    }

    #[must_use]
    pub fn project_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.project_instructions = Some(instructions.into());
        self
    }

    pub(crate) fn render(self) -> (String, Option<String>) {
        let mut lines = vec![
            "# Environment".to_owned(),
            "You have been invoked in the following environment: ".to_owned(),
        ];
        if let Some(directory) = &self.working_directory {
            lines.push(format!("Primary working directory: {directory}"));
        }
        if let Some(repository) = self.git_repository {
            lines.push(format!("Is a git repository: {repository}"));
        }
        if let Some(platform) = &self.platform {
            lines.push(format!("Platform: {platform}"));
        }
        if let Some(shell) = &self.shell {
            lines.push(format!("Shell: {shell}"));
        }
        if let Some(version) = &self.os_version {
            lines.push(format!("OS Version: {version}"));
        }
        lines.push(format!(
            "Today's date: {}",
            self.date.unwrap_or_else(current_date)
        ));
        let instructions = self.project_instructions.map(|text| {
            let heading = self.working_directory.map_or_else(
                || "# AGENTS.md instructions".to_owned(),
                |directory| format!("# AGENTS.md instructions for {directory}"),
            );
            format!("{heading}\n\n<INSTRUCTIONS>\n{text}\n</INSTRUCTIONS>")
        });
        (lines.join("\n"), instructions)
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) fn project_instructions(workspace: &str) -> Option<String> {
    if workspace.is_empty() {
        return None;
    }
    let global = nanocode_home::AgentHome::from_env()
        .ok()
        .map(|home| home.global_instructions());
    let project = nanocode_home::ProjectHome::discover(workspace).read_instructions();
    for diagnostic in global
        .iter()
        .flat_map(|global| &global.diagnostics)
        .chain(&project.diagnostics)
        .filter(|diagnostic| matches!(diagnostic.level, nanocode_home::Level::Warning))
    {
        tracing::warn!(
            path = %diagnostic.path.display(),
            message = %diagnostic.message,
            "instruction file skipped"
        );
    }
    let text = nanocode_home::combine_instructions(
        global.and_then(|global| global.combined()).as_deref(),
        project.combined().as_deref(),
    )?;
    Some(format!(
        "# AGENTS.md instructions for {workspace}\n\n<INSTRUCTIONS>\n{text}\n</INSTRUCTIONS>"
    ))
}

#[cfg(target_family = "wasm")]
pub(crate) const fn project_instructions(_workspace: &str) -> Option<String> {
    None
}

#[cfg(not(target_family = "wasm"))]
fn is_git_repository(workspace: &str) -> bool {
    std::path::Path::new(workspace)
        .ancestors()
        .any(|directory| directory.join(".git").exists())
}

#[cfg(target_family = "wasm")]
const fn is_git_repository(_workspace: &str) -> bool {
    false
}

#[cfg(not(target_family = "wasm"))]
fn host_lines() -> Vec<String> {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let mut lines = vec![format!("Platform: {platform}")];
    if let Some(shell) = std::env::var_os("SHELL")
        .map(std::path::PathBuf::from)
        .and_then(|shell| {
            shell
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
    {
        lines.push(format!("Shell: {shell}"));
    }
    if let Some(version) = nanocode_home::shell::os_version().filter(|version| !version.is_empty())
    {
        lines.push(format!("OS Version: {version}"));
    }
    lines.extend(session_store_line());
    lines
}

#[cfg(not(target_family = "wasm"))]
fn session_store_line() -> Option<String> {
    let home = nanocode_home::AgentHome::from_env().ok()?;
    let store = home.nanocode_home();
    store.join("sessions.jsonl").is_file().then(|| {
        format!(
            "Nanocode sessions: {} (list them with `nc sessions --all`; print one's conversation with `nc sessions <ID>`)",
            store.display()
        )
    })
}

#[cfg(target_family = "wasm")]
const fn host_lines() -> Vec<String> {
    Vec::new()
}

#[cfg(not(target_family = "wasm"))]
fn current_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(target_family = "wasm")]
fn current_date() -> String {
    use chrono::Datelike;
    let date = chrono::DateTime::<chrono::Utc>::from(nanocode_platform::system_time()).date_naive();
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

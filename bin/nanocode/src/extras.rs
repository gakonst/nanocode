use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use nanocode::agent::input::{Prompt, UserInput};
use nanocode_tui::{
    file_finder::FileFinder,
    picker::{Picker, PickerItem},
    theme::ThemeMode,
    transcript::{CellKind, NoticeLevel, ToolBody, ToolCell, ToolStatus},
};

use crate::app::{App, AppEvent, Overlay, PickerPurpose};

pub const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

pub struct Finder {
    pub start: usize,
    pub finder: FileFinder,
}

pub fn prompt_for(text: &str, attachments: &[PathBuf]) -> Prompt {
    if attachments.is_empty() {
        return Prompt::new(text);
    }
    Prompt::content(
        std::iter::once(UserInput::Text {
            text: text.to_owned(),
        })
        .chain(attachments.iter().map(|path| UserInput::LocalImage {
            path: path.clone(),
            detail: None,
        })),
    )
}

pub fn image_path(text: &str, workspace: &Path) -> Option<PathBuf> {
    let trimmed = text.trim().trim_matches(['\'', '"']);
    if trimmed.contains('\n') || trimmed.is_empty() {
        return None;
    }
    let unescaped = trimmed.replace("\\ ", " ");
    let path = PathBuf::from(&unescaped);
    let path = if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    };
    let extension = path.extension()?.to_str()?.to_lowercase();
    (IMAGE_EXTENSIONS.contains(&extension.as_str()) && path.is_file()).then_some(path)
}

pub fn open_url(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        tokio::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = tokio::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        tokio::process::Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

fn workspace_files(workspace: &Path) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("could not run git ls-files: {error}"))?;
    if output.status.success() && !output.stdout.is_empty() {
        return Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect());
    }
    let mut files = Vec::new();
    let mut pending = vec![workspace.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("could not read {}: {error}", directory.display()))?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(path),
                Ok(kind) if kind.is_file() => {
                    if let Ok(relative) = path.strip_prefix(workspace) {
                        files.push(relative.to_string_lossy().into_owned());
                    }
                }
                _ => {}
            }
        }
    }
    files.sort();
    Ok(files)
}

fn clipboard_image(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "set png to (the clipboard as «class PNGf»)\nset f to open for access POSIX file \"{}\" with write permission\nset eof f to 0\nwrite png to f\nclose access f",
            path.display()
        );
        let output = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|error| format!("could not run osascript: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        drop(std::fs::remove_file(path));
        Err("the clipboard does not contain an image".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let attempts: [(&str, &[&str]); 2] = [
            ("wl-paste", &["--type", "image/png"]),
            (
                "xclip",
                &["-selection", "clipboard", "-t", "image/png", "-o"],
            ),
        ];
        let mut errors = Vec::new();
        for (program, args) in attempts {
            match std::process::Command::new(program).args(args).output() {
                Ok(output) if output.status.success() && !output.stdout.is_empty() => {
                    return std::fs::write(path, output.stdout)
                        .map_err(|error| format!("could not write {}: {error}", path.display()));
                }
                Ok(_) => errors.push(format!("{program} found no image")),
                Err(error) => errors.push(format!("{program}: {error}")),
            }
        }
        Err(errors.join("; "))
    }
}

impl App {
    pub(crate) fn update_finder(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "update_finder", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some((start, query)) = self.composer.mention('@') else {
            self.finder = None;
            self.finder_dismissed = None;
            return;
        };
        if self.finder_dismissed == Some(start) {
            return;
        }
        let query = query.to_owned();
        match &self.workspace_files {
            Some(files) => {
                let finder = self.finder.get_or_insert_with(|| Finder {
                    start,
                    finder: FileFinder::new(files.clone()),
                });
                finder.start = start;
                finder.finder.set_query(&query);
            }
            None if !self.files_loading => {
                self.files_loading = true;
                let workspace = self.host.config.workspace.clone();
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    drop(tx.send(AppEvent::Files(workspace_files(&workspace))));
                });
            }
            None => {}
        }
    }

    pub(crate) fn files_loaded(&mut self, files: Result<Vec<String>, String>) {
        self.files_loading = false;
        match files {
            Ok(files) => {
                self.workspace_files = Some(files);
                self.update_finder();
            }
            Err(error) => self.notice(NoticeLevel::Error, format!("file finder: {error}")),
        }
    }

    pub(crate) fn accept_finder(&mut self) -> bool {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "accept_finder", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let Some(finder) = self.finder.take() else {
            return false;
        };
        match finder.finder.selected() {
            Some(path) => {
                let replacement = format!("@{path} ");
                self.composer
                    .replace_before_cursor(finder.start, &replacement);
                true
            }
            None => {
                self.finder = Some(finder);
                false
            }
        }
    }

    pub(crate) fn dismiss_finder(&mut self) {
        if let Some(finder) = self.finder.take() {
            self.finder_dismissed = Some(finder.start);
        }
    }

    pub(crate) fn run_shell(&mut self, command: String) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "run_shell", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let mut cell = ToolCell::new(self.local_id_pub("shell"), "shell");
        cell.title = command.clone();
        let index = self.transcript.push(CellKind::Tool(cell));
        self.transcript.scroll_to_bottom();
        let workspace = self.host.config.workspace.clone();
        let tx = self.tx.clone();
        let started = std::time::Instant::now();
        let span = tracing::info_span!(target: "nanocode", "nc.shell", command.bytes = command.len(), session.id = self.agent.as_ref().map(|agent| agent.session_id()), exit.code = tracing::field::Empty, otel.status_code = tracing::field::Empty);
        tokio::spawn(tracing::Instrument::instrument(
            async move {
                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .current_dir(&workspace)
                    .stdin(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .output()
                    .await;
                match &output {
                    Ok(output) => {
                        tracing::Span::current().record("exit.code", output.status.code());
                        if !output.status.success() {
                            tracing::Span::current().record("otel.status_code", "ERROR");
                        }
                    }
                    Err(_) => {
                        tracing::Span::current().record("otel.status_code", "ERROR");
                    }
                }
                let result = output
                    .map(|output| {
                        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        if !stderr.trim().is_empty() {
                            if !text.is_empty() && !text.ends_with('\n') {
                                text.push('\n');
                            }
                            text.push_str(&stderr);
                        }
                        (output.status.code(), text)
                    })
                    .map_err(|error| error.to_string());
                drop(tx.send(AppEvent::ShellDone {
                    index,
                    result,
                    duration: started.elapsed(),
                }));
            },
            span,
        ));
    }

    pub(crate) fn shell_done(
        &mut self,
        index: usize,
        result: Result<(Option<i32>, String), String>,
        duration: Duration,
    ) {
        self.transcript.update(index, |cell| {
            if let CellKind::Tool(tool) = cell {
                tool.duration = Some(duration);
                tool.output_body = ToolBody::Text;
                match result {
                    Ok((Some(0), output)) => {
                        tool.status = ToolStatus::Completed;
                        tool.output = output;
                    }
                    Ok((code, output)) => {
                        tool.status = ToolStatus::Failed;
                        let code =
                            code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
                        tool.output = format!("{output}\nexit {code}");
                    }
                    Err(error) => {
                        tool.status = ToolStatus::Failed;
                        tool.output = error;
                    }
                }
            }
        });
    }

    pub(crate) fn paste_image(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "paste_image", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let directory = self.home.join("images");
        let path = directory.join(format!("{}.png", nanocode::platform::uuid_v7()));
        let tx = self.tx.clone();
        self.flash("reading the clipboard…");
        tokio::task::spawn_blocking(move || {
            let result = std::fs::create_dir_all(&directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))
                .and_then(|()| clipboard_image(&path))
                .map(|()| path);
            drop(tx.send(AppEvent::ImagePasted(result)));
        });
    }

    pub(crate) fn attach(&mut self, path: PathBuf) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "attach", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        self.flash(format!("attached {}", path.display()));
        self.attachments.push(path);
    }

    pub(crate) fn open_theme_picker(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_theme_picker", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let current = self.theme.mode();
        let items = [ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light]
            .into_iter()
            .map(|mode| {
                PickerItem::choice(theme_name(mode))
                    .detail(match mode {
                        ThemeMode::Auto => "follow the terminal (dark when unknown)",
                        ThemeMode::Dark => "dark palette",
                        ThemeMode::Light => "light palette",
                    })
                    .current(mode == current)
            })
            .collect();
        self.overlay = Some(Overlay::Picker(
            PickerPurpose::Theme,
            Picker::new("Theme", items).footer("enter select · esc cancel"),
        ));
    }

    pub(crate) fn set_theme(&mut self, name: &str) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "set_theme", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let mode = match name {
            "auto" => ThemeMode::Auto,
            "dark" => ThemeMode::Dark,
            "light" => ThemeMode::Light,
            other => {
                self.notice(
                    NoticeLevel::Error,
                    format!("unknown theme {other:?}; use auto, dark or light"),
                );
                return;
            }
        };
        self.theme.set_mode(mode);
        self.transcript.invalidate();
        self.flash(format!("theme {name}"));
    }

    pub(crate) fn copy_last_reply(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "copy_last_reply", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let transcript = if self.panes.side_active() {
            self.active_transcript()
        } else {
            &mut self.transcript
        };
        let reply = (0..transcript.len())
            .rev()
            .find_map(|index| match transcript.get(index) {
                Some(CellKind::Assistant(text)) => Some(text.clone()),
                Some(CellKind::Btw {
                    answer, done: true, ..
                }) => Some(answer.clone()),
                _ => None,
            });
        match reply {
            Some(text) => {
                self.pending_copy = Some((text, "copied the last reply to the clipboard"));
            }
            None => self.flash("no assistant reply to copy"),
        }
    }

    pub(crate) fn context_report(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "context_report", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let usage = self.mapper.session_usage();
        let last = self.mapper.last_usage();
        let session = self.session_id().unwrap_or_else(|| "not started".into());
        let cells = self.transcript.len();
        let report = format!(
            "context · session {session} · {} ({})\nlast turn: {} input tokens ({} cached), {} output\nsession total: {} turns · {} input ({} cached) · {} output · {} tool calls\ntranscript: {cells} entries · workspace {}",
            self.model,
            self.model.display_name(),
            last.input,
            last.cached,
            last.output,
            usage.turns,
            usage.input,
            usage.cached,
            usage.output,
            usage.tool_calls,
            self.host.config.workspace.display(),
        );
        self.notice(NoticeLevel::Info, report);
    }

    pub(crate) fn open_skills(&mut self) {
        let _span = tracing::info_span!(target: "nanocode", "nc.action", action.name = "open_skills", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let skills = match nanocode_home::AgentHome::from_env() {
            Ok(home) => home.skills_for(&home.project(&self.host.config.workspace)),
            Err(error) => {
                self.notice(NoticeLevel::Error, format!("skills: {error}"));
                return;
            }
        };
        if skills.skills.is_empty() {
            self.flash("no skills found for this workspace");
            return;
        }
        let names = skills
            .skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>();
        let items = skills
            .skills
            .iter()
            .map(|skill| {
                PickerItem::choice(skill.name.clone()).detail(skill.dir.display().to_string())
            })
            .collect();
        self.overlay = Some(Overlay::Picker(
            PickerPurpose::Skill(names),
            Picker::new("Skills", items)
                .filterable()
                .footer("type to filter · enter insert · esc cancel"),
        ));
    }
}

pub const fn theme_name(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Auto => "auto",
        ThemeMode::Dark => "dark",
        ThemeMode::Light => "light",
    }
}

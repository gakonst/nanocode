use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
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

const SHELL_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
const SHELL_OUTPUT_BYTES: usize = 1024 * 1024;

pub(crate) struct Shell {
    index: usize,
    started: std::time::Instant,
    pending: Vec<u8>,
    task: nanocode_executor::AbortHandle,
}

struct ShellSink {
    index: usize,
    tx: nanocode_sync::mpsc::UnboundedSender<AppEvent>,
    sent: AtomicUsize,
}

impl nanocode_process::OutputSink for ShellSink {
    fn write(&self, bytes: &[u8]) {
        let sent = self.sent.fetch_add(bytes.len(), Ordering::Relaxed);
        let room = SHELL_OUTPUT_BYTES.saturating_sub(sent).min(bytes.len());
        if room > 0 {
            drop(self.tx.send(AppEvent::ShellOutput {
                index: self.index,
                bytes: bytes[..room].to_vec(),
            }));
        }
    }
}

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
        nanocode_executor::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = nanocode_executor::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        nanocode_executor::process::Command::new("xdg-open")
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "update_finder", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
                nanocode_platform::spawn_blocking(move || {
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "accept_finder", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "run_shell", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let mut cell = ToolCell::new(self.local_id_pub("shell"), "shell");
        cell.title = command.clone();
        cell.output_body = ToolBody::Text;
        let index = self.transcript.push(CellKind::Tool(cell));
        self.transcript.scroll_to_bottom();
        let workspace = self.host.config.workspace.clone();
        let tx = self.tx.clone();
        let span = nanocode_trace::info_span!(target: "nanocode", "nc.shell", command.bytes = command.len(), session.id = self.agent.as_ref().map(|agent| agent.session_id()), exit.code = nanocode_trace::field::Empty, otel.status_code = nanocode_trace::field::Empty);
        let task = nanocode_platform::spawn_task(nanocode_trace::Instrument::instrument(
            async move {
                let sink = ShellSink {
                    index,
                    tx: tx.clone(),
                    sent: AtomicUsize::new(0),
                };
                let outcome = nanocode_process::run_bash(
                    &workspace,
                    &command,
                    SHELL_TIMEOUT,
                    SHELL_OUTPUT_BYTES,
                    SHELL_OUTPUT_BYTES,
                    None,
                    Some(&sink),
                )
                .await;
                match &outcome {
                    Ok(output) => {
                        nanocode_trace::Span::current().record("exit.code", output.exit_code);
                        if output.exit_code != 0 {
                            nanocode_trace::Span::current().record("otel.status_code", "ERROR");
                        }
                    }
                    Err(_) => {
                        nanocode_trace::Span::current().record("otel.status_code", "ERROR");
                    }
                }
                let cut = sink.sent.load(Ordering::Relaxed) > SHELL_OUTPUT_BYTES;
                drop(tx.send(AppEvent::ShellDone {
                    index,
                    result: outcome.map(|output| (output.exit_code, cut || output.truncated)),
                }));
            },
            span,
        ));
        self.shells.push(Shell {
            index,
            started: std::time::Instant::now(),
            pending: Vec::new(),
            task: task.abort_handle(),
        });
    }

    pub(crate) const fn shells_running(&self) -> bool {
        !self.shells.is_empty()
    }

    pub(crate) fn shell_output(&mut self, index: usize, bytes: &[u8]) {
        let Some(shell) = self.shells.iter_mut().find(|shell| shell.index == index) else {
            return;
        };
        shell.pending.extend_from_slice(bytes);
        let whole = match std::str::from_utf8(&shell.pending) {
            Ok(_) => shell.pending.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => shell.pending.len(),
        };
        let text = String::from_utf8_lossy(&shell.pending[..whole]).into_owned();
        shell.pending.drain(..whole);
        self.transcript.update(index, |cell| {
            if let CellKind::Tool(tool) = cell {
                tool.output.push_str(&text);
            }
        });
    }

    pub(crate) fn interrupt_shells(&mut self) {
        for shell in std::mem::take(&mut self.shells) {
            shell.task.abort();
            let duration = shell.started.elapsed();
            let rest = String::from_utf8_lossy(&shell.pending).into_owned();
            self.transcript.update(shell.index, |cell| {
                if let CellKind::Tool(tool) = cell {
                    tool.duration = Some(duration);
                    tool.status = ToolStatus::Cancelled;
                    tool.output.push_str(&rest);
                    if !tool.output.is_empty() && !tool.output.ends_with('\n') {
                        tool.output.push('\n');
                    }
                    tool.output.push_str(&format!(
                        "interrupted after {:.1}s; its process group was killed",
                        duration.as_secs_f64()
                    ));
                }
            });
        }
    }

    pub(crate) fn shell_done(&mut self, index: usize, result: Result<(i32, bool), String>) {
        let Some(position) = self.shells.iter().position(|shell| shell.index == index) else {
            return;
        };
        let shell = self.shells.swap_remove(position);
        let duration = shell.started.elapsed();
        let rest = String::from_utf8_lossy(&shell.pending).into_owned();
        self.transcript.update(index, |cell| {
            if let CellKind::Tool(tool) = cell {
                tool.duration = Some(duration);
                tool.output.push_str(&rest);
                let ending = match result {
                    Ok((0, cut)) => {
                        tool.status = ToolStatus::Completed;
                        cut.then(|| format!("output past {SHELL_OUTPUT_BYTES} bytes not shown"))
                    }
                    Ok((code, cut)) => {
                        tool.status = ToolStatus::Failed;
                        Some(if cut {
                            format!("output past {SHELL_OUTPUT_BYTES} bytes not shown\nexit {code}")
                        } else {
                            format!("exit {code}")
                        })
                    }
                    Err(error) => {
                        tool.status = ToolStatus::Failed;
                        Some(error)
                    }
                };
                if let Some(ending) = ending {
                    if !tool.output.is_empty() && !tool.output.ends_with('\n') {
                        tool.output.push('\n');
                    }
                    tool.output.push_str(&ending);
                }
            }
        });
    }

    pub(crate) fn paste_image(&mut self) {
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "paste_image", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        let directory = self.home.join("images");
        let path = directory.join(format!("{}.png", nanocode::platform::uuid_v7()));
        let tx = self.tx.clone();
        self.flash("reading the clipboard…");
        nanocode_platform::spawn_blocking(move || {
            let result = std::fs::create_dir_all(&directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))
                .and_then(|()| clipboard_image(&path))
                .map(|()| path);
            drop(tx.send(AppEvent::ImagePasted(result)));
        });
    }

    pub(crate) fn attach(&mut self, path: PathBuf) {
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "attach", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
        self.flash(format!("attached {}", path.display()));
        self.attachments.push(path);
    }

    pub(crate) fn open_theme_picker(&mut self) {
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "open_theme_picker", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "set_theme", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "copy_last_reply", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "context_report", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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
        let _span = nanocode_trace::info_span!(target: "nanocode", "nc.action", action.name = "open_skills", session.id = self.agent.as_ref().map(|agent| agent.session_id())).entered();
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

use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
};

use nanocode_process_contracts::FileHost;
use nanocode_tools::ToolDefinition;
use serde_json::json;

use super::{StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolResult};

mod parser;
mod seek_sequence;
mod streaming_parser;

use parser::{Hunk, UpdateFileChunk, parse_patch};

#[cfg(not(target_family = "wasm"))]
pub(crate) struct ApplyPatchHandler {
    workspace: PathBuf,
}

#[cfg(not(target_family = "wasm"))]
impl ApplyPatchHandler {
    pub(crate) const fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Tool for ApplyPatchHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::ApplyPatch.definition()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input = input.into_freeform()?;
                let workspace = self.workspace.clone();
                Ok(
                    match tokio::task::spawn_blocking(move || apply(&input, &workspace)).await {
                        Ok(Ok(output)) => {
                            ToolOutput::text(output).with_structured_result(json!({}))
                        }
                        Ok(Err(error)) => {
                            ToolOutput::error(format!("apply_patch verification failed: {error}"))
                        }
                        Err(error) => {
                            ToolOutput::error(format!("apply_patch task failed: {error}"))
                        }
                    },
                )
            };
            output
        })
    }
}

pub(crate) struct HostApplyPatchHandler {
    workspace: PathBuf,
    files: std::sync::Arc<dyn FileHost>,
}

impl HostApplyPatchHandler {
    pub(crate) const fn new(workspace: PathBuf, files: std::sync::Arc<dyn FileHost>) -> Self {
        Self { workspace, files }
    }

    async fn apply(&self, patch: &str) -> Result<String, String> {
        let hunks = parse(patch)?;
        validate_unique_targets(&hunks, |path| {
            normalize_path(&resolve(&self.workspace, path))
        })?;
        let mut initial_files = HashMap::new();
        for path in required_files_from_hunks(&hunks) {
            let source = resolve(&self.workspace, &path);
            let contents = self
                .files
                .read(&source)
                .await
                .and_then(|contents| contents.ok_or_else(|| "No such file or directory".into()))
                .map_err(|error| {
                    format!(
                        "Failed to read file to update {}: {error}",
                        source.display()
                    )
                })?;
            initial_files.insert(path, contents);
        }
        let plan = plan_hunks(hunks, &initial_files)?;
        for operation in plan.operations() {
            match operation {
                PatchOperation::Write { path, contents } => {
                    let target = resolve(&self.workspace, path);
                    self.files.write(&target, contents).await.map_err(|error| {
                        format!("Failed to write file {}: {error}", target.display())
                    })?;
                }
                PatchOperation::Delete { path } => {
                    let target = resolve(&self.workspace, path);
                    self.files.remove(&target).await.map_err(|error| {
                        format!("Failed to delete file {}: {error}", target.display())
                    })?;
                }
            }
        }
        Ok(plan.summary().to_owned())
    }
}

impl Tool for HostApplyPatchHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::ApplyPatch.definition()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> nanocode_tools::ToolCallFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input = input.into_freeform()?;
                Ok(match self.apply(&input).await {
                    Ok(output) => ToolOutput::text(output).with_structured_result(json!({})),
                    Err(error) => {
                        ToolOutput::error(format!("apply_patch verification failed: {error}"))
                    }
                })
            };
            output
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchOperation {
    Write { path: PathBuf, contents: String },

    Delete { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchPlan {
    operations: Vec<PatchOperation>,
    summary: String,
}

impl PatchPlan {
    #[must_use]
    pub fn operations(&self) -> &[PatchOperation] {
        &self.operations
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

pub fn required_files(patch: &str) -> Result<Vec<PathBuf>, String> {
    let hunks = parse(patch)?;
    Ok(required_files_from_hunks(&hunks))
}

fn required_files_from_hunks(hunks: &[Hunk]) -> Vec<PathBuf> {
    let mut produced = HashSet::new();
    let mut required = Vec::new();
    let mut retained = HashSet::new();
    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, .. } => {
                produced.insert(path.clone());
            }
            Hunk::DeleteFile { path } => {
                produced.remove(path);
            }
            Hunk::UpdateFile {
                path, move_path, ..
            } => {
                if !produced.contains(path) && retained.insert(path.clone()) {
                    required.push(path.clone());
                }
                produced.remove(path);
                produced.insert(move_path.as_ref().unwrap_or(path).clone());
            }
        }
    }
    required
}

pub fn plan(patch: &str, initial_files: &HashMap<PathBuf, String>) -> Result<PatchPlan, String> {
    let hunks = parse(patch)?;
    plan_hunks(hunks, initial_files)
}

fn plan_hunks(
    hunks: Vec<Hunk>,
    initial_files: &HashMap<PathBuf, String>,
) -> Result<PatchPlan, String> {
    let mut files = initial_files.clone();
    let mut operations = Vec::new();
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for hunk in hunks {
        match hunk {
            Hunk::AddFile { path, contents } => {
                files.insert(path.clone(), contents.clone());
                operations.push(PatchOperation::Write {
                    path: path.clone(),
                    contents,
                });
                added.push(path);
            }
            Hunk::DeleteFile { path } => {
                files.remove(&path);
                operations.push(PatchOperation::Delete { path: path.clone() });
                deleted.push(path);
            }
            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let original = files
                    .get(&path)
                    .ok_or_else(|| format!("Failed to read file to update {}", path.display()))?;
                let updated = apply_chunks(original, &chunks, &path)?;
                if let Some(destination) = move_path {
                    files.remove(&path);
                    files.insert(destination.clone(), updated.clone());
                    operations.push(PatchOperation::Write {
                        path: destination.clone(),
                        contents: updated,
                    });
                    operations.push(PatchOperation::Delete { path });
                    modified.push(destination);
                } else {
                    files.insert(path.clone(), updated.clone());
                    operations.push(PatchOperation::Write {
                        path: path.clone(),
                        contents: updated,
                    });
                    modified.push(path);
                }
            }
        }
    }

    let mut summary = String::from("Success. Updated the following files:\n");
    for path in &added {
        push_summary_line(&mut summary, 'A', path);
    }
    for path in &modified {
        push_summary_line(&mut summary, 'M', path);
    }
    for path in &deleted {
        push_summary_line(&mut summary, 'D', path);
    }
    Ok(PatchPlan {
        operations,
        summary,
    })
}

#[derive(Debug, PartialEq)]
struct ApplyPatchArgs {
    patch: String,
    hunks: Vec<Hunk>,
    workdir: Option<String>,
    environment_id: Option<String>,
}

#[cfg(not(target_family = "wasm"))]
pub(super) fn apply(patch: &str, workspace: &Path) -> Result<String, String> {
    let hunks = parse(patch)?;
    validate_unique_targets(&hunks, |path| normalize_path(&resolve(workspace, path)))?;

    let mut initial_files = HashMap::new();
    for path in required_files_from_hunks(&hunks) {
        let source = resolve(workspace, &path);
        let contents = std::fs::read_to_string(&source).map_err(|error| {
            format!(
                "Failed to read file to update {}: {error}",
                source.display()
            )
        })?;
        initial_files.insert(path, contents);
    }
    let plan = plan_hunks(hunks, &initial_files)?;
    for operation in plan.operations() {
        match operation {
            PatchOperation::Write { path, contents } => {
                write_file(&resolve(workspace, path), contents.as_bytes())?;
            }
            PatchOperation::Delete { path } => {
                let target = resolve(workspace, path);
                std::fs::remove_file(&target).map_err(|error| {
                    format!("Failed to delete file {}: {error}", target.display())
                })?;
            }
        }
    }
    Ok(plan.summary().to_owned())
}

fn parse(patch: &str) -> Result<Vec<Hunk>, String> {
    let ApplyPatchArgs {
        hunks,
        patch: _,
        workdir: _,
        environment_id: _,
    } = parse_patch(patch).map_err(|error| error.to_string())?;
    if hunks.is_empty() {
        return Err("No files were modified.".to_owned());
    }
    Ok(hunks.into_iter().map(normalize_hunk_paths).collect())
}

fn validate_unique_targets(
    hunks: &[Hunk],
    mut resolve: impl FnMut(&Path) -> PathBuf,
) -> Result<(), String> {
    let mut targets = HashSet::new();
    for hunk in hunks {
        let (source, destination) = match hunk {
            Hunk::AddFile { path, .. } | Hunk::DeleteFile { path } => (path, None),
            Hunk::UpdateFile {
                path, move_path, ..
            } => (path, move_path.as_deref()),
        };

        for path in std::iter::once(source.as_path()).chain(destination) {
            let target = resolve(path);
            if !targets.insert(target.clone()) {
                return Err(format!("multiple operations target {}", target.display()));
            }
        }
    }
    Ok(())
}

fn normalize_hunk_paths(hunk: Hunk) -> Hunk {
    match hunk {
        Hunk::AddFile { path, contents } => Hunk::AddFile {
            path: normalize_path(&path),
            contents,
        },
        Hunk::DeleteFile { path } => Hunk::DeleteFile {
            path: normalize_path(&path),
        },
        Hunk::UpdateFile {
            path,
            move_path,
            chunks,
        } => Hunk::UpdateFile {
            path: normalize_path(&path),
            move_path: move_path.map(|path| normalize_path(&path)),
            chunks,
        },
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) =>
            {
                normalized.pop();
            }
            Component::ParentDir if normalized.has_root() => {}
            Component::ParentDir => normalized.push(component.as_os_str()),
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

fn apply_chunks(original: &str, chunks: &[UpdateFileChunk], path: &Path) -> Result<String, String> {
    let mut original_lines = original
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let mut replacements = Vec::new();
    let mut line_index = 0;
    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            let context = [context.clone()];
            let found = seek_sequence::seek_sequence(&original_lines, &context, line_index, false)
                .ok_or_else(|| {
                    format!(
                        "Failed to find context '{}' in {}",
                        context[0],
                        path.display()
                    )
                })?;
            line_index = found + 1;
        }

        if chunk.old_lines.is_empty() {
            let insertion_index = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_index, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut old_lines = chunk.old_lines.as_slice();
        let mut new_lines = chunk.new_lines.as_slice();
        let mut found = seek_sequence::seek_sequence(
            &original_lines,
            old_lines,
            line_index,
            chunk.is_end_of_file,
        );
        if found.is_none() && old_lines.last().is_some_and(String::is_empty) {
            old_lines = &old_lines[..old_lines.len() - 1];
            if new_lines.last().is_some_and(String::is_empty) {
                new_lines = &new_lines[..new_lines.len() - 1];
            }
            found = seek_sequence::seek_sequence(
                &original_lines,
                old_lines,
                line_index,
                chunk.is_end_of_file,
            );
        }
        let found = found.ok_or_else(|| {
            format!(
                "Failed to find expected lines in {}:\n{}",
                path.display(),
                chunk.old_lines.join("\n")
            )
        })?;
        replacements.push((found, old_lines.len(), new_lines.to_vec()));
        line_index = found + old_lines.len();
    }

    replacements.sort_by_key(|(start, _, _)| *start);
    for (start, old_len, new_lines) in replacements.into_iter().rev() {
        original_lines.splice(start..start + old_len, new_lines);
    }
    if !original_lines.last().is_some_and(String::is_empty) {
        original_lines.push(String::new());
    }
    Ok(original_lines.join("\n"))
}

fn resolve(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

#[cfg(not(target_family = "wasm"))]
fn write_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create parent directories for {}: {error}",
                path.display()
            )
        })?;
    }
    std::fs::write(path, contents)
        .map_err(|error| format!("Failed to write file {}: {error}", path.display()))
}

fn push_summary_line(summary: &mut String, operation: char, path: &Path) {
    summary.push(operation);
    summary.push(' ');
    summary.push_str(&path.to_string_lossy());
    summary.push('\n');
}

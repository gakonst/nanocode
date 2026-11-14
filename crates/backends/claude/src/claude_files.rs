use std::{path::PathBuf, sync::Arc};

use crate::tools::{ClaudeWorkspaceFiles, ImageSource, ToolContent, ToolResultBlock};
use nanocode_agent_core::input::ImageDetail;
use nanocode_tool_host::{
    SessionEnvironment, Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
    Tools, ToolsBuildError, contract::ToolOutputContent,
};
use serde_json::Value;

pub(super) fn selection(
    files: &Arc<ClaudeWorkspaceFiles>,
    workspace: PathBuf,
    cwd: Arc<nanocode_sync::Atom<PathBuf>>,
    session: Option<SessionEnvironment>,
    jobs: Arc<super::claude_bash::Jobs>,
    executor: Arc<dyn nanocode_process_contracts::ForegroundExecutor>,
) -> Result<Tools, ToolsBuildError> {
    let mut tools = Tools::builder().without_defaults();
    for schema in ClaudeWorkspaceFiles::definitions() {
        let name = schema["name"].as_str().unwrap_or_default().to_owned();
        let description = schema["description"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        tools = tools.tool(ClaudeFileTool {
            definition: ToolDefinition::function(
                name.clone(),
                description,
                schema["input_schema"].clone(),
            )
            .with_output_schema(super::claude_output::schema(&name)),
            name,
            files: Arc::clone(files),
            session: session.clone(),
        });
    }
    let notices = Arc::new(super::claude_bash::JobNotices(Arc::clone(&jobs)));
    super::claude_bash::install(tools, workspace, cwd, session, jobs, executor)
        .build()
        .map(|tools| {
            tools
                .with_code_mode_policy(crate::code_mode_policy::claude_tool_policy())
                .with_code_mode_policy(notices)
        })
}

struct ClaudeFileTool {
    name: String,
    definition: ToolDefinition,
    files: Arc<ClaudeWorkspaceFiles>,
    session: Option<SessionEnvironment>,
}

impl Tool for ClaudeFileTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolResult> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let input: Value = input.decode_json()?;
                let output = match self
                    .files
                    .as_ref()
                    .clone()
                    .with_read_token_budget(context.output_token_budget())
                    .execute_output_in_session(&self.name, input, self.session.clone())
                    .await
                {
                    Ok(output) => output,
                    Err(error) => {
                        return Ok(ToolOutput::error(format!(
                            "<tool_use_error>{error}</tool_use_error>"
                        )));
                    }
                };
                let result = match output.content {
                    ToolContent::Text(text) if output.is_error => ToolOutput::error(text),
                    ToolContent::Text(text) => ToolOutput::text(text),
                    ToolContent::Blocks(blocks) => {
                        super::claude_output::media(blocks.into_iter().map(item).collect())
                    }
                };
                Ok(match output.metadata {
                    Some(metadata) => result.with_metadata(metadata),
                    None => result,
                })
            };
            output
        })
    }
}

fn item(block: ToolResultBlock) -> ToolOutputContent {
    match block {
        ToolResultBlock::Text { text } => ToolOutputContent::InputText { text },
        ToolResultBlock::Image {
            source: ImageSource::Base64 { media_type, data },
        } => ToolOutputContent::InputImage {
            image_url: format!("data:{media_type};base64,{data}"),
            detail: ImageDetail::Auto,
        },
        ToolResultBlock::Image {
            source: ImageSource::Url { url },
        } => ToolOutputContent::InputImage {
            image_url: url,
            detail: ImageDetail::Auto,
        },
        ToolResultBlock::Document { file_data } => ToolOutputContent::InputFile { file_data },
        ToolResultBlock::UnsupportedMedia { media_type } => ToolOutputContent::InputText {
            text: format!("[unsupported media omitted: {media_type}]"),
        },
    }
}

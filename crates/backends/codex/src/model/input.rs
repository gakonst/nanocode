use crate::builtin_tools::StandardTool;
use crate::client::{
    ModelConfig, Prompt, PromptMessageRole,
    responses::{
        ContentItem, FunctionOutputBody, FunctionOutputContent, MessageRole, ResponseItem,
    },
};
use nanocode_code_mode::contract::{ToolOutputBody, ToolOutputContent};
use nanocode_tool_host::Tools;
use serde_json::Value;

use super::context::ContextSnapshot;

pub(in crate::model) fn task_input(
    config: &ModelConfig,
    tools: &Tools,
    prompt: &Prompt,
    user_content: Vec<ContentItem>,
    context: &ContextSnapshot,
) -> Vec<ResponseItem> {
    let mut input = vec![developer_context(config, tools), context.full_item()];
    input.extend(prompt_messages(prompt, user_content));
    input
}

pub(in crate::model) fn prompt_messages(
    prompt: &Prompt,
    user_content: Vec<ContentItem>,
) -> Vec<ResponseItem> {
    let mut input = Vec::with_capacity(prompt.transcript().len() + 1);
    input.extend(prompt.transcript().iter().map(|message| {
        let role = match message.role() {
            PromptMessageRole::User => MessageRole::User,
            PromptMessageRole::Assistant => MessageRole::Assistant,
        };
        let content = match message.role() {
            PromptMessageRole::User => ContentItem::input_text(message.content()),
            PromptMessageRole::Assistant => ContentItem::output_text(message.content()),
        };
        ResponseItem::message(role, [content])
    }));
    input.push(ResponseItem::message(MessageRole::User, user_content));
    input
}

pub(in crate::model) fn turn_aborted() -> ResponseItem {
    ResponseItem::message(
        MessageRole::User,
        [ContentItem::InputText {
            text: concat!(
                "<turn_aborted>\n",
                "The user interrupted the previous turn on purpose. Any running unified exec ",
                "processes may still be running in the background. If any tools/commands were ",
                "aborted, they may have partially executed.\n",
                "</turn_aborted>"
            )
            .into(),
        }],
    )
}

pub(in crate::model) fn empty_response() -> ResponseItem {
    ResponseItem::message(
        MessageRole::User,
        [ContentItem::InputText {
            text: concat!(
                "<empty_response>\n",
                "Your last response had no message and no exec call, so nothing happened. ",
                "Continue the task: call exec, or reply with your final message.\n",
                "</empty_response>"
            )
            .into(),
        }],
    )
}

pub(in crate::model) fn developer_context(config: &ModelConfig, tools: &Tools) -> ResponseItem {
    ResponseItem::message(
        MessageRole::Developer,
        [ContentItem::InputText {
            text: format!(
                "{}\n\n{}{}",
                permissions_instructions(),
                code_mode_instructions(tools),
                config.host_instructions()
            )
            .into(),
        }],
    )
}

fn code_mode_instructions(tools: &Tools) -> String {
    if !tools.exposure().is_available_in_code_mode() {
        return String::new();
    }
    let exec_command = StandardTool::ExecCommand.name();
    let write_stdin = StandardTool::WriteStdin.name();
    format!(
        "<code_mode_orchestration>\nUse code mode to orchestrate related tool calls when the next calls can be determined from tool results without additional model judgment or user input. Keep the complete lifecycle in one code-mode program: use `Promise.all` for independent calls, and use loops and conditionals for dependent calls. When `{exec_command}` is available and returns a `session_id`, continue calling `{write_stdin}` in that program until the process exits. If the outer code-mode cell yields, wait on that cell with its `cell_id`; do not restart its work or move nested process polling into separate model turns. Complete or terminate yielded cells before finishing your turn. Return only the results needed for the next reasoning step. Use separate code-mode calls when an intermediate result requires model judgment, user input, or a progress update.\n</code_mode_orchestration>\n\n"
    )
}

pub(in crate::model) fn host_context(instructions: String) -> ResponseItem {
    ResponseItem::message(
        MessageRole::Developer,
        [ContentItem::input_text(instructions)],
    )
}

fn permissions_instructions() -> String {
    #[cfg(not(target_family = "wasm"))]
    {
        let sandbox = include_str!("prompts/danger_full_access.md")
            .replace("{{ network_access }}", "enabled");
        format!(
            "<permissions instructions>\n{sandbox}{}</permissions instructions>",
            include_str!("prompts/never.md"),
        )
    }
    #[cfg(target_family = "wasm")]
    {
        "<host_execution_context>\nExecution permissions are supplied and enforced by the host for each tool and selected environment. The embedded runtime does not grant filesystem, network, or escalation access.\n</host_execution_context>".to_owned()
    }
}

pub(in crate::model) fn custom_tool_output(
    call_id: String,
    output: ToolOutputBody,
) -> ResponseItem {
    ResponseItem::custom_tool_output(call_id, None, function_output(output))
}

pub(in crate::model) fn custom_tool_notification(call_id: String, text: String) -> ResponseItem {
    ResponseItem::custom_tool_output(
        call_id,
        Some("exec".to_owned()),
        FunctionOutputBody::Text(text.into_boxed_str()),
    )
}

pub(in crate::model) fn function_tool_output(
    call_id: String,
    output: ToolOutputBody,
) -> ResponseItem {
    ResponseItem::function_call_output(call_id, function_output(output))
}

pub(in crate::model) fn tool_search_output(call_id: String, tools: Vec<Value>) -> ResponseItem {
    ResponseItem::ToolSearchOutput {
        id: None,
        call_id: Some(call_id.into_boxed_str()),
        status: "completed".into(),
        execution: "client".into(),
        tools: tools.into_iter().map(Into::into).collect(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_output(output: ToolOutputBody) -> FunctionOutputBody {
    match output {
        ToolOutputBody::Text(text) => FunctionOutputBody::Text(text.into_boxed_str()),
        ToolOutputBody::Content(content) => FunctionOutputBody::Content(
            content
                .into_iter()
                .map(|item| match item {
                    ToolOutputContent::InputText { text } => FunctionOutputContent::InputText {
                        text: text.into_boxed_str(),
                    },
                    ToolOutputContent::InputImage {
                        image_url,
                        detail: _,
                    } => FunctionOutputContent::InputImage {
                        image_url: image_url.into_boxed_str(),
                        detail: None,
                    },
                    ToolOutputContent::InputImageFile { file_id, detail: _ } => {
                        FunctionOutputContent::InputImageFile {
                            file_id: file_id.into_boxed_str(),
                            detail: None,
                        }
                    }
                    ToolOutputContent::InputAudio { audio_url } => {
                        FunctionOutputContent::InputAudio {
                            audio_url: audio_url.into_boxed_str(),
                        }
                    }
                    ToolOutputContent::EncryptedContent { encrypted_content } => {
                        FunctionOutputContent::EncryptedContent {
                            encrypted_content: encrypted_content.into_boxed_str(),
                        }
                    }
                })
                .collect(),
        ),
    }
}

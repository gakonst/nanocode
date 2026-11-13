use nanocode_tool_host::{ToolOutput, contract::ToolOutputContent};
use serde_json::{Value, json};

pub(super) fn media(content: Vec<ToolOutputContent>) -> ToolOutput {
    let value = json!({ "content": &content });
    ToolOutput::content(content).with_structured_result(value)
}

pub(super) fn schema(name: &str) -> Value {
    match name {
        "Read" => read(json!({"type": "array", "items": {"oneOf": [
            {"type": "object", "properties": {
                "type": {"const": "input_text"}, "text": {"type": "string"}
            }, "required": ["type", "text"], "additionalProperties": false},
            image(),
            {"type": "object", "description": "a whole PDF, which the exec result shows you as a document", "properties": {
                "type": {"const": "input_file"}, "file_data": {"type": "string"}
            }, "required": ["type", "file_data"], "additionalProperties": false}
        ]}})),
        "Write" => json!({"type": "string", "description": "Wrote {shown_path}"}),
        "Edit" => {
            json!({"type": "string", "description": "Edited {shown_path} ({count} replacement(s))"})
        }
        "Bash" => bash(
            "the last line says so in brackets and names `output_file`, which holds all of it.",
            json!({"type": ["string", "null"], "description": "The file with everything the command printed, stdout and stderr together, when `truncated` or `background` is true: read it in parts with another command."}),
        ),
        "TaskStop" => json!({"type": "object", "properties": {
            "message": {"type": "string"}, "task_id": {"type": "string"},
            "task_type": {"enum": ["local_bash", "local_agent"]}, "command": {"type": "string"}
        }, "required": ["message", "task_id", "task_type", "command"], "additionalProperties": false}),
        _ => unreachable!("no Claude output schema for {name}"),
    }
}

pub(super) fn host_schema(name: &str) -> Value {
    match name {
        "Read" => read(json!({"type": "array", "items": image(), "minItems": 1, "maxItems": 1})),
        "Bash" => bash(
            "the last line says so in brackets.",
            json!({"type": "null"}),
        ),
        "TaskOutput" => task_output(),
        _ => schema(name),
    }
}

fn read(blocks: Value) -> Value {
    json!({
        "description": "Rendered file text, or media as a tool result's content blocks. Text reads are strings, not FileReadOutput objects; pass an image block to image(), as in image(result.content[0]).",
        "oneOf": [
            {"type": "string", "description": "line-numbered text or system reminder"},
            {"type": "object", "properties": {"content": blocks}, "required": ["content"], "additionalProperties": false}
        ]
    })
}

fn image() -> Value {
    json!({"type": "object", "properties": {
        "type": {"const": "input_image"}, "image_url": {"type": "string"}, "detail": {"const": "auto"}
    }, "required": ["type", "image_url", "detail"], "additionalProperties": false})
}

fn bash(cut: &str, output_file: Value) -> Value {
    let stdout = format!(
        "At most {} bytes of what the command printed: when it printed more, or is still running in the background, {cut}",
        crate::tools::bash::NESTED_STREAM_BYTES
    );
    json!({"type": "object", "properties": {
        "stdout": {"type": "string", "description": stdout},
        "stderr": {"type": "string"},
        "exit_code": {"type": ["integer", "null"], "description": "null while the command is still running in the background."},
        "interrupted": {"const": false},
        "truncated": {"type": "boolean"},
        "background": {"type": "boolean", "description": "true when the command passed its timeout or was started with run_in_background, and is still running."},
        "status": {"enum": ["completed", "running"]},
        "task_id": {"type": ["string", "null"]},
        "output_file": output_file,
        "output_bytes": {"type": ["integer", "null"], "description": "The size of `output_file` when `truncated` is true."},
        "message": {"type": ["string", "null"], "description": "What a background command's `stdout` says, or that the call's `timeout` was over the maximum and was reduced to it."}
    }, "required": ["stdout", "stderr", "exit_code", "interrupted", "truncated", "background", "status", "task_id", "output_file", "output_bytes", "message"], "additionalProperties": false})
}

fn task_output() -> Value {
    json!({
        "description": "Nano SDK extension for session-owned Bash tasks. Task outcomes are returned as values; invalid tool calls reject.",
        "type": "object", "properties": {
            "task_id": {"type": "string"},
            "status": {"enum": ["running", "completed", "failed", "killed"]},
            "stdout": {"type": "string", "description": "What the task printed, with a last line in brackets when it was cut; while it is still running, the line that says so."},
            "stderr": {"type": "string", "description": "What the task printed to stderr; for a task that failed without a result, its error."},
            "exit_code": {"type": ["integer", "null"], "description": "null while the task is running, and when it ended without a result."},
            "truncated": {"type": "boolean"},
            "error": {"type": ["string", "null"]}
        }, "required": ["task_id", "status", "stdout", "stderr", "exit_code", "truncated", "error"], "additionalProperties": false
    })
}

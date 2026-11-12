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
            image()
        ]}})),
        "Write" => json!({"type": "string", "description": "Wrote {shown_path}"}),
        "Edit" => {
            json!({"type": "string", "description": "Edited {shown_path} ({count} replacement(s))"})
        }
        "Bash" => bash(json!({"type": ["string", "null"]})),
        "TaskStop" => json!({"type": "object", "properties": {
            "message": {"type": "string"}, "task_id": {"type": "string"},
            "task_type": {"const": "local_bash"}, "command": {"type": "string"}
        }, "required": ["message", "task_id", "task_type", "command"], "additionalProperties": false}),
        _ => unreachable!("no Claude output schema for {name}"),
    }
}

pub(super) fn host_schema(name: &str) -> Value {
    match name {
        "Read" => read(json!({"type": "array", "items": image(), "minItems": 1, "maxItems": 1})),
        "Bash" => bash(json!({"type": "null"})),
        "TaskOutput" => task_output(
            json!({"enum": ["failed", "killed"]}),
            json!({"enum": ["completed", "failed", "killed"]}),
        ),
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

fn bash(output_file: Value) -> Value {
    json!({"oneOf": [
        {"type": "object", "properties": {
            "stdout": {"type": "string"}, "stderr": {"type": "string"},
            "exit_code": {"type": "integer"}, "interrupted": {"const": false}, "truncated": {"type": "boolean"}
        }, "required": ["stdout", "stderr", "exit_code", "interrupted", "truncated"], "additionalProperties": false},
        {"type": "object", "properties": {
            "task_id": {"type": "string"}, "status": {"const": "running"}, "background": {"const": true},
            "output_file": output_file, "message": {"type": "string"}
        }, "required": ["task_id", "status", "background", "output_file", "message"], "additionalProperties": false}
    ]})
}

fn task_output(error_status: Value, result_status: Value) -> Value {
    json!({
        "description": "Nano SDK extension for session-owned Bash tasks. Task outcomes are returned as values; invalid tool calls reject.",
        "oneOf": [
            {"type": "object", "properties": {
                "task_id": {"type": "string"}, "status": {"const": "running"}
            }, "required": ["task_id", "status"], "additionalProperties": false},
            {"type": "object", "properties": {
                "task_id": {"type": "string"}, "status": error_status, "error": {"type": "string"}
            }, "required": ["task_id", "status", "error"], "additionalProperties": false},
            {"type": "object", "properties": {
                "task_id": {"type": "string"}, "status": result_status,
                "stdout": {"type": "string"}, "stderr": {"type": "string"},
                "exit_code": {"type": "integer"}, "truncated": {"type": "boolean"}
            }, "required": ["task_id", "status", "stdout", "stderr", "exit_code", "truncated"], "additionalProperties": false}
        ]
    })
}

use nanocode_tool_contracts::ToolDefinition;
use nanocode_types_core::responses::CustomToolFormat;
use serde_json::json;

use crate::code_mode_description as description;

const GRAMMAR: &str = r"
start: pragma_source | plain_source
pragma_source: PRAGMA_LINE NEWLINE SOURCE
plain_source: SOURCE

PRAGMA_LINE: /[ \t]*\/\/ @exec:[^\r\n]*/
NEWLINE: /\r?\n/
SOURCE: /[\s\S]+/
";

struct ExecSpec {
    definitions: Vec<ToolDefinition>,
    provider_summaries: Vec<(String, String)>,
    flags: (bool, bool, bool),
    spec: ToolDefinition,
}

pub fn exec_spec(
    definitions: &[ToolDefinition],
    provider_summaries: &[(String, String)],
    has_deferred_tools: bool,
    code_mode_only: bool,
    compact: bool,
) -> ToolDefinition {
    static LAST: std::sync::OnceLock<nanocode_sync::Atom<Option<std::sync::Arc<ExecSpec>>>> =
        std::sync::OnceLock::new();
    let last = LAST.get_or_init(nanocode_sync::Atom::default);
    let flags = (has_deferred_tools, code_mode_only, compact);
    if let Some(spec) = last.read(|last| {
        last.as_ref()
            .filter(|last| {
                last.flags == flags
                    && last.definitions == definitions
                    && last.provider_summaries == provider_summaries
            })
            .map(|last| last.spec.clone())
    }) {
        return spec;
    }
    let spec = build_exec_spec(
        definitions,
        provider_summaries,
        has_deferred_tools,
        code_mode_only,
        compact,
    );
    last.set(Some(std::sync::Arc::new(ExecSpec {
        definitions: definitions.to_vec(),
        provider_summaries: provider_summaries.to_vec(),
        flags,
        spec: spec.clone(),
    })));
    spec
}

fn build_exec_spec(
    definitions: &[ToolDefinition],
    provider_summaries: &[(String, String)],
    has_deferred_tools: bool,
    code_mode_only: bool,
    compact: bool,
) -> ToolDefinition {
    ToolDefinition::custom(
        "exec",
        description::exec_description(
            definitions,
            provider_summaries,
            has_deferred_tools,
            code_mode_only,
            compact,
        ),
        CustomToolFormat::grammar("lark", GRAMMAR),
    )
}

pub fn wait_spec() -> ToolDefinition {
    ToolDefinition::function(
        "wait",
        "Waits on a yielded `exec` cell and returns new output or completion.\n- Use `wait` only after `exec` returns `Script running with cell ID ...`.\n- `cell_id` identifies the running `exec` cell to resume.\n- `yield_time_ms` controls how long to wait for more output before yielding again. Defaults to 10000 ms.\n- `max_tokens` limits how much new output this wait call returns. Omit for unlimited output.\n- `terminate: true` stops the running cell; false or omitted waits for output.\n- `wait` returns only the new output since the last yield, or the final completion or termination result for that cell.\n- If the cell is still running, `wait` may yield again with the same `cell_id`.\n- If the cell has already finished, `wait` returns the completed result and closes the cell.",
        json!({
            "type": "object",
            "properties": {
                "cell_id": {
                    "type": "string",
                    "description": "Identifier of the running exec cell."
                },
                "yield_time_ms": {
                    "type": "number",
                    "description": "Wait before yielding more output. Defaults to 10000 ms."
                },
                "max_tokens": {
                    "type": "number",
                    "description": "Output token budget for this wait call. Omit for unlimited output."
                },
                "terminate": {
                    "type": "boolean",
                    "description": "True stops the running exec cell; false or omitted waits for output."
                }
            },
            "required": ["cell_id"],
            "additionalProperties": false
        }),
    )
}

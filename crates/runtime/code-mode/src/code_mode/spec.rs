use nanocode_tool_contracts::ToolDefinition;
use nanocode_types_core::responses::CustomToolFormat;
use serde_json::{Value, json};

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
    layout: description::ExecLayout,
    spec: ToolDefinition,
}

pub fn exec_spec(
    definitions: &[ToolDefinition],
    provider_summaries: &[(String, String)],
    has_deferred_tools: bool,
    code_mode_only: bool,
    compact: bool,
) -> ToolDefinition {
    exec_spec_with(
        definitions,
        provider_summaries,
        has_deferred_tools,
        code_mode_only,
        compact,
        &description::ExecLayout::default(),
    )
}

pub fn exec_spec_with(
    definitions: &[ToolDefinition],
    provider_summaries: &[(String, String)],
    has_deferred_tools: bool,
    code_mode_only: bool,
    compact: bool,
    layout: &description::ExecLayout,
) -> ToolDefinition {
    static LAST: std::sync::OnceLock<nanocode_sync::Atom<Option<std::sync::Arc<ExecSpec>>>> =
        std::sync::OnceLock::new();
    let last = LAST.get_or_init(nanocode_sync::Atom::default);
    let flags = (has_deferred_tools, code_mode_only, compact);
    if let Some(spec) = last.read(|last| {
        last.as_ref()
            .filter(|last| {
                last.flags == flags
                    && last.layout == *layout
                    && last.definitions == definitions
                    && last.provider_summaries == provider_summaries
            })
            .map(|last| last.spec.clone())
    }) {
        return spec;
    }
    let spec = ToolDefinition::custom(
        "exec",
        description::exec_description_with(
            definitions,
            provider_summaries,
            has_deferred_tools,
            code_mode_only,
            compact,
            layout,
        ),
        CustomToolFormat::grammar("lark", GRAMMAR),
    );
    last.set(Some(std::sync::Arc::new(ExecSpec {
        definitions: definitions.to_vec(),
        provider_summaries: provider_summaries.to_vec(),
        flags,
        layout: layout.clone(),
        spec: spec.clone(),
    })));
    spec
}

pub const DEFAULT_YIELD: std::time::Duration = std::time::Duration::from_secs(10);

pub fn wait_spec() -> ToolDefinition {
    static SPEC: std::sync::LazyLock<ToolDefinition> =
        std::sync::LazyLock::new(|| wait_spec_yielding(DEFAULT_YIELD));
    SPEC.clone()
}

pub fn wait_spec_yielding(after: std::time::Duration) -> ToolDefinition {
    let after = after.as_millis();
    ToolDefinition::function(
            "wait",
            format!("Waits on a yielded `exec` cell and returns new output or completion.\n- Use `wait` only after `exec` returns `Script running with cell ID ...`.\n- `cell_id` identifies the running `exec` cell to resume.\n- `yield_time_ms` controls how long to wait for more output before yielding again. Defaults to {after} ms.\n- `max_tokens` limits how much new output this wait call returns. Omit for unlimited output.\n- `terminate: true` stops the running cell; false or omitted waits for output.\n- `wait` returns only the new output since the last yield, or the final completion or termination result for that cell.\n- If the cell is still running, `wait` may yield again with the same `cell_id`.\n- If the cell has already finished, `wait` returns the completed result and closes the cell."),
            json!({
                "type": "object",
                "properties": {
                    "cell_id": {
                        "type": "string",
                        "description": "Identifier of the running exec cell."
                    },
                    "yield_time_ms": {
                        "type": "number",
                        "description": format!("Wait before yielding more output. Defaults to {after} ms.")
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

pub fn exec_spec_yielding(spec: ToolDefinition, after: std::time::Duration) -> ToolDefinition {
    let (default, after) = (DEFAULT_YIELD.as_millis(), after.as_millis());
    let description = spec
        .description()
        .replace(
            &format!("Defaults to {default} ms"),
            &format!("Defaults to {after} ms"),
        )
        .replace(
            &format!("default {default} ms"),
            &format!("default {after} ms"),
        );
    spec.with_description(description)
}

pub(crate) const EXEC_PRAGMA_PREFIX: &str = "// @exec:";
pub(crate) const MAX_JS_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

pub(crate) fn integer_value(value: &Value) -> Result<u64, String> {
    let integer = match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|float| {
                    float.fract() == 0.0 && (0.0..=MAX_JS_SAFE_INTEGER as f64).contains(float)
                })
                .map(|float| float as u64)
        }),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    integer
        .filter(|integer| *integer <= MAX_JS_SAFE_INTEGER)
        .ok_or_else(|| format!("expected a non-negative safe integer, got {value}"))
}

pub fn source_with_options<'a>(
    source: &'a str,
    yield_time_ms: Option<&Value>,
    max_output_tokens: Option<&Value>,
) -> Result<std::borrow::Cow<'a, str>, String> {
    let mut pragma = serde_json::Map::new();
    for (name, value) in [
        ("yield_time_ms", yield_time_ms),
        ("max_output_tokens", max_output_tokens),
    ] {
        if let Some(value) = value.filter(|value| !value.is_null()) {
            let integer =
                integer_value(value).map_err(|error| format!("exec argument `{name}`: {error}"))?;
            pragma.insert(name.into(), integer.into());
        }
    }
    if pragma.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(source));
    }
    if source.trim_start().starts_with(EXEC_PRAGMA_PREFIX) {
        return Err(
            "exec got `yield_time_ms` or `max_output_tokens` as arguments and a `// @exec:` pragma in its source; give them one way"
                .to_owned(),
        );
    }
    Ok(std::borrow::Cow::Owned(format!(
        "{EXEC_PRAGMA_PREFIX} {}\n{source}",
        Value::Object(pragma)
    )))
}

fn yield_ms(after: std::time::Duration) -> Value {
    u64::try_from(after.as_millis()).unwrap_or(u64::MAX).into()
}

pub fn source_yielding(source: &str, after: std::time::Duration) -> std::borrow::Cow<'_, str> {
    let (first, rest) = source.split_once('\n').unwrap_or((source, ""));
    let Some(pragma) = first.trim_start().strip_prefix(EXEC_PRAGMA_PREFIX) else {
        if source.trim().is_empty() || json_wrapper_key(source).is_some() {
            return std::borrow::Cow::Borrowed(source);
        }
        return std::borrow::Cow::Owned(format!(
            "{EXEC_PRAGMA_PREFIX} {}\n{source}",
            json!({ "yield_time_ms": yield_ms(after) })
        ));
    };
    match serde_json::from_str(pragma) {
        Ok(Value::Object(mut pragma))
            if pragma.get("yield_time_ms").is_none_or(Value::is_null) =>
        {
            pragma.insert("yield_time_ms".into(), yield_ms(after));
            std::borrow::Cow::Owned(format!(
                "{EXEC_PRAGMA_PREFIX} {}\n{rest}",
                Value::Object(pragma)
            ))
        }
        _ => std::borrow::Cow::Borrowed(source),
    }
}

pub fn wait_yielding(input: &str, after: std::time::Duration) -> std::borrow::Cow<'_, str> {
    match serde_json::from_str(input) {
        Ok(Value::Object(mut arguments))
            if !arguments.contains_key("yield_time_ms")
                && arguments.get("terminate") != Some(&Value::Bool(true)) =>
        {
            arguments.insert("yield_time_ms".into(), yield_ms(after));
            std::borrow::Cow::Owned(Value::Object(arguments).to_string())
        }
        _ => std::borrow::Cow::Borrowed(input),
    }
}

const WRAPPER_KEYS: [&str; 2] = ["input", "code"];

#[must_use]
pub fn json_wrapper_key(source: &str) -> Option<&'static str> {
    let rest = source.trim_start().strip_prefix('{')?.trim_start();
    WRAPPER_KEYS.into_iter().find(|key| {
        rest.strip_prefix('"')
            .and_then(|rest| rest.strip_prefix(key))
            .and_then(|rest| rest.strip_prefix('"'))
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    })
}

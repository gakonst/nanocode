use std::{path::PathBuf, sync::Arc};

use crate::{
    ClaudeNestedToolUpdate, ClaudeToolInvocation, ClaudeToolProgress, ClaudeToolReply, ClaudeTools,
    ToolDefinition, ToolResultContent,
};
use nanocode_agent_core::{AgentHandle, NanocodeError, Result};
use nanocode_tool_host::{
    ToolContext, ToolDefinition as RuntimeDefinition, ToolInput, ToolOutput, Tools,
    contract::{ToolOutputBody, ToolOutputContent},
    embedded::{
        CodeModeExecution, CodeModeObserver, CodeModeUpdate, NestedToolCall, OwnedToolContext,
    },
    runtime::ToolRuntime,
};
use serde_json::{Value, json};

static EXEC_SCHEMA: std::sync::LazyLock<Arc<Value>> = std::sync::LazyLock::new(|| {
    Arc::new(json!({
        "type": "object",
        "properties": {"code": {"type": "string", "description": "JavaScript source to evaluate."}},
        "required": ["code"],
        "additionalProperties": false,
    }))
});

fn shared_description(name: &str, description: String) -> Arc<str> {
    static DESCRIPTIONS: std::sync::LazyLock<
        nanocode_sync::Atom<std::collections::HashMap<String, Arc<str>>>,
    > = std::sync::LazyLock::new(nanocode_sync::Atom::default);
    if let Some(shared) = DESCRIPTIONS.read(|known| {
        known
            .get(name)
            .filter(|known| ***known == *description)
            .cloned()
    }) {
        return shared;
    }
    let shared: Arc<str> = Arc::from(description);
    DESCRIPTIONS.update(|known| {
        known.insert(name.to_owned(), Arc::clone(&shared));
    });
    shared
}

struct ExecDescription {
    source: Arc<str>,
    shared: Arc<str>,
}

fn exec_description(description: &Arc<str>) -> Arc<str> {
    static LATEST: std::sync::LazyLock<nanocode_sync::Atom<Option<ExecDescription>>> =
        std::sync::LazyLock::new(nanocode_sync::Atom::default);
    if let Some(shared) = LATEST.read(|latest| {
        latest
            .as_ref()
            .filter(|latest| Arc::ptr_eq(&latest.source, description))
            .map(|latest| Arc::clone(&latest.shared))
    }) {
        return shared;
    }
    let shared = shared_description(
        "exec",
        format!("{description}\n\nPass the complete JavaScript source as the `code` string."),
    );
    LATEST.set(Some(ExecDescription {
        source: Arc::clone(description),
        shared: Arc::clone(&shared),
    }));
    shared
}

pub fn install_shared_tools(
    workspace: PathBuf,
    handle: &AgentHandle,
    selected: Tools,
) -> Result<ClaudeTools> {
    install(&selected, workspace, handle)
}

pub(super) fn install(
    tools: &Tools,
    workspace: PathBuf,
    handle: &AgentHandle,
) -> Result<ClaudeTools> {
    let tools = tools
        .clone()
        .for_session(&handle.session_environment())
        .for_backend(handle.session_id(), "claude")
        .with_code_mode_policy(crate::code_mode_policy::claude_tool_policy());
    let runtime = Arc::new(ToolRuntime::new_with_tools(
        workspace.clone(),
        Vec::new(),
        &tools,
    ));

    let guard = Arc::new(RuntimeGuard {
        control: runtime.control(),
        #[cfg(not(target_family = "wasm"))]
        cancel_on_drop: !tools.has_code_mode_host(),
    });
    let mut native = ClaudeTools::new();
    if cfg!(target_family = "wasm") || tools.has_code_mode_host() {
        let shutdown = Arc::clone(&guard);
        native = native.adapter_shutdown(move || {
            let shutdown = Arc::clone(&shutdown);
            async move { shutdown.control.cancel().await }
        });
        native.shared_shutdown = native.adapter_shutdown.last().cloned();
    }
    let nested = Arc::clone(&runtime);
    native.nested_tools = Some(Arc::new(move |name| nested.contains(name)));
    let recovery = Arc::clone(&runtime);
    native.recovery = Some(Arc::new(move |name, invocation| {
        let runtime = Arc::clone(&recovery);
        let name = name.to_owned();
        Box::pin(async move { Call::recover(&runtime, &name, &invocation).await })
    }));
    let preempt = Arc::clone(&guard);
    native.code_turn_preempt.push(Arc::new(move || {
        let guard = Arc::clone(&preempt);
        Box::pin(async move { guard.control.preempt_turn().await })
    }));
    let quiet = Arc::clone(&guard);
    native
        .code_quiet
        .push(Arc::new(move || quiet.control.quiet()));
    let settle = Arc::clone(&guard);
    native.code_turn_settle.push(Arc::new(move || {
        let guard = Arc::clone(&settle);
        Box::pin(async move { guard.control.settle_preempt() })
    }));
    native.code_turn_cleanup.push(Arc::new(move |progress| {
        let guard = Arc::clone(&guard);
        Box::pin(async move {
            guard
                .control
                .cancel_turn_with_updates(&mut Progress(Some(&progress)))
                .await;
            guard.control.begin_turn();
        })
    }));
    for definition in flatten(runtime.model_specs(handle.session_id())) {
        let strict = match &definition {
            RuntimeDefinition::Function {
                strict: true,
                parameters,
                ..
            } if !recursive_schema(parameters.as_value()) => Some(true),
            _ => None,
        };
        let output_schema = definition.output_schema().cloned();
        let (name, description, input_schema, call) = match definition {
            RuntimeDefinition::Custom {
                name, description, ..
            } if &*name == "exec" => (
                name,
                exec_description(&description),
                Arc::clone(&EXEC_SCHEMA),
                Call::Exec,
            ),
            RuntimeDefinition::Function {
                name,
                description,
                parameters,
                ..
            } if &*name == "wait" => (name, description, parameters.shared(), Call::Wait),
            RuntimeDefinition::Function {
                name,
                description,
                parameters,
                ..
            } => {
                let call = Call::Function(Arc::from(name.as_ref()));
                (name, description, parameters.shared(), call)
            }
            RuntimeDefinition::ToolSearch {
                description,
                parameters,
                ..
            } => (
                "tool_search".into(),
                description,
                parameters.shared(),
                Call::Function(Arc::from("tool_search")),
            ),
            other => {
                return Err(NanocodeError::InvalidRequest(format!(
                    "Claude requires a function tool; `{}` cannot be installed directly",
                    other.name()
                )));
            }
        };
        let description = match output_schema {
            Some(schema) => shared_description(
                &name,
                format!("{description}\nOutput schema: {}", schema.as_value()),
            ),
            None => description,
        };
        let tool_name = name.to_string();
        native.shared_tool_names.insert(tool_name.clone());
        let definition = ToolDefinition {
            description,
            name: tool_name.clone(),
            input_schema,
            strict,
            defer_loading: false,
            cache_control: None,
        };
        let runtime = Arc::clone(&runtime);
        native = native.tool_with_context(definition, move |input, invocation| {
            let runtime = Arc::clone(&runtime);
            let call = call.clone();
            async move { call.run(&runtime, input, &invocation).await }
        });
    }
    native.shared_catalog = Some((tools, workspace, handle.clone()));
    Ok(native)
}

#[derive(Clone)]
enum Call {
    Exec,
    Wait,
    Function(Arc<str>),
}

struct RuntimeGuard {
    control: nanocode_tool_host::runtime::ToolRuntimeControl,
    #[cfg(not(target_family = "wasm"))]
    cancel_on_drop: bool,
}

#[cfg(not(target_family = "wasm"))]
impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        if !self.cancel_on_drop {
            return;
        }
        let control = self.control.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { control.cancel().await });
        }
    }
}

impl Call {
    async fn run(
        self,
        runtime: &ToolRuntime,
        input: Value,
        invocation: &ClaudeToolInvocation,
    ) -> std::result::Result<ClaudeToolReply, String> {
        let context = tool_context(invocation);
        let mut progress = Progress(invocation.progress.as_ref());
        let execution = match self {
            Self::Exec => {
                let code = input
                    .get("code")
                    .and_then(Value::as_str)
                    .ok_or("exec requires a `code` string")?;
                runtime
                    .execute_code_owned_with_updates(
                        code,
                        OwnedToolContext::from_context(context),
                        &mut progress,
                    )
                    .await
            }
            Self::Wait => {
                runtime
                    .wait_for_code_with_updates(&input.to_string(), context, &mut progress)
                    .await
            }
            Self::Function(name) => {
                let raw =
                    serde_json::value::to_raw_value(&input).map_err(|error| error.to_string())?;
                let output = runtime
                    .execute_tool(&name, ToolInput::Function(raw), context)
                    .await
                    .map_err(|error| error.to_string())?;
                return tool_reply(output);
            }
        }
        .map_err(|error| error.to_string())?;
        Ok(code_reply(execution, &invocation.call_id))
    }

    async fn recover(
        runtime: &ToolRuntime,
        name: &str,
        invocation: &ClaudeToolInvocation,
    ) -> std::result::Result<ClaudeToolReply, String> {
        let context = tool_context(invocation);
        if matches!(name, "exec" | "wait") {
            return match runtime.recover_code(context).await {
                Ok(Some(execution)) => Ok(code_reply(execution, &invocation.call_id)),
                Ok(None) => Err("not recovered".into()),
                Err(error) => Err(error.to_string()),
            };
        }
        match runtime.recover_tool(name, context).await {
            Some(output) => tool_reply(output),
            None => Err("not recovered".into()),
        }
    }
}

fn tool_context(invocation: &ClaudeToolInvocation) -> ToolContext<'_> {
    ToolContext::new(
        &invocation.model,
        &invocation.session_id,
        &invocation.call_id,
        &[],
        usize::MAX,
    )
    .with_turn_id(Some(&invocation.turn_id))
    .with_host_context(invocation.host_context.as_deref())
    .with_instruction_revision(invocation.instruction_revision)
}

fn tool_reply(mut output: ToolOutput) -> std::result::Result<ClaudeToolReply, String> {
    let structured_result = Some(output.take_structured_result());
    let metadata = output
        .metadata
        .take()
        .map(|value| serde_json::from_str(value.get()))
        .transpose()
        .map_err(|error| error.to_string())?;
    Ok(reply(
        output.output,
        output.success,
        metadata,
        structured_result,
    ))
}

struct Progress<'a>(Option<&'a ClaudeToolProgress>);

impl CodeModeObserver for Progress<'_> {
    fn cell_terminated(&mut self, origin_call_id: &str, output: &str) {
        if let Some(progress) = self.0 {
            progress.update(ClaudeNestedToolUpdate::Terminated {
                call_id: origin_call_id.to_owned(),
                output: output.to_owned(),
            });
        }
    }

    fn update(&mut self, update: CodeModeUpdate<'_>) {
        let Some(progress) = self.0 else {
            return;
        };
        progress.update(match update {
            CodeModeUpdate::NestedCallStarted {
                call_id,
                name,
                input,
            } => ClaudeNestedToolUpdate::Started {
                call_id: call_id.to_owned(),
                name: name.to_owned(),
                input: input.clone(),
            },
            CodeModeUpdate::NestedCallCompleted(call) => {
                ClaudeNestedToolUpdate::Completed(nested_call(call))
            }
            CodeModeUpdate::Notification(notification) => ClaudeNestedToolUpdate::Notification {
                call_id: notification.call_id.clone(),
                text: notification.text.clone(),
            },
        });
    }
}

fn nested_call(call: &NestedToolCall) -> Value {
    json!({
        "call_id": call.call_id,
        "name": call.name,
        "input": call.input,
        "output": call.output,
        "structured_result": call.structured_result,
        "metadata": call.metadata,
        "success": call.success,
        "started_after_ns": call.started_after_ns,
        "duration_ns": call.duration_ns,
    })
}

fn code_reply(execution: CodeModeExecution, call_id: &str) -> ClaudeToolReply {
    let metadata = json!({
        "_nanocode_code": {
            "calls": execution.nested_calls.iter().map(nested_call).collect::<Vec<_>>(),
            "origin_call_id": execution
                .cell
                .as_ref()
                .map_or(call_id, |cell| cell.origin_call_id.as_str()),
            "running": execution.cell.as_ref().is_some_and(|cell| cell.running),
        }
    });
    let mut reply = reply(execution.output, execution.success, Some(metadata), None);
    for notification in execution.notifications {
        match &mut reply.content {
            ToolResultContent::Text(text) => {
                text.push('\n');
                text.push_str(&notification.text);
            }
            ToolResultContent::Blocks(blocks) => {
                blocks.push(json!({"type": "text", "text": notification.text}));
            }
        }
    }
    reply
}

fn flatten(definitions: Vec<RuntimeDefinition>) -> Vec<RuntimeDefinition> {
    let mut flat = Vec::new();
    for definition in definitions {
        match definition {
            RuntimeDefinition::Namespace {
                name: namespace,
                tools,
                ..
            } => {
                for mut tool in tools {
                    match &mut tool {
                        RuntimeDefinition::Function { name, .. }
                        | RuntimeDefinition::Custom { name, .. } => {
                            *name = format!("{namespace}__{name}").into();
                        }
                        _ => continue,
                    }
                    flat.push(tool);
                }
            }
            definition => flat.push(definition),
        }
    }
    flat
}

fn reply(
    output: ToolOutputBody,
    success: bool,
    metadata: Option<Value>,
    structured_result: Option<Value>,
) -> ClaudeToolReply {
    let content = match output {
        ToolOutputBody::Text(text) => ToolResultContent::Text(text),
        ToolOutputBody::Content(items) => {
            ToolResultContent::Blocks(items.into_iter().map(block).collect())
        }
    };
    ClaudeToolReply {
        content,
        is_error: !success,
        metadata,
        structured_result,
    }
}

fn block(item: ToolOutputContent) -> Value {
    match item {
        ToolOutputContent::InputText { text } => json!({"type": "text", "text": text}),
        ToolOutputContent::InputImage { image_url, .. } => {
            match image_url
                .strip_prefix("data:")
                .and_then(|rest| rest.split_once(";base64,"))
            {
                Some((media_type, data)) => json!({
                    "type": "image",
                    "source": {"type": "base64", "media_type": media_type, "data": data},
                }),
                None => json!({"type": "image", "source": {"type": "url", "url": image_url}}),
            }
        }
        other => json!({
            "type": "text",
            "text": serde_json::to_string(&other).unwrap_or_default(),
        }),
    }
}

fn recursive_schema(schema: &Value) -> bool {
    fn visit(value: &Value, root: &Value, references: &mut Vec<String>) -> bool {
        match value {
            Value::Object(object) => {
                if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                    && let Some(pointer) = reference.strip_prefix('#')
                    && let Some(target) = root.pointer(pointer)
                {
                    if references.iter().any(|active| active == reference) {
                        return true;
                    }
                    references.push(reference.to_owned());
                    let recursive = visit(target, root, references);
                    references.pop();
                    if recursive {
                        return true;
                    }
                }
                object.iter().any(|(key, value)| {
                    !matches!(
                        key.as_str(),
                        "$defs" | "definitions" | "const" | "enum" | "default" | "examples"
                    ) && visit(value, root, references)
                })
            }
            Value::Array(values) => values.iter().any(|value| visit(value, root, references)),
            _ => false,
        }
    }
    visit(schema, schema, &mut Vec::new())
}

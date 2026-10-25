use std::{collections::HashSet, path::PathBuf, sync::Arc};

use arc_swap::ArcSwap;

use nanocode_tools::{Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput};
use nanocode_types::responses::CustomToolFormat;

use super::{
    CodeModeExecution, CodeModeHost, CodeModeHostError, CodeModeObserver, EmbeddedToolMode,
    OwnedToolContext,
};
use crate::{ToolExposure, Tools};

const EXEC_GRAMMAR: &str = r"start: /[\s\S]+/";
const EXEC_DESCRIPTION: &str = r"Run JavaScript in the embedded host.
- `tools` contains the application-defined async tools listed below.
- `ALL_TOOLS` lists the callable nested tools for this execution. Separately exposed host tools may require direct invocation; do not assume they have a `tools` binding.
- `text(value)` and `image(value)` append output for the model.
- `generatedImage(result)` appends an image-generation result for the model.
- `store(key, value)` and `load(key)` retain serializable values across calls.
- JavaScript runs inside the Node or browser host supplied by the embedding application.";
const DEFERRED_TOOLS_DESCRIPTION: &str = r"Some deferred nested tools are omitted from this description. They remain available on the global `tools` object and are listed in `ALL_TOOLS`. Use `tool_search` to discover remote tools before calling them.";

pub struct EmbeddedToolRuntime {
    working_directory: Arc<str>,
    host: Option<Arc<dyn CodeModeHost>>,
    session_id: Option<Arc<str>>,
    local: Vec<LocalTool>,
    callable_tool_names: ArcSwap<HashSet<String>>,
    policies: Vec<Arc<dyn crate::CodeModeToolPolicy>>,
    recovery: Option<Arc<dyn crate::ToolRecovery>>,
    brief: Arc<[String]>,
}

struct LocalTool {
    name: Arc<str>,
    handler: Arc<dyn Tool>,
    model_visible: bool,
}

#[derive(Clone, Default)]
pub struct EmbeddedToolRuntimeControl {
    host: Option<Arc<dyn CodeModeHost>>,
    session_id: Option<Arc<str>>,
}

impl EmbeddedToolRuntime {
    pub fn new(workspace: impl Into<PathBuf>, builtin_tools: Vec<Arc<dyn Tool>>) -> Self {
        let workspace = workspace.into();
        Self {
            working_directory: Arc::from(workspace.to_string_lossy().into_owned()),
            host: None,
            session_id: None,
            local: builtin_tools
                .into_iter()
                .map(|handler| LocalTool {
                    name: Arc::from(handler.definition().name()),
                    handler,
                    model_visible: true,
                })
                .collect(),
            callable_tool_names: ArcSwap::default(),
            policies: Vec::new(),
            recovery: None,
            brief: Arc::from([]),
        }
    }

    #[must_use]
    pub fn new_with_tools(
        workspace: impl Into<PathBuf>,
        builtin_tools: Vec<Arc<dyn Tool>>,
        tools: &Tools,
    ) -> Self {
        Self::new(workspace, builtin_tools).with_tools(tools)
    }

    fn with_tools(mut self, tools: &Tools) -> Self {
        self.host.clone_from(&tools.embedded_host);
        self.session_id.clone_from(&tools.embedded_session_id);
        self.policies = tools.code_mode_policies();
        self.recovery = tools.recovery();
        self.brief = Arc::clone(&tools.brief_code_tools);
        self.local
            .extend(tools.registered.iter().map(|registered| LocalTool {
                name: Arc::from(registered.handler.definition().name()),
                handler: Arc::clone(&registered.handler),
                model_visible: registered.exposure.unwrap_or_else(|| tools.exposure())
                    != ToolExposure::Hidden,
            }));
        self
    }

    #[must_use]
    pub const fn default_shell_name(&self) -> &'static str {
        "javascript"
    }

    #[must_use]
    pub fn working_directory(&self) -> &str {
        &self.working_directory
    }

    #[must_use]
    pub fn control(&self) -> EmbeddedToolRuntimeControl {
        EmbeddedToolRuntimeControl {
            host: self.host.clone(),
            session_id: self.session_id.clone(),
        }
    }

    #[must_use]
    pub fn model_specs(&self, session_id: &str) -> Vec<ToolDefinition> {
        self.model_contract(session_id).0
    }

    pub(crate) fn model_contract(
        &self,
        session_id: &str,
    ) -> (Vec<ToolDefinition>, Vec<(String, String)>) {
        let mode = self
            .host
            .as_ref()
            .map_or(EmbeddedToolMode::Direct, |host| host.tool_mode());
        let mut definitions = self.host.as_ref().map_or_else(Vec::new, |host| {
            match host.tool_definitions(session_id) {
                Ok(definitions) => definitions,
                Err(error) => {
                    tracing::warn!(
                        target: "nanocode_code_mode",
                        %error,
                        "embedded Code Mode tool discovery failed"
                    );
                    Vec::new()
                }
            }
        });
        definitions.retain(|definition| {
            !self
                .local
                .iter()
                .any(|tool| tool.name.as_ref() == definition.name())
        });
        crate::code_mode_order::sort_definitions(&mut definitions);
        self.callable_tool_names.store(Arc::new(
            definitions
                .iter()
                .map(|definition| definition.name().to_owned())
                .collect(),
        ));
        if mode == EmbeddedToolMode::Direct {
            definitions.extend(
                self.local
                    .iter()
                    .filter(|tool| tool.model_visible)
                    .map(|tool| tool.handler.definition()),
            );
            crate::code_mode_order::sort_definitions(&mut definitions);
            return (definitions, Vec::new());
        }
        if mode == EmbeddedToolMode::CodeOnly {
            definitions = definitions
                .into_iter()
                .map(|definition| match definition {
                    ToolDefinition::ToolSearch {
                        description,
                        parameters,
                        ..
                    } => ToolDefinition::function(
                        "tool_search",
                        format!("{description} Invoke tools.tool_search inside exec; discovered tools become callable in the next exec cell."),
                        parameters,
                    ),
                    definition => definition,
                })
                .collect();
            definitions.extend(
                self.local
                    .iter()
                    .filter(|tool| tool.model_visible)
                    .map(|tool| tool.handler.definition()),
            );
            crate::code_mode_order::sort_definitions(&mut definitions);
        }
        let has_deferred_tools = definitions.iter().any(|definition| {
            matches!(
                definition,
                ToolDefinition::ToolSearch { .. }
                    | ToolDefinition::Function {
                        defer_loading: Some(true),
                        ..
                    }
                    | ToolDefinition::Custom {
                        defer_loading: Some(true),
                        ..
                    }
            )
        });
        let (mut direct_definitions, code_mode_definitions): (Vec<_>, Vec<_>) =
            definitions.into_iter().partition(|definition| {
                mode != EmbeddedToolMode::CodeOnly
                    && (matches!(definition, ToolDefinition::ToolSearch { .. })
                        || self
                            .policies
                            .iter()
                            .any(|policy| policy.direct_host_tool(definition.name())))
            });
        direct_definitions = direct_definitions
            .into_iter()
            .map(crate::code_mode_description::augment_definition_for_code_mode)
            .collect();
        direct_definitions.extend(
            self.local
                .iter()
                .filter(|tool| tool.model_visible && mode != EmbeddedToolMode::CodeOnly)
                .map(|tool| tool.handler.definition()),
        );
        let code_mode_tool_names = code_mode_definitions
            .iter()
            .map(|definition| {
                (
                    normalize_identifier(definition.name()),
                    definition.name().to_owned(),
                )
            })
            .collect();
        let resumable = self.host.as_ref().is_some_and(|host| host.supports_cells());
        if resumable {
            let visible_definitions = code_mode_definitions
                .into_iter()
                .filter(|definition| {
                    !matches!(
                        definition,
                        ToolDefinition::Function {
                            defer_loading: Some(true),
                            ..
                        } | ToolDefinition::Custom {
                            defer_loading: Some(true),
                            ..
                        }
                    )
                })
                .collect::<Vec<_>>();
            let (brief, visible_definitions): (Vec<_>, Vec<_>) = visible_definitions
                .into_iter()
                .partition(|definition| self.brief.iter().any(|name| name == definition.name()));
            let summaries = brief.iter().map(summary).collect::<Vec<_>>();
            let mut exec = crate::code_mode_spec::exec_spec(
                &visible_definitions,
                &summaries,
                has_deferred_tools,
                true,
            );
            if let ToolDefinition::Custom { description, .. } = &mut exec {
                if mode == EmbeddedToolMode::CodeOnly {
                    *description = description
                        .replace("Use `tool_search`", "Use `tools.tool_search`")
                        .into_boxed_str();
                }
                *description = description
                    .replace("Runs raw JavaScript -- no Node, no file system, no network access, no console.", "Runs JavaScript inside the evaluator supplied by the embedding application.").into_boxed_str();
            }
            let mut model_definitions = vec![exec, crate::code_mode_spec::wait_spec()];
            model_definitions.extend(direct_definitions);
            return (model_definitions, code_mode_tool_names);
        }
        let mut description = EXEC_DESCRIPTION.to_owned();
        for definition in code_mode_definitions {
            if matches!(
                definition,
                ToolDefinition::Function {
                    defer_loading: Some(true),
                    ..
                } | ToolDefinition::Custom {
                    defer_loading: Some(true),
                    ..
                }
            ) {
                continue;
            }
            let definition =
                crate::code_mode_description::augment_definition_for_code_mode(definition);
            description.push_str("\n\n- `tools.");
            description.push_str(definition.name());
            description.push_str("`: ");
            description.push_str(definition.description().trim());
        }
        if has_deferred_tools {
            description.push_str("\n\n");
            description.push_str(DEFERRED_TOOLS_DESCRIPTION);
        }
        if mode == EmbeddedToolMode::CodeOnly {
            description = description.replace("Use `tool_search`", "Use `tools.tool_search`");
        }
        let mut model_definitions = vec![ToolDefinition::custom(
            "exec",
            description,
            CustomToolFormat::grammar("lark", EXEC_GRAMMAR),
        )];
        model_definitions.extend(direct_definitions);
        (model_definitions, code_mode_tool_names)
    }

    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn registered_code_mode_definitions(&self) -> Vec<ToolDefinition> {
        self.local
            .iter()
            .filter(|tool| tool.model_visible)
            .map(|tool| tool.handler.definition())
            .collect()
    }

    #[must_use]
    pub const fn supports_parallel_tool_calls(&self, _name: &str) -> bool {
        false
    }

    #[must_use]
    pub fn is_code_only(&self) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.tool_mode() == EmbeddedToolMode::CodeOnly)
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        if self.local.iter().any(|tool| tool.name.as_ref() == name) {
            return true;
        }
        if self.callable_tool_names.load().contains(name) {
            return true;
        }
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Ok(definitions) = host.tool_definitions(session_id)
        {
            let found = definitions
                .iter()
                .any(|definition| definition.name() == name);
            self.callable_tool_names.store(Arc::new(
                definitions
                    .into_iter()
                    .map(|definition| definition.name().to_owned())
                    .collect(),
            ));
            return found;
        }
        false
    }

    #[allow(
        clippy::unused_async,
        reason = "matches the native tool-runtime contract"
    )]
    pub async fn execute_tool(
        &self,
        name: &str,
        input: ToolInput,
        context: ToolContext<'_>,
    ) -> Result<ToolOutput, CodeModeHostError> {
        if self
            .host
            .as_ref()
            .is_some_and(|host| host.tool_mode() == EmbeddedToolMode::CodeOnly)
        {
            return Ok(ToolOutput::error(format!(
                "tool `{name}` requires Code Mode; no direct handler was invoked in this attempt. If this call was recovered, a previous attempt may have run with an unknown outcome; reconcile its effects before repeating through tools inside exec"
            )));
        }
        if let Some(tool) = self.local.iter().find(|tool| tool.name.as_ref() == name) {
            return Ok(tool
                .handler
                .execute(input, context)
                .await
                .unwrap_or_else(|error| ToolOutput::error(error.to_string())));
        }
        let Some(host) = &self.host else {
            return Ok(ToolOutput::error(format!(
                "tool `{name}` is unavailable because no embedded tool adapter is configured; no handler was invoked"
            )));
        };
        if !self.contains(name) {
            return Ok(ToolOutput::error(format!(
                "direct embedded tool `{name}` is unavailable"
            )));
        }
        host.execute_tool(name, input, context).await
    }

    pub async fn execute_code(
        &self,
        source: &str,
        context: ToolContext<'_>,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        let Some(host) = &self.host else {
            return Ok(code_mode_unavailable("exec"));
        };
        if host.tool_mode() == EmbeddedToolMode::CodeOnly {
            return host
                .execute_with_local_tools(
                    source,
                    context,
                    self.local
                        .iter()
                        .filter(|tool| tool.model_visible)
                        .map(|tool| Arc::clone(&tool.handler))
                        .collect(),
                    None,
                )
                .await;
        }
        host.execute(source, context).await
    }

    pub async fn execute_code_with_updates(
        &self,
        source: &str,
        context: ToolContext<'_>,
        observer: &mut dyn CodeModeObserver,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        self.execute_code_owned_with_updates(
            source,
            OwnedToolContext::from_context(context),
            observer,
        )
        .await
    }

    pub async fn execute_code_owned(
        &self,
        source: &str,
        context: OwnedToolContext,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        self.execute_code(source, context.as_context()).await
    }

    pub async fn execute_code_owned_with_updates(
        &self,
        source: &str,
        context: OwnedToolContext,
        observer: &mut dyn CodeModeObserver,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        let Some(host) = &self.host else {
            return Ok(code_mode_unavailable("exec"));
        };
        if host.tool_mode() == EmbeddedToolMode::CodeOnly {
            return host
                .execute_with_local_tools(
                    source,
                    context.as_context(),
                    self.local
                        .iter()
                        .filter(|tool| tool.model_visible)
                        .map(|tool| Arc::clone(&tool.handler))
                        .collect(),
                    Some(observer),
                )
                .await;
        }
        host.execute_with_updates(source, context.as_context(), observer)
            .await
    }

    pub async fn recover_tool(&self, name: &str, context: ToolContext<'_>) -> Option<ToolOutput> {
        self.recovery.as_ref()?.recover(name, context).await
    }

    pub async fn recover_code(
        &self,
        context: ToolContext<'_>,
    ) -> Result<Option<CodeModeExecution>, CodeModeHostError> {
        match &self.host {
            Some(host) => host.recover(context).await,
            None => Ok(None),
        }
    }

    pub async fn wait_for_code(
        &self,
        input: &str,
        context: ToolContext<'_>,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        self.wait_for_code_with_updates(input, context, &mut IgnoreUpdates)
            .await
    }

    pub async fn wait_for_code_with_updates(
        &self,
        input: &str,
        context: ToolContext<'_>,
        observer: &mut dyn CodeModeObserver,
    ) -> Result<CodeModeExecution, CodeModeHostError> {
        let Some(host) = &self.host else {
            return Ok(code_mode_unavailable("wait"));
        };
        host.wait_with_updates(input, context, observer).await
    }
}

fn code_mode_unavailable(name: &str) -> CodeModeExecution {
    CodeModeExecution {
        cell: None,
        output: ToolOutput::error(format!(
            "`{name}` is unavailable because no embedded Code Mode adapter is configured; no code was executed"
        ))
        .output,
        success: false,
        nested_calls: Vec::new(),
        notifications: Vec::new(),
    }
}

struct IgnoreUpdates;
impl CodeModeObserver for IgnoreUpdates {
    fn update(&mut self, _update: super::CodeModeUpdate<'_>) {}
}

fn normalize_identifier(name: &str) -> String {
    let mut identifier = String::new();
    for (index, character) in name.chars().enumerate() {
        let valid = if index == 0 {
            character == '_' || character == '$' || character.is_ascii_alphabetic()
        } else {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        };
        identifier.push(if valid { character } else { '_' });
    }
    if identifier.is_empty() {
        "_".to_owned()
    } else {
        identifier
    }
}

impl EmbeddedToolRuntimeControl {
    pub async fn preempt_turn(&self) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Err(error) = host.preempt_turn(session_id).await
        {
            tracing::warn!(target: "nanocode_tools", %error, "embedded Code Mode observation preemption failed");
        }
    }

    pub fn begin_turn(&self) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id) {
            host.begin_turn(session_id);
        }
    }

    pub fn settle_preempt(&self) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id) {
            host.settle_preempt(session_id);
        }
    }

    pub async fn cancel_turn(&self) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Err(error) = host.cancel_turn(session_id).await
        {
            tracing::warn!(target: "nanocode_code_mode", %error, "embedded Code Mode turn cancellation failed");
        }
    }

    pub async fn detach_turn_with_updates(
        &self,
        observer: &mut (dyn FnMut(&str) -> Box<dyn CodeModeObserver> + Send),
    ) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id) {
            host.detach_turn(session_id, observer);
        }
    }

    pub async fn cancel_turn_with_updates(&self, observer: &mut dyn CodeModeObserver) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Err(error) = host.cancel_turn_with_updates(session_id, observer).await
        {
            tracing::warn!(target: "nanocode_code_mode", %error, "embedded Code Mode turn cancellation failed");
        }
    }

    pub async fn cancel_with_updates(&self, observer: &mut dyn CodeModeObserver) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Err(error) = host.cancel_with_updates(session_id, observer).await
        {
            tracing::warn!(target: "nanocode_code_mode", %error, "embedded Code Mode cancellation failed");
        }
    }

    pub async fn cancel(&self) {
        if let (Some(host), Some(session_id)) = (&self.host, &self.session_id)
            && let Err(error) = host.cancel(session_id).await
        {
            tracing::warn!(
                target: "nanocode_code_mode",
                %error,
                "embedded Code Mode cancellation failed"
            );
        }
    }
}

fn summary(definition: &ToolDefinition) -> (String, String) {
    let description = definition.description().trim();
    let first = description
        .split_once(". ")
        .map_or(description, |(sentence, _)| sentence)
        .trim_end_matches('.');
    let schema = definition.parameters().map(|schema| schema.as_value());
    let required = schema
        .and_then(|schema| schema.get("required"))
        .and_then(serde_json::Value::as_array);
    let arguments = schema
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
        .map(|properties| {
            properties
                .keys()
                .map(|name| {
                    let needed = required.is_some_and(|required| {
                        required.iter().any(|value| value.as_str() == Some(name))
                    });
                    if needed {
                        name.clone()
                    } else {
                        format!("{name}?")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    (
        normalize_identifier(definition.name()),
        format!("{first}. Arguments: {{ {arguments} }}."),
    )
}

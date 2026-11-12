use super::execution::{
    OpenToolSpan, panicked_tool_output, record_tool_content, tool_execution_span,
};
use super::*;
use crate::attachment::{ProcessRouteStore, RemoteRoute, RunnerRouting};
use nanocode_code_mode::embedded::{declares_hand, take_hand, tool_not_available};
use nanocode_sync::Atom;
use std::sync::OnceLock;

pub struct RoutingCatalog<'a> {
    registry: &'a mut ToolRegistry,
}

impl RoutingCatalog<'_> {
    pub fn add(&mut self, tool: Arc<dyn Tool>, exposure: ToolExposure) {
        self.registry.extend([(tool, exposure)]);
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.registry.by_name.contains_key(name)
    }

    #[must_use]
    pub fn names(&self) -> HashSet<Box<str>> {
        self.registry.by_name.keys().cloned().collect()
    }

    #[must_use]
    pub fn providers(&self) -> &[Arc<dyn DynamicToolProvider>] {
        &self.registry.providers
    }

    pub fn wrap(
        &mut self,
        mut wrap: impl FnMut(&Arc<dyn Tool>, &ToolDefinition) -> Option<Arc<dyn Tool>>,
    ) {
        let registry = &mut *self.registry;
        for (index, tool) in registry.ordered.iter_mut().enumerate() {
            if let Some(wrapped) = wrap(tool, &registry.definitions[index]) {
                *tool = wrapped;
                registry.definitions[index] = tool.definition();
                registry.catalog = OnceLock::new();
            }
        }
    }

    #[must_use]
    pub fn policies(&self) -> &[Arc<dyn crate::CodeModeToolPolicy>] {
        &self.registry.policies
    }

    pub fn add_policy(&mut self, policy: Arc<dyn crate::CodeModeToolPolicy>) {
        self.registry.policies.push(policy);
    }

    pub fn insert_provider_first(&mut self, provider: Arc<dyn DynamicToolProvider>) {
        self.registry.providers.insert(0, provider);
    }
}

#[derive(Clone)]
pub(crate) struct ToolRegistry {
    ordered: Vec<Arc<dyn Tool>>,
    definitions: Vec<ToolDefinition>,
    exposures: Vec<ToolExposure>,
    by_name: HashMap<Box<str>, usize>,
    pub(super) providers: Vec<Arc<dyn DynamicToolProvider>>,
    pub(super) policies: Vec<Arc<dyn crate::CodeModeToolPolicy>>,
    pub(super) hand_calls: Option<Arc<dyn crate::embedded::HandCalls>>,
    nested_gate: Arc<super::admission::AdmissionGate>,
    excluded_code_tools: HashSet<String>,
    nested_metadata: Arc<Atom<Option<Arc<NestedMetadata>>>>,
    catalog: OnceLock<Arc<Catalog>>,
}

pub(super) struct Catalog {
    definitions: Vec<ToolDefinition>,
    exposures: Vec<ToolExposure>,
    excluded: HashSet<String>,
    pub(super) registered: Vec<ToolDefinition>,
    pub(super) ordered: Vec<ToolDefinition>,
    pub(super) names: HashSet<String>,
    normalized: HashSet<String>,
    nested: Vec<ToolDefinition>,
    metadata: OnceLock<Arc<[Value]>>,
}

impl Catalog {
    fn of(
        definitions: &[ToolDefinition],
        exposures: &[ToolExposure],
        excluded: &HashSet<String>,
    ) -> Arc<Self> {
        static LIVE: std::sync::LazyLock<Atom<Vec<std::sync::Weak<Catalog>>>> =
            std::sync::LazyLock::new(Atom::default);
        let live = LIVE.read(|live| {
            live.iter()
                .filter_map(std::sync::Weak::upgrade)
                .find(|catalog| {
                    catalog.definitions == definitions
                        && catalog.exposures == exposures
                        && catalog.excluded == *excluded
                })
        });
        if let Some(catalog) = live {
            return catalog;
        }
        let offered = || {
            definitions
                .iter()
                .zip(exposures)
                .filter(|(definition, exposure)| {
                    exposure.is_available_in_code_mode() && !excluded.contains(definition.name())
                })
                .map(|(definition, _)| nested_definition(definition.clone()))
        };
        let registered = first_normalized_definitions(offered());
        let mut ordered = registered.clone();
        crate::code_mode_order::sort_definitions(&mut ordered);
        let catalog = Arc::new(Self {
            definitions: definitions.to_vec(),
            exposures: exposures.to_vec(),
            excluded: excluded.clone(),
            names: registered
                .iter()
                .map(|definition| definition.name().to_owned())
                .collect(),
            normalized: registered
                .iter()
                .map(|definition| crate::selection::normalize_public_tool_name(definition.name()))
                .collect(),
            nested: first_normalized_definitions(
                offered().filter(|definition| !host_owned_name(definition.name())),
            ),
            registered,
            ordered,
            metadata: OnceLock::new(),
        });
        let weak = Arc::downgrade(&catalog);
        LIVE.update(|live| {
            live.retain(|catalog| catalog.strong_count() > 0);
            live.push(std::sync::Weak::clone(&weak));
        });
        catalog
    }

    fn metadata(&self) -> Arc<[Value]> {
        Arc::clone(self.metadata.get_or_init(|| {
            self.nested
                .iter()
                .map(|definition| definition_metadata(definition.name(), definition))
                .collect()
        }))
    }
}

struct NestedMetadata {
    definitions: Vec<ToolDefinition>,
    metadata: Arc<[Value]>,
}

impl NestedMetadata {
    fn offers(&self, definitions: &[ToolDefinition]) -> bool {
        self.definitions.len() == definitions.len()
            && self
                .definitions
                .iter()
                .zip(definitions)
                .all(|(built, current)| same_metadata(built, current))
    }
}

impl ToolRegistry {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
            || (!host_owned_name(name)
                && self
                    .providers
                    .iter()
                    .any(|provider| provider.contains(name)))
    }

    pub(crate) fn supports_parallel_tool_calls(&self, name: &str) -> bool {
        if let Some((handler, _)) = self.get(name) {
            return handler.supports_parallel_tool_calls();
        }
        self.providers
            .iter()
            .find(|provider| provider.contains(name))
            .is_some_and(|provider| provider.supports_parallel_tool_calls(name))
    }

    pub(super) async fn execute_direct(
        &self,
        name: &str,
        input: ToolInput,
        context: ToolContext<'_>,
    ) -> ToolOutput {
        let trace_content = tracing::enabled!(
            target: "nanocode_code_mode",
            tracing::Level::INFO
        );
        let (arguments_kind, arguments) = match &input {
            ToolInput::Function(arguments) => ("function", arguments.get()),
            ToolInput::Freeform(arguments) => ("freeform", arguments.as_str()),
        };
        let arguments_content = trace_content.then(|| arguments.to_owned());
        let span = tool_execution_span(name, context, arguments.len(), arguments_kind, 1, "");
        let open = OpenToolSpan::new(&span);
        if let Some(arguments_content) = &arguments_content {
            record_tool_content(&span, "tool.arguments", arguments_content);
        }
        let started_at = std::time::Instant::now();
        let dispatch = async {
            if let Some((handler, _definition)) = self.get(name) {
                return match handler.execute(input, context).await {
                    Ok(execution) => execution,
                    Err(error) => ToolOutput::error(error.to_string()),
                };
            }
            let provider_input = match input {
                ToolInput::Function(arguments) => {
                    match serde_json::from_str::<Value>(arguments.get()) {
                        Ok(arguments) => arguments,
                        Err(error) => {
                            return ToolOutput::error(format!(
                                "failed to decode {name} arguments: {error}"
                            ));
                        }
                    }
                }
                ToolInput::Freeform(_) => {
                    return ToolOutput::error(format!(
                        "dynamic tool {name} requires object arguments"
                    ));
                }
            };
            let Some(provider) = (!host_owned_name(name))
                .then(|| {
                    self.providers
                        .iter()
                        .find(|provider| provider.contains(name))
                })
                .flatten()
            else {
                return ToolOutput::error(format!("unsupported tool call: {name}"));
            };
            if let Some(execution) = provider.execute(name, provider_input, context).await {
                return execution;
            }
            ToolOutput::error(format!("unsupported tool call: {name}"))
        }
        .instrument(span.clone());
        let execution = match AssertUnwindSafe(dispatch).catch_unwind().await {
            Ok(execution) => execution,
            Err(payload) => panicked_tool_output(&span, payload),
        };
        let output_content = trace_content
            .then(|| serde_json::to_string(&execution.output).ok())
            .flatten();
        open.finish(started_at, &execution, output_content.as_deref());
        execution
    }

    pub(crate) async fn execute_nested(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> ToolOutput {
        let trace_content = tracing::enabled!(
            target: "nanocode_code_mode",
            tracing::Level::INFO
        );
        let arguments_content = trace_content
            .then(|| serde_json::to_string(&input).ok())
            .flatten();
        let arguments_bytes = arguments_content.as_ref().map_or(0, String::len);
        let arguments_kind = match &input {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let arguments_count = input.as_object().map_or_else(
            || input.as_array().map_or(1, Vec::len),
            serde_json::Map::len,
        );
        let argument_keys = trace_content
            .then(|| {
                input.as_object().map(|object| {
                    object
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
            })
            .flatten()
            .unwrap_or_default();
        let span = tool_execution_span(
            name,
            context,
            arguments_bytes,
            arguments_kind,
            arguments_count,
            &argument_keys,
        );
        let open = OpenToolSpan::new(&span);
        if let Some(arguments_content) = &arguments_content {
            record_tool_content(&span, "tool.arguments", arguments_content);
        }
        if self.excluded_code_tools.contains(name) {
            let refused = ToolOutput::error(format!(
                "hand tool `{name}` is not permitted for this session"
            ));
            open.refuse(&refused);
            return refused;
        }
        if !self.lists(name)
            && !self.routes_to_hand(&input)
            && !self.resolve(name).instrument(span.clone()).await
        {
            let refused = tool_not_available(name);
            open.refuse(&refused);
            return refused;
        }
        let (input, hand) = match &self.hand_calls {
            Some(calls) => match take_hand(self.declares_hand(name), input) {
                Ok((input, hand)) => (
                    input,
                    hand.filter(|hand| !calls.here(hand)).zip(Some(calls)),
                ),
                Err(refused) => {
                    open.refuse(&refused);
                    return refused;
                }
            },
            None => (input, None),
        };
        if let Some((hand, calls)) = hand {
            span.record("tool.hand", hand.as_str());
            let started_at = std::time::Instant::now();
            let execution = calls
                .call(&hand, name, input, context)
                .instrument(span.clone())
                .await;
            let output_content = trace_content
                .then(|| serde_json::to_string(&execution.output).ok())
                .flatten();
            open.finish(started_at, &execution, output_content.as_deref());
            return execution;
        }
        let queued_at = std::time::Instant::now();
        let exclusive = self.policies.iter().any(|policy| policy.exclusive(name));
        let origin = (!exclusive)
            .then(|| context.call_id().rsplit_once('/').map(|(origin, _)| origin))
            .flatten();
        let _admission = self
            .nested_gate
            .admit(exclusive, origin)
            .instrument(span.clone())
            .await;
        span.record(
            "tool.admission",
            if exclusive { "exclusive" } else { "shared" },
        );
        span.record(
            "tool.admission.wait_ns",
            u64::try_from(queued_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        let started_at = std::time::Instant::now();
        let dispatch = self
            .execute_nested_inner(name, input, context)
            .instrument(span.clone());
        let execution = match AssertUnwindSafe(dispatch).catch_unwind().await {
            Ok(execution) => execution,
            Err(payload) => panicked_tool_output(&span, payload),
        };
        let output_content = trace_content
            .then(|| serde_json::to_string(&execution.output).ok())
            .flatten();
        open.finish(started_at, &execution, output_content.as_deref());
        execution
    }

    async fn execute_nested_inner(
        &self,
        name: &str,
        input: Value,
        context: ToolContext<'_>,
    ) -> ToolOutput {
        if self
            .by_name
            .get(name)
            .is_some_and(|index| !self.exposures[*index].is_available_in_code_mode())
        {
            return ToolOutput::error(format!("unsupported nested tool call: {name}"));
        }
        let Some((handler, definition)) = self.get(name) else {
            let Some(provider) = self.provider_of(name) else {
                return tool_not_available(name);
            };
            if let Some(execution) = provider.execute(name, input, context).await {
                return execution;
            }
            return ToolOutput::error(format!("unsupported nested tool call: {name}"));
        };
        let input = match nested_input(definition, name, &input) {
            Ok(input) => input,
            Err(refused) => return refused,
        };
        match handler.execute(input, context).await {
            Ok(execution) => execution,
            Err(error) => ToolOutput::error(error.to_string()),
        }
    }

    fn recover_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> Option<crate::embedded::HostFuture<'a, Option<ToolOutput>>> {
        let input = match &self.hand_calls {
            Some(calls) => match take_hand(self.declares_hand(name), input) {
                Ok((_, Some(hand))) if !calls.here(&hand) => {
                    return Some(Box::pin(async move { calls.recover(&hand, context).await }));
                }
                Ok((input, _)) => input,
                Err(_) => return None,
            },
            None => input,
        };
        if let Some((handler, definition)) = self.get(name) {
            return handler.recover(nested_input(definition, name, &input).ok()?, context);
        }
        if self.providers.is_empty() {
            return None;
        }
        Some(Box::pin(async move {
            if !self.resolve(name).await {
                return None;
            }
            let provider = self.provider_of(name)?;
            if !provider.resumes(name) {
                return None;
            }
            provider.recover(name, input, context).await
        }))
    }

    pub(crate) fn workspace_tool(&self, name: &str) -> bool {
        self.policies
            .iter()
            .any(|policy| policy.workspace_routing(name).is_some())
    }

    pub(crate) fn callback_tools(&self, remote: &HashSet<String>) -> Vec<Arc<dyn Tool>> {
        self.ordered
            .iter()
            .zip(&self.definitions)
            .zip(&self.exposures)
            .filter(|((_, definition), exposure)| {
                let name = definition.name();
                exposure.is_available_in_code_mode()
                    && matches!(
                        definition,
                        ToolDefinition::Function { .. } | ToolDefinition::Custom { .. }
                    )
                    && !host_owned_name(name)
                    && !remote.contains(name)
                    && !self.workspace_tool(name)
            })
            .map(|((tool, _), _)| Arc::clone(tool))
            .collect()
    }

    pub(super) fn catalog(&self) -> &Arc<Catalog> {
        self.catalog.get_or_init(|| {
            Catalog::of(
                &self.definitions,
                &self.exposures,
                &self.excluded_code_tools,
            )
        })
    }

    pub(super) fn exclude_code_tools(&mut self, excluded: &HashSet<String>) {
        self.excluded_code_tools.clone_from(excluded);
        self.catalog = OnceLock::new();
    }

    fn provided_definitions(&self) -> Vec<ToolDefinition> {
        self.providers
            .iter()
            .flat_map(|provider| provider.available_definitions())
            .collect()
    }

    pub(crate) fn nested_tool_metadata(&self) -> Vec<Value> {
        self.shared_nested_tool_metadata().to_vec()
    }

    pub(crate) fn shared_nested_tool_metadata(&self) -> Arc<[Value]> {
        let provided = self.provided_definitions();
        if provided.is_empty() {
            return self.catalog().metadata();
        }
        let definitions = self.code_mode_definitions_with(provided);
        static LIVE: std::sync::LazyLock<Atom<Vec<std::sync::Weak<NestedMetadata>>>> =
            std::sync::LazyLock::new(Atom::default);
        let cached = self.nested_metadata.read(|cached| {
            cached
                .as_ref()
                .filter(|cached| cached.offers(&definitions))
                .map(|cached| Arc::clone(&cached.metadata))
        });
        if let Some(metadata) = cached {
            return metadata;
        }
        let live = LIVE.read(|live| {
            live.iter()
                .filter_map(std::sync::Weak::upgrade)
                .find(|entry| entry.offers(&definitions))
        });
        let entry = match live {
            Some(entry) => entry,
            None => {
                let metadata = definitions
                    .iter()
                    .map(|definition| definition_metadata(definition.name(), definition))
                    .collect();
                let entry = Arc::new(NestedMetadata {
                    definitions,
                    metadata,
                });
                let weak = Arc::downgrade(&entry);
                LIVE.update(|live| {
                    live.retain(|entry| entry.strong_count() > 0);
                    live.push(std::sync::Weak::clone(&weak));
                });
                entry
            }
        };
        let metadata = Arc::clone(&entry.metadata);
        self.nested_metadata.set(Some(entry));
        metadata
    }

    pub(super) fn from_ordered(ordered: Vec<Arc<dyn Tool>>) -> Self {
        let definitions = ordered
            .iter()
            .map(|tool| tool.definition())
            .collect::<Vec<_>>();
        let by_name = definitions
            .iter()
            .enumerate()
            .map(|(index, definition)| (definition.name().into(), index))
            .collect();
        Self {
            exposures: vec![ToolExposure::CodeModeOnly; definitions.len()],
            ordered,
            definitions,
            by_name,
            providers: Vec::new(),
            policies: Vec::new(),
            hand_calls: None,
            nested_gate: Arc::default(),
            excluded_code_tools: HashSet::new(),
            nested_metadata: Arc::default(),
            catalog: OnceLock::new(),
        }
    }

    pub(super) fn route_workspaces(
        &mut self,
        routes: &[RemoteRoute],
        store: Option<Arc<dyn ProcessRouteStore>>,
        pool: Option<&RunnerRouting>,
    ) {
        let router = match (pool, routes.first()) {
            (Some(pool), _) => Arc::clone(pool.router()),
            (None, Some(route)) => route.tools.router(),
            (None, None) => return,
        };
        router.route(&mut RoutingCatalog { registry: self }, routes, store, pool);
    }

    pub(super) fn set_all_exposures(&mut self, exposure: ToolExposure) {
        self.exposures.fill(exposure);
        self.catalog = OnceLock::new();
    }

    pub(super) fn extend(
        &mut self,
        tools: impl IntoIterator<Item = (Arc<dyn Tool>, ToolExposure)>,
    ) {
        for (tool, exposure) in tools {
            let definition = tool.definition();
            if self.by_name.contains_key(definition.name()) {
                tracing::warn!(
                    tool_name = definition.name(),
                    "skipping duplicate tool that is already registered"
                );
                continue;
            }
            let index = self.ordered.len();
            self.by_name.insert(definition.name().into(), index);
            self.definitions.push(definition);
            self.exposures.push(exposure);
            self.ordered.push(tool);
            self.catalog = OnceLock::new();
        }
    }

    fn declares_hand(&self, name: &str) -> bool {
        match self.get(name) {
            Some((_, definition)) => declares_hand(definition),
            None => self.providers.iter().any(|provider| {
                provider
                    .available_definitions()
                    .iter()
                    .any(|definition| definition.name() == name && declares_hand(definition))
            }),
        }
    }

    fn provider_of(&self, name: &str) -> Option<&Arc<dyn DynamicToolProvider>> {
        self.providers.iter().find(|provider| {
            provider
                .available_definitions()
                .iter()
                .any(|definition| definition.name() == name)
        })
    }

    fn lists(&self, name: &str) -> bool {
        self.by_name.contains_key(name) || self.provider_of(name).is_some()
    }

    fn routes_to_hand(&self, input: &Value) -> bool {
        self.hand_calls.as_ref().is_some_and(|calls| {
            input
                .get("hand")
                .and_then(Value::as_str)
                .is_some_and(|hand| !calls.here(hand))
        })
    }

    async fn resolve(&self, name: &str) -> bool {
        for provider in &self.providers {
            if provider.resolve(name).await {
                break;
            }
        }
        self.provider_of(name).is_some()
    }

    fn get(&self, name: &str) -> Option<(&Arc<dyn Tool>, &ToolDefinition)> {
        let index = *self.by_name.get(name)?;
        Some((self.ordered.get(index)?, self.definitions.get(index)?))
    }

    pub(crate) fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub(crate) fn direct_definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.definitions
            .iter()
            .zip(&self.exposures)
            .filter_map(|(definition, exposure)| exposure.is_direct().then_some(definition))
    }

    pub(crate) fn code_mode_tool_names(&self) -> Vec<(String, String)> {
        let mut names = self
            .model_code_mode_definitions()
            .into_iter()
            .map(|definition| {
                (
                    crate::selection::normalize_public_tool_name(definition.name()),
                    definition.name().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub(crate) fn registered_code_mode_definitions(&self) -> Vec<ToolDefinition> {
        self.catalog().registered.clone()
    }

    pub(crate) fn code_mode_tool_summaries(&self) -> Vec<(String, String)> {
        let registered = &self.catalog().normalized;
        let mut seen = HashSet::new();
        let mut summaries = self
            .providers
            .iter()
            .flat_map(|provider| provider.code_mode_tool_summaries())
            .filter_map(|(name, description)| {
                let normalized = crate::selection::normalize_public_tool_name(&name);
                (!host_owned_name(&name)
                    && !registered.contains(&normalized)
                    && seen.insert(normalized.clone()))
                .then_some((normalized, description))
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.0.cmp(&right.0));
        summaries
    }

    fn model_code_mode_definitions(&self) -> Vec<ToolDefinition> {
        first_normalized_definitions(
            self.catalog().registered.iter().cloned().chain(
                self.providers
                    .iter()
                    .filter(|provider| provider.model_catalog_visible())
                    .flat_map(|provider| provider.available_definitions())
                    .filter(|definition| !self.excluded_code_tools.contains(definition.name())),
            ),
        )
    }

    fn code_mode_definitions_with(&self, provided: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        first_normalized_definitions(
            self.catalog().nested.iter().cloned().chain(
                provided
                    .into_iter()
                    .filter(|definition| {
                        !host_owned_name(definition.name())
                            && !self.excluded_code_tools.contains(definition.name())
                    })
                    .map(nested_definition),
            ),
        )
    }
}

fn first_normalized_definitions(
    definitions: impl IntoIterator<Item = ToolDefinition>,
) -> Vec<ToolDefinition> {
    let mut seen = HashMap::new();
    definitions
        .into_iter()
        .filter(|definition| {
            let normalized = crate::selection::normalize_public_tool_name(definition.name());
            match seen.get(&normalized) {
                None => {
                    seen.insert(normalized, definition.name().to_owned());
                    true
                }
                Some(first) => {
                    if first != definition.name() {
                        tracing::warn!(
                            tool_name = definition.name(),
                            code_mode_name = normalized,
                            "skipping tool with a duplicate normalized Code Mode name"
                        );
                    }
                    false
                }
            }
        })
        .collect()
}

fn definition_metadata(name: &str, definition: &ToolDefinition) -> Value {
    let kind = match definition {
        ToolDefinition::Function { .. } => "function",
        ToolDefinition::Namespace { .. } => "namespace",
        ToolDefinition::Custom { .. } => "freeform",
        ToolDefinition::ToolSearch { .. } => "tool_search",
    };
    let metadata_name = crate::selection::normalize_public_tool_name(name);
    json!({
        "name": metadata_name,
        "tool_name": name,
        "description": definition.description(),
        "kind": kind,
        "input_schema": definition.parameters().map(|schema| schema.as_value()),
        "output_schema": definition.output_schema().map(|schema| schema.as_value()),
    })
}

fn same_metadata(built: &ToolDefinition, current: &ToolDefinition) -> bool {
    fn same<T: PartialEq + ?Sized>(built: Option<&T>, current: Option<&T>) -> bool {
        match (built, current) {
            (Some(built), Some(current)) => std::ptr::eq(built, current) || built == current,
            (None, None) => true,
            _ => false,
        }
    }
    std::mem::discriminant(built) == std::mem::discriminant(current)
        && built.name() == current.name()
        && same(Some(built.description()), Some(current.description()))
        && same(
            built.parameters().map(|schema| schema.as_value()),
            current.parameters().map(|schema| schema.as_value()),
        )
        && same(
            built.output_schema().map(|schema| schema.as_value()),
            current.output_schema().map(|schema| schema.as_value()),
        )
}

fn nested_definition(definition: ToolDefinition) -> ToolDefinition {
    match definition {
        ToolDefinition::ToolSearch {
            description,
            parameters,
            ..
        } => ToolDefinition::function("tool_search", description, parameters),
        other => other,
    }
}

fn nested_input(
    definition: &ToolDefinition,
    name: &str,
    input: &Value,
) -> std::result::Result<ToolInput, ToolOutput> {
    match definition {
        ToolDefinition::Function { .. } | ToolDefinition::ToolSearch { .. }
            if !input.is_object() =>
        {
            Err(ToolOutput::error(format!(
                "nested function tool {name} requires an object argument"
            )))
        }
        ToolDefinition::Function { .. } | ToolDefinition::ToolSearch { .. } => to_raw_value(input)
            .map(ToolInput::Function)
            .map_err(|error| ToolOutput::error(format!("failed to encode {name} input: {error}"))),
        ToolDefinition::Custom { .. } => freeform(input)
            .map(|input| ToolInput::Freeform(input.to_owned()))
            .ok_or_else(|| {
                ToolOutput::error(format!(
                    "nested freeform tool {name} requires a string argument or {{ input }}"
                ))
            }),
        ToolDefinition::Namespace { .. } => Err(ToolOutput::error(
            "Responses namespace definitions cannot execute as nested Code Mode tools",
        )),
    }
}

fn freeform(input: &Value) -> Option<&str> {
    match input {
        Value::String(input) => Some(input),
        Value::Object(fields) if fields.len() == 1 => fields.get("input")?.as_str(),
        _ => None,
    }
}

impl nanocode_code_mode::CodeModeToolset for ToolRegistry {
    fn nested_tool_metadata(&self) -> Vec<Value> {
        Self::nested_tool_metadata(self)
    }

    fn shared_nested_tool_metadata(&self) -> Arc<[Value]> {
        Self::shared_nested_tool_metadata(self)
    }

    fn execute_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> crate::embedded::HostFuture<'a, ToolOutput> {
        Box::pin(Self::execute_nested(self, name, input, context))
    }

    fn recover_nested<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> Option<crate::embedded::HostFuture<'a, Option<ToolOutput>>> {
        Self::recover_nested(self, name, input, context)
    }

    fn cell_observed(&self, origin_call_id: &str, observed: bool) {
        self.nested_gate.observed(origin_call_id, observed);
    }

    fn append_observation_content(
        &self,
        calls: &[nanocode_code_mode::CodeModeToolObservation<'_>],
        content: &mut Vec<crate::ToolOutputContent>,
    ) {
        for policy in &self.policies {
            policy.append_observation_content(calls, content);
        }
    }

    fn session_notices(&self, session_id: &str) -> Vec<String> {
        self.policies
            .iter()
            .flat_map(|policy| policy.session_notices(session_id))
            .collect()
    }
}

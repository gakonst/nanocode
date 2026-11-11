use super::{
    capacity::TurnCapacity,
    message::MAX_MESSAGE_BYTES,
    model::{
        AgentDescriptor, AgentId, AgentStatus, AgentUpdate, MessageId, MessagePriority,
        MessagePurpose, agent_prompt,
    },
    policy::{SpawnParent, SpawnPolicy, SpawnPolicyError, SpawnProvider},
    runtime::{
        AgentDirectoryEntry, AgentSummary, OutputContract, Registry, SubmissionOutcome,
        forward_events,
    },
};
use async_trait::async_trait;
use futures_util::future::join_all;
use nanocode_agent_core::usage::{UsdAmount, usd_number};
use nanocode_agent_core::{
    AgentHandle, ClaudeModel, ForkRequest, HarnessFamily, Model, ModelSelection, ReasoningMode,
    SpawnOptions, Thinking,
};
use nanocode_sync::Atom;
use nanocode_tool_contracts::{
    Tool, ToolCollection, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};
use tokio::sync::oneshot;

const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

macro_rules! recovers {
    ($this:ident, $input:ident, $context:ident, $answer:expr) => {
        #[cfg(not(target_family = "wasm"))]
        fn recover<'life0, 'life1, 'async_trait>(
            &'life0 $this,
            $input: ToolInput,
            $context: ToolContext<'life1>,
        ) -> Option<
            std::pin::Pin<
                Box<dyn std::future::Future<Output = Option<ToolOutput>> + Send + 'async_trait>,
            >,
        >
        where
            'life0: 'async_trait,
            'life1: 'async_trait,
            Self: 'async_trait,
        {
            Some(Box::pin(async move { Some(answered($answer.await)) }))
        }

        #[cfg(target_family = "wasm")]
        fn recover<'life0, 'life1, 'async_trait>(
            &'life0 $this,
            $input: ToolInput,
            $context: ToolContext<'life1>,
        ) -> Option<
            std::pin::Pin<Box<dyn std::future::Future<Output = Option<ToolOutput>> + 'async_trait>>,
        >
        where
            'life0: 'async_trait,
            'life1: 'async_trait,
            Self: 'async_trait,
        {
            Some(Box::pin(async move { Some(answered($answer.await)) }))
        }
    };
}

fn answered(result: ToolResult) -> ToolOutput {
    result.unwrap_or_else(|error| ToolOutput::error(error.to_string()))
}
const SPAWN_AGENT_TOOL: &str = "spawn_agent";
const SUBMIT_RESULT_TOOL: &str = "submit_result";
const SEND_AGENT_MESSAGE_TOOL: &str = "send_agent_message";
const LIST_AGENTS_TOOL: &str = "list_agents";
const LIST_MODELS_TOOL: &str = "list_models";
const WAIT_AGENT_TOOL: &str = "wait_agent";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTask {
    pub role: String,
    pub task: String,
    pub output_schema: Value,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum OutputContractNode {
    Object { fields: Vec<OutputContractField> },
    Array { items: Box<Self> },
    String,
    StringEnum { values: Vec<String> },
    Integer,
    Number,
    Boolean,
    Null,
    Any,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputContractField {
    name: String,
    schema: OutputContractNode,
    required: bool,
}

impl OutputContractNode {
    fn into_schema(self) -> std::io::Result<Value> {
        Ok(match self {
            Self::Object { fields } => {
                let mut deduplicated = BTreeMap::new();
                for field in fields {
                    let name = field.name;
                    if deduplicated.contains_key(&name) {
                        return Err(std::io::Error::other(format!(
                            "duplicate output_contract field name: {name}"
                        )));
                    }
                    deduplicated.insert(name, (field.schema, field.required));
                }
                let mut properties = Map::new();
                let mut required = Vec::new();
                for (name, (schema, is_required)) in deduplicated {
                    if is_required {
                        required.push(name.clone());
                    }
                    properties.insert(name, schema.into_schema()?);
                }
                json!({ "type": "object", "properties": properties,
                    "required": required, "additionalProperties": false })
            }
            Self::Array { items } => json!({ "type": "array", "items": items.into_schema()? }),
            Self::String => json!({ "type": "string" }),
            Self::StringEnum { values } => {
                if values.is_empty() {
                    return Err(std::io::Error::other(
                        "output_contract string_enum requires at least one value",
                    ));
                }
                json!({ "type": "string", "enum": values })
            }
            Self::Integer => json!({ "type": "integer" }),
            Self::Number => json!({ "type": "number" }),
            Self::Boolean => json!({ "type": "boolean" }),
            Self::Null => json!({ "type": "null" }),
            Self::Any => Value::Bool(true),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentTask {
    role: String,
    task: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    harness: Option<HarnessFamily>,
    #[serde(default)]
    thinking: Option<Thinking>,
    #[serde(default)]
    reasoning_mode: Option<ReasoningMode>,
    #[serde(default, with = "usd_number")]
    budget_usd: Option<UsdAmount>,

    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    output_contract: Option<OutputContractNode>,
}

impl SpawnAgentTask {
    fn into_parts_with_parent(
        self,
        parent: Option<HarnessFamily>,
        providers: Option<&[SpawnProvider]>,
    ) -> std::io::Result<(AgentTask, SpawnOptions, Option<UsdAmount>)> {
        let output_schema = match (self.output_contract, self.output_schema) {
            (Some(contract), None) => contract.into_schema()?,
            (None, Some(schema)) => schema,
            _ => {
                return Err(std::io::Error::other(
                    "provide exactly one of output_contract or legacy output_schema",
                ));
            }
        };
        let mut options =
            SpawnOptions::new().reasoning_mode(self.reasoning_mode.unwrap_or_default());
        if let Some(model) = self.model {
            let routed = providers.unwrap_or_default().iter().find_map(|provider| {
                let id = model
                    .strip_prefix(provider.name.as_str())?
                    .strip_prefix('/')?;
                Some((provider, id.to_owned()))
            });
            let (provider, model) = match routed {
                Some((provider, id)) => (Some(provider), id),
                None => (None, model),
            };
            let backend = match self.harness.clone() {
                Some(harness) => harness,
                None => match provider.and_then(|provider| listed_family(provider, &model)) {
                    Some(family) => family,
                    None => inferred_harness(parent, &model)?,
                },
            };
            let model = ModelSelection::new(backend, model);
            let model = if let Some(known) = model.as_codex() {
                ModelSelection::Codex(known)
            } else if let Some(known) = model.as_claude() {
                ModelSelection::Claude(known)
            } else {
                model
            };
            if let Some(provider) = provider {
                options = options.provider(&provider.name);
            }
            options = options.harness_model(model);
        }
        if let Some(harness) = self.harness {
            options = options.harness(harness);
        }
        if let Some(thinking) = self.thinking {
            options = options.thinking(thinking);
        }
        options.validate_harness().map_err(std::io::Error::other)?;
        Ok((
            AgentTask {
                role: self.role,
                task: self.task,
                output_schema,
            },
            options,
            self.budget_usd,
        ))
    }
}

#[derive(Serialize)]
pub struct AgentStartReport {
    pub agent_id: AgentId,
    pub role: String,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    pub reasoning_mode: ReasoningMode,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "usd_number::serialize"
    )]
    pub budget_usd: Option<UsdAmount>,
}

fn listed_family(provider: &SpawnProvider, model: &str) -> Option<HarnessFamily> {
    provider
        .models
        .iter()
        .find(|listed| listed.as_str() == model)
        .map(ModelSelection::family)
}

fn inferred_harness(parent: Option<HarnessFamily>, model: &str) -> std::io::Result<HarnessFamily> {
    let parent = parent
        .ok_or_else(|| std::io::Error::other("a model requires a backend selection or parent"))?;
    if !HarnessFamily::builtin().contains(&parent) {
        return Ok(parent);
    }
    if Model::known(model).is_some() {
        return Ok(HarnessFamily::Codex);
    }
    if model.parse::<ClaudeModel>().is_ok() {
        return Ok(HarnessFamily::Claude);
    }
    Ok(parent)
}

async fn spawn_parent(
    parent: &AgentHandle,
    registry: &Registry,
    session_id: &str,
) -> AgentToolResult<SpawnParent> {
    let registered = registry.descriptor_for_session(session_id).await.is_some();
    let (model, thinking) = parent.settings().await?;
    Ok(SpawnParent {
        model,
        registered,
        thinking: Some(thinking),
        reasoning_mode: parent.reasoning_mode().await?,
    })
}

fn admit_options(
    policy: &SpawnPolicy,
    parent: &SpawnParent,
    options: SpawnOptions,
) -> AgentToolResult<SpawnOptions> {
    let thinking = parent
        .thinking
        .ok_or(SpawnPolicyError::ParentThinkingUnknown)?;
    let options = options.resolve(parent.model.clone(), thinking)?;
    policy.admit(
        parent,
        options.selected_harness_model().as_ref(),
        options.selected_thinking(),
    )?;
    policy.admit_reasoning_mode(
        parent,
        options.selected_reasoning_mode().unwrap_or_default(),
    )?;
    Ok(options)
}

async fn child_settings(
    child: &nanocode_agent_core::Agent,
    policy: Option<(&Registry, &SpawnParent)>,
) -> std::io::Result<(Option<ModelSelection>, Option<Thinking>, ReasoningMode)> {
    match child.settings().await {
        Ok((model, thinking, reasoning_mode)) => {
            if let Some((registry, parent)) = policy {
                let policy = registry.spawn_policy();
                policy.admit(parent, Some(&model), Some(thinking))?;
                policy.admit_reasoning_mode(parent, reasoning_mode)?;
            }
            Ok((Some(model), Some(thinking), reasoning_mode))
        }
        Err(error) => match policy {
            Some((registry, _)) if !registry.spawn_policy().is_unrestricted() => {
                Err(SpawnPolicyError::ChildSettingsUnknown {
                    reason: error.to_string(),
                }
                .into())
            }
            _ => Ok((None, None, ReasoningMode::Standard)),
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitTask {
    agent_ids: Vec<AgentId>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetAgent {
    agent_id: AgentId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryTask {
    #[serde(default)]
    include_completed: bool,
    #[serde(default)]
    include_self: bool,
    #[serde(default)]
    after_agent_id: Option<AgentId>,
    #[serde(default)]
    limit: Option<std::num::NonZeroUsize>,
}

#[derive(Serialize)]
struct AgentDirectory {
    agents: Vec<AgentDirectoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_after_agent_id: Option<AgentId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendMessageTask {
    agent_id: AgentId,
    message: String,
    #[serde(default)]
    priority: MessagePriority,
    #[serde(default)]
    purpose: MessagePurpose,
    #[serde(default)]
    in_reply_to: Option<MessageId>,
}

#[derive(Serialize)]
struct WaitReport {
    agents: Vec<Value>,
    timed_out: bool,
}

#[derive(Serialize)]
struct LifecycleReport {
    agents: Vec<Value>,
}

fn agent_reports(
    agents: Vec<AgentSummary>,
    delivered: &std::collections::HashSet<AgentId>,
) -> serde_json::Result<Vec<Value>> {
    agents
        .into_iter()
        .map(|agent| {
            let id = agent.agent_id;
            let mut report = serde_json::to_value(AgentSummary {
                task: crate::runtime::bounded_summary(&agent.task),
                ..agent
            })?;
            if delivered.contains(&id)
                && let Some(status) = report.get_mut("status").and_then(Value::as_object_mut)
            {
                status.remove("output");
            }
            Ok(report)
        })
        .collect()
}

fn json_output(value: &impl Serialize) -> ToolResult {
    Ok(ToolOutput::from_json(serde_json::to_value(value)?, true))
}

pub type AgentToolResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

pub async fn start_agent(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
) -> AgentToolResult<AgentStartReport> {
    start_agent_with(parent, registry, session_id, task, SpawnOptions::new()).await
}

pub async fn start_agents(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    tasks: Vec<AgentTask>,
) -> AgentToolResult<Vec<AgentStartReport>> {
    start_agents_observed(parent, registry, session_id, tasks, |_| {}).await
}

#[doc(hidden)]
pub async fn start_agents_observed(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    tasks: Vec<AgentTask>,
    observe_session: impl Fn(&str) + Send + Sync + 'static,
) -> AgentToolResult<Vec<AgentStartReport>> {
    registry.register_handle(parent.clone());

    let policy_parent = spawn_parent(parent, registry, session_id).await?;
    let prepared = prepare_batch(tasks)?;
    let capacities = registry.reserve_turns(prepared.len()).await?;
    let admission = registry.admit_spawn(session_id).await?;
    let mut startup = registry.batch_startup();
    let reservations = registry.reserve_many(session_id, prepared.len()).await?;
    drop(admission);
    let host_context = registry.host_context_for_session(session_id).await;
    let children = if let Some(router) = registry.spawn_router() {
        let mut routes = Vec::with_capacity(prepared.len());
        for (task, _) in &prepared {
            let route = router
                .resolve_spawn(
                    session_id,
                    &task.role,
                    &task.task,
                    SpawnOptions::new(),
                    host_context.as_deref(),
                )
                .await?;
            route.validate(SpawnOptions::new())?;
            let options = admit_options(
                &registry.spawn_policy(),
                &policy_parent,
                route.options(SpawnOptions::new()),
            )?;
            routes.push((route, options));
        }
        let mut children: Vec<(nanocode_agent_core::Agent, nanocode_agent_core::AgentEvents)> =
            Vec::with_capacity(routes.len());
        for (route, options) in routes {
            let options = match admit_options(&registry.spawn_policy(), &policy_parent, options) {
                Ok(options) => options,
                Err(error) => {
                    for (child, _) in &children {
                        child.abandon_created().await;
                    }
                    return Err(error);
                }
            };
            let outcome = parent
                .spawn_with_host_context(options, host_context.as_ref().map(Arc::clone))
                .await;
            let child = match outcome {
                Ok(child) => child,
                Err(error) => {
                    for (child, _) in &children {
                        child.abandon_created().await;
                    }
                    return Err(error.into());
                }
            };
            observe_session(child.0.session_id());
            if let Err(error) = router.bind(
                session_id,
                child.0.session_id(),
                route.reference(),
                host_context.as_deref(),
            ) {
                child.0.abandon_created().await;
                for (child, _) in &children {
                    child.abandon_created().await;
                }
                return Err(error.into());
            }
            children.push(child);
        }
        children
    } else {
        admit_options(
            &registry.spawn_policy(),
            &policy_parent,
            SpawnOptions::new(),
        )?;
        parent
            .spawn_many_observed_with_host_context(
                prepared.len(),
                observe_session,
                host_context.as_ref().map(Arc::clone),
            )
            .await?
    };

    let mut settings = Vec::with_capacity(children.len());
    for (child, _) in &children {
        match child_settings(child, Some((registry, &policy_parent))).await {
            Ok(value) => settings.push(value),
            Err(error) => {
                for (child, _) in &children {
                    child.abandon_created().await;
                }
                return Err(error.into());
            }
        }
    }

    let mut reports = Vec::with_capacity(prepared.len());
    let mut launches = Vec::with_capacity(prepared.len());
    let mut additions = Vec::with_capacity(prepared.len());
    for (
        (((reservation, (task, contract)), (child, events)), capacity),
        (model, thinking, reasoning_mode),
    ) in reservations
        .into_iter()
        .zip(prepared)
        .zip(children)
        .zip(capacities)
        .zip(settings)
    {
        let id = reservation.id;
        let descriptor = AgentDescriptor {
            id,
            session_id: child.session_id().to_string(),
            role: task.role.clone(),
            task: task.task.clone(),
            parent: reservation.parent,
            model: model.clone(),
            thinking,
            reasoning_mode,
            budget_usd: None,
        };
        let (start_events, events_ready) = oneshot::channel();
        let event_task = forward_events(
            reservation.root_session_id.clone(),
            id,
            reservation.parent,
            events,
            events_ready,
            Arc::downgrade(registry),
            registry.updates.clone(),
        );
        let marks = match registry
            .insert(
                reservation.root_session_id.clone(),
                descriptor.clone(),
                host_context.as_ref().map(Arc::clone),
                child,
                event_task,
                contract,
                None,
            )
            .await
        {
            Ok(marks) => marks,
            Err(error) => return Err(startup.fail(error).await.into()),
        };
        startup.track(&reservation.root_session_id, id);
        reports.push(AgentStartReport {
            agent_id: id,
            role: task.role,
            status: AgentStatus::Running,
            model: model.clone(),
            thinking,
            reasoning_mode,
            budget_usd: None,
        });
        additions.push((
            reservation.root_session_id.clone(),
            descriptor,
            start_events,
            marks,
        ));
        launches.push((
            reservation.root_session_id,
            id,
            agent_prompt(id, &task.task, None),
            capacity,
        ));
    }

    for (root_session_id, descriptor, start_events, marks) in additions {
        registry.send(&root_session_id, AgentUpdate::Added(descriptor));
        let _ = start_events.send((0, marks));
    }

    let launches = launches
        .into_iter()
        .map(|(root_session_id, id, prompt, capacity)| async move {
            registry
                .launch_initial_turn(&root_session_id, id, prompt, capacity)
                .await
        });
    if let Some(error) = join_all(launches).await.into_iter().find_map(Result::err) {
        return Err(startup.fail(error).await.into());
    }
    startup.commit().await?;
    Ok(reports)
}

fn prepare_batch(tasks: Vec<AgentTask>) -> AgentToolResult<Vec<(AgentTask, OutputContract)>> {
    tasks
        .into_iter()
        .map(|task| {
            let contract = OutputContract::compile(&task.output_schema)?;
            Ok((task, contract))
        })
        .collect()
}

pub async fn start_agent_with(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
    options: SpawnOptions,
) -> AgentToolResult<AgentStartReport> {
    let host_context = registry.host_context_for_session(session_id).await;
    start_agent_with_host_context(parent, registry, session_id, task, options, host_context).await
}

pub async fn start_fork_agent(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
) -> AgentToolResult<AgentStartReport> {
    let host_context = registry.host_context_for_session(session_id).await;
    start_child(
        parent,
        registry,
        session_id,
        task,
        SpawnOptions::new(),
        None,
        host_context,
        true,
        None,
        None,
    )
    .await
}

async fn start_agent_with_host_context(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
    options: SpawnOptions,
    host_context: Option<Arc<str>>,
) -> AgentToolResult<AgentStartReport> {
    start_child(
        parent,
        registry,
        session_id,
        task,
        options,
        None,
        host_context,
        false,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn start_child(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
    options: SpawnOptions,
    budget: Option<UsdAmount>,
    host_context: Option<Arc<str>>,
    fork: bool,
    capacity: Option<TurnCapacity>,
    call_id: Option<&str>,
) -> AgentToolResult<AgentStartReport> {
    if fork && parent.session_id() != session_id {
        return Err("child caller identity must match its native parent handle".into());
    }
    if let Some(budget) = budget {
        admit_budget(registry, session_id, budget).await?;
    }
    registry.register_handle(parent.clone());
    let policy_parent = spawn_parent(parent, registry, session_id).await?;
    if !fork {
        registry.spawn_policy().admit_reasoning_mode(
            &policy_parent,
            options.selected_reasoning_mode().unwrap_or_default(),
        )?;
    }

    let AgentTask {
        role,
        task,
        output_schema,
    } = task;
    let contract = OutputContract::compile(&output_schema)?;
    let capacity = match capacity {
        Some(capacity) => capacity,
        None => registry.reserve_turn().await?,
    };
    let admission = registry.admit_spawn(session_id).await?;
    let mut startup = registry.batch_startup();
    let reservation = registry.reserve(session_id).await?;
    drop(admission);
    let id = reservation.id;
    let host_context = match host_context {
        Some(host_context) => Some(host_context),
        None => registry.host_context_for_session(session_id).await,
    };
    let router = if fork { None } else { registry.spawn_router() };
    let route = if let Some(router) = &router {
        let route = router
            .resolve_spawn(
                session_id,
                &role,
                &task,
                options.clone(),
                host_context.as_deref(),
            )
            .await?;
        route.validate(options.clone())?;
        Some(route)
    } else {
        None
    };
    let options = if fork {
        options
    } else {
        admit_options(
            &registry.spawn_policy(),
            &policy_parent,
            route
                .as_ref()
                .map_or_else(|| options.clone(), |route| route.options(options.clone())),
        )?
    };
    let (child, events) = if fork {
        parent.fork(ForkRequest::latest()).await?
    } else {
        parent
            .spawn_with_host_context(options, host_context.as_ref().map(Arc::clone))
            .await?
    };
    let (model, thinking, reasoning_mode) = match child_settings(
        &child,
        (!fork).then_some((registry.as_ref(), &policy_parent)),
    )
    .await
    {
        Ok(settings) => settings,
        Err(error) => {
            child.abandon_created().await;
            return Err(error.into());
        }
    };
    if let (Some(router), Some(route)) = (&router, &route)
        && let Err(error) = router.bind(
            session_id,
            child.session_id(),
            route.reference(),
            host_context.as_deref(),
        )
    {
        let _ = child.shutdown().await;
        return Err(error.into());
    }
    let caller = session_id;
    let session_id = child.session_id().to_string();
    let descriptor = AgentDescriptor {
        id,
        session_id,
        role: role.clone(),
        task: task.clone(),
        parent: reservation.parent,
        model: model.clone(),
        thinking,
        reasoning_mode,
        budget_usd: budget,
    };
    let (start_events, events_ready) = oneshot::channel();
    let event_task = forward_events(
        reservation.root_session_id.clone(),
        id,
        reservation.parent,
        events,
        events_ready,
        Arc::downgrade(registry),
        registry.updates.clone(),
    );
    let marks = registry
        .insert(
            reservation.root_session_id.clone(),
            descriptor.clone(),
            host_context,
            child,
            event_task,
            contract,
            call_id.map(|call| super::durable::Spawned {
                session: caller.to_owned(),
                call: call.to_owned(),
            }),
        )
        .await?;
    startup.track(&reservation.root_session_id, id);
    registry.send(&reservation.root_session_id, AgentUpdate::Added(descriptor));
    let _ = start_events.send((0, marks));

    if let Err(error) = registry
        .launch_initial_turn(
            &reservation.root_session_id,
            id,
            agent_prompt(id, &task, budget),
            capacity,
        )
        .await
    {
        return Err(startup.fail(error).await.into());
    }
    startup.commit().await?;
    Ok(AgentStartReport {
        agent_id: id,
        role,
        status: AgentStatus::Running,
        model,
        thinking,
        reasoning_mode,
        budget_usd: budget,
    })
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_child(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
    options: SpawnOptions,
    budget: Option<UsdAmount>,
    host_context: Option<Arc<str>>,
    call_id: &str,
) -> AgentToolResult<AgentStartReport> {
    if let Some(budget) = budget {
        admit_budget(registry, session_id, budget).await?;
    }
    registry.register_handle(parent.clone());
    let policy_parent = spawn_parent(parent, registry, session_id).await?;
    registry.spawn_policy().admit_reasoning_mode(
        &policy_parent,
        options.selected_reasoning_mode().unwrap_or_default(),
    )?;
    let AgentTask {
        role,
        task,
        output_schema,
    } = task;
    let contract = OutputContract::compile(&output_schema)?;
    let admitted = match registry.spawn_router() {
        Some(_) => options.clone(),
        None => admit_options(&registry.spawn_policy(), &policy_parent, options.clone())?,
    };
    let admission = registry.admit_spawn(session_id).await?;
    let reservation = registry.reserve(session_id).await?;
    drop(admission);
    let id = reservation.id;
    let host_context = match host_context {
        Some(host_context) => Some(host_context),
        None => registry.host_context_for_session(session_id).await,
    };
    let descriptor = AgentDescriptor {
        id,
        session_id: nanocode_platform::uuid_v7().to_string(),
        role: role.clone(),
        task,
        parent: reservation.parent,
        model: admitted.selected_harness_model(),
        thinking: admitted.selected_thinking(),
        reasoning_mode: admitted.selected_reasoning_mode().unwrap_or_default(),
        budget_usd: budget,
    };
    registry
        .insert_unstarted(
            reservation.root_session_id.clone(),
            descriptor.clone(),
            host_context,
            contract,
            Some(super::durable::Spawned {
                session: session_id.to_owned(),
                call: call_id.to_owned(),
            }),
            super::durable::Unstarted::of(&options),
        )
        .await?;
    registry.send(
        &reservation.root_session_id,
        AgentUpdate::Added(descriptor.clone()),
    );
    Ok(AgentStartReport {
        agent_id: id,
        role,
        status: AgentStatus::Running,
        model: descriptor.model,
        thinking: descriptor.thinking,
        reasoning_mode: descriptor.reasoning_mode,
        budget_usd: budget,
    })
}

pub(super) async fn start_unstarted(
    registry: &Arc<Registry>,
    root_session_id: &str,
    id: AgentId,
    capacity: TurnCapacity,
) {
    if let Err(error) = build_unstarted(registry, root_session_id, id, capacity).await {
        registry
            .fail_unstarted(
                root_session_id,
                id,
                format!("subagent {id} could not start: {error}"),
            )
            .await;
    }
}

async fn build_unstarted(
    registry: &Arc<Registry>,
    root_session_id: &str,
    id: AgentId,
    capacity: TurnCapacity,
) -> AgentToolResult<()> {
    let Some(child) = registry.unstarted(root_session_id, id).await else {
        return Ok(());
    };
    let _resident = child.lock.enter().await;
    if let Some(parent) = child.descriptor.parent {
        registry.ensure_resident(root_session_id, parent).await?;
    }
    let parent = registry
        .session_handle(&child.parent_session)
        .ok_or("the runtime of the agent that spawned it is unavailable")?;
    let policy_parent = spawn_parent(&parent, registry, &child.parent_session).await?;
    let options = child.unstarted.options(&child.descriptor.session_id);
    let router = registry.spawn_router();
    let route = if let Some(router) = &router {
        let route = router
            .resolve_spawn(
                &child.parent_session,
                &child.descriptor.role,
                &child.descriptor.task,
                options.clone(),
                child.host_context.as_deref(),
            )
            .await?;
        route.validate(options.clone())?;
        Some(route)
    } else {
        None
    };
    let options = admit_options(
        &registry.spawn_policy(),
        &policy_parent,
        route
            .as_ref()
            .map_or_else(|| options.clone(), |route| route.options(options.clone())),
    )?
    .session_id(&child.descriptor.session_id);
    let schema = OutputContract::compile(&child.output_schema)?.into_schema();
    let (agent, events) = parent
        .spawn_with_host_context(options, child.host_context.clone())
        .await?;
    let (model, thinking, reasoning_mode) =
        match child_settings(&agent, Some((registry.as_ref(), &policy_parent))).await {
            Ok(settings) => settings,
            Err(error) => {
                agent.abandon_created().await;
                return Err(error.into());
            }
        };
    if let (Some(router), Some(route)) = (&router, &route)
        && let Err(error) = router.bind(
            &child.parent_session,
            agent.session_id(),
            route.reference(),
            child.host_context.as_deref(),
        )
    {
        let _ = agent.shutdown().await;
        return Err(error.into());
    }
    registry
        .attach_started(
            root_session_id,
            id,
            agent,
            events,
            model,
            thinking,
            reasoning_mode,
            schema,
        )
        .await?;
    registry
        .launch_initial_turn(
            root_session_id,
            id,
            agent_prompt(id, &child.descriptor.task, child.descriptor.budget_usd),
            capacity,
        )
        .await?;
    Ok(())
}

async fn admit_budget(
    registry: &Registry,
    session_id: &str,
    budget: UsdAmount,
) -> AgentToolResult<()> {
    if !registry.spawn_policy().budgets {
        return Err(
            "budget_usd is not offered here: this host does not meter what subagents spend".into(),
        );
    }
    if budget.nano_usd() == 0 {
        return Err("budget_usd must be more than 0".into());
    }
    let bound = registry
        .budgets(session_id)
        .await
        .into_iter()
        .min_by_key(|(_, limit)| limit.nano_usd());
    if let Some((owner, limit)) = bound
        && budget.nano_usd() > limit.nano_usd()
    {
        return Err(format!(
            "budget_usd {budget} exceeds the {limit} that agent {owner} and its subagents may spend together"
        )
        .into());
    }
    Ok(())
}

struct SpawnAgent {
    parent: AgentHandle,
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for SpawnAgent {
    fn definition(&self) -> ToolDefinition {
        static LATEST: LazyLock<Atom<Option<Offered>>> = LazyLock::new(Atom::default);
        let Some(registry) = self.registry.upgrade() else {
            return spawn_agent_definition(SpawnPolicy::default());
        };
        let latest = registry.spawn_policy.read(|policy| {
            LATEST.read(|latest| {
                latest
                    .as_ref()
                    .filter(|latest| std::ptr::eq(Arc::as_ptr(&latest.policy), policy))
                    .map(|latest| latest.definition.clone())
            })
        });
        if let Some(definition) = latest {
            return definition;
        }
        let policy = registry.spawn_policy.get();
        let definition = spawn_agent_definition(SpawnPolicy::clone(&policy));
        LATEST.set(Some(Offered {
            policy,
            definition: definition.clone(),
        }));
        definition
    }

    recovers!(self, input, context, self.spawn(input, context, true));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.spawn(input, context, false).await
    }
}

struct Offered {
    policy: Arc<SpawnPolicy>,
    definition: ToolDefinition,
}

fn spawn_agent_definition(mut policy: SpawnPolicy) -> ToolDefinition {
    static DEFINITIONS: LazyLock<Atom<HashMap<String, ToolDefinition>>> =
        LazyLock::new(Atom::default);
    for provider in policy.providers.iter_mut().flatten() {
        provider.models.clear();
        provider.note = None;
    }
    let key = format!("{policy:?}");
    if let Some(definition) = DEFINITIONS.read(|definitions| definitions.get(&key).cloned()) {
        return definition;
    }
    let description = "Starts a reusable clean-room subagent without inherited conversation history and immediately returns its ID. When the root agent is durable, the task tree survives runtime restarts with the same IDs and interrupted children resume from their latest committed checkpoint; otherwise children are dropped when the parent runtime restarts. Delegate meaningful, separable work and handle trivial or tightly coupled work directly. When Code Mode is available, build multi-agent pipelines in it: start independent subtasks in parallel, await and reduce their results, then dispatch dependent stages. Do not repeat delegated work; double-check results against the relevant evidence before relying on them. Use output contracts that expose the fields downstream stages need, keep concurrent write scopes disjoint, and own final synthesis and verification.";
    let description = match policy.providers {
        Some(_) => format!(
            "{description} A model a provider serves runs as <provider>/<model>, paid as that provider charges (list_models lists them), so a question about models or providers can be answered by spawning a subagent on each."
        ),
        None => description.to_owned(),
    };
    let definition = ToolDefinition::function(
        SPAWN_AGENT_TOOL,
        description,
        spawn_agent_parameters_for(&policy),
    )
    .with_strict_parameters()
    .with_output_schema(SPAWN_AGENT_OUTPUT_SCHEMA.clone());
    DEFINITIONS.update(|definitions| {
        definitions.insert(key.clone(), definition.clone());
    });
    definition
}

impl SpawnAgent {
    async fn spawn(&self, input: ToolInput, context: ToolContext<'_>, resumed: bool) -> ToolResult {
        if resumed {
            let registry = self
                .registry
                .upgrade()
                .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
            if let Some(descriptor) = registry
                .respawned(context.session_id(), context.call_id())
                .await?
            {
                registry
                    .spawned_in_turn(context.session_id(), descriptor.id)
                    .await;
                return json_output(&AgentStartReport {
                    agent_id: descriptor.id,
                    role: descriptor.role,
                    status: AgentStatus::Running,
                    model: descriptor.model,
                    thinking: descriptor.thinking,
                    reasoning_mode: descriptor.reasoning_mode,
                    budget_usd: descriptor.budget_usd,
                });
            }
        }
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let policy = registry.spawn_policy();
        let (task, options, budget) = input
            .decode_json::<SpawnAgentTask>()?
            .into_parts_with_parent(
                Some(self.parent.harness_family()),
                policy.providers.as_deref(),
            )?;
        let host_context = context.host_context().map(Arc::<str>::from);
        let lineage = Arc::clone(&registry);
        if registry.queues_spawns() {
            let report = enqueue_child(
                &self.parent,
                &registry,
                context.session_id(),
                task,
                options,
                budget,
                host_context,
                context.call_id(),
            )
            .await?;
            lineage
                .spawned_in_turn(context.session_id(), report.agent_id)
                .await;
            return json_output(&report);
        }
        let capacity = registry.reserve_turn().await?;
        let report = {
            let parent = self.parent.clone();
            let session_id = context.session_id().to_owned();
            let call_id = context.call_id().to_owned();
            let pending = super::platform::spawn(async move {
                start_child(
                    &parent,
                    &registry,
                    &session_id,
                    task,
                    options,
                    budget,
                    host_context,
                    false,
                    Some(capacity),
                    Some(&call_id),
                )
                .await
            });
            let _cancel = pending.abort_on_drop();
            pending
                .await
                .map_err(|_| std::io::Error::other("subagent startup was cancelled"))??
        };
        lineage
            .spawned_in_turn(context.session_id(), report.agent_id)
            .await;
        json_output(&report)
    }
}

fn memoized(key: &'static str, build: impl FnOnce() -> ToolDefinition) -> ToolDefinition {
    static DEFINITIONS: LazyLock<Atom<HashMap<&'static str, ToolDefinition>>> =
        LazyLock::new(Atom::default);
    if let Some(definition) = DEFINITIONS.read(|definitions| definitions.get(key).cloned()) {
        return definition;
    }
    let definition = build();
    DEFINITIONS.update(|definitions| {
        definitions.insert(key, definition.clone());
    });
    definition
}

fn spawn_agent_parameters_for(policy: &SpawnPolicy) -> Value {
    static PARAMETERS: LazyLock<Value> = LazyLock::new(spawn_agent_parameters);
    let mut parameters = PARAMETERS.clone();
    let properties = &mut parameters["properties"];
    if let Some(models) = policy.offered_models() {
        let mut names = Vec::new();
        for model in models {
            let name = json!(model.as_str());
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.push(Value::Null);
        let mut families = Vec::new();
        for model in models {
            let family = json!(model.family().as_str());
            if !families.contains(&family) {
                families.push(family);
            }
        }
        families.push(Value::Null);
        properties["model"]["enum"] = Value::Array(names);
        properties["harness"]["enum"] = Value::Array(families);
    }
    if policy.min_thinking.is_some() || policy.max_thinking.is_some() {
        let mut levels = policy
            .offered_thinking()
            .into_iter()
            .map(|thinking| json!(thinking.as_str()))
            .collect::<Vec<_>>();
        levels.push(Value::Null);
        properties["thinking"]["enum"] = Value::Array(levels);
    }
    if policy.bounded_by_parent_thinking {
        let description = format!(
            "{} A subagent cannot request more effort than its spawning agent's own.",
            properties["thinking"]["description"]
                .as_str()
                .unwrap_or_default()
        );
        properties["thinking"]["description"] = json!(description);
    }
    if let Some(providers) = &policy.providers {
        let offered = providers
            .iter()
            .map(|provider| format!("{}/ ({})", provider.name, provider.about))
            .collect::<Vec<_>>()
            .join("; ");
        let description = format!(
            "{} A model reached through a provider is named <provider>/<model>: {offered}; list_models lists their models.",
            parameters["properties"]["model"]["description"]
                .as_str()
                .unwrap_or_default()
        );
        parameters["properties"]["model"]["description"] = json!(description);
    }
    if policy.budgets {
        parameters["properties"]["budget_usd"] = json!({
            "type": ["number", "null"],
            "description": "The most, in USD, this subagent and the subagents it delegates to may spend together, at most your own budget when you have one; null bounds it only by yours. The host refuses its requests past it."
        });
        if let Some(required) = parameters["required"].as_array_mut() {
            required.push(json!("budget_usd"));
        }
    }
    parameters
}

fn schema(text: &str) -> Value {
    serde_json::from_str(text).expect("subagent tool schemas are valid JSON")
}

fn spawn_agent_parameters() -> Value {
    schema(
        r##"{
        "type": "object",
        "properties": {
            "role": { "type": "string", "description": "A short role describing the subagent's specialty." },
            "task": { "type": "string", "description": "A complete, focused task for the subagent." },
            "harness": {
                "type": ["string", "null"],
                "description": "Native agent-loop family; null inherits the parent's family. A family switch selects that family's defaults."
            },
            "model": {
                "type": ["string", "null"],
                "description": "Model must belong to the selected harness. Null inherits within the same family; a family switch uses its default model."
            },
            "thinking": {
                "type": ["string", "null"],
                "enum": ["none", "low", "medium", "high", "xhigh", "max", null],
                "description": "Reasoning override; null inherits the parent's level."
            },
            "reasoning_mode": {
                "type": ["string", "null"],
                "enum": ["standard", "pro", null],
                "description": "Preferred reasoning mode; null selects standard. Pro requires authority before unsupported models resolve to standard. A child's actual mode bounds its descendants."
            },
            "output_contract": { "$ref": "#/$defs/node" }
        },
        "required": ["role", "task", "harness", "model", "thinking", "reasoning_mode", "output_contract"],
        "additionalProperties": false,
        "$defs": {
            "node": { "anyOf": [
                { "$ref": "#/$defs/object" },
                { "$ref": "#/$defs/array" },
                { "$ref": "#/$defs/string_enum" },
                { "$ref": "#/$defs/scalar" }
            ] },
            "object": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "const": "object" },
                    "fields": { "type": "array", "items": { "$ref": "#/$defs/field" } }
                },
                "required": ["kind", "fields"], "additionalProperties": false
            },
            "array": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "const": "array" },
                    "items": { "$ref": "#/$defs/node" }
                },
                "required": ["kind", "items"], "additionalProperties": false
            },
            "string_enum": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "const": "string_enum" },
                    "values": { "type": "array", "minItems": 1, "items": { "type": "string" } }
                },
                "required": ["kind", "values"], "additionalProperties": false
            },
            "scalar": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": [
                        "string", "integer", "number", "boolean", "null", "any"
                    ] }
                },
                "required": ["kind"], "additionalProperties": false
            },
            "field": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "schema": { "$ref": "#/$defs/node" },
                    "required": { "type": "boolean" }
                },
                "required": ["name", "schema", "required"], "additionalProperties": false
            }
        }
    }"##,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitResultArgs {
    output: Value,
}

struct SubmitResult {
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for SubmitResult {
    fn definition(&self) -> ToolDefinition {
        memoized(SUBMIT_RESULT_TOOL, || {
            ToolDefinition::function(
            SUBMIT_RESULT_TOOL,
            "Submits the current child subagent turn's final JSON output. Supply only output matching the task schema. After an accepted receipt, send a brief final assistant message with no further tool calls; do not finish with an empty response. If superseded, incorporate pending instructions and submit the updated result. Invalid values can be corrected and retried.",
            schema(r##"{
                "type": "object",
                "properties": {
                    "output": {
                        "anyOf": [
                            { "type": "object" }, { "type": "array", "items": {} },
                            { "type": "string" }, { "type": "number" },
                            { "type": "boolean" }, { "type": "null" }
                        ],
                        "description": "The final JSON value required by this agent's output schema. Pass objects and arrays directly, not as JSON-encoded strings. Use a string only when the output schema permits a string. A JSON-encoded object or array is decoded once only if it matches the required schema; the receipt reports decoded_json_text."

                    }
                },
                "required": ["output"],
                "additionalProperties": false
            }"##),
        )
        .with_output_schema(schema(r##"{
            "type": "object",
            "properties": {
                "accepted": { "type": "boolean" },
                "status": { "type": "string", "enum": ["accepted", "superseded"] },
                "durable": { "type": "boolean" },
                "note": { "type": "string" },
                "next": { "type": "string" },
                "decoded_json_text": { "type": "boolean", "const": true }
            },
            "required": ["accepted", "status"],
            "additionalProperties": false
        }"##))
        })
    }

    recovers!(self, input, context, self.submit(input, context, true));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.submit(input, context, false).await
    }
}

impl SubmitResult {
    async fn submit(
        &self,
        input: ToolInput,
        context: ToolContext<'_>,
        resumed: bool,
    ) -> ToolResult {
        let SubmitResultArgs { output } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let (outcome, durable) = registry
            .submit_result_durably(
                context.session_id(),
                context.instruction_revision(),
                output,
                resumed,
            )
            .await?;
        let output = match outcome {
            SubmissionOutcome::Accepted { decoded_json_text } => {
                let mut receipt = json!({
                    "accepted": true,
                    "status": "accepted",
                    "durable": durable,
                    "next": "Your parent receives this result, not your final message. End the turn now with one short sentence and no tool calls.",
                });
                if !durable {
                    receipt["note"] = json!(
                        "Accepted in memory; not yet durable across a restart. Do not submit again."
                    );
                }
                if decoded_json_text {
                    receipt["decoded_json_text"] = json!(true);
                }
                receipt
            }
            SubmissionOutcome::Superseded => json!({ "accepted": false, "status": "superseded" }),
        };
        Ok(ToolOutput::from_json(output, true))
    }
}

struct SendAgentMessage {
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for SendAgentMessage {
    fn definition(&self) -> ToolDefinition {
        memoized(SEND_AGENT_MESSAGE_TOOL, || {
            ToolDefinition::function(
                SEND_AGENT_MESSAGE_TOOL,
                "Sends a bounded directed message to any other agent in the same task tree. Deferred messages start an idle agent or queue behind its active turn. If a send is queued, do not wait for it inside the current turn; finish the turn so queued messages can be delivered. Urgent messages steer a running agent at its next safe model boundary. Delegate messages replace the recipient's assigned task, retain its output schema, and require management authority.",
                {
                    let mut parameters = schema(
                        r##"{
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The recipient from list_agents. Any non-closing agent in the same task tree can receive coordination messages."
                    },
                    "message": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The focused message body. The runtime enforces a 64 KiB UTF-8 limit."
                    },
                    "priority": {
                        "type": "string",
                        "enum": ["deferred", "urgent"],
                        "default": "deferred",
                        "description": "Urgent steers an active turn; deferred preserves turn boundaries. A queued deferred send requires the current turn to finish before delivery."
                    },
                    "purpose": {
                        "type": "string",
                        "enum": ["delegate", "coordinate", "finding", "question", "reply"],
                        "default": "coordinate",
                        "description": "A typed coordination intent, independent of thread correlation. Reply requires in_reply_to and reverses the referenced message direction. Delegate is restricted to agents the sender can manage."
                    },
                    "in_reply_to": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Continue the referenced two-party thread without changing purpose. Non-reply messages may continue in either direction, including follow-ups to your own messages. With purpose=reply, answer a received message and reverse its direction."
                    }
                },
                "required": ["agent_id", "message"],
                "additionalProperties": false
            }"##,
                    );
                    parameters["properties"]["message"]["maxLength"] = MAX_MESSAGE_BYTES.into();
                    parameters
                },
            )
        })
    }

    recovers!(self, input, context, self.send(input, context, true));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.send(input, context, false).await
    }
}

impl SendAgentMessage {
    async fn send(&self, input: ToolInput, context: ToolContext<'_>, resumed: bool) -> ToolResult {
        let SendMessageTask {
            agent_id,
            message,
            priority,
            purpose,
            in_reply_to,
        } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        if resumed
            && let Some(receipt) = registry
                .resent_to(context.session_id(), context.call_id(), agent_id)
                .await?
        {
            return json_output(&receipt);
        }
        #[cfg(not(target_family = "wasm"))]
        let receipt = registry
            .send_message(
                context.session_id(),
                Some(context.call_id()),
                agent_id,
                priority,
                purpose,
                in_reply_to,
                message,
            )
            .await?;
        #[cfg(target_family = "wasm")]
        let receipt = {
            let session_id = context.session_id().to_owned();
            let call_id = context.call_id().to_owned();
            let pending = super::platform::spawn(async move {
                registry
                    .send_message(
                        &session_id,
                        Some(&call_id),
                        agent_id,
                        priority,
                        purpose,
                        in_reply_to,
                        message,
                    )
                    .await
            });
            let _cancel = pending.abort_on_drop();
            pending
                .await
                .map_err(|_| std::io::Error::other("subagent message was cancelled"))??
        };
        json_output(&receipt)
    }
}

struct ListModels {
    registry: Weak<Registry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelQuery {
    #[serde(default)]
    query: Option<String>,
}

#[derive(Serialize)]
struct ModelCatalog<'a> {
    providers: Vec<CatalogProvider<'a>>,
}

#[derive(Serialize)]
struct CatalogProvider<'a> {
    name: &'a str,
    about: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
    models: Vec<CatalogModel>,
}

#[derive(Serialize)]
struct CatalogModel {
    model: String,
    harness: HarnessFamily,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for ListModels {
    fn definition(&self) -> ToolDefinition {
        memoized(LIST_MODELS_TOOL, || {
            ToolDefinition::function(
                LIST_MODELS_TOOL,
                "Lists the models this host reaches through providers, each named <provider>/<model> with the harness that runs it, and what each provider is and how it is paid. Pass one as spawn_agent's model to run a subagent on it.",
                schema(
                    r##"{
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Lists only the models whose name contains this, ignoring case."
                    }
                },
                "additionalProperties": false
            }"##,
                ),
            )
        })
    }

    recovers!(self, input, context, self.execute(input, context));

    async fn execute(&self, input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        let ModelQuery { query } = input.decode_json()?;
        let query = query.unwrap_or_default().to_lowercase();
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let policy = registry.spawn_policy();
        json_output(&ModelCatalog {
            providers: policy
                .providers
                .iter()
                .flatten()
                .map(|provider| CatalogProvider {
                    name: &provider.name,
                    about: &provider.about,
                    note: provider.note.as_deref(),
                    models: provider
                        .models
                        .iter()
                        .map(|model| CatalogModel {
                            model: format!("{}/{}", provider.name, model.as_str()),
                            harness: model.family(),
                        })
                        .filter(|model| model.model.to_lowercase().contains(&query))
                        .collect(),
                })
                .collect(),
        })
    }
}

struct ListAgents {
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for ListAgents {
    fn definition(&self) -> ToolDefinition {
        memoized(LIST_AGENTS_TOOL, || {
            ToolDefinition::function(
                LIST_AGENTS_TOOL,
                "Lists agents in the same task tree in agent_id order. Pending, running, and recoverable interrupted agents are returned by default. Set include_completed to retrieve terminal statuses and completed results in status.output. After reuse or closure, last_output retains the latest completed result. In a large tree, page with limit: the result's next_after_agent_id, present while more agents remain, is the after_agent_id of the next page.",
                schema(
                    r##"{
                "type": "object",
                "properties": {
                    "include_completed": {
                        "type": "boolean",
                        "default": false,
                        "description": "Also includes completed, failed, closed, and nonrecoverable interrupted agents."
                    },
                    "include_self": {
                        "type": "boolean",
                        "default": false,
                        "description": "Includes the calling agent for topology inspection. Self-messaging remains unavailable."
                    },
                    "after_agent_id": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Lists only agents whose agent_id is greater than this one."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Lists at most this many agents."
                    }
                },
                "additionalProperties": false
            }"##,
                ),
            )
        })
    }

    recovers!(self, input, context, self.execute(input, context));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let DirectoryTask {
            include_completed,
            include_self,
            after_agent_id,
            limit,
        } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let (agents, next_after_agent_id) = registry
            .directory_page(
                context.session_id(),
                include_completed,
                include_self,
                after_agent_id,
                limit,
            )
            .await?;
        let receipt = registry
            .observe_listed(context.session_id(), &agents, context.call_id())
            .await;
        let output = json_output(&AgentDirectory {
            agents,
            next_after_agent_id,
        })?;
        Ok(match receipt {
            Some(receipt) => output.with_receipt(receipt),
            None => output,
        })
    }
}

struct WaitAgent {
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for WaitAgent {
    fn definition(&self) -> ToolDefinition {
        let continues = self
            .registry
            .upgrade()
            .is_some_and(|registry| registry.continues_root());
        let key = if continues {
            "wait_agent continued"
        } else {
            WAIT_AGENT_TOOL
        };
        memoized(key, || {
            let mut description = "Waits until any requested subagent reaches a terminal status and returns a snapshot of every requested agent. The result includes pending, running, and closing agents; consumers must preserve those nonterminal agents and act only on completed, failed, interrupted, or closed entries. Each entry's status is an object; read its terminal state from status.state. Only results this caller has not yet received end the wait: already-reported terminal agents do not, so waiting again with the same IDs blocks for the next result. When every requested agent is terminal and already reported, the snapshot returns immediately; repeating that identical call errors because nothing is left to wait for. Use one call with multiple IDs instead of polling the workspace. Tasks are abbreviated. A completed entry omits status.output when this caller already received that result from an earlier wait_agent call or a completion notice; list_agents with include_completed=true returns it again.".to_owned();
            if continues {
                description.push_str(" Use wait_agent only while other work still needs this turn to stay alive. When the root agent has no other work and is only waiting on running direct children, end the turn instead of blocking on them: the runtime continues the root when any direct child completes, fails, or is interrupted. Subagents are not continued by their children, so a subagent keeps its turn alive until it can submit its result.");
            }
            ToolDefinition::function(
            WAIT_AGENT_TOOL,
            description,
            schema(r##"{
                "type": "object",
                "properties": {
                    "agent_ids": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 1 },
                        "minItems": 1,
                        "description": "Agent IDs returned by spawn_agent. Waiting returns when any one becomes terminal."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Wait in milliseconds. Defaults to 30000."
                    }
                },
                "required": ["agent_ids"],
                "additionalProperties": false
            }"##),
        )
        .with_output_schema(WAIT_AGENT_OUTPUT_SCHEMA.clone())
        })
    }

    recovers!(self, input, context, self.wait(input, context, true));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        self.wait(input, context, false).await
    }
}

impl WaitAgent {
    async fn wait(&self, input: ToolInput, context: ToolContext<'_>, resumed: bool) -> ToolResult {
        let WaitTask {
            agent_ids,
            timeout_ms,
        } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let duration = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_WAIT_TIMEOUT);
        let answered = if resumed {
            registry
                .rewait(context.session_id(), context.call_id(), &agent_ids)
                .await?
        } else {
            None
        };
        let (agents, delivered, timed_out, receipt) = match answered {
            Some(answered) => answered,
            None => {
                registry
                    .wait(
                        context.session_id(),
                        context.call_id(),
                        &agent_ids,
                        duration,
                    )
                    .await?
            }
        };
        let output = json_output(&WaitReport {
            agents: agent_reports(agents, &delivered)?,
            timed_out,
        })?;
        Ok(match receipt {
            Some(receipt) => output.with_receipt(receipt),
            None => output,
        })
    }
}

#[derive(Clone, Copy)]
enum LifecycleOperation {
    Interrupt,
    Close,
}

struct ChangeAgentLifecycle {
    registry: Weak<Registry>,
    operation: LifecycleOperation,
}

impl ChangeAgentLifecycle {
    const fn tool_name(&self) -> &'static str {
        match self.operation {
            LifecycleOperation::Interrupt => "interrupt_agent",
            LifecycleOperation::Close => "close_agent",
        }
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for ChangeAgentLifecycle {
    fn definition(&self) -> ToolDefinition {
        memoized(self.tool_name(), || {
            let description = match self.operation {
                LifecycleOperation::Interrupt => {
                    "Interrupts an agent's active turn and every active descendant, waits for their model and tool resources to stop, and keeps the sessions reusable within the running parent runtime."
                }
                LifecycleOperation::Close => {
                    "Closes an agent and its entire descendant subtree, waiting for active model and tool resources to stop before returning. Closed agents remain inspectable within the running parent runtime but are not reusable."
                }
            };
            ToolDefinition::function(
                self.tool_name(),
                description,
                schema(
                    r##"{
                "type": "object",
                "properties": {
                    "agent_id": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The root of the subagent subtree to stop."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            }"##,
                ),
            )
        })
    }

    recovers!(self, input, context, self.execute(input, context));

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let TargetAgent { agent_id } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let agents = match self.operation {
            LifecycleOperation::Interrupt => {
                registry.interrupt(context.session_id(), agent_id).await?
            }
            LifecycleOperation::Close => registry.close(context.session_id(), agent_id).await?,
        };
        json_output(&LifecycleReport {
            agents: agent_reports(agents, &std::collections::HashSet::new())?,
        })
    }
}

fn shared_tools(parent: AgentHandle, registry: &Arc<Registry>) -> Vec<Arc<dyn Tool>> {
    registry.register_handle(parent.clone());
    let mut tools: Vec<Arc<dyn Tool>> = vec![
        Arc::new(SubmitResult {
            registry: Arc::downgrade(registry),
        }),
        Arc::new(SpawnAgent {
            parent,
            registry: Arc::downgrade(registry),
        }),
        Arc::new(SendAgentMessage {
            registry: Arc::downgrade(registry),
        }),
        Arc::new(ListAgents {
            registry: Arc::downgrade(registry),
        }),
        Arc::new(WaitAgent {
            registry: Arc::downgrade(registry),
        }),
        Arc::new(ChangeAgentLifecycle {
            registry: Arc::downgrade(registry),
            operation: LifecycleOperation::Interrupt,
        }),
        Arc::new(ChangeAgentLifecycle {
            registry: Arc::downgrade(registry),
            operation: LifecycleOperation::Close,
        }),
    ];
    if registry.spawn_policy().providers.is_some() {
        tools.push(Arc::new(ListModels {
            registry: Arc::downgrade(registry),
        }));
    }
    tools
}

pub fn install_tools<T: ToolCollection>(
    tools: T,
    parent: AgentHandle,
    registry: Arc<Registry>,
) -> Result<T, T::Error> {
    tools.add_shared_tools(shared_tools(parent, &registry))
}

pub fn install_claude_tools<T: nanocode_tool_contracts::ToolCollection>(
    tools: T,
    parent: AgentHandle,
    registry: Arc<Registry>,
) -> Result<T, T::Error> {
    tools.add_shared_tools(
        shared_tools(parent, &registry)
            .into_iter()
            .map(|tool| Arc::new(TurnAuthority(tool)) as Arc<dyn nanocode_tool_contracts::Tool>),
    )
}

struct TurnAuthority(Arc<dyn Tool>);

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for TurnAuthority {
    fn definition(&self) -> ToolDefinition {
        self.0.definition()
    }
    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let host_context = context.host_context().or(context.turn_id());
        self.0
            .execute(input, context.with_host_context(host_context))
            .await
    }

    #[cfg(not(target_family = "wasm"))]
    fn recover<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> Option<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<ToolOutput>> + Send + 'async_trait>,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let host_context = context.host_context().or(context.turn_id());
        self.0
            .recover(input, context.with_host_context(host_context))
    }

    #[cfg(target_family = "wasm")]
    fn recover<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> Option<
        std::pin::Pin<Box<dyn std::future::Future<Output = Option<ToolOutput>> + 'async_trait>>,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let host_context = context.host_context().or(context.turn_id());
        self.0
            .recover(input, context.with_host_context(host_context))
    }
}

static SPAWN_AGENT_OUTPUT_SCHEMA: LazyLock<Value> = LazyLock::new(spawn_agent_output_schema);

static WAIT_AGENT_OUTPUT_SCHEMA: LazyLock<Value> = LazyLock::new(wait_agent_output_schema);

fn spawn_agent_output_schema() -> Value {
    schema(
        r##"{
        "type": "object",
        "properties": {
            "agent_id": { "type": "integer" },
            "role": { "type": "string" },
            "model": {},
            "thinking": { "type": "string" },
            "reasoning_mode": { "type": "string", "enum": ["standard", "pro"] },
            "budget_usd": { "type": "number" },
            "status": {
                "type": "object",
                "properties": { "state": { "type": "string", "const": "running" } },
                "required": ["state"],
                "additionalProperties": false
            }
        },
        "required": ["agent_id", "role", "status"],
        "additionalProperties": false
    }"##,
    )
}

fn wait_agent_output_schema() -> Value {
    {
        let mut output = schema(
            r##"{
        "type": "object",
        "properties": {
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "integer" },
                        "role": { "type": "string" },
                        "task": { "type": "string" },
                        "parent_agent_id": { "type": ["integer", "null"] },
                        "model": {},
                        "thinking": { "type": "string" },
                        "reasoning_mode": { "type": "string", "enum": ["standard", "pro"] },
                        "budget_usd": { "type": "number" },
                        "last_output": {},
                        "completion_revision": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["agent_id", "role", "task", "parent_agent_id", "status"],
                    "additionalProperties": false
                }
            },
            "timed_out": { "type": "boolean" }
        },
        "required": ["agents", "timed_out"],
        "additionalProperties": false
    }"##,
        );
        output["properties"]["agents"]["items"]["properties"]["status"] = agent_status_schema();
        output
    }
}

fn agent_status_schema() -> Value {
    let state_only = ["pending", "running", "interrupted", "closing", "closed"].map(|state| {
        json!({
            "type": "object",
            "properties": { "state": { "type": "string", "const": state } },
            "required": ["state"],
            "additionalProperties": false
        })
    });
    let mut variants = state_only.into_iter().collect::<Vec<_>>();
    variants.push(json!({
        "type": "object",
        "properties": {
            "state": { "type": "string", "const": "completed" },
            "output": {}
        },
        "required": ["state"],
        "additionalProperties": false
    }));
    variants.push(json!({
        "type": "object",
        "properties": {
            "state": { "type": "string", "const": "failed" },
            "error": { "type": "string" }
        },
        "required": ["state", "error"],
        "additionalProperties": false
    }));
    json!({ "oneOf": variants })
}

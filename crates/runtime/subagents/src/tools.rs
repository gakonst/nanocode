use super::{
    message::MAX_MESSAGE_BYTES,
    model::{
        AgentDescriptor, AgentId, AgentStatus, AgentUpdate, MessageId, MessagePriority,
        MessagePurpose, agent_prompt,
    },
    policy::{SpawnParent, SpawnPolicy, SpawnPolicyError},
    runtime::{
        AgentDirectoryEntry, AgentSummary, OutputContract, Registry, SubmissionOutcome,
        forward_events,
    },
};
use async_trait::async_trait;
use futures_util::future::join_all;
use nanocode_agent_core::{
    AgentHandle, ClaudeModel, ForkRequest, HarnessFamily, Model, ModelSelection, ReasoningMode,
    SpawnOptions, Thinking,
};
use nanocode_tool_contracts::{
    Tool, ToolCollection, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Weak},
    time::Duration,
};
use tokio::sync::oneshot;

const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const SPAWN_AGENT_TOOL: &str = "spawn_agent";
const SUBMIT_RESULT_TOOL: &str = "submit_result";
const SEND_AGENT_MESSAGE_TOOL: &str = "send_agent_message";
const LIST_AGENTS_TOOL: &str = "list_agents";
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

    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    output_contract: Option<OutputContractNode>,
}

impl SpawnAgentTask {
    fn into_parts_with_parent(
        self,
        parent: Option<HarnessFamily>,
    ) -> std::io::Result<(AgentTask, SpawnOptions)> {
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
            let backend = match self.harness.clone() {
                Some(harness) => harness,
                None => inferred_harness(parent, &model)?,
            };
            let model = ModelSelection::new(backend, model);
            let model = if let Some(known) = model.as_codex() {
                ModelSelection::Codex(known)
            } else if let Some(known) = model.as_claude() {
                ModelSelection::Claude(known)
            } else {
                model
            };
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
    match child.checkpoint().await {
        Ok(checkpoint) => {
            let (model, thinking) = (checkpoint.model(), checkpoint.thinking());
            if let Some((registry, parent)) = policy {
                let policy = registry.spawn_policy();
                policy.admit(parent, Some(&model), Some(thinking))?;
                policy.admit_reasoning_mode(parent, checkpoint.reasoning_mode())?;
            }
            Ok((Some(model), Some(thinking), checkpoint.reasoning_mode()))
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
}

#[derive(Serialize)]
struct AgentDirectory {
    agents: Vec<AgentDirectoryEntry>,
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

    let admission = registry.admit_spawn(session_id).await?;
    let policy_parent = spawn_parent(parent, registry, session_id).await?;
    let prepared = prepare_batch(tasks)?;
    let mut startup = registry.batch_startup();
    let capacities = registry.reserve_turns(prepared.len())?;
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
        };
        let (start_events, events_ready) = oneshot::channel();
        let event_task = forward_events(
            reservation.root_session_id.clone(),
            id,
            events,
            events_ready,
            Arc::downgrade(registry),
            registry.updates.clone(),
        );
        if let Err(error) = registry
            .insert(
                reservation.root_session_id.clone(),
                descriptor.clone(),
                host_context.as_ref().map(Arc::clone),
                child,
                event_task,
                contract,
            )
            .await
        {
            return Err(startup.fail(error).await.into());
        }
        startup.track(&reservation.root_session_id, id);
        reports.push(AgentStartReport {
            agent_id: id,
            role: task.role,
            status: AgentStatus::Running,
            model: model.clone(),
            thinking,
            reasoning_mode,
        });
        additions.push((
            reservation.root_session_id.clone(),
            descriptor,
            start_events,
        ));
        launches.push((
            reservation.root_session_id,
            id,
            agent_prompt(id, &task.task),
            capacity,
        ));
    }

    for (root_session_id, descriptor, start_events) in additions {
        registry.send(&root_session_id, AgentUpdate::Added(descriptor));
        let _ = start_events.send(());
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
        host_context,
        true,
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
        host_context,
        false,
    )
    .await
}

async fn start_child(
    parent: &AgentHandle,
    registry: &Arc<Registry>,
    session_id: &str,
    task: AgentTask,
    options: SpawnOptions,
    host_context: Option<Arc<str>>,
    fork: bool,
) -> AgentToolResult<AgentStartReport> {
    if fork && parent.session_id() != session_id {
        return Err("child caller identity must match its native parent handle".into());
    }
    registry.register_handle(parent.clone());
    let admission = registry.admit_spawn(session_id).await?;
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
    let mut startup = registry.batch_startup();
    let capacity = registry.reserve_turn()?;
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
    };
    let (start_events, events_ready) = oneshot::channel();
    let event_task = forward_events(
        reservation.root_session_id.clone(),
        id,
        events,
        events_ready,
        Arc::downgrade(registry),
        registry.updates.clone(),
    );
    registry
        .insert(
            reservation.root_session_id.clone(),
            descriptor.clone(),
            host_context,
            child,
            event_task,
            contract,
        )
        .await?;
    startup.track(&reservation.root_session_id, id);
    registry.send(&reservation.root_session_id, AgentUpdate::Added(descriptor));
    let _ = start_events.send(());

    if let Err(error) = registry
        .launch_initial_turn(
            &reservation.root_session_id,
            id,
            agent_prompt(id, &task),
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
    })
}

struct SpawnAgent {
    parent: AgentHandle,
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for SpawnAgent {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            SPAWN_AGENT_TOOL,
            "Starts a reusable clean-room subagent without inherited conversation history and immediately returns its ID. When the root agent is durable, the task tree survives runtime restarts with the same IDs and interrupted children resume from their latest committed checkpoint; otherwise children are dropped when the parent runtime restarts. Delegate meaningful, separable work and handle trivial or tightly coupled work directly. When Code Mode is available, build multi-agent pipelines in it: start independent subtasks in parallel, await and reduce their results, then dispatch dependent stages. Do not repeat delegated work; double-check results against the relevant evidence before relying on them. Use output contracts that expose the fields downstream stages need, keep concurrent write scopes disjoint, and own final synthesis and verification.",
            spawn_agent_parameters_for(
                &self
                    .registry
                    .upgrade()
                    .map(|registry| registry.spawn_policy())
                    .unwrap_or_default(),
            ),
        )
        .with_strict_parameters()
        .with_output_schema(SPAWN_AGENT_OUTPUT_SCHEMA.clone())
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let (task, options) = input
            .decode_json::<SpawnAgentTask>()?
            .into_parts_with_parent(Some(self.parent.harness_family()))?;
        let host_context = context.host_context().map(Arc::<str>::from);
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let lineage = Arc::clone(&registry);
        #[cfg(not(target_family = "wasm"))]
        let report = start_agent_with_host_context(
            &self.parent,
            &registry,
            context.session_id(),
            task,
            options,
            host_context,
        )
        .await?;

        #[cfg(target_family = "wasm")]
        let report = {
            let parent = self.parent.clone();
            let session_id = context.session_id().to_owned();
            let pending = super::platform::spawn(async move {
                start_agent_with_host_context(
                    &parent,
                    &registry,
                    &session_id,
                    task,
                    options,
                    host_context,
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

fn spawn_agent_parameters_for(policy: &SpawnPolicy) -> Value {
    static PARAMETERS: LazyLock<Value> = LazyLock::new(spawn_agent_parameters);
    let mut parameters = PARAMETERS.clone();
    let properties = &mut parameters["properties"];
    if let Some(models) = policy.offered_models() {
        let mut names = models
            .iter()
            .map(|model| json!(model.as_str()))
            .collect::<Vec<_>>();
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
            "{} A registered subagent cannot request more effort than its own.",
            properties["thinking"]["description"]
                .as_str()
                .unwrap_or_default()
        );
        properties["thinking"]["description"] = json!(description);
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
                "decoded_json_text": { "type": "boolean", "const": true }
            },
            "required": ["accepted", "status"],
            "additionalProperties": false
        }"##))
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let SubmitResultArgs { output } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let (outcome, durable) = registry
            .submit_result_durably(context.session_id(), context.instruction_revision(), output)
            .await?;
        let output = match outcome {
            SubmissionOutcome::Accepted { decoded_json_text } => {
                let mut receipt =
                    json!({ "accepted": true, "status": "accepted", "durable": durable });
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
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
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
        #[cfg(not(target_family = "wasm"))]
        let receipt = registry
            .send_message(
                context.session_id(),
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
            let pending = super::platform::spawn(async move {
                registry
                    .send_message(
                        &session_id,
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

struct ListAgents {
    registry: Weak<Registry>,
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl Tool for ListAgents {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::function(
            LIST_AGENTS_TOOL,
            "Lists agents in the same task tree. Pending, running, and recoverable interrupted agents are returned by default. Set include_completed to retrieve terminal statuses and completed results in status.output. After reuse or closure, last_output retains the latest completed result.",
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
                    }
                },
                "additionalProperties": false
            }"##,
            ),
        )
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let DirectoryTask {
            include_completed,
            include_self,
        } = input.decode_json()?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| std::io::Error::other("subagent runtime is closed"))?;
        let agents = registry
            .directory(context.session_id(), include_completed, include_self)
            .await?;
        let receipt = registry
            .observe_listed(context.session_id(), &agents, context.call_id())
            .await;
        let output = json_output(&AgentDirectory { agents })?;
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
        let mut description = "Waits until any requested subagent reaches a terminal status and returns a snapshot of every requested agent. The result includes pending, running, and closing agents; consumers must preserve those nonterminal agents and act only on completed, failed, interrupted, or closed entries. Each entry's status is an object; read its terminal state from status.state. Only results this caller has not yet received end the wait: already-reported terminal agents do not, so waiting again with the same IDs blocks for the next result. When every requested agent is terminal and already reported, the snapshot returns immediately; repeating that identical call errors because nothing is left to wait for. Use one call with multiple IDs instead of polling the workspace. Tasks are abbreviated. A completed entry omits status.output when this caller already received that result from an earlier wait_agent call or a completion notice; list_agents with include_completed=true returns it again.".to_owned();
        if self
            .registry
            .upgrade()
            .is_some_and(|registry| registry.continues_root())
        {
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
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
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
        let (agents, delivered, timed_out, receipt) = registry
            .wait(
                context.session_id(),
                context.call_id(),
                &agent_ids,
                duration,
            )
            .await?;
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
    }

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
    let child = parent.session_id() != parent.root_session_id();
    let mut tools: Vec<Arc<dyn Tool>> = vec![
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
    if child {
        tools.insert(
            0,
            Arc::new(SubmitResult {
                registry: Arc::downgrade(registry),
            }),
        );
    }
    tools
}

pub fn install_tools<T: ToolCollection>(
    mut tools: T,
    parent: AgentHandle,
    registry: Arc<Registry>,
) -> Result<T, T::Error> {
    for tool in shared_tools(parent, &registry) {
        tools = tools.add_shared_tool(tool)?;
    }
    Ok(tools)
}

pub fn install_claude_tools<T: nanocode_tool_contracts::ToolCollection>(
    mut tools: T,
    parent: AgentHandle,
    registry: Arc<Registry>,
) -> Result<T, T::Error> {
    for tool in shared_tools(parent, &registry) {
        tools = tools.add_shared_tool(Arc::new(TurnAuthority(tool)))?;
    }
    Ok(tools)
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

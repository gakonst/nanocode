use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn build_agent(
    mut config: Arc<ModelConfig>,
    tools: ToolsConfiguration,
    workspace: Option<PathBuf>,
    session_id: Option<SessionId>,
    prompt_cache: PromptCacheConfig,
    codex: CodexCompatibility,
    resume: Option<SessionSnapshot>,
    lineage: Option<Lineage>,
    service_factory: ServiceFactory,
) -> Result<(Agent, AgentEvents)> {
    #[cfg(not(target_family = "wasm"))]
    crate::client::transport::install_default_rustls_crypto_provider();
    let session_id = session_id.unwrap_or_default();
    let session_id_text = session_id.to_string();
    let context_source =
        tracing::info_span!("codex.session.context").in_scope(|| codex.context.build());
    let PromptCacheConfig {
        key,
        per_session,
        shared,
    } = prompt_cache;

    let is_resume = resume.is_some()
        || lineage
            .as_ref()
            .is_some_and(|lineage| !matches!(lineage.origin, crate::Origin::Root));
    let (lineage_id, prompt_cache_key, initial_resume) = if let Some(snapshot) = resume {
        let resume = snapshot.into_resume()?;
        let model = resume.model;
        let lineage_id = Arc::clone(&resume.lineage_id);
        let restored_cache_key = Arc::clone(&resume.prompt_cache_key);
        Arc::make_mut(&mut config).model = model;
        validate_model_thinking(config.model, config.thinking)?;
        validate_model_reasoning_mode(config.model, config.reasoning_mode)?;
        if config.context_window_tokens > model.max_context_window_tokens() {
            Arc::make_mut(&mut config).context_window_tokens = model.max_context_window_tokens();
        }
        if key
            .as_deref()
            .is_some_and(|key| key != restored_cache_key.as_ref())
        {
            return Err(NanocodeError::InvalidCheckpoint(
                "configured prompt cache key does not match the resumed session".to_owned(),
            ));
        }
        let initial = InitialResume::from_resume(resume);
        (lineage_id, Some(restored_cache_key), Some(initial))
    } else {
        (
            Arc::<str>::from(session_id_text.as_str()),
            key.map(Arc::from)
                .or_else(|| per_session.then(|| Arc::from(session_id_text.as_str()))),
            None,
        )
    };
    let configured_workspace = workspace
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map(Arc::<str>::from)
                .map_err(|path| NanocodeError::WorkspaceNotUtf8 {
                    path: PathBuf::from(path),
                })
        })
        .transpose()?;
    let workspace = if let Some(initial) = initial_resume.as_ref() {
        let restored = context_source.resolve_workspace(Some(initial.workspace()))?;
        if restored != initial.workspace() {
            return Err(NanocodeError::InvalidCheckpoint(
                "workspace no longer resolves to the stored location".to_owned(),
            ));
        }
        if let Some(configured) = configured_workspace
            && context_source.execution_environment().is_none()
        {
            let requested = context_source.resolve_workspace(Some(&configured))?;
            if requested != restored {
                return Err(NanocodeError::WorkspaceChanged {
                    current: restored,
                    requested,
                });
            }
        }
        Some(Arc::<str>::from(restored))
    } else {
        Some(Arc::<str>::from(
            context_source.resolve_workspace(configured_workspace.as_deref())?,
        ))
    };
    let service = tracing::info_span!("codex.session.service")
        .in_scope(|| service_factory(Arc::clone(&config)));
    let provider_session_id = Arc::clone(&lineage_id);
    spawn_agent_driver(
        BranchSpawner {
            config,
            tools,
            spawn_factory: codex.spawn_factory,
            child_journal: codex.child_journal,
            lineage_id,
            provider_session_id,
            prompt_cache_key,
            shared_prompt_cache: shared,
            before_compaction: codex.before_compaction,
            images: codex.images,
            instant_tool_steering: codex.instant_tool_steering,
            context_config: codex.context,
            context_source,
            lineage: lineage.unwrap_or_else(|| Lineage::root(session_id_text.as_str())),
            execution: codex.execution,
            restored_snapshot: None,
            host_context: codex.host_context,
            service_factory,
            initial_developer_messages: codex.initial_developer_messages,
        },
        session_id,
        workspace,
        service,
        initial_resume,
        if is_resume {
            crate::session::SessionStart::Resume
        } else {
            crate::session::SessionStart::New(crate::Origin::Root)
        },
    )
}

pub(super) fn spawn_agent_driver(
    spawner: BranchSpawner,
    session_id: SessionId,
    workspace: Option<Arc<str>>,
    service: ResponsesClientBox,
    initial_resume: Option<InitialResume>,
    start: crate::session::SessionStart,
) -> Result<(Agent, AgentEvents)> {
    let session_id_text = session_id.to_string();
    let origin = AgentOrigin {
        start,
        lineage: spawner.lineage.clone(),
    };
    let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
    let shutdown = DriverShutdown::default();
    let mut child_handle = AgentHandle::new(
        Arc::<str>::from(session_id_text.as_str()),
        crate::ModelSelection::Codex(spawner.config.model),
        Arc::new(super::handle::OpenAiAgentFactory {
            commands: commands.downgrade(),
            shutdown: shutdown.clone(),
            conversation_id: Arc::clone(&spawner.lineage_id),
        }),
    )
    .with_root_session_id(origin.lineage.root_session_id.as_str());
    if let Some(factory) = &spawner.spawn_factory {
        child_handle = child_handle.with_spawn_factory(factory.clone());
    }
    child_handle = child_handle.with_child_journal(spawner.child_journal.clone());
    let tools = tracing::info_span!("codex.session.tools").in_scope(|| {
        spawner
            .tools
            .materialize(child_handle.clone())
            .map(|tools| {
                tools
                    .for_session(&child_handle.session_environment())
                    .for_backend(child_handle.session_id(), "codex")
            })
    })?;

    if tools.exposure() == nanocode_tool_host::ToolExposure::Hidden {
        return Err(NanocodeError::InvalidRequest(
            "Nanocode agents require Code Mode or direct tool exposure".to_owned(),
        ));
    }
    let prompt_cache_key = spawner
        .prompt_cache_key
        .as_deref()
        .unwrap_or(&spawner.lineage_id);
    let execution = spawner.execution.start(
        &session_id_text,
        prompt_cache_key,
        workspace.as_deref(),
        &spawner.config.system_prompt(),
        origin.start,
        origin.lineage.origin,
        origin.recorded_parent(),
        origin
            .recorded_parent()
            .map_or(session_id_text.as_str(), |_| {
                origin.lineage.root_session_id.as_str()
            }),
        initial_resume.as_ref().map(InitialResume::history_len),
    )?;
    let (runtime, event_stream) = BackendRuntime::new(session_id.to_string());
    let runtime = runtime.with_lineage(origin.lineage.clone());
    let events = EventSink::from_publisher(runtime.events());
    shutdown.set_execution_policy_owned(execution.identifies_prompts());
    let initial_model = initial_resume
        .map(|initial| match initial {
            InitialResume::Exact(checkpoint) => prepare_resumed_checkpoint(
                *checkpoint,
                &spawner.config,
                &tools,
                &session_id_text,
                spawner.context_source.clone(),
            ),
            InitialResume::History(resume) => prepare_history_checkpoint(
                *resume,
                &spawner.config,
                &tools,
                &session_id_text,
                spawner.context_source.clone(),
            ),
        })
        .transpose()?;
    let transport_stats = Arc::new(TransportStats::default());
    let checkpoints = Arc::new(CheckpointSource::new(
        Arc::from(session_id_text.as_str()),
        origin.lineage.clone(),
        Arc::clone(&spawner.lineage_id),
        is_stateless_http(&spawner.config),
        spawner.config.reasoning_mode,
    ));
    let (initial_persisted, initial_ready) = oneshot::channel();
    let agent = runtime.bind(LocalLifecycle {
        child_handle,
        commands,
        execution: execution.clone(),
        shutdown: shutdown.clone(),
        checkpoints: Arc::clone(&checkpoints),
        initial_ready: Arc::new(nanocode_sync::Slot::new(Some(initial_ready))),
        images: spawner.images.clone(),
    });

    tools.start_providers();
    let driver = AgentDriver {
        commands: receiver,
        events,
        client: service,
        transport_stats,
        tools,
        workspace,
        spawner,
        initial_model,
        origin,
        checkpoints,
        execution: execution.clone(),
        initial_persisted: Some(initial_persisted),
    };
    let driver_task = async move {
        let outcome = driver.run().await;
        let outcome = outcome.and(execution.shutdown().await);
        if let Err(error) = &outcome {
            tracing::error!(
                target: "nanocode",
                error = %error,
                "agent driver stopped with an error"
            );
        }
        shutdown.complete(outcome);
    };
    spawn_driver(Box::pin(driver_task))?;
    Ok((agent, event_stream))
}

pub(super) fn validate(config: &ModelConfig, prompt_cache_key: Option<&str>) -> Result<()> {
    config
        .auth
        .validate()
        .map_err(|error| NanocodeError::InvalidRequest(error.to_string()))?;
    if config.context_window_tokens == 0 {
        return Err(NanocodeError::InvalidRequest(
            "model context window must be greater than zero".to_owned(),
        ));
    }
    if matches!(config.responses_transport, ResponsesTransport::WebSocket)
        && config.websocket_url.trim().is_empty()
    {
        return Err(NanocodeError::InvalidRequest(
            "Responses WebSocket URL must not be empty".to_owned(),
        ));
    }
    if matches!(config.responses_transport, ResponsesTransport::Https)
        && config.api_base_url.trim().is_empty()
    {
        return Err(NanocodeError::InvalidRequest(
            "OpenAI API base URL must not be empty".to_owned(),
        ));
    }
    if config.auth.mode() == OpenAiAuthMode::ChatGpt && config.store_responses {
        return Err(NanocodeError::InvalidRequest(
            "ChatGPT subscription authentication does not support store: true".to_owned(),
        ));
    }
    if matches!(config.responses_transport, ResponsesTransport::Https)
        && !config.store_responses
        && matches!(config.responses_history, ResponsesHistory::Incremental)
    {
        return Err(NanocodeError::InvalidRequest(
            "HTTPS with store: false requires full client-history replay".to_owned(),
        ));
    }
    if prompt_cache_key.is_some_and(|prompt_cache_key| prompt_cache_key.trim().is_empty()) {
        return Err(NanocodeError::InvalidRequest(
            "prompt_cache_key must not be empty".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_model_thinking(model: Model, thinking: Thinking) -> Result<()> {
    crate::ModelSelection::Codex(model)
        .capabilities(crate::ModelTransport::Native)
        .check_thinking(thinking)
}

pub(super) fn validate_model_reasoning_mode(
    model: Model,
    reasoning_mode: ReasoningMode,
) -> Result<()> {
    crate::ModelSelection::Codex(model)
        .capabilities(crate::ModelTransport::Native)
        .check_reasoning_mode(reasoning_mode)
}

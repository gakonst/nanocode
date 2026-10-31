use super::*;

type SharedShutdownResult = std::result::Result<(), Arc<NanocodeError>>;

#[derive(Default)]
struct ShutdownState {
    requested: std::sync::atomic::AtomicBool,
    done: nanocode_sync::Signal,
    outcome: arc_swap::ArcSwapOption<SharedShutdownResult>,
}

impl ShutdownState {
    async fn outcome(&self) -> SharedShutdownResult {
        self.done.wait().await;
        self.outcome.load_full().map_or_else(
            || Err(Arc::new(NanocodeError::AgentStopped)),
            |outcome| SharedShutdownResult::clone(&outcome),
        )
    }
}

#[derive(Clone, Default)]
pub(in crate::agent) struct DriverShutdown {
    state: Arc<ShutdownState>,
    execution_policy_owned: Arc<std::sync::atomic::AtomicBool>,
}

impl DriverShutdown {
    pub(in crate::agent) fn is_running(&self) -> bool {
        !self
            .state
            .requested
            .load(std::sync::atomic::Ordering::Acquire)
            && self.state.outcome.load().is_none()
    }

    pub(in crate::agent) fn set_execution_policy_owned(&self, owned: bool) {
        self.execution_policy_owned
            .store(owned, std::sync::atomic::Ordering::Release);
    }

    pub(in crate::agent) fn request(&self) -> bool {
        !self
            .state
            .requested
            .swap(true, std::sync::atomic::Ordering::AcqRel)
            && self.state.outcome.load().is_none()
    }

    pub(in crate::agent) async fn outcome(&self) -> SharedShutdownResult {
        self.state.outcome().await
    }

    pub(in crate::agent) fn complete(&self, outcome: Result<()>) {
        self.state
            .outcome
            .store(Some(Arc::new(outcome.map_err(Arc::new))));
        self.state.done.fire();
    }

    pub(in crate::agent) async fn stopped_error(&self) -> NanocodeError {
        let error = match self.state.outcome().await {
            Err(error)
                if matches!(
                    error.execution_policy_disposition(),
                    Some(crate::ExecutionPolicyDisposition::Reopen)
                ) =>
            {
                NanocodeError::Shutdown(error)
            }
            Ok(()) | Err(_) => NanocodeError::AgentStopped,
        };
        if matches!(error, NanocodeError::AgentStopped)
            && self
                .execution_policy_owned
                .load(std::sync::atomic::Ordering::Acquire)
        {
            NanocodeError::ExecutionPolicyOwnerStopped
        } else {
            error
        }
    }
}

pub(super) fn queued_execution_operation(
    queued_turns: &VecDeque<QueuedTurn>,
    target: TurnKey,
) -> Option<(Option<ExecutionOperation>, Prompt)> {
    queued_turns.iter().find_map(|queued| match queued {
        QueuedTurn::Pending {
            key,
            execution_operation,
            prompt,
            ..
        } if *key == target => Some((execution_operation.clone(), prompt.clone())),
        _ => None,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) const fn queued_prompt(
    key: TurnKey,
    prompt: Prompt,
    execution_operation: Option<ExecutionOperation>,
    cancel_on_admission: bool,
    thinking: Thinking,
    service_tier: ServiceTier,
    parent: Option<tracing::Span>,
    events: EventSink,
    result: oneshot::Sender<Result<TurnResult>>,
) -> QueuedTurn {
    if cancel_on_admission {
        QueuedTurn::Cancelled {
            key,
            prompt,
            execution_operation,
            cancellation_committed: false,
            thinking,
            service_tier,
            parent,
            events,
            result,
        }
    } else {
        QueuedTurn::Pending {
            key,
            prompt,
            execution_operation,
            thinking,
            service_tier,
            parent,
            events,
            result,
        }
    }
}

pub(super) fn cancel_queued_turn(
    queued_turns: &mut VecDeque<QueuedTurn>,
    target: TurnKey,
    cancellation_committed: bool,
) -> bool {
    let Some(position) = queued_turns
        .iter()
        .position(|queued| matches!(queued, QueuedTurn::Pending { key, .. } if *key == target))
    else {
        return false;
    };
    let Some(queued) = queued_turns.remove(position) else {
        return false;
    };
    let QueuedTurn::Pending {
        key,
        prompt,
        execution_operation,
        thinking,
        service_tier,
        parent,
        events,
        result,
        ..
    } = queued
    else {
        return false;
    };
    queued_turns.insert(
        position,
        QueuedTurn::Cancelled {
            key,
            prompt,
            execution_operation,
            cancellation_committed,
            thinking,
            service_tier,
            parent,
            events,
            result,
        },
    );
    true
}

pub(super) fn stop_queued_turns(queued_turns: &mut VecDeque<QueuedTurn>) {
    let accepted = std::mem::take(queued_turns);
    queued_turns.extend(accepted.into_iter().map(|queued| match queued {
        QueuedTurn::Pending {
            prompt,
            execution_operation: Some(operation),
            thinking,
            service_tier,
            events,
            result,
            ..
        } if operation.survives_shutdown() => QueuedTurn::Stopped {
            prompt,
            operation_id: operation.into_id(),
            thinking,
            service_tier,
            events,
            result,
        },
        QueuedTurn::Pending {
            key,
            prompt,
            execution_operation,
            thinking,
            service_tier,
            parent,
            events,
            result,
            ..
        } => QueuedTurn::Cancelled {
            key,
            prompt,
            execution_operation,
            cancellation_committed: false,
            thinking,
            service_tier,
            parent,
            events,
            result,
        },
        queued @ (QueuedTurn::Cancelled { .. } | QueuedTurn::Stopped { .. }) => queued,
    }));
}

pub(super) async fn begin_shutdown(
    commands: &mut mpsc::Receiver<Command>,
    queued_turns: &mut VecDeque<QueuedTurn>,
    default_thinking: Thinking,
    default_service_tier: ServiceTier,
) {
    commands.close();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Prompt {
                accepted: Some(accepted),
                result,
                ..
            } => {
                drop(accepted.send(Err(NanocodeError::AgentStopped)));
                drop(result);
            }
            Command::Prompt {
                key,
                prompt,
                execution_operation,
                accepted: None,
                cancel_on_admission,
                thinking,
                service_tier,
                parent,
                events,
                result,
            } => {
                queued_turns.push_back(queued_prompt(
                    key,
                    prompt,
                    execution_operation,
                    cancel_on_admission,
                    thinking.unwrap_or(default_thinking),
                    service_tier.unwrap_or(default_service_tier),
                    parent,
                    events,
                    result,
                ));
            }
            Command::RoutePrompt {
                route_result,
                turn_result,
                ..
            } => {
                drop(route_result.send(Err(NanocodeError::AgentStopped)));
                drop(turn_result);
            }
            Command::ChildSnapshot { result } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::Fork { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::Spawn { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::SpawnBatch { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::AppendDeveloperMessage { result, .. } => {
                result.fail(NanocodeError::AgentStopped);
            }
            Command::Context { result } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::SideQuestion { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::WithdrawSteer { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::SteerWithId { result, .. }
            | Command::Steer { result, .. }
            | Command::Cancel { result, .. }
            | Command::CancelActive { result }
            | Command::SetModel { result, .. }
            | Command::SetThinking { result, .. }
            | Command::SetAutoCompactTokenLimit { result, .. }
            | Command::SetServiceTier { result, .. }
            | Command::Preconnect { result }
            | Command::Compact { result, .. } => {
                drop(result.send(Err(NanocodeError::AgentStopped)));
            }
            Command::Shutdown => {}
        }
    }
    stop_queued_turns(queued_turns);
}

fn spawn_side_question(
    latest: Option<&Arc<CommittedSession>>,
    spawner: &BranchSpawner,
    defaults: TurnDefaults,
    session_id: &str,
    question: String,
    result: oneshot::Sender<Result<nanocode_agent_core::SideAnswer>>,
) {
    let Some(latest) = latest.map(Arc::clone) else {
        drop(result.send(Err(NanocodeError::InvalidRequest(
            "a side question requires a completed turn".to_owned(),
        ))));
        return;
    };
    let config = Arc::clone(&spawner.config);
    let images = spawner.images.clone();
    let service = (spawner.service_factory)(Arc::clone(&config));
    let thread_id = session_id.to_owned();
    let origin = crate::client::transport::layer::origin::Origin {
        context: spawner.host_context.as_ref().map(Arc::clone),
        session: Some(Arc::from(session_id)),
    };
    let task = origin.scope(async move {
        let outcome = latest
            .model()
            .side_question(
                &config,
                &images,
                service,
                defaults.model,
                defaults.thinking,
                defaults.service_tier,
                &thread_id,
                &question,
            )
            .await;
        drop(result.send(outcome));
    });
    if let Err(error) = crate::agent::executor::spawn_driver(Box::pin(task)) {
        tracing::warn!(target: "nanocode", %error, "side question could not start");
    }
}

#[derive(Clone, Copy)]
pub(super) struct TurnDefaults {
    pub(super) model: Model,
    pub(super) thinking: Thinking,
    pub(super) service_tier: ServiceTier,
}

impl TurnDefaults {
    pub(super) fn latest_request(self, prompt_cache: &ModelPromptCache) -> Self {
        let (thinking, service_tier) = prompt_cache
            .request()
            .unwrap_or((self.thinking, self.service_tier));
        Self {
            thinking,
            service_tier,
            ..self
        }
    }
}

pub(super) fn handle_idle_command(
    command: Command,
    latest: Option<&Arc<CommittedSession>>,
    spawner: &BranchSpawner,
    defaults: TurnDefaults,
    session_id: &str,
    workspace: Option<Arc<str>>,
    execution: &Execution,
) {
    match command {
        Command::ChildSnapshot { result } => {
            drop(result.send(Ok(ChildState {
                session_id: session_id.to_owned(),
                model: defaults.model,
                thinking: defaults.thinking,
                reasoning_mode: spawner.config.reasoning_mode,
                service_tier: defaults.service_tier,
                stateless_http: is_stateless_http(&spawner.config),
                conversation: latest.map(|checkpoint| checkpoint.snapshot()),
                lineage: spawner.lineage.clone(),
                conversation_id: Arc::clone(&spawner.lineage_id),
            })));
        }
        Command::Fork {
            origin,
            point,
            session_id: child_session_id,
            policy,
            result,
        } => {
            let outcome = spawner.spawn_fork(
                point,
                latest,
                session_id,
                defaults.model,
                defaults.thinking,
                defaults.service_tier,
                spawner.host_context.as_ref().map(Arc::clone),
                origin,
                child_session_id,
                policy,
            );
            drop(result.send(outcome));
        }
        Command::Spawn {
            restore,
            options,
            host_context,
            result,
        } => {
            let stateless_http = options.stateless_http;
            let outcome = options
                .validate_harness()
                .and_then(|()| {
                    if options
                        .selected_family()
                        .is_some_and(|family| family != crate::HarnessFamily::Codex)
                    {
                        return Err(NanocodeError::InvalidRequest(
                            "selected harness requires a configured child factory".into(),
                        ));
                    }

                    let resolved = options.resolve(
                        crate::ModelSelection::Codex(defaults.model),
                        defaults.thinking,
                    )?;
                    let Some(model) = resolved
                        .selected_harness_model()
                        .and_then(|model| model.as_codex())
                    else {
                        return Err(NanocodeError::InvalidRequest(
                            "selected harness requires a configured child factory".into(),
                        ));
                    };
                    let thinking = resolved.thinking.unwrap_or(defaults.thinking);
                    let reasoning_mode = crate::ModelSelection::Codex(model)
                        .capabilities(crate::ModelTransport::Native)
                        .normalize_reasoning_mode(
                            resolved.selected_reasoning_mode().unwrap_or_default(),
                        );
                    validate_model_thinking(model, thinking)?;
                    Ok((model, thinking, reasoning_mode))
                })
                .and_then(|(model, thinking, reasoning_mode)| {
                    if let Some(snapshot) = restore {
                        return spawner.restore_child(
                            snapshot,
                            workspace,
                            session_id,
                            host_context,
                            execution,
                        );
                    }
                    spawner.spawn_clean(
                        workspace,
                        session_id,
                        model,
                        thinking,
                        reasoning_mode,
                        defaults.service_tier,
                        stateless_http,
                        host_context.or_else(|| spawner.host_context.as_ref().map(Arc::clone)),
                        execution,
                    )
                });
            drop(result.send(outcome));
        }
        Command::SpawnBatch {
            count,
            observer,
            host_context,
            result,
        } => {
            let outcome = spawner.spawn_clean_many(
                workspace,
                session_id,
                defaults,
                count,
                observer.as_deref(),
                host_context,
                execution,
            );
            drop(result.send(outcome));
        }
        Command::SideQuestion { question, result } => {
            spawn_side_question(latest, spawner, defaults, session_id, question, result);
        }
        Command::WithdrawSteer { result, .. } => {
            drop(result.send(Ok(false)));
        }
        Command::SteerWithId { result, .. } | Command::Steer { result, .. } => {
            drop(result.send(Err(NanocodeError::TurnNotSteerable)));
        }
        Command::RoutePrompt {
            route_result,
            turn_result,
            ..
        } => {
            drop(route_result.send(Err(NanocodeError::AgentStopped)));
            drop(turn_result);
        }
        Command::Cancel { result, .. } | Command::CancelActive { result } => {
            drop(result.send(Err(NanocodeError::TurnNotCancellable)));
        }
        Command::SetThinking { result, .. }
        | Command::SetAutoCompactTokenLimit { result, .. }
        | Command::SetServiceTier { result, .. }
        | Command::Preconnect { result } => {
            drop(result.send(Ok(())));
        }
        Command::SetModel { result, .. } => {
            drop(result.send(Err(model_change_locked())));
        }
        Command::Shutdown => {}
        Command::Compact { result, .. } => {
            drop(result.send(Err(NanocodeError::AgentStopped)));
        }
        Command::AppendDeveloperMessage { result, .. } => {
            result.fail(NanocodeError::AgentStopped);
        }
        Command::Context { result } => {
            drop(result.send(super::agent_session_context(
                latest.map(AsRef::as_ref),
                workspace.as_deref(),
                &spawner.context_source,
            )));
        }
        Command::Prompt { .. } => {}
    }
}

pub(super) fn model_change_locked() -> NanocodeError {
    NanocodeError::InvalidRequest(
        "the model is fixed for this thread; start a new thread to use another model".to_owned(),
    )
}

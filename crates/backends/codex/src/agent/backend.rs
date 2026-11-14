use super::handle::request_fork;
use super::*;
pub use nanocode_agent_core::backend::AgentFactory;
pub use nanocode_agent_core::backend::*;

pub(super) fn resolve_fork_point(
    point: crate::session::ForkPoint,
    conversation_id: &str,
) -> Result<ForkFrom> {
    match point {
        crate::session::ForkPoint::Latest => Ok(ForkFrom::Latest),
        crate::session::ForkPoint::Turn(completed) => {
            let boundary = completed
                .boundary()
                .ok_or(NanocodeError::ReplayedCheckpointUnavailable)?;
            let Some(committed) = boundary.downcast::<CommittedSession>() else {
                if !boundary.is::<super::checkpoint::ReplayedBoundary>()
                    && let Ok(checkpoint) = boundary.checkpoint()
                {
                    checkpoint.require_family(crate::HarnessFamily::Codex)?;
                }
                return Err(NanocodeError::ReplayedCheckpointUnavailable);
            };
            if committed.lineage_id() != conversation_id {
                return Err(NanocodeError::CheckpointLineageMismatch);
            }
            Ok(ForkFrom::Live(committed))
        }
        crate::session::ForkPoint::Checkpoint(checkpoint) => {
            checkpoint.require_family(crate::HarnessFamily::Codex)?;
            if checkpoint.conversation_id() != conversation_id {
                return Err(NanocodeError::CheckpointLineageMismatch);
            }
            let conversation = ChildState::from_checkpoint(checkpoint)?
                .conversation
                .ok_or(NanocodeError::ForkBeforeCompletedTurn)?;
            Ok(ForkFrom::Snapshot(Box::new(conversation)))
        }
        _ => Err(NanocodeError::ReplayedCheckpointUnavailable),
    }
}

pub(super) struct LocalLifecycle {
    pub(super) child_handle: AgentHandle,
    pub(super) commands: mpsc::Sender<Command>,
    pub(super) execution: Execution,
    pub(super) shutdown: DriverShutdown,
    pub(super) checkpoints: Arc<CheckpointSource>,

    pub(super) initial_ready: Arc<nanocode_sync::Slot<oneshot::Receiver<Result<()>>>>,
    pub(super) images: ImageHistory,
}

async fn store_prompt_images(images: &ImageHistory, prompt: &mut Prompt) -> Result<()> {
    images
        .store_prompt(&mut prompt.instruction)
        .await
        .map_err(nanocode_durability::agent_store_error)
}

const CODEX_CAPABILITIES: Capabilities = {
    let mut capabilities = Capabilities::NONE;
    capabilities.checkpoint = true;
    capabilities.resume = true;
    capabilities.fork = true;
    capabilities.fork_at = true;
    capabilities.side_conversation = true;
    capabilities.side_question = true;
    capabilities.spawn = true;
    capabilities.steering = true;
    capabilities.identified_steering = true;
    capabilities.compaction = true;
    capabilities.developer_messages = true;
    capabilities.context = true;
    capabilities.thinking = crate::session::Mutability::Anytime;
    capabilities.service_tier = crate::session::Mutability::Anytime;
    capabilities.ultrafast_service_tier = true;
    capabilities
};

impl LifecycleBackend for LocalLifecycle {
    fn harness_family(&self) -> crate::HarnessFamily {
        crate::HarnessFamily::Codex
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = CODEX_CAPABILITIES;

        let model = self
            .child_handle
            .harness_model()
            .capabilities(crate::ModelTransport::Native);
        capabilities.ultrafast_service_tier = model.supports_service_tier(ServiceTier::Ultrafast);
        if !model.fast_mode() {
            capabilities.service_tier = crate::session::Mutability::Fixed;
        }
        capabilities
    }

    fn persistence(&self) -> Option<Persistence> {
        self.execution.durable_state_id().map(Persistence::durable)
    }

    fn submit(&self, mut request: BackendPrompt) -> BackendFuture<Result<BackendTurn>> {
        let commands = self.commands.clone();
        let execution = self.execution.clone();
        let shutdown = self.shutdown.clone();
        let images = self.images.clone();
        Box::pin(async move {
            store_prompt_images(&images, &mut request.prompt).await?;
            let execution_operation =
                request
                    .request_id
                    .map(ExecutionOperation::Caller)
                    .or_else(|| {
                        execution
                            .identifies_prompts()
                            .then(|| ExecutionOperation::Automatic(SessionId::new().to_string()))
                    });
            let (result, receiver) = oneshot::channel();
            let (accepted, acceptance) = if execution_operation.is_some() {
                let (accepted, acceptance) = oneshot::channel();
                (Some(accepted), Some(acceptance))
            } else {
                (None, None)
            };
            let parent = nanocode_trace::Span::current();
            let parent = (!parent.is_disabled()).then_some(parent);
            if commands
                .send(Command::Prompt {
                    key: TurnKey(request.key.0),
                    prompt: request.prompt,
                    execution_operation,
                    accepted,
                    cancel_on_admission: request.cancel_on_admission,
                    thinking: None,
                    service_tier: None,
                    parent,
                    events: EventSink::from_publisher(request.events),
                    result,
                })
                .await
                .is_err()
            {
                return Err(shutdown.stopped_error().await);
            }
            let request_id = if let Some(acceptance) = acceptance {
                Some(match acceptance.await {
                    Ok(Ok(request_id)) => request_id,
                    Ok(Err(NanocodeError::AgentStopped)) | Err(_) => {
                        return Err(shutdown.stopped_error().await);
                    }
                    Ok(Err(error)) => return Err(error),
                })
            } else {
                None
            };
            Ok(BackendTurn {
                request_id,
                result: Box::pin(
                    async move { receiver.await.map_err(|_| NanocodeError::TurnStopped)? },
                ),
            })
        })
    }

    fn route(&self, mut request: BackendPrompt) -> BackendFuture<Result<BackendPromptRoute>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        let execution = self.execution.clone();
        let images = self.images.clone();
        Box::pin(async move {
            store_prompt_images(&images, &mut request.prompt).await?;
            if let Some(id) = &request.request_id
                && execution
                    .identified_steer_accepted(id.clone(), &request.prompt)
                    .await?
            {
                return Ok(BackendPromptRoute::Steered);
            }
            let parent = nanocode_trace::Span::current();
            let parent = (!parent.is_disabled()).then_some(parent);
            let (turn_result, turn_receiver) = oneshot::channel();
            let (route_result, route_receiver) = oneshot::channel();
            if commands
                .send(Command::RoutePrompt {
                    key: TurnKey(request.key.0),
                    prompt: request.prompt,
                    request_id: request.request_id,
                    cancel_on_admission: request.cancel_on_admission,
                    parent,
                    events: EventSink::from_publisher(request.events),
                    turn_result,
                    route_result,
                })
                .await
                .is_err()
            {
                return Err(shutdown.stopped_error().await);
            }
            match route_receiver.await {
                Ok(Ok(PromptRouteKind::Started { request_id })) => {
                    Ok(BackendPromptRoute::Started(BackendTurn {
                        request_id,
                        result: Box::pin(async move {
                            turn_receiver
                                .await
                                .map_err(|_| NanocodeError::TurnStopped)?
                        }),
                    }))
                }
                Ok(Ok(PromptRouteKind::Steered)) => Ok(BackendPromptRoute::Steered),
                Ok(Err(NanocodeError::AgentStopped)) | Err(_) => {
                    Err(shutdown.stopped_error().await)
                }
                Ok(Err(error)) => Err(error),
            }
        })
    }

    fn steer(&self, key: BackendTurnKey, mut prompt: Prompt) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        let images = self.images.clone();
        Box::pin(async move {
            store_prompt_images(&images, &mut prompt).await?;
            request_command(&commands, &shutdown, |result| Command::Steer {
                key: TurnKey(key.0),
                prompt,
                result,
            })
            .await
        })
    }

    fn steer_with_id(
        &self,
        key: BackendTurnKey,
        id: String,
        mut prompt: Prompt,
    ) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        let images = self.images.clone();
        Box::pin(async move {
            store_prompt_images(&images, &mut prompt).await?;
            request_command(&commands, &shutdown, |result| Command::SteerWithId {
                key: TurnKey(key.0),
                id,
                prompt,
                result,
            })
            .await
        })
    }

    fn unfinished_request(&self, id: String) -> BackendFuture<Result<bool>> {
        let execution = self.execution.clone();
        Box::pin(async move { execution.unfinished_operation(id).await })
    }

    fn steer_receipt(
        &self,
        id: String,
        mut prompt: Prompt,
    ) -> BackendFuture<Result<SteerReceiptStatus>> {
        let execution = self.execution.clone();
        let images = self.images.clone();
        Box::pin(async move {
            store_prompt_images(&images, &mut prompt).await?;
            execution.steer_receipt(id, &prompt).await
        })
    }

    fn withdraw_steer(&self, key: BackendTurnKey, id: String) -> BackendFuture<Result<bool>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::WithdrawSteer {
                key: TurnKey(key.0),
                id,
                result,
            })
            .await
        })
    }

    fn cancel(&self, key: BackendTurnKey) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::Cancel {
                key: TurnKey(key.0),
                result,
            })
            .await
        })
    }

    fn cancel_active(&self) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::CancelActive {
                result,
            })
            .await
        })
    }

    fn set_harness_model(&self, model: crate::ModelSelection) -> BackendFuture<Result<()>> {
        let Some(model) = model.as_codex() else {
            return Box::pin(async {
                Err(NanocodeError::InvalidRequest(
                    "model belongs to another harness family".into(),
                ))
            });
        };
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::SetModel {
                model,
                result,
            })
            .await
        })
    }

    fn set_thinking(&self, thinking: Thinking) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::SetThinking {
                thinking,
                result,
            })
            .await
        })
    }

    fn set_auto_compact_token_limit(&self, limit: Option<u64>) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| {
                Command::SetAutoCompactTokenLimit { limit, result }
            })
            .await
        })
    }

    fn set_compaction_thinking(&self, thinking: Option<Thinking>) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| {
                Command::SetCompactionThinking { thinking, result }
            })
            .await
        })
    }

    fn set_fast_mode(&self, enabled: bool) -> BackendFuture<Result<()>> {
        self.set_service_tier(ServiceTier::from_fast_mode(enabled))
    }

    fn set_service_tier(&self, service_tier: ServiceTier) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::SetServiceTier {
                service_tier,
                result,
            })
            .await
        })
    }

    fn preconnect(&self) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::Preconnect {
                result,
            })
            .await
        })
    }

    fn compact(&self) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            let parent = nanocode_trace::Span::current();
            let parent = (!parent.is_disabled()).then_some(parent);
            request_command(&commands, &shutdown, |result| Command::Compact {
                parent,
                result,
            })
            .await
        })
    }

    fn append_developer_message(&self, text: String) -> BackendFuture<Result<AgentSessionContext>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| {
                Command::AppendDeveloperMessage {
                    text,
                    result: super::turn::DeveloperMessageResult::Context(result),
                }
            })
            .await
        })
    }

    fn append_developer_notice(
        &self,
        text: String,
        prefix: Option<String>,
    ) -> BackendFuture<Result<bool>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| {
                Command::AppendDeveloperMessage {
                    text,
                    result: super::turn::DeveloperMessageResult::Notice { prefix, result },
                }
            })
            .await
        })
    }

    fn context(&self) -> BackendFuture<Result<AgentSessionContext>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::Context { result }).await
        })
    }

    fn transcript(&self) -> BackendFuture<Result<Vec<crate::input::PromptMessage>>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            let state = request_command(&commands, &shutdown, |result| Command::ChildSnapshot {
                result,
            })
            .await?;
            Ok(state
                .conversation
                .as_ref()
                .map(crate::session::SessionSnapshot::transcript)
                .unwrap_or_default())
        })
    }

    fn side_question(
        &self,
        question: String,
    ) -> BackendFuture<Result<nanocode_agent_core::SideAnswer>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::SideQuestion {
                question,
                result,
            })
            .await
        })
    }

    fn checkpoint(&self) -> BackendFuture<Result<SessionCheckpoint>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            request_command(&commands, &shutdown, |result| Command::ChildSnapshot {
                result,
            })
            .await?
            .into_checkpoint()
        })
    }

    fn settings(&self) -> BackendFuture<Result<(crate::ModelSelection, Thinking, ReasoningMode)>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            let state = request_command(&commands, &shutdown, |result| Command::ChildSnapshot {
                result,
            })
            .await?;
            Ok((
                crate::ModelSelection::Codex(state.model),
                state.thinking,
                state.reasoning_mode,
            ))
        })
    }

    fn persist_initial(&self) -> BackendFuture<Result<()>> {
        let ready = self.initial_ready.take();
        Box::pin(async move {
            match ready {
                Some(ready) => ready.await.map_err(|_| NanocodeError::AgentStopped)?,
                None => Ok(()),
            }
        })
    }

    fn discard_initial(&self) -> BackendFuture<Result<()>> {
        let execution = self.execution.clone();
        Box::pin(async move { execution.discard_initial_checkpoint().await })
    }

    fn spawn(&self, options: SpawnOptions) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let parent = self.child_handle.clone();
        Box::pin(async move { parent.spawn_with(options).await })
    }

    fn fork(&self, request: ForkRequest) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let commands = self.commands.clone();
        let shutdown = self.shutdown.clone();
        let checkpoints = Arc::clone(&self.checkpoints);
        let execution = self.execution.clone();
        Box::pin(async move {
            let (point, origin) = request.into_parts();
            let point = resolve_fork_point(point, checkpoints.conversation_id())?;
            let session_id = SessionId::new();
            let policy = execution.branch_policy(&checkpoints.child_info(&session_id, origin))?;
            request_fork(&commands, &shutdown, point, origin, session_id, policy).await
        })
    }

    fn flush(&self) -> BackendFuture<Result<()>> {
        #[cfg(not(target_family = "wasm"))]
        {
            let execution = self.execution.clone();
            Box::pin(async move { execution.flush().await })
        }
        #[cfg(target_family = "wasm")]
        {
            Box::pin(async { Ok(()) })
        }
    }

    fn shutdown(&self) -> BackendFuture<Result<()>> {
        let commands = self.commands.clone();
        let execution = self.execution.clone();
        let shutdown = self.shutdown.clone();
        Box::pin(async move {
            if shutdown.request() && commands.send(Command::Shutdown).await.is_err() {
                let outcome = match execution.shutdown().await {
                    Ok(()) => Err(NanocodeError::AgentStopped),
                    Err(error) => Err(error),
                };
                shutdown.complete(outcome);
            }
            shutdown.outcome().await.map_err(NanocodeError::Shutdown)
        })
    }
}

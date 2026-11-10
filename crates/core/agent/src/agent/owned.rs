use super::backend::{
    BackendFuture, BackendPrompt, BackendPromptRoute, BackendTurn, BackendTurnKey,
    LifecycleBackend, SteerReceiptStatus,
};
use super::handle::{Interrupted, OwnershipProbe, Signal};
use super::*;
use nanocode_sync::{Swap, SwapOption};
use std::sync::atomic::AtomicU8;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Life {
    Live,
    Stale,
    Closed,
}

struct Lifecycle(AtomicU8);

impl Lifecycle {
    const fn new() -> Self {
        Self(AtomicU8::new(Life::Live as u8))
    }

    fn get(&self) -> Life {
        match self.0.load(Ordering::Acquire) {
            0 => Life::Live,
            1 => Life::Stale,
            _ => Life::Closed,
        }
    }

    fn turn(&self, from: Life, to: Life) {
        let _ = self
            .0
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire);
    }

    fn close(&self) {
        self.0.store(Life::Closed as u8, Ordering::Release);
    }
}

#[doc(hidden)]
pub trait AgentOwner: Send + Sync + 'static {
    fn admit(&self) -> Result<()>;

    fn finish(&self);

    fn adopt(&self, key: usize, agent: Agent) -> bool;

    fn release(&self, key: usize);

    fn reopen(&self, _session_id: &str) -> Option<BackendFuture<Result<(Agent, AgentEvents)>>> {
        None
    }
}

#[doc(hidden)]
pub async fn adopt(agent: Agent, owner: &Arc<dyn AgentOwner>) -> Result<Agent> {
    let key = Arc::as_ptr(&agent.backend).cast::<()>() as usize;
    let backend: Arc<dyn LifecycleBackend> = Arc::new(Owned(Arc::new(Shared {
        inner: Swap::from_pointee(Arc::clone(&agent.backend)),
        probe: SwapOption::from(agent.ownership.clone().map(Arc::new)),
        owner: Arc::downgrade(owner),
        key,
        session_id: Arc::from(agent.session_id()),
        events: agent.events.downgrade(),
        interrupted: Arc::clone(&agent.interrupted),
        next_turn: Arc::clone(&agent.next_turn),
        healing: SwapOption::empty(),
        life: Lifecycle::new(),
    })));
    let owned = Agent {
        backend,
        events: agent.events.clone(),
        next_turn: Arc::clone(&agent.next_turn),
        agent_id: Arc::clone(&agent.agent_id),
        session: Arc::clone(&agent.session),
        driver_turn_ids: agent.driver_turn_ids,
        seed: Arc::clone(&agent.seed),
        interrupted: Arc::clone(&agent.interrupted),
        requests: Arc::clone(&agent.requests),
        ownership: None,
    };
    if owner.adopt(key, owned.clone()) {
        return Ok(owned);
    }

    agent.backend.shutdown().await?;
    Err(NanocodeError::AgentStopped)
}

struct Admitted(Arc<dyn AgentOwner>);

impl Drop for Admitted {
    fn drop(&mut self) {
        self.0.finish();
    }
}

async fn adopt_created(
    owner: std::sync::Weak<dyn AgentOwner>,
    create: impl FnOnce() -> BackendFuture<Result<(Agent, AgentEvents)>>,
) -> Result<(Agent, AgentEvents)> {
    let Some(owner) = owner.upgrade() else {
        return create().await;
    };
    owner.admit()?;
    let admitted = Admitted(Arc::clone(&owner));
    let (agent, events) = create().await?;
    let agent = adopt(agent, &owner).await?;
    drop(admitted);
    Ok((agent, events))
}

struct Shared {
    inner: Swap<Arc<dyn LifecycleBackend>>,
    probe: SwapOption<OwnershipProbe>,
    owner: std::sync::Weak<dyn AgentOwner>,
    key: usize,
    session_id: Arc<str>,
    events: nanocode_types::events::WeakAgentEventPublisher,
    interrupted: Arc<Interrupted>,
    next_turn: Arc<AtomicU64>,
    healing: SwapOption<Signal>,
    life: Lifecycle,
}

struct Healing<'a> {
    healing: &'a SwapOption<Signal>,
    signal: Arc<Signal>,
}

impl Drop for Healing<'_> {
    fn drop(&mut self) {
        self.healing.rcu(|current| match current {
            Some(current) if Arc::ptr_eq(current, &self.signal) => None,
            other => other.clone(),
        });
        self.signal.fire();
    }
}

impl Shared {
    fn backend(&self) -> Arc<dyn LifecycleBackend> {
        Arc::clone(&self.inner.load())
    }

    fn observe<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result
            && requires_reopen(error)
        {
            self.life.turn(Life::Live, Life::Stale);
        }
        result
    }

    fn observed_turn(self: &Arc<Self>, turn: BackendTurn) -> BackendTurn {
        let shared = Arc::clone(self);
        let result = turn.result;
        BackendTurn {
            request_id: turn.request_id,
            result: Box::pin(async move { shared.observe(result.await) }),
        }
    }

    fn heals(&self, error: &NanocodeError, prompt: &BackendPrompt) -> bool {
        requires_reopen(error)
            && self.life.get() != Life::Closed
            && !prompt.events.turn_is_terminal()
    }

    fn stale(&self) -> bool {
        self.life.get() == Life::Stale
    }

    async fn healed(&self, retry: &mut BackendPrompt) -> Arc<dyn LifecycleBackend> {
        let backend = self.backend();
        let events = retry.events.clone();
        self.heal(&backend, &events, Some(retry))
            .await
            .unwrap_or(backend)
    }

    async fn heal(
        &self,
        failed: &Arc<dyn LifecycleBackend>,
        events: &nanocode_types::events::AgentEventPublisher,
        mut retry: Option<&mut BackendPrompt>,
    ) -> Result<Arc<dyn LifecycleBackend>> {
        loop {
            let current = self.backend();
            if !Arc::ptr_eq(&current, failed) {
                return Ok(current);
            }
            let signal = Arc::new(Signal::new());
            let previous = self
                .healing
                .rcu(|healing| healing.clone().or_else(|| Some(Arc::clone(&signal))));
            if let Some(running) = previous {
                running.wait().await;
                continue;
            }
            let _healing = Healing {
                healing: &self.healing,
                signal,
            };
            return self.reopen(events, retry.take()).await;
        }
    }

    async fn reopen(
        &self,
        events: &nanocode_types::events::AgentEventPublisher,
        retry: Option<&mut BackendPrompt>,
    ) -> Result<Arc<dyn LifecycleBackend>> {
        let owner = self.owner.upgrade().ok_or(NanocodeError::AgentStopped)?;
        let reopened = owner
            .reopen(&self.session_id)
            .ok_or(NanocodeError::AgentStopped)?;
        let (agent, agent_events) = reopened.await?;
        let backend = Arc::clone(&agent.backend);
        let previous = self.inner.swap(Arc::new(Arc::clone(&backend)));
        self.probe.store(agent.ownership.clone().map(Arc::new));
        self.life.turn(Life::Stale, Life::Live);
        forward_events(agent_events, self.events.clone());
        spawn_detached(async move {
            let _ = previous.shutdown().await;
        });
        let mut turns = self.interrupted.resumable(agent.interrupted.take_turns());
        let Some(retry) = retry else {
            self.interrupted.replace(turns);
            return Ok(backend);
        };
        let mut prompts = Vec::with_capacity(turns.len());
        for (_, input) in &turns {
            prompts.push(backend.decode_durable_prompt(input.clone())?);
        }
        let retried = serde_json::to_value(&retry.prompt).ok();
        let matched =
            turns
                .iter()
                .zip(&prompts)
                .position(|((id, _), prompt)| match &retry.request_id {
                    Some(request_id) => request_id == id,
                    None => serde_json::to_value(prompt).ok() == retried,
                });
        let later = match matched {
            Some(position) => {
                let later = turns.split_off(position + 1);
                if let Some((id, _)) = turns.pop() {
                    retry.request_id = Some(id);
                }
                later
            }
            None => Vec::new(),
        };
        for ((id, _), prompt) in turns.into_iter().zip(prompts) {
            let (turn_events, _turn) = events.mirrored_channel();
            backend
                .submit(BackendPrompt {
                    key: BackendTurnKey(self.next_turn.fetch_add(1, Ordering::Relaxed)),
                    prompt,
                    request_id: Some(id.clone()),
                    cancel_on_admission: false,
                    events: turn_events.with_turn_id(id),
                })
                .await?;
        }
        self.interrupted.replace(later);
        Ok(backend)
    }
}

fn forward_events(
    mut events: AgentEvents,
    publisher: nanocode_types::events::WeakAgentEventPublisher,
) {
    spawn_detached(async move {
        while let Some(event) = events.recv().await {
            let Some(publisher) = publisher.upgrade() else {
                return;
            };
            let _ = publisher.publish_next(event.kind, event.payload);
        }
    });
}

#[cfg(not(target_family = "wasm"))]
fn spawn_detached(future: impl Future<Output = ()> + Send + 'static) {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(future));
    }
}

#[cfg(target_family = "wasm")]
fn spawn_detached(future: impl Future<Output = ()> + 'static) {
    nanocode_platform::spawn_local(future);
}

struct Owned(Arc<Shared>);

fn requires_reopen(error: &NanocodeError) -> bool {
    matches!(
        error,
        NanocodeError::AgentStopped
            | NanocodeError::TurnStopped
            | NanocodeError::ExecutionPolicyOwnerStopped
    ) || error.execution_policy_disposition() == Some(crate::ExecutionPolicyDisposition::Reopen)
}

impl LifecycleBackend for Owned {
    fn decode_durable_prompt(&self, input: serde_json::Value) -> Result<Prompt> {
        self.0.backend().decode_durable_prompt(input)
    }
    fn harness_family(&self) -> crate::HarnessFamily {
        self.0.backend().harness_family()
    }
    fn capabilities(&self) -> Capabilities {
        self.0.backend().capabilities()
    }
    fn persistence(&self) -> Option<Persistence> {
        self.0.backend().persistence()
    }
    fn submit(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendTurn>> {
        let shared = Arc::clone(&self.0);
        let mut retry = prompt.clone();
        let backend = shared.backend();
        let eager = (!shared.stale()).then(|| backend.submit(prompt));
        Box::pin(async move {
            let (backend, submitted) = match eager {
                Some(submitted) => (backend, submitted.await),
                None => {
                    let backend = shared.healed(&mut retry).await;
                    let submitted = backend.submit(retry.clone()).await;
                    (backend, submitted)
                }
            };
            let turn = match submitted {
                Err(error) if shared.heals(&error, &retry) => {
                    let events = retry.events.clone();
                    match shared.heal(&backend, &events, Some(&mut retry)).await {
                        Ok(backend) => backend.submit(retry).await,
                        Err(_) => Err(error),
                    }
                }
                outcome => outcome,
            };
            shared.observe(turn).map(|turn| shared.observed_turn(turn))
        })
    }
    fn route(&self, prompt: BackendPrompt) -> BackendFuture<Result<BackendPromptRoute>> {
        let shared = Arc::clone(&self.0);
        let mut retry = prompt.clone();
        let backend = shared.backend();
        let eager = (!shared.stale()).then(|| backend.route(prompt));
        Box::pin(async move {
            let (backend, routed) = match eager {
                Some(routed) => (backend, routed.await),
                None => {
                    let backend = shared.healed(&mut retry).await;
                    let routed = backend.route(retry.clone()).await;
                    (backend, routed)
                }
            };
            let route = match routed {
                Err(error) if shared.heals(&error, &retry) => {
                    let events = retry.events.clone();
                    match shared.heal(&backend, &events, Some(&mut retry)).await {
                        Ok(backend) => backend.route(retry).await,
                        Err(_) => Err(error),
                    }
                }
                outcome => outcome,
            };
            shared.observe(route).map(|route| match route {
                BackendPromptRoute::Started(turn) => {
                    BackendPromptRoute::Started(shared.observed_turn(turn))
                }
                BackendPromptRoute::Steered => BackendPromptRoute::Steered,
            })
        })
    }
    fn is_stopped(&self) -> bool {
        self.0.life.get() != Life::Live || self.0.backend().is_stopped()
    }
    fn owns_session(&self) -> BackendFuture<Result<bool>> {
        match self.0.probe.load_full() {
            Some(probe) => probe(),
            None => self.0.backend().owns_session(),
        }
    }
    fn reopen(
        &self,
        events: nanocode_types::events::AgentEventPublisher,
    ) -> BackendFuture<Result<()>> {
        let shared = Arc::clone(&self.0);
        Box::pin(async move {
            if shared.life.get() == Life::Closed {
                return Err(NanocodeError::AgentStopped);
            }
            let backend = shared.backend();
            shared.heal(&backend, &events, None).await.map(drop)
        })
    }
    fn steer(&self, key: BackendTurnKey, prompt: Prompt) -> BackendFuture<Result<()>> {
        self.0.backend().steer(key, prompt)
    }
    fn steer_with_id(
        &self,
        key: BackendTurnKey,
        id: String,
        prompt: Prompt,
    ) -> BackendFuture<Result<()>> {
        self.0.backend().steer_with_id(key, id, prompt)
    }
    fn withdraw_steer(&self, key: BackendTurnKey, id: String) -> BackendFuture<Result<bool>> {
        self.0.backend().withdraw_steer(key, id)
    }
    fn steer_receipt(
        &self,
        id: String,
        prompt: Prompt,
    ) -> BackendFuture<Result<SteerReceiptStatus>> {
        self.0.backend().steer_receipt(id, prompt)
    }
    fn cancel(&self, key: BackendTurnKey) -> BackendFuture<Result<()>> {
        self.0.backend().cancel(key)
    }
    fn cancel_active(&self) -> BackendFuture<Result<()>> {
        self.0.backend().cancel_active()
    }
    fn set_harness_model(&self, model: crate::ModelSelection) -> BackendFuture<Result<()>> {
        self.0.backend().set_harness_model(model)
    }
    fn set_thinking(&self, thinking: Thinking) -> BackendFuture<Result<()>> {
        self.0.backend().set_thinking(thinking)
    }
    fn set_fast_mode(&self, enabled: bool) -> BackendFuture<Result<()>> {
        self.0.backend().set_fast_mode(enabled)
    }
    fn set_auto_compact_token_limit(&self, limit: Option<u64>) -> BackendFuture<Result<()>> {
        self.0.backend().set_auto_compact_token_limit(limit)
    }
    fn set_compaction_thinking(&self, thinking: Option<Thinking>) -> BackendFuture<Result<()>> {
        self.0.backend().set_compaction_thinking(thinking)
    }
    fn set_service_tier(&self, service_tier: ServiceTier) -> BackendFuture<Result<()>> {
        self.0.backend().set_service_tier(service_tier)
    }
    fn compact(&self) -> BackendFuture<Result<()>> {
        self.0.backend().compact()
    }
    fn preconnect(&self) -> BackendFuture<Result<()>> {
        self.0.backend().preconnect()
    }
    fn append_developer_message(
        &self,
        text: String,
    ) -> BackendFuture<Result<crate::AgentSessionContext>> {
        self.0.backend().append_developer_message(text)
    }
    fn append_developer_notice(
        &self,
        text: String,
        prefix: Option<String>,
    ) -> BackendFuture<Result<bool>> {
        self.0.backend().append_developer_notice(text, prefix)
    }
    fn context(&self) -> BackendFuture<Result<crate::AgentSessionContext>> {
        self.0.backend().context()
    }
    fn side_question(&self, question: String) -> BackendFuture<Result<crate::SideAnswer>> {
        self.0.backend().side_question(question)
    }
    fn transcript(&self) -> BackendFuture<Result<Vec<nanocode_types::PromptMessage>>> {
        self.0.backend().transcript()
    }
    fn checkpoint(&self) -> BackendFuture<Result<SessionCheckpoint>> {
        self.0.backend().checkpoint()
    }
    fn persist_initial(&self) -> BackendFuture<Result<()>> {
        self.0.backend().persist_initial()
    }
    fn discard_initial(&self) -> BackendFuture<Result<()>> {
        self.0.backend().discard_initial()
    }
    fn spawn(&self, options: SpawnOptions) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let inner = self.0.backend();
        Box::pin(adopt_created(self.0.owner.clone(), move || {
            inner.spawn(options)
        }))
    }
    fn fork(&self, request: ForkRequest) -> BackendFuture<Result<(Agent, AgentEvents)>> {
        let inner = self.0.backend();
        Box::pin(adopt_created(self.0.owner.clone(), move || {
            inner.fork(request)
        }))
    }
    fn flush(&self) -> BackendFuture<Result<()>> {
        self.0.backend().flush()
    }
    fn disconnect(&self) -> BackendFuture<Result<()>> {
        self.0.life.close();
        let disconnected = self.0.backend().disconnect();
        let owner = self.0.owner.clone();
        let key = self.0.key;
        Box::pin(async move {
            let result = disconnected.await;
            if let Some(owner) = owner.upgrade() {
                owner.release(key);
            }
            result
        })
    }
    fn shutdown(&self) -> BackendFuture<Result<()>> {
        self.0.life.close();
        let stopped = self.0.backend().shutdown();
        let owner = self.0.owner.clone();
        let key = self.0.key;
        Box::pin(async move {
            let result = stopped.await;
            if let Some(owner) = owner.upgrade() {
                owner.release(key);
            }
            result
        })
    }
    fn set_parking(&self, parking: bool) {
        self.0.backend().set_parking(parking);
    }
    fn parked(&self, after: u64) -> BackendFuture<u64> {
        self.0.backend().parked(after)
    }
    fn park(&self) -> BackendFuture<Result<Option<Arc<dyn super::backend::ParkedCall>>>> {
        let shared = Arc::clone(&self.0);
        let parking = shared.backend().park();
        Box::pin(async move {
            let parked = parking.await?;
            if parked.is_some() {
                shared.life.close();
                if let Some(owner) = shared.owner.upgrade() {
                    owner.release(shared.key);
                }
            }
            Ok(parked)
        })
    }
    fn unpark(&self, call: Arc<dyn super::backend::ParkedCall>) {
        self.0.backend().unpark(call);
    }
}

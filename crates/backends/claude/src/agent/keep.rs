use super::{
    CACHE_LIFETIME, HOUR_CACHE_LIFETIME, HistoryMessage, MessagesRequest, Result, State, add_usage,
    images, provider_error, spawn_task,
};
use crate::FrozenWireProfile;
use nanocode_agent_core::{ClaudeModel, Origin};
use nanocode_platform::sleep;
use nanocode_sync::Notify;
use nanocode_trace::Instrument as _;
use std::{
    sync::{Arc, Weak, atomic::Ordering},
    time::Duration,
};

pub(super) struct KeptRequest {
    pub(super) request: Arc<MessagesRequest>,
    pub(super) sent: Arc<Vec<HistoryMessage>>,
    pub(super) wire_profile: Option<FrozenWireProfile>,
    pub(super) origin: Option<Arc<str>>,
    pub(super) reach: Duration,
}

impl State {
    pub(super) fn keep(&self, kept: KeptRequest) {
        self.kept.set(Some(Arc::new(kept)));
        self.refreshes.store(0, Ordering::SeqCst);
        self.kept_changed.notify_waiters();
        self.keep_watching();
    }

    fn kept_wait(&self) -> Option<Duration> {
        let kept = Option::clone(&self.kept.get())?;
        let wait = self.refresh_due(&kept);
        if wait.is_none() {
            self.kept.update(|current| {
                if current
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &kept))
                {
                    *current = None;
                }
            });
        }
        wait
    }

    fn keep_watching(&self) {
        if self.keeping.swap(true, Ordering::SeqCst) {
            return;
        }
        match self.this.get() {
            Some(state) => spawn_task(Box::pin(keep_cache(
                Weak::clone(state),
                Arc::clone(&self.kept_changed),
            ))),
            None => self.keeping.store(false, Ordering::SeqCst),
        }
    }

    fn refresh_due(&self, kept: &KeptRequest) -> Option<Duration> {
        let one_hour = kept.request.cache_control.as_ref()?.ttl == Some(crate::CacheTtl::OneHour);
        let continues = self.lineage.depth == 0
            || matches!(self.lineage.origin, Origin::Fork | Origin::SideConversation);
        if !continues && self.conversation.try_acquire().is_some() {
            return None;
        }
        let allowed = kept
            .request
            .model
            .parse::<ClaudeModel>()
            .ok()?
            .cache_refreshes(one_hour, kept.request.speed == Some(crate::Speed::Fast))?;
        if self.refreshes.load(Ordering::SeqCst) >= allowed {
            return None;
        }
        let lifetime = if one_hour {
            HOUR_CACHE_LIFETIME
        } else {
            CACHE_LIFETIME
        };
        let elapsed = self.requested.read(|requested| *requested)?.elapsed();
        if elapsed >= lifetime {
            return None;
        }
        Some(
            lifetime
                .saturating_sub(kept.reach.saturating_mul(2))
                .saturating_sub(elapsed),
        )
    }

    async fn refresh_kept(&self) {
        let Some(kept) = Option::clone(&self.kept.get()) else {
            return;
        };
        if self.refresh_due(&kept).is_none_or(|wait| !wait.is_zero()) {
            return;
        }
        let started = nanocode_platform::now();
        let span = nanocode_trace::info_span!(
            "claude.cache.refresh",
            session.id = self.session_id.as_str(),
            model = kept.request.model.as_str(),
            cached_input_tokens = nanocode_trace::field::Empty,
            cache_write_input_tokens = nanocode_trace::field::Empty,
            input_tokens = nanocode_trace::field::Empty,
        );
        let refreshed = self.refresh(&kept).instrument(span.clone()).await;
        match refreshed {
            Ok(mut usage) => {
                usage.estimated_cost = self.estimate_cost(Some(&kept.request), &usage);
                self.refreshed.update(|spent| match spent {
                    Some(spent) => add_usage(spent, &usage),
                    None => *spent = Some(usage.clone()),
                });
                span.record("cached_input_tokens", usage.cache_read_input_tokens);
                span.record(
                    "cache_write_input_tokens",
                    usage.cache_creation_input_tokens,
                );
                span.record("input_tokens", usage.input_tokens);
                if usage.cache_read_input_tokens == 0 || usage.cache_creation_input_tokens > 0 {
                    nanocode_trace::warn!(
                        session_id = %self.session_id,
                        read = usage.cache_read_input_tokens,
                        written = usage.cache_creation_input_tokens,
                        "a prompt cache refresh arrived after the cache lapsed; refreshing stops until the next request"
                    );
                    self.refreshes.store(u64::MAX, Ordering::SeqCst);
                    return;
                }
                self.requested.update(|requested| {
                    if requested.is_none_or(|at| at < started) {
                        *requested = Some(started);
                    }
                });
                self.requested_at
                    .store(nanocode_platform::unix_millis() / 1000, Ordering::SeqCst);
                self.refreshes.fetch_add(1, Ordering::SeqCst);
            }
            Err(error) => {
                nanocode_trace::warn!(
                    session_id = %self.session_id,
                    %error,
                    "a prompt cache refresh failed; refreshing stops until the next request"
                );
                self.refreshes.store(u64::MAX, Ordering::SeqCst);
            }
        }
    }

    async fn refresh(&self, kept: &KeptRequest) -> Result<crate::Usage> {
        let client = self.client.restore_wire_profile(kept.wire_profile.as_ref());
        let mut request = MessagesRequest::clone(&kept.request);
        request.max_tokens = 0;
        let inlined = images::inline_images(&self.images, &kept.sent).await?;
        let wire = client
            .prepare_cached_wire(
                &request,
                inlined.as_deref().unwrap_or(&kept.sent),
                false,
                Some(&self.wire_cache),
            )
            .map_err(provider_error)?;
        let origin = nanocode_http::layer::origin::Origin {
            context: kept.origin.clone(),
            session: Some(Arc::from(self.session_id.as_str())),
        };
        origin
            .scope(client.create_prepared(&wire))
            .await
            .map(|response| response.usage)
            .map_err(provider_error)
    }
}

async fn keep_cache(state: Weak<State>, changed: Arc<Notify>) {
    loop {
        let notified = changed.notified();
        let mut notified = std::pin::pin!(notified);
        notified.as_mut().enable();
        let wait = match state.upgrade() {
            Some(state) if !state.life.stopped() => match state.kept_wait() {
                Some(wait) => wait,
                None => {
                    state.keeping.store(false, Ordering::SeqCst);
                    if state.kept_wait().is_some() && !state.keeping.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    return;
                }
            },
            _ => return,
        };
        nanocode_sync::select! {
            () = &mut notified => continue,
            () = sleep(wait) => {}
        }
        let Some(state) = state.upgrade() else {
            return;
        };
        if state.life.stopped() {
            return;
        }
        state.refresh_kept().await;
    }
}

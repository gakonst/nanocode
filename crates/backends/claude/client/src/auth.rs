use std::{sync::Arc, time::Duration};

use http::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use nanocode_platform::SystemTime;
use nanocode_sync::{SingleFlight, SwapOption};

use crate::{ClaudeAuthFuture, ClaudeAuthProvider, ClaudeAuthUnavailable};

pub struct ClaudeAccessToken {
    authorization: HeaderValue,
    expires_at: SystemTime,
}

impl ClaudeAccessToken {
    pub fn new(
        token: impl AsRef<str>,
        expires_at: SystemTime,
    ) -> Result<Self, ClaudeAuthUnavailable> {
        let token = token.as_ref();
        if token.is_empty()
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte))
        {
            return Err(ClaudeAuthUnavailable::new(
                "the Claude access token is empty or contains characters outside the OAuth token alphabet",
            ));
        }
        let mut authorization =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                ClaudeAuthUnavailable::new(
                    "the Claude access token is not a valid HTTP header value",
                )
            })?;
        authorization.set_sensitive(true);
        Ok(Self {
            authorization,
            expires_at,
        })
    }

    fn usable(&self, refresh_margin: Duration) -> bool {
        self.expires_at
            .duration_since(nanocode_platform::system_time())
            .is_ok_and(|remaining| remaining > refresh_margin)
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.authorization.clone());
        headers
    }
}

pub trait ClaudeTokenSource: Send + Sync {
    fn refresh(&self) -> ClaudeAuthFuture<'_, Result<ClaudeAccessToken, ClaudeAuthUnavailable>>;

    fn refresh_rejected<'a>(
        &'a self,
        _rejected: &'a HeaderValue,
    ) -> ClaudeAuthFuture<'a, Result<Option<ClaudeAccessToken>, ClaudeAuthUnavailable>> {
        Box::pin(async { Ok(None) })
    }
}

pub struct RefreshingClaudeAuth {
    source: Arc<dyn ClaudeTokenSource>,
    refresh_margin: Duration,
    token: Arc<SwapOption<ClaudeAccessToken>>,
    refreshing: SingleFlight<Result<(), String>>,
}

const STALE_REFRESH: &str =
    "the refreshed Claude access token is already expired or within its refresh margin";

impl RefreshingClaudeAuth {
    pub fn new(source: Arc<dyn ClaudeTokenSource>, refresh_margin: Duration) -> Self {
        Self {
            source,
            refresh_margin,
            token: Arc::default(),
            refreshing: SingleFlight::default(),
        }
    }
}

impl ClaudeAuthProvider for RefreshingClaudeAuth {
    fn headers(&self) -> ClaudeAuthFuture<'_, Result<HeaderMap, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            loop {
                if let Some(token) = self
                    .token
                    .load()
                    .as_ref()
                    .filter(|token| token.usable(self.refresh_margin))
                {
                    return Ok(token.headers());
                }
                let (source, cache, margin) = (
                    Arc::clone(&self.source),
                    Arc::clone(&self.token),
                    self.refresh_margin,
                );
                self.refreshing
                    .run(|| async move {
                        let token = source
                            .refresh()
                            .await
                            .map_err(|error| error.reason().to_owned())?;
                        if !token.usable(margin) {
                            return Err(STALE_REFRESH.to_owned());
                        }
                        cache.store(Some(Arc::new(token)));
                        Ok(())
                    })
                    .await
                    .map_err(ClaudeAuthUnavailable::new)?;
            }
        })
    }

    fn recover_unauthorized<'a>(
        &'a self,
        rejected: &'a HeaderMap,
    ) -> ClaudeAuthFuture<'a, Result<bool, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            let rejected = rejected.get(AUTHORIZATION).cloned();
            self.token.rcu(|cached| match cached {
                Some(token) if rejected.as_ref() == Some(&token.authorization) => None,
                other => other.clone(),
            });
            let Some(rejected) = rejected else {
                return Ok(true);
            };
            let (source, cache, margin) = (
                Arc::clone(&self.source),
                Arc::clone(&self.token),
                self.refresh_margin,
            );
            self.refreshing
                .run(|| async move {
                    if let Some(token) = source
                        .refresh_rejected(&rejected)
                        .await
                        .map_err(|error| error.reason().to_owned())?
                    {
                        if !token.usable(margin) {
                            return Err(STALE_REFRESH.to_owned());
                        }
                        cache.store(Some(Arc::new(token)));
                    }
                    Ok(())
                })
                .await
                .map_err(ClaudeAuthUnavailable::new)?;
            Ok(true)
        })
    }
}

use std::{sync::Arc, time::Duration};

use nanocode_http::types::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use nanocode_platform::SystemTime;
use nanocode_sync::{Atom, SingleFlight, SwapOption};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaudeFall {
    pub key: bool,
    pub switched: bool,
}

#[derive(Clone, Copy)]
enum Signing {
    First,
    Key(Option<SystemTime>),
}

type Fell = Arc<dyn Fn(&ClaudeAuthUnavailable, ClaudeFall) + Send + Sync>;

pub struct ClaudeAuthOrKey {
    first: Arc<dyn ClaudeAuthProvider>,
    key: Option<HeaderMap>,
    back_after: Option<Duration>,
    signing: Atom<Signing>,
    fell: Option<Fell>,
}

impl ClaudeAuthOrKey {
    pub fn new(
        first: Arc<dyn ClaudeAuthProvider>,
        key: Option<&str>,
    ) -> Result<Self, ClaudeAuthUnavailable> {
        let key = key
            .map(|key| {
                let mut value = HeaderValue::from_str(key).map_err(|_| {
                    ClaudeAuthUnavailable::new(
                        "the Claude API key is not a valid HTTP header value",
                    )
                })?;
                value.set_sensitive(true);
                let mut headers = HeaderMap::new();
                headers.insert("x-api-key", value);
                Ok(headers)
            })
            .transpose()?;
        Ok(Self {
            first,
            key,
            back_after: None,
            signing: Atom::new(Signing::First),
            fell: None,
        })
    }

    #[must_use]
    pub const fn back_after(mut self, after: Duration) -> Self {
        self.back_after = Some(after);
        self
    }

    #[must_use]
    pub fn on_fall(
        mut self,
        fell: impl Fn(&ClaudeAuthUnavailable, ClaudeFall) + Send + Sync + 'static,
    ) -> Self {
        self.fell = Some(Arc::new(fell));
        self
    }

    fn on_key(signing: Signing) -> bool {
        match signing {
            Signing::First => false,
            Signing::Key(None) => true,
            Signing::Key(Some(until)) => nanocode_platform::system_time() < until,
        }
    }

    fn fall(&self, error: &ClaudeAuthUnavailable) -> Option<HeaderMap> {
        let key = self.key.clone();
        let switched = key.is_some() && {
            let until = self
                .back_after
                .map(|after| nanocode_platform::system_time() + after);
            self.signing.update(|signing| {
                let switched = !Self::on_key(*signing);
                *signing = Signing::Key(until);
                switched
            })
        };
        if let Some(fell) = &self.fell {
            fell(
                error,
                ClaudeFall {
                    key: key.is_some(),
                    switched,
                },
            );
        }
        key
    }
}

impl ClaudeAuthProvider for ClaudeAuthOrKey {
    fn headers(&self) -> ClaudeAuthFuture<'_, Result<HeaderMap, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            if Self::on_key(*self.signing.get())
                && let Some(key) = &self.key
            {
                return Ok(key.clone());
            }
            match self.first.headers().await {
                Ok(headers) => Ok(headers),
                Err(error) => self.fall(&error).ok_or(error),
            }
        })
    }

    fn recover_unauthorized<'a>(
        &'a self,
        rejected: &'a HeaderMap,
    ) -> ClaudeAuthFuture<'a, Result<bool, ClaudeAuthUnavailable>> {
        Box::pin(async move {
            if rejected.contains_key("x-api-key") {
                return Ok(false);
            }
            match self.first.recover_unauthorized(rejected).await {
                Ok(recovered) => Ok(recovered),
                Err(error) => self.fall(&error).map(|_| true).ok_or(error),
            }
        })
    }
}

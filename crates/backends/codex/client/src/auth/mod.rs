use std::{
    fmt,
    future::{Future, ready},
    pin::Pin,
    sync::Arc,
};

#[cfg(not(target_family = "wasm"))]
mod chatgpt;
mod subscription;

#[cfg(not(target_family = "wasm"))]
pub use chatgpt::{
    ChatGptAuthError, ChatGptAuthStatus, ChatGptLogin, chatgpt_access_token, chatgpt_auth_status,
    load_chatgpt_auth, logout_chatgpt, resolve_chatgpt_auth_status,
};
pub use subscription::{
    ChatGptCredential, ChatGptCredentialSeed, ChatGptLoginStatus, ChatGptSubscription,
    ChatGptSubscriptionError, ChatGptSubscriptionHost, SubscriptionCommit, SubscriptionFuture,
    SubscriptionHostError, SubscriptionHttpRequest, SubscriptionHttpResponse,
    SubscriptionStoreValue,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenAiAuthMode {
    ApiKey,

    ChatGpt,
}

impl OpenAiAuthMode {
    #[must_use]
    pub const fn supports_stored_responses(self) -> bool {
        matches!(self, Self::ApiKey)
    }

    #[must_use]
    pub const fn default_api_base_url(self) -> &'static str {
        match self {
            Self::ApiKey => "https://api.openai.com/v1",
            Self::ChatGpt => "https://chatgpt.com/backend-api/codex",
        }
    }

    #[must_use]
    pub const fn default_websocket_url(self) -> &'static str {
        match self {
            Self::ApiKey => "wss://api.openai.com/v1/responses",
            Self::ChatGpt => "wss://chatgpt.com/backend-api/codex/responses",
        }
    }
}

#[derive(Clone)]
pub struct OpenAiAuthSnapshot {
    mode: OpenAiAuthMode,
    bearer: Arc<str>,
    account_id: Option<Arc<str>>,
    fedramp: bool,
    revision: u64,
}

impl OpenAiAuthSnapshot {
    #[doc(hidden)]
    #[must_use]
    pub fn new(
        mode: OpenAiAuthMode,
        bearer: impl Into<Arc<str>>,
        account_id: Option<impl Into<Arc<str>>>,
        fedramp: bool,
        revision: u64,
    ) -> Self {
        Self {
            mode,
            bearer: bearer.into(),
            account_id: account_id.map(Into::into),
            fedramp,
            revision,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> OpenAiAuthMode {
        self.mode
    }

    #[doc(hidden)]
    #[must_use]
    pub fn bearer(&self) -> &str {
        &self.bearer
    }

    pub(crate) const fn shared_bearer(&self) -> &Arc<str> {
        &self.bearer
    }

    #[must_use]
    pub fn sends_bearer(&self) -> bool {
        !self.bearer.is_empty()
    }

    #[must_use]
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    #[must_use]
    pub const fn is_fedramp(&self) -> bool {
        self.fedramp
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

impl fmt::Debug for OpenAiAuthSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiAuthSnapshot")
            .field("mode", &self.mode)
            .field("bearer", &"[redacted]")
            .field("account_id", &self.account_id)
            .field("fedramp", &self.fedramp)
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone, Debug)]
pub enum OpenAiAuthError {
    Empty,

    Unavailable(Arc<str>),

    AccountChanged,

    LoginRequired(Arc<str>),

    Refresh(Arc<str>),
}

impl std::fmt::Display for OpenAiAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("OpenAI credentials are empty"),
            Self::Unavailable(value) => {
                write!(formatter, "OpenAI credentials are unavailable: {value}")
            }
            Self::AccountChanged => {
                formatter.write_str("the stored ChatGPT account changed while the agent was active")
            }
            Self::LoginRequired(value) => write!(
                formatter,
                "ChatGPT authorization must be refreshed by logging in again: {value}"
            ),
            Self::Refresh(value) => write!(
                formatter,
                "failed to refresh ChatGPT authorization: {value}"
            ),
        }
    }
}

impl std::error::Error for OpenAiAuthError {}

#[cfg(not(target_family = "wasm"))]
pub type OpenAiAuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_family = "wasm")]
pub type OpenAiAuthFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[doc(hidden)]
pub trait OpenAiAuthSource: Send + Sync {
    fn validate(&self) -> Result<(), OpenAiAuthError>;

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>>;

    fn recover_unauthorized(
        &self,
        rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>>;
}

#[derive(Clone)]
pub struct OpenAiAuth {
    mode: OpenAiAuthMode,
    source: Arc<dyn OpenAiAuthSource>,
    gateway_managed: bool,
}

impl OpenAiAuth {
    #[must_use]
    pub fn api_key(api_key: impl Into<Arc<str>>) -> Self {
        let source = ApiKeyAuth {
            api_key: api_key.into(),
        };
        Self {
            mode: OpenAiAuthMode::ApiKey,
            source: Arc::new(source),
            gateway_managed: false,
        }
    }

    #[must_use]
    pub fn gateway_managed() -> Self {
        Self {
            mode: OpenAiAuthMode::ApiKey,
            source: Arc::new(GatewayManagedAuth(OpenAiAuthMode::ApiKey)),
            gateway_managed: true,
        }
    }

    #[must_use]
    pub fn host_managed_chatgpt() -> Self {
        Self {
            mode: OpenAiAuthMode::ChatGpt,
            source: Arc::new(GatewayManagedAuth(OpenAiAuthMode::ChatGpt)),
            gateway_managed: true,
        }
    }

    #[must_use]
    pub const fn is_gateway_managed(&self) -> bool {
        self.gateway_managed
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn chatgpt() -> Self {
        Self::managed_chatgpt(Arc::new(ChatGptHomeAuth::default()))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn managed_chatgpt(source: Arc<dyn OpenAiAuthSource>) -> Self {
        Self {
            mode: OpenAiAuthMode::ChatGpt,
            source,
            gateway_managed: false,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> OpenAiAuthMode {
        self.mode
    }

    pub fn validate(&self) -> Result<(), OpenAiAuthError> {
        self.source.validate()
    }

    pub async fn snapshot(&self) -> Result<OpenAiAuthSnapshot, OpenAiAuthError> {
        self.source.snapshot().await
    }

    pub async fn recover_unauthorized(
        &self,
        rejected: &OpenAiAuthSnapshot,
    ) -> Result<(), OpenAiAuthError> {
        self.source.recover_unauthorized(rejected).await
    }
}

impl fmt::Debug for OpenAiAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiAuth")
            .field("mode", &self.mode)
            .field("gateway_managed", &self.gateway_managed)
            .finish_non_exhaustive()
    }
}

impl From<String> for OpenAiAuth {
    fn from(api_key: String) -> Self {
        Self::api_key(api_key)
    }
}

impl From<&str> for OpenAiAuth {
    fn from(api_key: &str) -> Self {
        Self::api_key(api_key.to_owned())
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn chatgpt_auth_file() -> Result<std::path::PathBuf, nanocode_home::HomeError> {
    Ok(nanocode_home::AgentHome::from_env()?
        .nanocode_home()
        .join("auth.json"))
}

#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
struct ChatGptHomeAuth {
    loaded: nanocode_sync::OnceCell<OpenAiAuth>,
}

#[cfg(not(target_family = "wasm"))]
impl ChatGptHomeAuth {
    async fn load(&self) -> Result<&OpenAiAuth, OpenAiAuthError> {
        self.loaded
            .get_or_try_init(|| async {
                nanocode_platform::spawn_blocking(|| {
                    let path = chatgpt_auth_file().map_err(|error| {
                        OpenAiAuthError::Unavailable(Arc::from(error.to_string()))
                    })?;
                    load_chatgpt_auth(path).map_err(|error| {
                        OpenAiAuthError::LoginRequired(Arc::from(format!(
                            "{error}; sign in with /login codex"
                        )))
                    })
                })
                .await
                .map_err(|error| OpenAiAuthError::Unavailable(Arc::from(error.to_string())))?
            })
            .await
    }
}

#[cfg(not(target_family = "wasm"))]
impl OpenAiAuthSource for ChatGptHomeAuth {
    fn validate(&self) -> Result<(), OpenAiAuthError> {
        Ok(())
    }

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>> {
        Box::pin(async move { self.load().await?.snapshot().await })
    }

    fn recover_unauthorized(
        &self,
        rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>> {
        let rejected = rejected.clone();
        Box::pin(async move { self.load().await?.recover_unauthorized(&rejected).await })
    }
}

struct GatewayManagedAuth(OpenAiAuthMode);

impl OpenAiAuthSource for GatewayManagedAuth {
    fn validate(&self) -> Result<(), OpenAiAuthError> {
        Ok(())
    }

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>> {
        Box::pin(ready(Ok(OpenAiAuthSnapshot::new(
            self.0,
            "",
            None::<Arc<str>>,
            false,
            0,
        ))))
    }

    fn recover_unauthorized(
        &self,
        _rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>> {
        Box::pin(ready(Err(OpenAiAuthError::LoginRequired(Arc::from(
            match self.0 {
                OpenAiAuthMode::ApiKey => "the gateway rejected its configured credentials",
                OpenAiAuthMode::ChatGpt => "the host transport rejected its ChatGPT authorization",
            },
        )))))
    }
}

#[derive(Debug)]
struct ApiKeyAuth {
    api_key: Arc<str>,
}

impl OpenAiAuthSource for ApiKeyAuth {
    fn validate(&self) -> Result<(), OpenAiAuthError> {
        if self.api_key.trim().is_empty() {
            Err(OpenAiAuthError::Empty)
        } else {
            Ok(())
        }
    }

    fn snapshot(&self) -> OpenAiAuthFuture<'_, Result<OpenAiAuthSnapshot, OpenAiAuthError>> {
        let result = self.validate().map(|()| {
            OpenAiAuthSnapshot::new(
                OpenAiAuthMode::ApiKey,
                Arc::clone(&self.api_key),
                None::<Arc<str>>,
                false,
                0,
            )
        });
        Box::pin(ready(result))
    }

    fn recover_unauthorized(
        &self,
        _rejected: &OpenAiAuthSnapshot,
    ) -> OpenAiAuthFuture<'_, Result<(), OpenAiAuthError>> {
        Box::pin(ready(Err(OpenAiAuthError::LoginRequired(Arc::from(
            "the API key was rejected",
        )))))
    }
}

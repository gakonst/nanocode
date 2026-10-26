use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use nanocode_mcp_http::{SecretSource, streamable_http::BearerSource};
use serde_json::Value;

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_mins(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum McpToolExposure {
    #[default]
    DeferredAndCodeMode,

    DeferredOnly,

    CodeModeOnly,

    Hidden,
}

impl McpToolExposure {
    pub(crate) const fn is_deferred(self) -> bool {
        matches!(self, Self::DeferredAndCodeMode | Self::DeferredOnly)
    }

    pub(crate) const fn is_available_in_code_mode(self) -> bool {
        matches!(self, Self::DeferredAndCodeMode | Self::CodeModeOnly)
    }

    pub(crate) const fn is_callable(self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

#[derive(Clone)]
pub struct McpServer {
    pub(crate) transport: McpTransport,
    pub(crate) description: Option<String>,
    pub(crate) startup_timeout: Duration,
    pub(crate) tool_timeout: Duration,
    pub(crate) supports_parallel_tool_calls: bool,
    pub(crate) parallel_tools: BTreeSet<String>,
    pub(crate) tool_exposure: McpToolExposure,
    pub(crate) enabled_tools: Option<Vec<String>>,
    pub(crate) disabled_tools: Vec<String>,
    pub(crate) payment: Option<Arc<dyn McpPaymentProvider>>,
    pub(crate) unsupported_option: Option<&'static str>,
    pub(crate) http: nanocode_mcp_http::streamable_http::HttpOptions,
}

#[async_trait]
pub trait McpPaymentProvider: Send + Sync {
    async fn prepare(
        &self,
        payment_required: &Value,
    ) -> Result<Option<Box<dyn McpPendingPayment>>, String>;

    async fn prepare_with_management(
        &self,
        payment_required: &Value,
        management: &dyn McpPaymentManagement,
    ) -> Result<Option<Box<dyn McpPendingPayment>>, String> {
        let _ = management;
        self.prepare(payment_required).await
    }
}

#[async_trait]
pub trait McpPaymentManagement: Send + Sync {
    async fn send(&self, credential: Value) -> Result<Option<Value>, String>;
}

#[async_trait]
pub trait McpPendingPayment: Send {
    fn credential(&self) -> &Value;
    fn receipt(&mut self, receipt: &Value) {
        let _ = receipt;
    }

    async fn commit(self: Box<Self>) -> Result<(), String>;

    async fn rollback(self: Box<Self>) -> Result<(), String>;
}

#[derive(Clone)]
pub(crate) enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    },
    StreamableHttp {
        url: String,
        bearer: Option<SecretSource>,
        source: Option<Arc<dyn BearerSource>>,
        headers: BTreeMap<String, SecretSource>,
    },
}

impl McpServer {
    #[must_use]
    pub fn stdio(command: impl Into<String>) -> Self {
        Self {
            transport: McpTransport::Stdio {
                command: command.into(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
            description: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            supports_parallel_tool_calls: false,
            parallel_tools: BTreeSet::new(),
            tool_exposure: McpToolExposure::default(),
            enabled_tools: None,
            disabled_tools: Vec::new(),
            payment: None,
            unsupported_option: None,
            http: nanocode_mcp_http::streamable_http::HttpOptions::default(),
        }
    }

    #[must_use]
    pub fn http(url: impl Into<String>) -> Self {
        Self {
            transport: McpTransport::StreamableHttp {
                url: url.into(),
                bearer: None,
                source: None,
                headers: BTreeMap::new(),
            },
            description: None,
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            tool_timeout: DEFAULT_TOOL_TIMEOUT,
            supports_parallel_tool_calls: false,
            parallel_tools: BTreeSet::new(),
            tool_exposure: McpToolExposure::default(),
            enabled_tools: None,
            disabled_tools: Vec::new(),
            payment: None,
            unsupported_option: None,
            http: nanocode_mcp_http::streamable_http::HttpOptions::default(),
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    #[must_use]
    pub const fn max_response_bytes(mut self, bytes: usize) -> Self {
        self.http.max_response_bytes = Some(bytes);
        self
    }

    #[must_use]
    pub const fn standalone_stream(mut self, open: bool) -> Self {
        self.http.standalone_stream = open;
        self
    }

    #[must_use]
    pub const fn reuse_connections(mut self, reuse: bool) -> Self {
        self.http.reuse_connections = reuse;
        self
    }

    #[must_use]
    pub const fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn supports_parallel_tool_calls(mut self, supports: bool) -> Self {
        self.supports_parallel_tool_calls = supports;
        self
    }

    #[must_use]
    pub fn parallel_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.parallel_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub const fn tool_exposure(mut self, exposure: McpToolExposure) -> Self {
        self.tool_exposure = exposure;
        self
    }

    #[must_use]
    pub fn enabled_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.enabled_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    #[must_use]
    pub fn disabled_tools(mut self, tools: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.disabled_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn payment_provider(mut self, provider: Arc<dyn McpPaymentProvider>) -> Self {
        self.payment = Some(provider);
        self
    }

    #[must_use]
    pub fn arg(mut self, argument: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::Stdio { args, .. } => args.push(argument.into()),
            McpTransport::StreamableHttp { .. } => self.unsupported_option = Some("arg"),
        }
        self
    }

    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl Into<String>>) -> Self {
        match &mut self.transport {
            McpTransport::Stdio { args, .. } => {
                args.extend(arguments.into_iter().map(Into::into));
            }
            McpTransport::StreamableHttp { .. } => self.unsupported_option = Some("args"),
        }
        self
    }

    #[must_use]
    pub fn env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::Stdio { env, .. } => {
                env.insert(name.into(), value.into());
            }
            McpTransport::StreamableHttp { .. } => self.unsupported_option = Some("env"),
        }
        self
    }

    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        match &mut self.transport {
            McpTransport::Stdio { cwd: current, .. } => *current = Some(cwd.into()),
            McpTransport::StreamableHttp { .. } => self.unsupported_option = Some("cwd"),
        }
        self
    }

    #[must_use]
    pub fn bearer_token(mut self, token: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::StreamableHttp { bearer, .. } => {
                *bearer = Some(SecretSource::Value(token.into()));
            }
            McpTransport::Stdio { .. } => self.unsupported_option = Some("bearer_token"),
        }
        self
    }

    #[must_use]
    pub fn bearer_token_env(mut self, variable: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::StreamableHttp { bearer, .. } => {
                *bearer = Some(SecretSource::Environment(variable.into()));
            }
            McpTransport::Stdio { .. } => self.unsupported_option = Some("bearer_token_env"),
        }
        self
    }

    #[must_use]
    pub fn bearer_source(mut self, bearer_source: Arc<dyn BearerSource>) -> Self {
        match &mut self.transport {
            McpTransport::StreamableHttp { source, .. } => *source = Some(bearer_source),
            McpTransport::Stdio { .. } => self.unsupported_option = Some("bearer_source"),
        }
        self
    }

    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::StreamableHttp { headers, .. } => {
                headers.insert(name.into(), SecretSource::Value(value.into()));
            }
            McpTransport::Stdio { .. } => self.unsupported_option = Some("header"),
        }
        self
    }

    #[must_use]
    pub fn header_env(mut self, name: impl Into<String>, variable: impl Into<String>) -> Self {
        match &mut self.transport {
            McpTransport::StreamableHttp { headers, .. } => {
                headers.insert(name.into(), SecretSource::Environment(variable.into()));
            }
            McpTransport::Stdio { .. } => self.unsupported_option = Some("header_env"),
        }
        self
    }

    pub(crate) fn includes_tool(&self, name: &str) -> bool {
        self.enabled_tools
            .as_ref()
            .is_none_or(|enabled| enabled.iter().any(|candidate| candidate == name))
            && !self
                .disabled_tools
                .iter()
                .any(|candidate| candidate == name)
    }
}

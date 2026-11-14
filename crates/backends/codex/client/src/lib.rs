#![deny(rustdoc::broken_intra_doc_links)]

#[cfg(all(target_family = "wasm", not(target_os = "unknown")))]
compile_error!(
    "nanocode-backend-codex-client supports native targets and hosted wasm*-unknown-unknown targets; WASI is not yet supported"
);

use crate as client;

pub mod auth;

pub mod events;

mod openai;

pub mod pricing;

pub mod responses;

mod route;

#[cfg(not(target_family = "wasm"))]
pub mod tls;

pub mod session;

pub mod tools;

pub mod tower;

pub mod transport;

#[doc(hidden)]
pub use auth::{OpenAiAuth, OpenAiAuthError, OpenAiAuthMode, OpenAiAuthSnapshot};

#[doc(hidden)]
pub use events::stream::EventSink;

#[doc(hidden)]
pub use events::{AgentEventKind, EventError, monotonic_now_ns};

#[doc(hidden)]
pub use openai::ModelConfig;

pub use openai::OpenAiError;

pub use openai::CyberAccessProgram;

#[doc(hidden)]
pub use pricing::{CostStatus, EstimatedUsdCost};
pub use responses::ResponseEvent;
pub use responses::ResponseItem;
pub use route::ModelRoute;

pub use responses::{
    ContentItem, FunctionOutputBody, FunctionOutputContent, MessagePhase, MessageRole,
    ResponseItemId, ToolDefinition, Usage,
};

pub use session::{
    CompletedResponse, Response, ResponseError, ResponseErrorKind, ResponseTurn, Session,
    SessionBuildError, SessionBuilder,
};

#[doc(hidden)]
pub use tower::attempt::{
    ResponsesAttempt, ResponsesAttemptFactory, ResponsesOutput, ResponsesServiceResponse,
    TransportStats,
};

#[doc(hidden)]
pub use tower::service::ResponsesService;

#[doc(hidden)]
pub use tower::{
    DefaultResponsesService, ResponsesClient, ResponsesRetryPolicy, ResponsesServiceError,
};

#[doc(hidden)]
pub use transport::EncodedRequest;

#[doc(hidden)]
pub use transport::{
    ResponsesError, ResponsesHistory, ResponsesTransport, RetryAdvice, RetryAfter,
};

pub(crate) use tower::{attempt, call, middleware, service, service_error, stream};
#[cfg(not(target_family = "wasm"))]
pub(crate) use transport::connector;

pub use client::session::compaction::auto_compact_token_limit;

pub(crate) use transport::http;

pub(crate) use transport::{socket, telemetry};

#[doc(hidden)]
pub mod __private {
    pub use crate::client::{
        events::stream::EventSink,
        openai::{
            CallerServiceFactory, LayeredServiceFactory, ModelConfig, ResponsesServiceFactory,
            StandardServiceFactory, apply_mode_defaults, validate,
        },
        session::{
            context::{
                ContextManager, assign_missing_response_item_id, responses_lite_request_prefix,
                shared_request_prefix,
            },
            state::{
                ManagedSessionState, ManagedSessionStateError, RejectedRequestRepair,
                RequestHistory,
            },
        },
        tower::attempt::{ResponsesAttemptFactory, TransportFallback},
    };

    pub mod compaction {
        pub use crate::client::session::compaction::{
            auto_compact_token_limit, context_window_compact_token_limit, install_history,
            local_trigger, remote_compaction, trigger, trim_tool_outputs_to_fit_context_window,
        };
    }

    pub fn with_code_mode_tool_names(
        profile: crate::client::responses::RequestProfile,
        names: Vec<(String, String)>,
    ) -> crate::client::responses::RequestProfile {
        profile.with_code_mode_tool_names(names)
    }
}

pub use nanocode_types::*;

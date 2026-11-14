pub(crate) mod attempt;
pub(crate) mod call;
pub(crate) mod client;
pub(crate) mod middleware;
pub(crate) mod service;
pub(crate) mod service_error;
pub(crate) mod stream;
mod transport_policy;

#[doc(inline)]
pub use crate::client::openai::{
    CallerServiceFactory, LayeredServiceFactory, ResponsesServiceConfig, ResponsesServiceFactory,
    StandardServiceFactory,
};
#[doc(inline)]
pub use attempt::{
    ResponsesAttempt, ResponsesAttemptKind, ResponsesOutput, ResponsesServiceResponse,
};
#[doc(inline)]
pub use call::{BoxError, Layer, Policy, Retry, Retrying, Service, ready};
#[doc(inline)]
pub use client::ResponsesClient;
#[doc(inline)]
pub use middleware::{DefaultResponsesService, ResponsesRetryPolicy, STREAM_MAX_RETRIES};
#[doc(inline)]
pub use service_error::ResponsesServiceError;
#[doc(inline)]
pub use stream::{
    CodeCall, CodeCallKind, CompactionOutput, GenerationOutput, ResponsePipelineStats,
    ResponseTiming, qualified_tool_name,
};

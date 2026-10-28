#[doc(hidden)]
pub(crate) mod compaction;
#[doc(hidden)]
pub(crate) mod context;

mod builder;
mod response;
#[doc(hidden)]
pub(crate) mod state;

pub use builder::{ResponseTurn, Session, SessionBuildError, SessionBuilder};
pub use compaction::truncate_middle_with_token_budget;
pub use context::TOOL_OUTPUT_TOKEN_LIMIT;
pub use response::{
    CompletedCompaction, CompletedResponse, Response, ResponseError, ResponseErrorKind,
    ResponseInput,
};
pub use state::{SessionId, SessionIdError};

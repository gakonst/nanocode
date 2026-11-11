use super::ExecutionFuture;

pub trait BeforeToolCall: Send + Sync {
    fn before(
        &self,
        call: ToolCallRequest,
    ) -> ExecutionFuture<'_, Result<ToolCallDecision, String>>;
}

#[derive(Clone, Debug)]
pub struct ToolCallRequest {
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub name: String,
    pub input: String,
}

#[derive(Clone, Debug)]
pub enum ToolCallDecision {
    Allow,
    Deny(String),
}

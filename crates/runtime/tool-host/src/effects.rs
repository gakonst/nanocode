use std::sync::Arc;

use nanocode_tools::{ToolContext, ToolInput, ToolOutput};
pub use nanocode_turnlog::EffectClass;

use crate::embedded::HostFuture;

pub struct EffectCall<'a> {
    pub call_id: &'a str,
    pub effect: EffectClass,
    pub input: &'a ToolInput,
    pub session_id: &'a str,
    pub tool: &'a str,
    pub turn_id: Option<&'a str>,
}

pub trait EffectGate: Send + Sync + 'static {
    fn effect(&self, tool: &str) -> EffectClass;

    fn before<'a>(&'a self, call: EffectCall<'a>) -> HostFuture<'a, Result<(), String>>;

    fn after<'a>(&'a self, call_id: &'a str, output: &'a ToolOutput) -> HostFuture<'a, ()>;
}

pub(crate) async fn gated<'c, E, F>(
    gate: Option<&Arc<dyn EffectGate>>,
    tool: &str,
    input: ToolInput,
    context: ToolContext<'c>,
    run: impl FnOnce(ToolInput, ToolContext<'c>) -> F,
) -> Result<ToolOutput, E>
where
    F: Future<Output = Result<ToolOutput, E>>,
{
    let Some(gate) = gate.filter(|gate| gate.effect(tool).needs_row()) else {
        return run(input, context).await;
    };
    let recorded = gate
        .before(EffectCall {
            call_id: context.call_id(),
            effect: EffectClass::External,
            input: &input,
            session_id: context.session_id(),
            tool,
            turn_id: context.turn_id(),
        })
        .await;
    if let Err(error) = recorded {
        return Ok(ToolOutput::error(format!(
            "tool `{tool}` was not run because its call could not be recorded durably: {error}"
        )));
    }
    let output = run(input, context).await?;
    gate.after(context.call_id(), &output).await;
    Ok(output)
}

use std::sync::Arc;

use serde::Serialize;

use crate::{Tool, ToolDefinition};

/// Lets a host keep an earlier wording of what the runtime itself writes
/// for the model: the descriptions of the tools it adds (`spawn_agent` and
/// the other subagent tools, `submit_result`) and Code Mode's `exec` and
/// `wait`. tempoxyz/pyramid keeps a warm session's wording this way across
/// a release that only rewords them, so the session's cached prompt still
/// matches. Only prose is pinned: each text comes with its contract, a hash
/// of everything it was rendered from but the runtime's own wording (a
/// tool's schema; for `exec`, every nested tool's definition and the
/// flags), so a host keeps an earlier text only for the same contract.
pub trait TextPins: Send + Sync + 'static {
    /// The text to send for `key` (`tool:<name>`, `exec` or `wait`)
    /// instead of `text`, the running code's, whose contract is `contract`;
    /// `None` sends `text`.
    fn pin(&self, key: &str, contract: &str, text: &str) -> Option<Arc<str>>;
}

fn digest(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    blake3::hash(&bytes).to_hex().to_string()
}

/// `definition`'s contract: all of it but its description.
pub(crate) fn contract(definition: &ToolDefinition) -> String {
    digest(&(
        definition.clone().with_description(""),
        definition.output_schema(),
    ))
}

/// `tool`, with the description `pins` keeps for it.
pub(crate) fn pin_tool(pins: &dyn TextPins, tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
    let definition = tool.definition();
    match pins.pin(
        &format!("tool:{}", definition.name()),
        &contract(&definition),
        definition.description(),
    ) {
        Some(description) if *description != *definition.description() => {
            Arc::new(crate::selection::Described {
                inner: tool,
                description,
            })
        }
        _ => tool,
    }
}

/// Code Mode's `exec` and `wait` among `specs`, with the descriptions
/// `pins` keeps for them. `inputs` is everything `exec` is rendered from.
pub(crate) fn pin_code_mode(
    pins: &dyn TextPins,
    specs: &mut [ToolDefinition],
    inputs: &impl Serialize,
) {
    for spec in specs {
        let contract = match spec.name() {
            "exec" => digest(&(contract(spec), inputs)),
            "wait" => contract(spec),
            _ => continue,
        };
        if let Some(description) = pins.pin(spec.name(), &contract, spec.description())
            && *description != *spec.description()
        {
            *spec = spec.clone().with_description(description);
        }
    }
}

use std::sync::Arc;

use serde::Serialize;

use crate::{Tool, ToolDefinition};

pub trait TextPins: Send + Sync + 'static {
    fn pin(&self, key: &str, contract: &str, text: &str) -> Option<Arc<str>>;
}

fn digest(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    nanocode_digest::blake3(&bytes).to_hex().to_string()
}

pub(crate) fn contract(definition: &ToolDefinition) -> String {
    digest(&(
        definition.clone().with_description(""),
        definition.output_schema(),
    ))
}

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

#[cfg(not(target_family = "wasm"))]
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

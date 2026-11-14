use crate::{
    driver::{McpClient, McpError},
    tool::{Limits, STOPPED, stepped},
};
use nanocode_platform::timeout;
use nanocode_sync::Owned;
use nanocode_tool_contracts::ToolOutput;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::process::Command;

type Spawn = Arc<dyn Fn() -> Command + Send + Sync>;

pub struct Mcp {
    spawn: Spawn,
    renames: Vec<(String, String)>,
    client: (String, String),
    process: Owned<Option<McpClient>>,
}

impl Mcp {
    pub fn new(spawn: impl Fn() -> Command + Send + Sync + 'static) -> Self {
        Self {
            spawn: Arc::new(spawn),
            renames: Vec::new(),
            client: ("nanocode-computer".into(), env!("CARGO_PKG_VERSION").into()),
            process: Owned::default(),
        }
    }

    #[must_use]
    pub fn rename(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.renames.push((from.into(), to.into()));
        self
    }

    #[must_use]
    pub fn client(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client = (name.into(), version.into());
        self
    }

    pub(crate) async fn step(
        &self,
        action: &str,
        input: &Value,
        scope: &str,
        limits: &Limits,
    ) -> ToolOutput {
        let mut arguments = Map::new();
        for (key, value) in input.as_object().into_iter().flatten() {
            if matches!(key.as_str(), "action" | "screenshot" | "_provider") {
                continue;
            }
            let key = self
                .renames
                .iter()
                .find(|(from, _)| from == key)
                .map_or(key.as_str(), |(_, to)| to.as_str());
            arguments.insert(key.into(), value.clone());
        }
        arguments.insert("_scope".into(), scope.into());
        let mut process = self.process.acquire().await;
        if process.is_none() {
            let start = McpClient::start((self.spawn)(), &self.client.0, &self.client.1);
            *process = match timeout(limits.start, start).await {
                Ok(Ok(started)) => Some(started),
                Ok(Err(error)) => {
                    return ToolOutput::error(format!(
                        "Computer use could not start ({error}), so nothing ran."
                    ));
                }
                Err(_) => {
                    return ToolOutput::error(format!(
                        "Computer use did not start within {} seconds, so nothing ran. Try again; if it keeps failing, ask the user to restart the app that provides it.",
                        limits.start.as_secs()
                    ));
                }
            };
        }
        let Some(provider) = process.as_mut() else {
            return ToolOutput::error(STOPPED);
        };
        let call = provider.call_tool(action, Value::Object(arguments));
        let result = match timeout(limits.step, call).await {
            Ok(Ok(result)) => result,
            Ok(Err(McpError::Rpc(error))) => return ToolOutput::error(limits.clip(error.message)),
            failed => {
                process.take();
                return ToolOutput::error(if failed.is_ok() {
                    STOPPED.to_owned()
                } else {
                    limits.slow()
                });
            }
        };
        let content = result["content"].as_array().cloned().unwrap_or_default();
        let texts: Vec<&str> = content
            .iter()
            .filter(|item| item["type"] == "text")
            .filter_map(|item| item["text"].as_str())
            .collect();
        let text = limits.clip(texts.join("\n"));
        if result["isError"] == true {
            return ToolOutput::error(text);
        }
        let wanted = input["screenshot"]
            .as_bool()
            .unwrap_or(action == "get_app_state");
        let image = content
            .iter()
            .find(|item| wanted && item["type"] == "image");
        let fits = image.filter(|image| {
            image["data"]
                .as_str()
                .is_some_and(|data| data.len() <= limits.screenshot)
        });
        let screenshot =
            fits.map(|image| json!({"media_type": image["mimeType"], "data": image["data"]}));
        stepped(input, text, screenshot, image.is_some() && fits.is_none())
    }
}

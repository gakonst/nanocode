use super::{AgentEventData, AssistantEvent};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct AssistantText {
    streaming: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct AssistantTextUpdate<'a> {
    pub key: String,
    pub text: &'a str,
    pub delta: Option<&'a str>,
    pub complete: bool,
}

impl AssistantText {
    pub fn key(item_id: Option<&str>, model_call_index: u32) -> String {
        item_id.map_or_else(|| format!("model-{model_call_index}"), str::to_owned)
    }

    pub fn observe<'a>(&'a mut self, data: &'a AgentEventData) -> Option<AssistantTextUpdate<'a>> {
        let AgentEventData::Assistant(event) = data else {
            return None;
        };
        match event {
            AssistantEvent::Delta(delta) => {
                let key = Self::key(delta.item_id.as_deref(), delta.model_call_index);
                let text = self.streaming.entry(key.clone()).or_default();
                text.push_str(&delta.text);
                Some(AssistantTextUpdate {
                    key,
                    text,
                    delta: Some(&delta.text),
                    complete: false,
                })
            }
            AssistantEvent::Message(message) => {
                let key = Self::key(message.item_id.as_deref(), message.model_call_index);
                self.streaming.remove(&key);
                Some(AssistantTextUpdate {
                    key,
                    text: &message.text,
                    delta: None,
                    complete: true,
                })
            }
        }
    }
}

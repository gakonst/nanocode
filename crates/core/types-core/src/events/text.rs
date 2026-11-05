use super::{AgentEventData, AgentEventKind, AssistantEvent, ModelEvent, TransportEvent};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug, Default)]
pub struct AssistantText {
    streaming: HashMap<String, String>,
    calls: HashMap<u32, Attempts>,
}

#[derive(Clone, Debug, Default)]
struct Attempts {
    shown: Vec<String>,
    replaced: VecDeque<String>,
    aliases: HashMap<String, String>,
    restarted: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct AssistantTextUpdate<'a> {
    pub key: String,
    pub text: &'a str,
    pub delta: Option<&'a str>,
    pub complete: bool,
}

#[derive(serde::Deserialize)]
struct Retrying {
    model_call_index: Option<u32>,
}

impl AssistantText {
    pub fn key(item_id: Option<&str>, model_call_index: u32) -> String {
        item_id.map_or_else(|| format!("model-{model_call_index}"), str::to_owned)
    }

    fn shown(&mut self, item_id: Option<&str>, model_call_index: u32) -> (String, bool) {
        let key = Self::key(item_id, model_call_index);
        let attempts = self.calls.entry(model_call_index).or_default();
        let key = match attempts.aliases.get(&key) {
            Some(shown) => shown.clone(),
            None if attempts.shown.contains(&key) => key,
            None => match attempts.replaced.pop_front() {
                Some(shown) => {
                    if shown != key {
                        attempts.aliases.insert(key, shown.clone());
                    }
                    shown
                }
                None => key,
            },
        };
        if !attempts.shown.contains(&key) {
            attempts.shown.push(key.clone());
        }
        let restarted = attempts.restarted.remove(&key);
        (key, restarted)
    }

    fn retrying(&mut self, event: &TransportEvent) {
        let Some(index) = event
            .decode_payload::<Retrying>()
            .ok()
            .and_then(|retrying| retrying.model_call_index)
        else {
            return;
        };
        let Some(attempts) = self.calls.get_mut(&index) else {
            return;
        };
        let mut replaced: VecDeque<String> = std::mem::take(&mut attempts.shown).into();
        replaced.extend(std::mem::take(&mut attempts.replaced));
        for key in &replaced {
            self.streaming.remove(key);
            attempts.restarted.insert(key.clone());
        }
        attempts.replaced = replaced;
    }

    pub fn observe<'a>(&'a mut self, data: &'a AgentEventData) -> Option<AssistantTextUpdate<'a>> {
        match data {
            AgentEventData::Assistant(AssistantEvent::Delta(delta)) => {
                let (key, restarted) = self.shown(delta.item_id.as_deref(), delta.model_call_index);
                let text = self.streaming.entry(key.clone()).or_default();
                text.push_str(&delta.text);
                Some(AssistantTextUpdate {
                    key,
                    text,
                    delta: (!restarted).then_some(delta.text.as_str()),
                    complete: false,
                })
            }
            AgentEventData::Assistant(AssistantEvent::Message(message)) => {
                let (key, _) = self.shown(message.item_id.as_deref(), message.model_call_index);
                self.streaming.remove(&key);
                Some(AssistantTextUpdate {
                    key,
                    text: &message.text,
                    delta: None,
                    complete: true,
                })
            }
            AgentEventData::Transport(event)
                if event.kind() == AgentEventKind::ModelAttemptRetrying =>
            {
                self.retrying(event);
                None
            }
            AgentEventData::Model(ModelEvent::CallCompleted(call)) => {
                self.calls.remove(&call.call_index);
                None
            }
            AgentEventData::Model(ModelEvent::CallFailed(call)) => {
                self.calls.remove(&call.call_index);
                None
            }
            _ => None,
        }
    }
}

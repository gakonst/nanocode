use super::model::{
    AgentId, AgentMessage, AgentThread, MessageDisposition, MessageId, MessagePriority,
    MessagePurpose, MessageSender, ThreadId,
};
use std::collections::HashMap;

pub(super) const MAX_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct MessageThreads {
    next_message_id: u64,
    threads: HashMap<ThreadId, AgentThread>,
    thread_by_message: HashMap<MessageId, ThreadId>,
    pending: HashMap<MessageId, AgentId>,
    pending_recipients: HashMap<AgentId, usize>,
    log: Vec<MessageId>,
    log_revision: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MessageLog {
    len: usize,
    revision: u64,
}

impl MessageThreads {
    pub(super) fn snapshot(
        &self,
        cache: &super::durable::JournalCache,
    ) -> (
        u64,
        super::durable::MessagesSnapshot,
        MessageLog,
        Vec<MessageId>,
    ) {
        let logged = MessageLog {
            len: self.log.len(),
            revision: self.log_revision,
        };
        let (kept, from) = match cache.logged() {
            Some(saved) if saved.revision == self.log_revision && saved.len <= self.log.len() => {
                (None, saved.len)
            }
            _ => {
                let shared = self
                    .log
                    .iter()
                    .zip(cache.logged_ids())
                    .take_while(|(id, logged)| id == logged)
                    .count()
                    .min(cache.sealed_messages());
                let kept = shared / super::durable::PAGE_MESSAGES;
                (Some(kept), kept * super::durable::PAGE_MESSAGES)
            }
        };
        let messages = super::durable::MessagesSnapshot {
            kept,
            messages: self.log[from..]
                .iter()
                .filter_map(|id| self.message(*id))
                .collect(),
        };
        let mut pending = self.pending.keys().copied().collect::<Vec<_>>();
        pending.sort_unstable();
        (self.next_message_id, messages, logged, pending)
    }

    pub(super) fn logged(&self, ids: &imbl::Vector<MessageId>) -> Option<MessageLog> {
        (self.log.len() == ids.len() && self.log.iter().eq(ids)).then_some(MessageLog {
            len: self.log.len(),
            revision: self.log_revision,
        })
    }

    pub(super) fn restore(
        next_message_id: u64,
        messages: Vec<AgentMessage>,
        pending: Vec<MessageId>,
    ) -> std::io::Result<Self> {
        let mut restored = Self {
            next_message_id,
            ..Self::default()
        };
        for message in messages {
            validate_body(&message.body)?;
            validate_reply(message.purpose, message.in_reply_to)?;
            if message.id.value() == 0
                || message.id.value() > next_message_id
                || restored.thread_by_message.contains_key(&message.id)
            {
                return Err(std::io::Error::other("invalid persisted message identity"));
            }
            let expected = match message.in_reply_to {
                Some(previous) => restored.reference_thread(
                    previous,
                    message.from,
                    message.to,
                    message.purpose,
                )?,
                None => ThreadId::for_message(message.id),
            };
            if expected != message.thread_id {
                return Err(std::io::Error::other("invalid persisted message thread"));
            }
            restored.commit(message);
        }
        restored.pending.clear();
        restored.pending_recipients.clear();
        for id in pending {
            let to = restored
                .message(id)
                .ok_or_else(|| {
                    std::io::Error::other("pending message is absent from the persisted thread")
                })?
                .to;
            restored.add_pending(id, to);
        }
        Ok(restored)
    }

    pub(super) fn has_pending_for(&self, agent_id: AgentId) -> bool {
        self.pending_recipients.contains_key(&agent_id)
    }

    pub(super) fn pending_recipients(&self) -> impl Iterator<Item = AgentId> + '_ {
        self.pending_recipients.keys().copied()
    }

    fn add_pending(&mut self, id: MessageId, to: AgentId) {
        if self.pending.insert(id, to).is_none() {
            *self.pending_recipients.entry(to).or_default() += 1;
        }
    }

    fn remove_pending(&mut self, id: MessageId) {
        let Some(to) = self.pending.remove(&id) else {
            return;
        };
        if let Some(count) = self.pending_recipients.get_mut(&to) {
            *count -= 1;
            if *count == 0 {
                self.pending_recipients.remove(&to);
            }
        }
    }

    pub(super) fn pending_messages(&self) -> Vec<AgentMessage> {
        let mut pending = self
            .pending
            .keys()
            .filter_map(|id| self.message(*id))
            .collect::<Vec<_>>();
        pending.sort_by_key(|message| message.id);
        pending
    }

    pub(super) fn prepare(
        &mut self,
        from: MessageSender,
        to: AgentId,
        priority: MessagePriority,
        purpose: MessagePurpose,
        in_reply_to: Option<MessageId>,
        body: String,
    ) -> std::io::Result<AgentMessage> {
        validate_body(&body)?;
        validate_reply(purpose, in_reply_to)?;
        let id = MessageId::next(&mut self.next_message_id);
        let thread_id = match in_reply_to {
            Some(previous_id) => self.reference_thread(previous_id, from, to, purpose)?,
            None => ThreadId::for_message(id),
        };
        Ok(AgentMessage {
            id,
            thread_id,
            from,
            to,
            priority,
            purpose,
            in_reply_to,
            body,
        })
    }

    pub(super) fn commit(&mut self, message: AgentMessage) -> AgentThread {
        let thread = self
            .threads
            .entry(message.thread_id)
            .or_insert_with(|| AgentThread {
                id: message.thread_id,
                participants: [
                    message.from,
                    MessageSender::Agent {
                        agent_id: message.to,
                    },
                ],
                messages: Vec::new(),
            });
        self.thread_by_message.insert(message.id, message.thread_id);
        let (id, to) = (message.id, message.to);
        match self.log.last() {
            Some(last) if *last > id => {
                let position = self.log.partition_point(|logged| *logged < id);
                self.log.insert(position, id);
                self.log_revision += 1;
            }
            _ => self.log.push(id),
        }
        thread.messages.push(message);
        let thread = thread.clone();
        self.add_pending(id, to);
        thread
    }

    pub(super) fn mark_admitted(&mut self, id: MessageId, disposition: MessageDisposition) {
        if disposition == MessageDisposition::Queued {
            return;
        }
        self.remove_pending(id);
    }

    pub(super) fn mark_terminal(&mut self, id: MessageId) {
        self.remove_pending(id);
    }

    pub(super) fn thread_for_message(&self, id: MessageId) -> Option<AgentThread> {
        self.thread_by_message
            .get(&id)
            .and_then(|thread_id| self.threads.get(thread_id))
            .cloned()
    }

    pub(super) fn message(&self, id: MessageId) -> Option<AgentMessage> {
        let thread_id = self.thread_by_message.get(&id)?;
        self.threads
            .get(thread_id)?
            .messages
            .iter()
            .find(|message| message.id == id)
            .cloned()
    }

    pub(super) fn rollback(&mut self, id: MessageId) {
        self.remove_pending(id);
        self.remove_message(id);
    }

    fn reference_thread(
        &self,
        previous_id: MessageId,
        from: MessageSender,
        to: AgentId,
        purpose: MessagePurpose,
    ) -> std::io::Result<ThreadId> {
        let thread_id = self.thread_by_message.get(&previous_id).ok_or_else(|| {
            std::io::Error::other(format!("unknown in_reply_to message {previous_id}"))
        })?;
        let thread = self
            .threads
            .get(thread_id)
            .expect("message index should reference an existing thread");
        let previous = thread
            .messages
            .iter()
            .find(|message| message.id == previous_id)
            .expect("message index should reference an existing message");
        let same_direction = from == previous.from && to == previous.to;
        let reverse_direction =
            from == (MessageSender::Agent {
                agent_id: previous.to,
            }) && previous.from == (MessageSender::Agent { agent_id: to });
        if purpose == MessagePurpose::Reply {
            if !reverse_direction {
                return Err(std::io::Error::other(format!(
                    "message {previous_id} can only be answered by its recipient; use a non-reply \
                     purpose to continue the same two-party thread in the original direction"
                )));
            }
        } else if !same_direction && !reverse_direction {
            return Err(std::io::Error::other(format!(
                "in_reply_to message {previous_id} belongs to a different two-party thread"
            )));
        }
        Ok(*thread_id)
    }

    fn remove_message(&mut self, id: MessageId) {
        let Some(thread_id) = self.thread_by_message.remove(&id) else {
            return;
        };
        if let Ok(position) = self.log.binary_search(&id) {
            self.log.remove(position);
            self.log_revision += 1;
        }
        let remove_thread = self.threads.get_mut(&thread_id).is_some_and(|thread| {
            thread.messages.retain(|message| message.id != id);
            thread.messages.is_empty()
        });
        if remove_thread {
            self.threads.remove(&thread_id);
        }
    }
}

fn validate_reply(purpose: MessagePurpose, in_reply_to: Option<MessageId>) -> std::io::Result<()> {
    match (purpose, in_reply_to) {
        (MessagePurpose::Reply, None) => Err(std::io::Error::other(
            "reply messages require an in_reply_to message ID",
        )),
        _ => Ok(()),
    }
}

fn validate_body(body: &str) -> std::io::Result<()> {
    if body.trim().is_empty() {
        return Err(std::io::Error::other("message must not be empty"));
    }
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(std::io::Error::other(format!(
            "message exceeds the {MAX_MESSAGE_BYTES}-byte limit"
        )));
    }
    Ok(())
}

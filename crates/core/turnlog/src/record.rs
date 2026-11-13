use nanocode_durability_core::StoreError;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::EffectClass;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Cancelled,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Cancelling,
    Finished {
        last: u64,
        outcome: Outcome,
        result: Box<RawValue>,
    },
    Interrupted {
        note: String,
    },
    Replaced {
        digest: String,
        edit: String,
        edit_id: String,
        prompt: Box<RawValue>,
    },
    Retrying {
        attempts: u64,
        error: String,
        kind: String,
        retry_at: u64,
    },
    Running,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Record {
    Admitted {
        digest: String,
        first: u64,
        hand: Option<String>,
        metadata: Box<RawValue>,
        prompt: Box<RawValue>,
        turn: String,
    },
    Call {
        call_id: String,
        effect: EffectClass,
        input: Box<RawValue>,
        tool: String,
        turn: Option<String>,
    },
    Cell {
        cell: u64,
        fresh: bool,
        session: String,
        update: Box<RawValue>,
    },
    Checkpoint {
        payload: String,
        revision: u64,
        state: String,
    },
    Event {
        at: u64,
        event: Box<RawValue>,
        seq: u64,
        turn: Option<String>,
    },
    Floor {
        seq: u64,
    },
    Forget {
        cell: u64,
        session: String,
    },
    Host {
        key: String,
        value: Box<RawValue>,
    },
    Model {
        response: Box<RawValue>,
        turn: String,
    },
    Recovery {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at: Option<u64>,
        attempt: u64,
        turn: String,
        unknown: Vec<String>,
    },
    Result {
        call_id: String,
        output: Box<RawValue>,
    },
    Segment {
        first: u64,
        last: u64,
        row: u64,
    },
    Status {
        status: Status,
        turn: String,
    },
    Stored {
        key: String,
        value: String,
    },
}

pub fn encode(records: &[Record]) -> Result<String, StoreError> {
    let mut body = String::new();
    for record in records {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&serde_json::to_string(record).map_err(|error| {
            StoreError::NotCommitted(format!("encode turn log record: {error}"))
        })?);
    }
    Ok(body)
}

pub fn decode(body: &str) -> Result<Vec<Record>, StoreError> {
    serde_json::Deserializer::from_str(body)
        .into_iter::<Record>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| StoreError::Backend(format!("turn log row is invalid: {error}")))
}

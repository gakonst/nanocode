use std::sync::Arc;

#[derive(Debug)]
pub enum EventError {
    Encode(serde_json::Error),

    Write(std::io::Error),

    ClosedBeforeTerminal,

    ProtocolVersionMismatch {
        expected: u32,
        actual: u32,
    },

    RequestIdMismatch {
        expected: Arc<str>,

        actual: Arc<str>,
    },

    SequenceMismatch {
        expected: u64,
        actual: u64,
    },

    SequenceExhausted,

    TurnAlreadyTerminal,
}

impl std::fmt::Display for EventError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode(..) => formatter.write_str("failed to encode agent event"),
            Self::Write(..) => formatter.write_str("failed to write agent event"),
            Self::ClosedBeforeTerminal => formatter
                .write_str("agent event stream closed before the turn emitted a terminal event"),
            Self::ProtocolVersionMismatch { expected, actual } => write!(
                formatter,
                "agent event protocol version mismatch: expected {expected}, received {actual}"
            ),
            Self::RequestIdMismatch { expected, actual } => write!(
                formatter,
                "agent event request identity mismatch: expected {expected}, received {actual}"
            ),
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "agent event sequence did not advance: minimum {expected}, received {actual}"
            ),
            Self::SequenceExhausted => formatter.write_str("agent event sequence is exhausted"),
            Self::TurnAlreadyTerminal => {
                formatter.write_str("agent turn event was published after its terminal event")
            }
        }
    }
}

impl std::error::Error for EventError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(value) => Some(value),
            Self::Write(value) => Some(value),
            _ => None,
        }
    }
}

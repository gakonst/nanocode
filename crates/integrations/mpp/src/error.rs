use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    Config(String),
    Http(String),
    Json(String),
    Io(String),
    Unsupported(String),
    Expired(Option<String>),
    InvalidChallenge(Option<String>),
    ChallengeRejected {
        id: String,
        reason: String,
    },
    BadRequest(String),
    InvalidReceipt(String),
    InvalidAmount(String),
    ChainIdMismatch {
        expected: u64,
        got: u64,
    },
    InsufficientBalance(Option<String>),
    OutcomeUnknown {
        transaction_hash: String,
    },
    SessionOutcomeUnknown {
        context: String,
    },
    AccessKeyNotProvisioned,
    SpendingLimitExceeded {
        token: String,
        limit: String,
        required: String,
    },
    TokenBalance {
        token: String,
        available: String,
        required: String,
    },
    Reverted(String),
    Internal(String),
}

impl Fault {
    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    pub(crate) fn http(message: impl Into<String>) -> Self {
        Self::Http(message.into())
    }

    pub const fn is_insufficient_balance(&self) -> bool {
        matches!(
            self,
            Self::InsufficientBalance(_) | Self::TokenBalance { .. }
        )
    }
}

fn reasoned(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    reason: Option<&String>,
    bare: &str,
) -> fmt::Result {
    match reason {
        Some(reason) => write!(f, "{label}: {reason}."),
        None => f.write_str(bare),
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(f, "Invalid configuration: {message}"),
            Self::Http(message) => write!(f, "HTTP error: {message}"),
            Self::Json(message) => write!(f, "JSON error: {message}"),
            Self::Io(message) => write!(f, "IO error: {message}"),
            Self::Unsupported(message) => write!(f, "Unsupported payment method: {message}"),
            Self::Expired(Some(expires)) => write!(f, "Payment expired at {expires}."),
            Self::Expired(None) => f.write_str("Payment has expired."),
            Self::InvalidChallenge(reason) => match reason {
                Some(reason) => write!(f, "Challenge is invalid: {reason}."),
                None => f.write_str("Challenge is invalid."),
            },
            Self::ChallengeRejected { id, reason } => {
                write!(f, "Challenge \"{id}\" is invalid: {reason}.")
            }
            Self::BadRequest(reason) => write!(f, "Bad request: {reason}."),
            Self::InvalidReceipt(message) => write!(f, "Receipt is invalid: {message}."),
            Self::InvalidAmount(message) => write!(f, "Invalid amount: {message}"),
            Self::ChainIdMismatch { expected, got } => {
                write!(f, "Chain ID mismatch: expected {expected}, got {got}")
            }
            Self::InsufficientBalance(reason) => reasoned(
                f,
                "Insufficient balance",
                reason.as_ref(),
                "Insufficient balance.",
            ),
            Self::OutcomeUnknown { transaction_hash } => write!(
                f,
                "push payment outcome unknown (tx {transaction_hash}); reconcile before retrying"
            ),
            Self::SessionOutcomeUnknown { context } => {
                write!(
                    f,
                    "session payment outcome unknown: {context}; reconcile before retrying"
                )
            }
            Self::AccessKeyNotProvisioned => f.write_str("Access key not provisioned on wallet"),
            Self::SpendingLimitExceeded {
                token,
                limit,
                required,
            } => write!(
                f,
                "Spending limit exceeded for {token}: limit is {limit}, need {required}"
            ),
            Self::TokenBalance {
                token,
                available,
                required,
            } => {
                write!(
                    f,
                    "Insufficient {token} balance: have {available}, need {required}"
                )
            }
            Self::Reverted(message) => write!(f, "Transaction reverted: {message}"),
            Self::Internal(message) => write!(f, "Internal payment error: {message}"),
        }
    }
}

impl std::error::Error for Fault {}

impl From<serde_json::Error> for Fault {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

impl From<std::io::Error> for Fault {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<nanocode_tempo::Error> for Fault {
    fn from(error: nanocode_tempo::Error) -> Self {
        Self::Config(error.to_string())
    }
}

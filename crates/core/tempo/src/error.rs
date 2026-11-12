use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidHex,
    InvalidBase64,
    InvalidLength { expected: usize, actual: usize },
    InvalidNumber,
    NumberOverflow,
    InvalidPrivateKey,
    InvalidPublicKey,
    InvalidSignature,
    InvalidRecoveryId(u8),
    SignatureFailed,
    Rlp(&'static str),
    Abi(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex => f.write_str("invalid hexadecimal"),
            Self::InvalidBase64 => f.write_str("invalid base64url"),
            Self::InvalidLength { expected, actual } => {
                write!(f, "expected {expected} bytes, got {actual}")
            }
            Self::InvalidNumber => f.write_str("invalid number"),
            Self::NumberOverflow => f.write_str("number exceeds 256 bits"),
            Self::InvalidPrivateKey => f.write_str("invalid private key"),
            Self::InvalidPublicKey => f.write_str("invalid public key"),
            Self::InvalidSignature => f.write_str("invalid signature"),
            Self::InvalidRecoveryId(id) => write!(f, "invalid recovery id {id}"),
            Self::SignatureFailed => f.write_str("signing produced a zero r or s"),
            Self::Rlp(reason) => write!(f, "invalid RLP: {reason}"),
            Self::Abi(reason) => write!(f, "invalid ABI data: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

use std::{error::Error, fmt, str::FromStr, sync::Arc};

#[derive(Clone, PartialEq, Eq, Hash)]
enum Repr {
    Static(&'static str),
    Extension(Arc<str>),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Method(Repr);

pub struct InvalidMethod {
    _private: (),
}

const STANDARD: &[&str] = &[
    "OPTIONS", "GET", "POST", "PUT", "DELETE", "HEAD", "TRACE", "CONNECT", "PATCH",
];

const fn is_token(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'!'
            | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'\''
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~'
    )
}

impl Method {
    pub const GET: Self = Self(Repr::Static("GET"));
    pub const POST: Self = Self(Repr::Static("POST"));
    pub const PUT: Self = Self(Repr::Static("PUT"));
    pub const DELETE: Self = Self(Repr::Static("DELETE"));
    pub const HEAD: Self = Self(Repr::Static("HEAD"));
    pub const OPTIONS: Self = Self(Repr::Static("OPTIONS"));
    pub const CONNECT: Self = Self(Repr::Static("CONNECT"));
    pub const PATCH: Self = Self(Repr::Static("PATCH"));
    pub const TRACE: Self = Self(Repr::Static("TRACE"));

    pub fn from_bytes(source: &[u8]) -> Result<Self, InvalidMethod> {
        if let Some(standard) = STANDARD
            .iter()
            .find(|standard| standard.as_bytes() == source)
        {
            return Ok(Self(Repr::Static(standard)));
        }
        if source.is_empty() || !source.iter().copied().all(is_token) {
            return Err(InvalidMethod { _private: () });
        }
        let text = std::str::from_utf8(source).map_err(|_| InvalidMethod { _private: () })?;
        Ok(Self(Repr::Extension(Arc::from(text))))
    }

    pub fn is_safe(&self) -> bool {
        matches!(self.as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE")
            && matches!(self.0, Repr::Static(_))
    }

    pub fn is_idempotent(&self) -> bool {
        self.is_safe() || matches!(self.0, Repr::Static("PUT" | "DELETE"))
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Static(name) => name,
            Repr::Extension(name) => name,
        }
    }
}

impl AsRef<str> for Method {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Ord for Method {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Method {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq<&Self> for Method {
    fn eq(&self, other: &&Self) -> bool {
        self == *other
    }
}

impl PartialEq<Method> for &Method {
    fn eq(&self, other: &Method) -> bool {
        *self == other
    }
}

impl PartialEq<str> for Method {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Method> for str {
    fn eq(&self, other: &Method) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<&str> for Method {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<Method> for &str {
    fn eq(&self, other: &Method) -> bool {
        *self == other.as_str()
    }
}

impl fmt::Debug for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Display for Method {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Default for Method {
    fn default() -> Self {
        Self::GET
    }
}

impl From<&Self> for Method {
    fn from(source: &Self) -> Self {
        source.clone()
    }
}

impl TryFrom<&[u8]> for Method {
    type Error = InvalidMethod;

    fn try_from(source: &[u8]) -> Result<Self, Self::Error> {
        Self::from_bytes(source)
    }
}

impl TryFrom<&str> for Method {
    type Error = InvalidMethod;

    fn try_from(source: &str) -> Result<Self, Self::Error> {
        Self::from_bytes(source.as_bytes())
    }
}

impl FromStr for Method {
    type Err = InvalidMethod;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        Self::from_bytes(source.as_bytes())
    }
}

impl fmt::Debug for InvalidMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InvalidMethod").finish()
    }
}

impl fmt::Display for InvalidMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid HTTP method")
    }
}

impl Error for InvalidMethod {}

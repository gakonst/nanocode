use std::{fmt, io};

#[derive(Debug)]
pub enum PgError {
    Io(io::Error),
    Tls(String),
    Protocol(String),
    Authentication(String),
    Configuration(String),
    Database(DatabaseError),
    Broken,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DatabaseError {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
}

impl PgError {
    #[must_use]
    pub fn unavailable(&self) -> bool {
        match self {
            Self::Io(_) | Self::Tls(_) | Self::Protocol(_) | Self::Broken => true,
            Self::Database(error) => {
                error.code.starts_with(CONNECTION_EXCEPTION)
                    || UNAVAILABLE_CODES.contains(&error.code.as_str())
            }
            Self::Authentication(_) | Self::Configuration(_) => false,
        }
    }

    #[must_use]
    pub const fn database(&self) -> Option<&DatabaseError> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

const CONNECTION_EXCEPTION: &str = "08";
const UNAVAILABLE_CODES: [&str; 4] = ["57P01", "57P02", "57P03", "25006"];

impl fmt::Display for PgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Postgres connection failed: {error}"),
            Self::Tls(error) => write!(formatter, "Postgres TLS failed: {error}"),
            Self::Protocol(error) => write!(formatter, "Postgres protocol error: {error}"),
            Self::Authentication(error) => {
                write!(formatter, "Postgres authentication failed: {error}")
            }
            Self::Configuration(error) => {
                write!(
                    formatter,
                    "Postgres connection options are invalid: {error}"
                )
            }
            Self::Database(error) => write!(formatter, "{error}"),
            Self::Broken => formatter
                .write_str("the Postgres connection was left mid-message by a cancelled operation"),
        }
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "error returned from database: {} ({} {})",
            self.message, self.severity, self.code
        )?;
        if let Some(detail) = &self.detail {
            write!(formatter, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl std::error::Error for DatabaseError {}

impl From<io::Error> for PgError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

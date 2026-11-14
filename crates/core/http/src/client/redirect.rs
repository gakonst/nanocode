use std::{error::Error as StdError, fmt};

use crate::types::StatusCode;
use nanocode_url::Url;

use super::body::BoxError;

const DEFAULT_REDIRECTS_MAX: usize = 10;

pub struct Policy {
    inner: PolicyKind,
}

enum PolicyKind {
    Custom(Box<dyn Fn(Attempt<'_>) -> Action + Send + Sync + 'static>),
    Limit(usize),
    None,
}

pub struct Attempt<'a> {
    status: StatusCode,
    next: &'a Url,
    previous: &'a [Url],
}

pub struct Action {
    inner: ActionKind,
}

pub(crate) enum ActionKind {
    Follow,
    Stop,
    Error(BoxError),
}

#[derive(Debug)]
struct TooManyRedirects;

impl fmt::Display for TooManyRedirects {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("too many redirects")
    }
}

impl StdError for TooManyRedirects {}

impl Policy {
    #[must_use]
    pub const fn limited(max: usize) -> Self {
        Self {
            inner: PolicyKind::Limit(max),
        }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self {
            inner: PolicyKind::None,
        }
    }

    pub fn custom<T>(policy: T) -> Self
    where
        T: Fn(Attempt<'_>) -> Action + Send + Sync + 'static,
    {
        Self {
            inner: PolicyKind::Custom(Box::new(policy)),
        }
    }

    pub(crate) const fn follows(&self) -> bool {
        !matches!(self.inner, PolicyKind::None)
    }

    pub(crate) fn check(&self, status: StatusCode, next: &Url, previous: &[Url]) -> ActionKind {
        let attempt = Attempt {
            status,
            next,
            previous,
        };
        match &self.inner {
            PolicyKind::Custom(custom) => custom(attempt).inner,
            PolicyKind::Limit(max) if previous.len() > *max => {
                ActionKind::Error(Box::new(TooManyRedirects))
            }
            PolicyKind::Limit(_) => ActionKind::Follow,
            PolicyKind::None => ActionKind::Stop,
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::limited(DEFAULT_REDIRECTS_MAX)
    }
}

impl fmt::Debug for Policy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            PolicyKind::Custom(_) => formatter.write_str("Policy(Custom)"),
            PolicyKind::Limit(max) => write!(formatter, "Policy(Limit({max}))"),
            PolicyKind::None => formatter.write_str("Policy(None)"),
        }
    }
}

impl Attempt<'_> {
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    #[must_use]
    pub const fn url(&self) -> &Url {
        self.next
    }

    #[must_use]
    pub const fn previous(&self) -> &[Url] {
        self.previous
    }

    #[must_use]
    pub const fn follow(self) -> Action {
        Action {
            inner: ActionKind::Follow,
        }
    }

    #[must_use]
    pub const fn stop(self) -> Action {
        Action {
            inner: ActionKind::Stop,
        }
    }

    pub fn error<E: Into<BoxError>>(self, error: E) -> Action {
        Action {
            inner: ActionKind::Error(error.into()),
        }
    }
}

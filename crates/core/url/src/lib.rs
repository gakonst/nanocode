mod encode;
pub mod form;
mod host;
mod parser;
pub mod punycode;

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::{FromStr, Split};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

pub use crate::encode::percent_decode;
use crate::encode::{EncodeSet, encode_str};
pub use crate::host::Host;
use crate::host::HostKind;

const HTTP_PORT: u16 = 80;
const HTTPS_PORT: u16 = 443;
const FTP_PORT: u16 = 21;
const AUTHORITY_MARKER: &str = "://";

#[must_use]
pub fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(HTTP_PORT),
        "https" | "wss" => Some(HTTPS_PORT),
        "ftp" => Some(FTP_PORT),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    EmptyHost,
    IdnaError,
    InvalidPort,
    InvalidIpv4Address,
    InvalidIpv6Address,
    InvalidDomainCharacter,
    RelativeUrlWithoutBase,
    RelativeUrlWithCannotBeABaseBase,
    SetHostOnCannotBeABaseUrl,
    Overflow,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptyHost => "empty host",
            Self::IdnaError => "invalid international domain name",
            Self::InvalidPort => "invalid port number",
            Self::InvalidIpv4Address => "invalid IPv4 address",
            Self::InvalidIpv6Address => "invalid IPv6 address",
            Self::InvalidDomainCharacter => "invalid domain character",
            Self::RelativeUrlWithoutBase => "relative URL without a base",
            Self::RelativeUrlWithCannotBeABaseBase => "relative URL with a cannot-be-a-base base",
            Self::SetHostOnCannotBeABaseUrl => "a cannot-be-a-base URL doesn't have a host to set",
            Self::Overflow => "URLs more than 4 GB are not supported",
        })
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone)]
pub struct Url {
    serialization: String,
    scheme_end: usize,
    username_end: usize,
    host_start: usize,
    host_end: usize,
    host: HostKind,
    port: Option<u16>,
    path_start: usize,
    query_start: Option<usize>,
    fragment_start: Option<usize>,
}

pub(crate) struct Authority<'a> {
    pub(crate) username: &'a str,
    pub(crate) password: &'a str,
    pub(crate) host: HostKind,
    pub(crate) host_text: &'a str,
    pub(crate) port: Option<u16>,
}

pub(crate) struct Components<'a> {
    pub(crate) scheme: &'a str,
    pub(crate) authority: Option<Authority<'a>>,
    pub(crate) path: &'a str,
    pub(crate) query: Option<&'a str>,
    pub(crate) fragment: Option<&'a str>,
}

impl Components<'_> {
    pub(crate) fn assemble(&self) -> Url {
        let mut serialization = String::new();
        serialization.push_str(self.scheme);
        let scheme_end = serialization.len();
        serialization.push(':');
        let (username_end, host_start, host_end, host, port) = match &self.authority {
            Some(authority) => {
                serialization.push_str("//");
                serialization.push_str(authority.username);
                let username_end = serialization.len();
                if !authority.password.is_empty() {
                    serialization.push(':');
                    serialization.push_str(authority.password);
                }
                if !authority.username.is_empty() || !authority.password.is_empty() {
                    serialization.push('@');
                }
                let host_start = serialization.len();
                serialization.push_str(authority.host_text);
                let host_end = serialization.len();
                if let Some(port) = authority.port {
                    serialization.push(':');
                    serialization.push_str(&port.to_string());
                }
                (
                    username_end,
                    host_start,
                    host_end,
                    authority.host,
                    authority.port,
                )
            }
            None => {
                let end = serialization.len();
                if self.path.starts_with("//") {
                    serialization.push_str("/.");
                }
                (end, end, end, HostKind::None, None)
            }
        };
        let path_start = serialization.len();
        serialization.push_str(self.path);
        let query_start = self.query.map(|query| {
            let start = serialization.len();
            serialization.push('?');
            serialization.push_str(query);
            start
        });
        let fragment_start = self.fragment.map(|fragment| {
            let start = serialization.len();
            serialization.push('#');
            serialization.push_str(fragment);
            start
        });
        Url {
            serialization,
            scheme_end,
            username_end,
            host_start,
            host_end,
            host,
            port,
            path_start,
            query_start,
            fragment_start,
        }
    }
}

impl Url {
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        parser::parse(input, None)
    }

    pub fn join(&self, input: &str) -> Result<Self, ParseError> {
        parser::parse(input, Some(self))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.serialization
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.serialization
    }

    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.serialization[..self.scheme_end]
    }

    #[must_use]
    pub fn has_authority(&self) -> bool {
        self.serialization[self.scheme_end..].starts_with(AUTHORITY_MARKER)
    }

    #[must_use]
    pub fn cannot_be_a_base(&self) -> bool {
        !self.serialization[self.scheme_end + 1..].starts_with('/')
    }

    #[must_use]
    pub fn username(&self) -> &str {
        let start = self.scheme_end + AUTHORITY_MARKER.len();
        if self.has_authority() && self.username_end > start {
            &self.serialization[start..self.username_end]
        } else {
            ""
        }
    }

    #[must_use]
    pub fn password(&self) -> Option<&str> {
        if self.has_authority()
            && self.username_end < self.host_start
            && self.serialization.as_bytes()[self.username_end] == b':'
        {
            Some(&self.serialization[self.username_end + 1..self.host_start - 1])
        } else {
            None
        }
    }

    #[must_use]
    pub const fn has_host(&self) -> bool {
        !matches!(self.host, HostKind::None)
    }

    #[must_use]
    pub fn host_str(&self) -> Option<&str> {
        self.has_host()
            .then(|| &self.serialization[self.host_start..self.host_end])
    }

    #[must_use]
    pub fn host(&self) -> Option<Host<&str>> {
        match self.host {
            HostKind::None => None,
            HostKind::Domain => Some(Host::Domain(
                &self.serialization[self.host_start..self.host_end],
            )),
            HostKind::Ipv4(address) => Some(Host::Ipv4(address)),
            HostKind::Ipv6(address) => Some(Host::Ipv6(address)),
        }
    }

    #[must_use]
    pub fn domain(&self) -> Option<&str> {
        match self.host {
            HostKind::Domain => Some(&self.serialization[self.host_start..self.host_end]),
            _ => None,
        }
    }

    #[must_use]
    pub const fn port(&self) -> Option<u16> {
        self.port
    }

    #[must_use]
    pub fn port_or_known_default(&self) -> Option<u16> {
        self.port.or_else(|| default_port(self.scheme()))
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        if self.has_authority() {
            let end = self.path_start;
            &self.serialization[self.scheme_end + AUTHORITY_MARKER.len()..end]
        } else {
            ""
        }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        let end = self
            .query_start
            .or(self.fragment_start)
            .unwrap_or(self.serialization.len());
        &self.serialization[self.path_start..end]
    }

    #[must_use]
    pub fn path_segments(&self) -> Option<Split<'_, char>> {
        self.path()
            .strip_prefix('/')
            .map(|remainder| remainder.split('/'))
    }

    #[must_use]
    pub fn query(&self) -> Option<&str> {
        let start = self.query_start?;
        let end = self.fragment_start.unwrap_or(self.serialization.len());
        Some(&self.serialization[start + 1..end])
    }

    #[must_use]
    pub fn fragment(&self) -> Option<&str> {
        self.fragment_start
            .map(|start| &self.serialization[start + 1..])
    }

    #[must_use]
    pub fn query_pairs(&self) -> form::Parse<'_> {
        form::parse(self.query().unwrap_or("").as_bytes())
    }

    pub fn query_pairs_mut(&mut self) -> QueryPairs<'_> {
        if self.query_start.is_none() {
            *self = {
                let mut components = self.components();
                components.query = Some("");
                components.assemble()
            };
        }
        QueryPairs { url: self }
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        match self.scheme() {
            "blob" => {
                Self::parse(self.path()).map_or_else(|_| Origin::new_opaque(), |url| url.origin())
            }
            "ftp" | "http" | "https" | "ws" | "wss" => {
                match (self.host(), self.port_or_known_default()) {
                    (Some(host), Some(port)) => {
                        Origin::Tuple(self.scheme().to_owned(), host.to_owned(), port)
                    }
                    _ => Origin::new_opaque(),
                }
            }
            _ => Origin::new_opaque(),
        }
    }

    fn components(&self) -> Components<'_> {
        Components {
            scheme: self.scheme(),
            authority: self.has_authority().then(|| Authority {
                username: self.username(),
                password: self.password().unwrap_or(""),
                host: self.host,
                host_text: &self.serialization[self.host_start..self.host_end],
                port: self.port,
            }),
            path: self.path(),
            query: self.query(),
            fragment: self.fragment(),
        }
    }

    fn strip_trailing_spaces_from_opaque_path(&mut self) {
        if !self.cannot_be_a_base() || self.query_start.is_some() || self.fragment_start.is_some() {
            return;
        }
        let kept = self.path().trim_end_matches(' ').len();
        if kept < self.path().len() {
            let path = self.path()[..kept].to_owned();
            *self = {
                let mut components = self.components();
                components.path = &path;
                components.assemble()
            };
        }
    }

    pub fn set_fragment(&mut self, fragment: Option<&str>) {
        match fragment {
            Some(fragment) => {
                let encoded = parser::encode_component(fragment, EncodeSet::Fragment);
                *self = {
                    let mut components = self.components();
                    components.fragment = Some(&encoded);
                    components.assemble()
                };
            }
            None => {
                *self = {
                    let mut components = self.components();
                    components.fragment = None;
                    components.assemble()
                };
                self.strip_trailing_spaces_from_opaque_path();
            }
        }
    }

    pub fn set_query(&mut self, query: Option<&str>) {
        match query {
            Some(query) => {
                let set = if parser::is_special(self.scheme()) {
                    EncodeSet::SpecialQuery
                } else {
                    EncodeSet::Query
                };
                let encoded = parser::encode_component(query, set);
                *self = {
                    let mut components = self.components();
                    components.query = Some(&encoded);
                    components.assemble()
                };
            }
            None => {
                *self = {
                    let mut components = self.components();
                    components.query = None;
                    components.assemble()
                };
                if self.fragment_start.is_none() {
                    self.strip_trailing_spaces_from_opaque_path();
                }
            }
        }
    }

    pub fn set_path(&mut self, path: &str) {
        let path = parser::path_for_setter(self.scheme(), self.cannot_be_a_base(), path);
        *self = {
            let mut components = self.components();
            components.path = &path;
            components.assemble()
        };
    }

    fn userinfo_settable(&self) -> bool {
        self.has_host() && self.scheme() != "file"
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_username(&mut self, username: &str) -> Result<(), ()> {
        if !self.userinfo_settable() {
            return Err(());
        }
        let mut encoded = String::new();
        encode_str(&mut encoded, username, EncodeSet::Userinfo);
        *self = {
            let mut components = self.components();
            if let Some(authority) = components.authority.as_mut() {
                authority.username = &encoded;
            }
            components.assemble()
        };
        Ok(())
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_password(&mut self, password: Option<&str>) -> Result<(), ()> {
        if !self.userinfo_settable() {
            return Err(());
        }
        let mut encoded = String::new();
        encode_str(&mut encoded, password.unwrap_or(""), EncodeSet::Userinfo);
        *self = {
            let mut components = self.components();
            if let Some(authority) = components.authority.as_mut() {
                authority.password = &encoded;
            }
            components.assemble()
        };
        Ok(())
    }
}

pub struct QueryPairs<'a> {
    url: &'a mut Url,
}

impl QueryPairs<'_> {
    fn extend_query(&mut self, change: impl FnOnce(&mut String)) {
        let mut query = self.url.query().unwrap_or("").to_owned();
        change(&mut query);
        *self.url = {
            let mut components = self.url.components();
            components.query = Some(&query);
            components.assemble()
        };
    }

    pub fn append_pair(&mut self, name: &str, value: &str) -> &mut Self {
        self.extend_query(|query| form::push_pair(query, name, value));
        self
    }

    pub fn append_key_only(&mut self, name: &str) -> &mut Self {
        self.extend_query(|query| form::push_key(query, name));
        self
    }

    pub fn extend_pairs<I, K, V>(&mut self, pairs: I) -> &mut Self
    where
        I: IntoIterator,
        I::Item: Borrow<(K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.extend_query(|query| {
            for pair in pairs {
                let (name, value) = pair.borrow();
                form::push_pair(query, name.as_ref(), value.as_ref());
            }
        });
        self
    }

    pub fn clear(&mut self) -> &mut Self {
        *self.url = {
            let mut components = self.url.components();
            components.query = Some("");
            components.assemble()
        };
        self
    }

    pub const fn finish(&mut self) -> &mut Url {
        self.url
    }
}

impl fmt::Display for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.serialization, formatter)
    }
}

impl fmt::Debug for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Url")
            .field("scheme", &self.scheme())
            .field("cannot_be_a_base", &self.cannot_be_a_base())
            .field("username", &self.username())
            .field("password", &self.password())
            .field("host", &self.host())
            .field("port", &self.port())
            .field("path", &self.path())
            .field("query", &self.query())
            .field("fragment", &self.fragment())
            .finish()
    }
}

impl PartialEq for Url {
    fn eq(&self, other: &Self) -> bool {
        self.serialization == other.serialization
    }
}

impl Eq for Url {}

impl PartialOrd for Url {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Url {
    fn cmp(&self, other: &Self) -> Ordering {
        self.serialization.cmp(&other.serialization)
    }
}

impl Hash for Url {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.serialization.hash(state);
    }
}

impl AsRef<str> for Url {
    fn as_ref(&self) -> &str {
        &self.serialization
    }
}

impl From<Url> for String {
    fn from(url: Url) -> Self {
        url.serialization
    }
}

impl FromStr for Url {
    type Err = ParseError;

    fn from_str(input: &str) -> Result<Self, ParseError> {
        Self::parse(input)
    }
}

impl TryFrom<&str> for Url {
    type Error = ParseError;

    fn try_from(input: &str) -> Result<Self, ParseError> {
        Self::parse(input)
    }
}

static NEXT_OPAQUE_ORIGIN: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OpaqueOrigin(usize);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    Opaque(OpaqueOrigin),
    Tuple(String, Host<String>, u16),
}

impl Origin {
    #[must_use]
    pub fn new_opaque() -> Self {
        Self::Opaque(OpaqueOrigin(
            NEXT_OPAQUE_ORIGIN.fetch_add(1, AtomicOrdering::Relaxed),
        ))
    }

    #[must_use]
    pub const fn is_tuple(&self) -> bool {
        matches!(self, Self::Tuple(..))
    }

    #[must_use]
    pub fn ascii_serialization(&self) -> String {
        match self {
            Self::Opaque(_) => "null".to_owned(),
            Self::Tuple(scheme, host, port) => {
                if default_port(scheme) == Some(*port) {
                    format!("{scheme}://{host}")
                } else {
                    format!("{scheme}://{host}:{port}")
                }
            }
        }
    }
}

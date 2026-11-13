use crate::postgres::PgError;
use std::path::PathBuf;

const DEFAULT_PORT: u16 = 5432;
const DEFAULT_HOST: &str = "localhost";
const SCHEMES: [&str; 2] = ["postgres://", "postgresql://"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlsMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyFull,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    Tcp(String),
    Unix(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgOptions {
    pub host: Host,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub tls: TlsMode,
    pub root_certificate: Option<PathBuf>,
    pub application_name: Option<String>,
}

impl PgOptions {
    #[must_use]
    pub fn new(host: Host, user: &str, database: &str) -> Self {
        Self {
            host,
            port: DEFAULT_PORT,
            user: user.to_owned(),
            password: None,
            database: database.to_owned(),
            tls: TlsMode::default(),
            root_certificate: None,
            application_name: None,
        }
    }

    pub fn parse(url: &str) -> Result<Self, PgError> {
        let invalid = |what: &str| PgError::Configuration(format!("{what} in a Postgres URL"));
        let rest = SCHEMES
            .iter()
            .find_map(|scheme| url.strip_prefix(scheme))
            .ok_or_else(|| invalid("no postgres:// or postgresql:// scheme"))?;
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, database) = rest.split_once('/').unwrap_or((rest, ""));
        let (credentials, address) = match authority.rsplit_once('@') {
            Some((credentials, address)) => (Some(credentials), address),
            None => (None, authority),
        };
        let (user, password) = match credentials {
            Some(credentials) => match credentials.split_once(':') {
                Some((user, password)) => (decode(user)?, Some(decode(password)?)),
                None => (decode(credentials)?, None),
            },
            None => (String::new(), None),
        };
        let (host, port) = split_port(address)?;
        let mut options = Self::new(
            Host::Tcp(if host.is_empty() {
                DEFAULT_HOST.to_owned()
            } else {
                decode(host)?
            }),
            &user,
            &decode(database)?,
        );
        options.password = password;
        if let Some(port) = port {
            options.port = port;
        }
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| invalid("a parameter without a value"))?;
            let value = decode(value)?;
            match key {
                "sslmode" => options.tls = tls_mode(&value)?,
                "sslrootcert" => options.root_certificate = Some(PathBuf::from(value)),
                "application_name" => options.application_name = Some(value),
                "host" if value.starts_with('/') => options.host = Host::Unix(PathBuf::from(value)),
                "host" => options.host = Host::Tcp(value),
                "port" => {
                    options.port = value
                        .parse()
                        .map_err(|_| invalid("a port that is not a number"))?;
                }
                "user" => options.user = value,
                "password" => options.password = Some(value),
                "dbname" => options.database = value,
                _ => {
                    return Err(PgError::Configuration(format!(
                        "the Postgres URL parameter {key} is not supported"
                    )));
                }
            }
        }
        if options.user.is_empty() {
            return Err(invalid("no user"));
        }
        if options.database.is_empty() {
            options.database.clone_from(&options.user);
        }
        Ok(options)
    }
}

fn split_port(address: &str) -> Result<(&str, Option<u16>), PgError> {
    let port_at = if address.starts_with('[') {
        address.find("]:").map(|at| at + 1)
    } else {
        address.rfind(':')
    };
    let Some(at) = port_at else {
        return Ok((address.trim_start_matches('[').trim_end_matches(']'), None));
    };
    let port = address[at + 1..].parse().map_err(|_| {
        PgError::Configuration("a port that is not a number in a Postgres URL".to_owned())
    })?;
    Ok((
        address[..at].trim_start_matches('[').trim_end_matches(']'),
        Some(port),
    ))
}

fn tls_mode(value: &str) -> Result<TlsMode, PgError> {
    match value {
        "disable" => Ok(TlsMode::Disable),
        "prefer" => Ok(TlsMode::Prefer),
        "require" => Ok(TlsMode::Require),
        "verify-full" => Ok(TlsMode::VerifyFull),
        _ => Err(PgError::Configuration(format!(
            "sslmode {value} is not supported; use disable, prefer, require or verify-full"
        ))),
    }
}

fn decode(text: &str) -> Result<String, PgError> {
    let invalid = || {
        PgError::Configuration(format!(
            "a bad percent escape in {text:?} in a Postgres URL"
        ))
    };
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            let hex = bytes.get(at + 1..at + 3).ok_or_else(invalid)?;
            let hex = std::str::from_utf8(hex).map_err(|_| invalid())?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| invalid())?);
            at += 3;
        } else {
            decoded.push(byte);
            at += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid())
}

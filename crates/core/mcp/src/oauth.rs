use std::{
    collections::{BTreeMap, HashSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Url, form_urlencoded};

pub const DISCOVERY_PROTOCOL_VERSION: &str = "2024-11-05";
pub const PKCE_VERIFIER_BYTES: usize = 32;
pub const STATE_BYTES: usize = 16;
pub const OFFLINE_ACCESS: &str = "offline_access";
pub const APPLICATION_TYPE: &str = "native";

const CLOUD_METADATA_HOSTS: &[&str] = &[
    "metadata",
    "metadata.google.internal",
    "metadata.azure.internal",
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
    #[serde(default)]
    pub jwks_uri: Option<String>,
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
    #[serde(default)]
    pub response_types_supported: Option<Vec<String>>,
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
    #[serde(flatten)]
    pub additional_fields: BTreeMap<String, Value>,
}

impl AuthorizationMetadata {
    #[must_use]
    pub fn legacy(base_url: &Url) -> Self {
        let endpoint = |path: &str| {
            let mut url = base_url.clone();
            url.set_query(None);
            url.set_fragment(None);
            url.set_path(path);
            url.to_string()
        };
        Self {
            authorization_endpoint: endpoint("/authorize"),
            token_endpoint: endpoint("/token"),
            registration_endpoint: Some(endpoint("/register")),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn flag(&self, name: &str) -> bool {
        self.additional_fields
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    #[must_use]
    pub fn requires_issuer_in_callback(&self) -> bool {
        self.flag("authorization_response_iss_parameter_supported")
    }

    #[must_use]
    pub fn uses_client_secret_post(&self) -> bool {
        self.additional_fields
            .get("token_endpoint_auth_methods_supported")
            .and_then(Value::as_array)
            .is_some_and(|methods| {
                let has = |name: &str| methods.iter().any(|method| method.as_str() == Some(name));
                has("client_secret_post") && !has("client_secret_basic")
            })
    }

    #[must_use]
    pub fn supports_offline_access(&self) -> bool {
        self.scopes_supported
            .as_ref()
            .is_some_and(|scopes| scopes.iter().any(|scope| scope == OFFLINE_ACCESS))
    }

    pub fn validate_authorization_code_support(&self) -> Result<(), String> {
        if let Some(response_types) = &self.response_types_supported
            && !response_types
                .iter()
                .any(|response_type| response_type == "code")
        {
            return Err("Invalid scope: code".to_owned());
        }
        match &self.code_challenge_methods_supported {
            Some(methods) if !methods.iter().any(|method| method == "S256") => Err(
                "Authorization server does not support the required PKCE code challenge method (S256)"
                    .to_owned(),
            ),
            Some(_) => Ok(()),
            None => {
                tracing::warn!(
                    "authorization server metadata omits code_challenge_methods_supported; proceeding with an S256 challenge anyway"
                );
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn revocation_endpoint(&self) -> Option<&str> {
        self.additional_fields
            .get("revocation_endpoint")
            .and_then(Value::as_str)
    }

    #[must_use]
    pub fn token_endpoint_auth_methods(&self) -> Vec<&str> {
        self.additional_fields
            .get("token_endpoint_auth_methods_supported")
            .and_then(Value::as_array)
            .map_or_else(
                || vec!["client_secret_basic"],
                |methods| methods.iter().filter_map(Value::as_str).collect(),
            )
    }

    #[must_use]
    pub fn issues_refresh_tokens(&self) -> bool {
        self.additional_fields
            .get("grant_types_supported")
            .and_then(Value::as_array)
            .is_none_or(|grants| {
                grants
                    .iter()
                    .any(|grant| grant.as_str() == Some("refresh_token"))
            })
    }

    pub fn add_offline_access(&self, scopes: &mut Vec<String>) {
        if scopes.is_empty() || scopes.iter().any(|scope| scope == OFFLINE_ACCESS) {
            return;
        }
        if self.supports_offline_access() {
            scopes.push(OFFLINE_ACCESS.to_owned());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ProtectedResourceMetadata {
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub authorization_server: Option<String>,
    #[serde(default)]
    pub authorization_servers: Option<Vec<String>>,
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
}

impl ProtectedResourceMetadata {
    pub fn validate_resource(&self, base_url: &Url) -> Result<(), String> {
        let Some(resource) = self.resource.as_deref() else {
            return Err(
                "Metadata error: Protected resource metadata missing required resource field"
                    .to_owned(),
            );
        };
        let Ok(resource_url) = Url::parse(resource) else {
            return Err(
                "Metadata error: Protected resource metadata resource field is not a valid URL"
                    .to_owned(),
            );
        };
        if resource_url.fragment().is_some() {
            return Err("Metadata error: Protected resource metadata resource does not permit fragment in URL as specified by RFC 8707".to_owned());
        }
        if !resource_identifier_matches(base_url, &resource_url) {
            return Err(format!(
                "Metadata error: Protected resource metadata resource mismatch: reference '{base_url}', permitted '{resource}'"
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn authorization_server_candidates(&self) -> Vec<String> {
        self.authorization_server
            .iter()
            .chain(self.authorization_servers.iter().flatten())
            .map(|candidate| candidate.trim())
            .filter(|candidate| !candidate.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WwwAuthenticate {
    pub resource_metadata_url: Option<Url>,
    pub scope: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

impl WwwAuthenticate {
    #[must_use]
    pub fn parse(header: &str, base_url: &Url) -> Self {
        let mut params = Self::default();
        let lowercase = header.to_ascii_lowercase();
        let resource_key = "resource_metadata=";
        let mut offset = 0;
        while let Some(position) = lowercase[offset..].find(resource_key) {
            let start = offset + position + resource_key.len();
            let Some((value, consumed)) = parse_header_value(&header[start..]) else {
                break;
            };
            if let Some(url) = resolve_resource_metadata_url(&value, base_url) {
                params.resource_metadata_url = Some(url);
                break;
            }
            offset = start + consumed;
        }
        let field = |key: &str| {
            lowercase
                .find(key)
                .and_then(|position| parse_header_value(&header[position + key.len()..]))
                .map(|(value, _)| value)
        };
        params.scope = field("scope=");
        params.error = field("error=");
        params.error_description = field("error_description=");
        params
    }

    #[must_use]
    pub fn scopes(&self) -> Vec<String> {
        self.scope
            .as_deref()
            .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

fn parse_header_value(fragment: &str) -> Option<(String, usize)> {
    let trimmed = fragment.trim_start();
    let leading = fragment.len() - trimmed.len();
    if let Some(quoted) = trimmed.strip_prefix('"') {
        let mut escaped = false;
        let mut value = String::new();
        for (index, character) in quoted.char_indices() {
            if escaped {
                value.push(character);
                escaped = false;
                continue;
            }
            match character {
                '\\' => escaped = true,
                '"' => return Some((value, leading + index + 2)),
                _ => value.push(character),
            }
        }
        None
    } else {
        let end = trimmed
            .find(|character: char| {
                character == ',' || character == ';' || character.is_whitespace()
            })
            .unwrap_or(trimmed.len());
        Some((trimmed[..end].to_owned(), leading + end))
    }
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

fn is_same_origin(base: &Url, candidate: &Url) -> bool {
    base.scheme() == candidate.scheme()
        && base
            .host_str()
            .zip(candidate.host_str())
            .is_some_and(|(base, candidate)| base.eq_ignore_ascii_case(candidate))
        && base.port_or_known_default() == candidate.port_or_known_default()
}

#[must_use]
pub fn resolve_resource_metadata_url(value: &str, base_url: &Url) -> Option<Url> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let url = Url::parse(value).or_else(|_| base_url.join(value)).ok()?;
    if is_http_url(&url) && is_same_origin(base_url, &url) {
        Some(url)
    } else {
        tracing::warn!(
            "rejecting resource metadata URL `{url}` because it is not same-origin with `{base_url}`"
        );
        None
    }
}

fn is_disallowed_ipv4(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    address.is_private()
        || address.is_loopback()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_unspecified()
        || address.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && matches!(octets[1], 18 | 19))
}

fn is_disallowed_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_disallowed_ipv4(mapped);
    }
    let segments = address.segments();
    address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xfe00) == 0xfc00
}

fn normalized_host(host: &str) -> String {
    host.trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn is_disallowed_host(host: &str) -> bool {
    let host = normalized_host(host);
    if host == "localhost"
        || host.ends_with(".localhost")
        || CLOUD_METADATA_HOSTS.contains(&host.as_str())
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => is_disallowed_ipv4(address),
        Ok(IpAddr::V6(address)) => is_disallowed_ipv6(address),
        Err(_) => false,
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = normalized_host(host);
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[must_use]
pub fn is_allowed_authorization_server_url(base_url: &Url, url: &Url) -> bool {
    if !is_http_url(url) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if !is_disallowed_host(host) {
        return true;
    }
    base_url.host_str().is_some_and(is_loopback_host) && is_loopback_host(host)
}

fn with_path(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.set_path(path);
    url
}

#[must_use]
pub fn protected_resource_metadata_urls(base_url: &Url) -> Vec<Url> {
    let trimmed = base_url.path().trim_matches('/');
    let canonical = "/.well-known/oauth-protected-resource".to_owned();
    let mut paths = Vec::new();
    if trimmed.is_empty() {
        paths.push(canonical);
    } else {
        for path in [
            format!("{canonical}/{trimmed}"),
            format!("/{trimmed}/.well-known/oauth-protected-resource"),
            canonical,
        ] {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    paths.iter().map(|path| with_path(base_url, path)).collect()
}

#[must_use]
pub fn authorization_server_metadata_urls(base_url: &Url) -> Vec<Url> {
    let trimmed = base_url.path().trim_matches('/');
    let paths = if trimmed.is_empty() {
        vec![
            "/.well-known/oauth-authorization-server".to_owned(),
            "/.well-known/openid-configuration".to_owned(),
        ]
    } else {
        vec![
            format!("/.well-known/oauth-authorization-server/{trimmed}"),
            format!("/.well-known/openid-configuration/{trimmed}"),
            format!("/{trimmed}/.well-known/openid-configuration"),
            "/.well-known/oauth-authorization-server".to_owned(),
        ]
    };
    paths.iter().map(|path| with_path(base_url, path)).collect()
}

#[must_use]
pub fn expected_issuer(discovery_url: &Url) -> Option<String> {
    let path = discovery_url.path();
    let oauth = "/.well-known/oauth-authorization-server";
    let oidc = "/.well-known/openid-configuration";
    let issuer_path = if path == oauth || path == oidc {
        ""
    } else if let Some(suffix) = path.strip_prefix(&format!("{oauth}/")) {
        suffix
    } else if let Some(suffix) = path.strip_prefix(&format!("{oidc}/")) {
        suffix
    } else {
        path.strip_suffix(oidc)?.trim_start_matches('/')
    };
    let mut issuer = discovery_url.clone();
    issuer.set_query(None);
    issuer.set_fragment(None);
    if issuer_path.is_empty() {
        issuer.set_path("");
    } else {
        issuer.set_path(&format!("/{issuer_path}"));
    }
    Some(issuer.to_string())
}

#[must_use]
pub fn issuers_match(received: &str, expected: &str) -> bool {
    if received == expected {
        return true;
    }
    let trim_root_slash = |issuer: &str| {
        issuer
            .strip_suffix('/')
            .filter(|without| {
                Url::parse(without).is_ok_and(|url| url.path().is_empty() || url.path() == "/")
            })
            .unwrap_or(issuer)
            .to_owned()
    };
    trim_root_slash(received) == trim_root_slash(expected)
}

pub fn validate_metadata_issuer(
    discovery_url: &Url,
    metadata: &AuthorizationMetadata,
) -> Result<(), String> {
    let Some(expected) = expected_issuer(discovery_url) else {
        return Ok(());
    };
    let Some(received) = metadata.issuer.as_deref() else {
        return Err(format!(
            "Authorization server response missing required issuer: expected {expected}"
        ));
    };
    if !issuers_match(received, &expected) {
        return Err(format!(
            "Authorization server issuer mismatch: expected {expected}, received {received}"
        ));
    }
    Ok(())
}

#[must_use]
pub fn resource_identifier_matches(expected: &Url, actual: &Url) -> bool {
    if expected == actual {
        return true;
    }
    if expected.scheme() != actual.scheme()
        || expected.host_str() != actual.host_str()
        || expected.port_or_known_default() != actual.port_or_known_default()
    {
        return false;
    }
    let expected_path = expected.path();
    let actual_path = actual.path();
    if expected_path == actual_path && expected.query() == actual.query() {
        return true;
    }
    expected_path.starts_with(actual_path)
        && expected.query().is_none()
        && actual.query().is_none()
        && (actual_path.ends_with('/')
            || expected_path.as_bytes().get(actual_path.len()) == Some(&b'/'))
}

#[must_use]
pub fn dedup_scopes(scopes: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    scopes
        .into_iter()
        .filter(|scope| seen.insert(scope.clone()))
        .collect()
}

#[must_use]
pub fn select_scopes(
    challenge_scopes: &[String],
    resource_scopes: &[String],
    metadata: &AuthorizationMetadata,
) -> Vec<String> {
    let mut scopes = if !challenge_scopes.is_empty() {
        dedup_scopes(challenge_scopes.iter().cloned())
    } else if !resource_scopes.is_empty() {
        dedup_scopes(resource_scopes.iter().cloned())
    } else {
        Vec::new()
    };
    metadata.add_offline_access(&mut scopes);
    scopes
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    #[must_use]
    pub fn from_random(random: &[u8; PKCE_VERIFIER_BYTES]) -> Self {
        let verifier = URL_SAFE_NO_PAD.encode(random);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

#[must_use]
pub fn state_from_random(random: &[u8; STATE_BYTES]) -> String {
    URL_SAFE_NO_PAD.encode(random)
}

pub struct AuthorizationUrl<'a> {
    pub metadata: &'a AuthorizationMetadata,
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub code_challenge: &'a str,
    pub scopes: &'a [String],
    pub resource: &'a str,
}

impl AuthorizationUrl<'_> {
    pub fn build(&self) -> Result<Url, String> {
        self.build_with(&[])
    }

    pub fn build_with(&self, params: &[(&str, &str)]) -> Result<Url, String> {
        let mut url = Url::parse(&self.metadata.authorization_endpoint)
            .map_err(|error| format!("OAuth error: Invalid authorization URL: {error}"))?;
        Url::parse(self.redirect_uri)
            .map_err(|error| format!("OAuth error: Invalid redirect URL: {error}"))?;
        let scopes = self.scopes.join(" ");
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("response_type", "code")
                .append_pair("client_id", self.client_id)
                .append_pair("state", self.state)
                .append_pair("code_challenge", self.code_challenge)
                .append_pair("code_challenge_method", "S256")
                .append_pair("redirect_uri", self.redirect_uri);
            if !scopes.is_empty() {
                query.append_pair("scope", &scopes);
            }
            if !self.resource.is_empty() {
                query.append_pair("resource", self.resource);
            }
            for (name, value) in params {
                query.append_pair(name, value);
            }
        }
        Ok(url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientAuthentication {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub secret_in_body: bool,
}

impl ClientAuthentication {
    #[must_use]
    pub fn public(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: None,
            secret_in_body: false,
        }
    }

    pub fn with_method(
        client_id: impl Into<String>,
        client_secret: Option<String>,
        method: &str,
    ) -> Result<Self, String> {
        let client_id = client_id.into();
        let client_secret = client_secret.filter(|secret| !secret.is_empty());
        let secret_in_body = match method {
            "none" => return Ok(Self::public(client_id)),
            "client_secret_basic" => false,
            "client_secret_post" => true,
            method => {
                return Err(format!(
                    "unsupported token endpoint authentication method `{method}`"
                ));
            }
        };
        if client_secret.is_none() {
            return Err(format!(
                "token endpoint authentication method `{method}` needs a client secret"
            ));
        }
        Ok(Self {
            client_id,
            client_secret,
            secret_in_body,
        })
    }

    #[must_use]
    pub const fn method(&self) -> &'static str {
        match (&self.client_secret, self.secret_in_body) {
            (None, _) => "none",
            (Some(_), false) => "client_secret_basic",
            (Some(_), true) => "client_secret_post",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRequest {
    pub body: String,
    pub authorization: Option<String>,
}

fn token_request(
    mut params: Vec<(&'static str, String)>,
    scopes: &[String],
    client: &ClientAuthentication,
    redirect_uri: Option<&str>,
    resource: &str,
) -> TokenRequest {
    if !scopes.is_empty() {
        params.push(("scope", scopes.join(" ")));
    }
    let mut authorization = None;
    match (&client.client_secret, client.secret_in_body) {
        (Some(secret), false) => {
            let id: String = form_urlencoded::byte_serialize(client.client_id.as_bytes()).collect();
            let secret: String = form_urlencoded::byte_serialize(secret.as_bytes()).collect();
            authorization = Some(format!(
                "Basic {}",
                STANDARD.encode(format!("{id}:{secret}"))
            ));
        }
        (secret, _) => {
            params.push(("client_id", client.client_id.clone()));
            if let Some(secret) = secret {
                params.push(("client_secret", secret.clone()));
            }
        }
    }
    if let Some(redirect_uri) = redirect_uri {
        params.push(("redirect_uri", redirect_uri.to_owned()));
    }
    if !resource.is_empty() {
        params.push(("resource", resource.to_owned()));
    }
    TokenRequest {
        body: form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params)
            .finish(),
        authorization,
    }
}

#[must_use]
pub fn authorization_code_request(
    client: &ClientAuthentication,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    resource: &str,
) -> TokenRequest {
    token_request(
        vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("code_verifier", code_verifier.to_owned()),
        ],
        &[],
        client,
        Some(redirect_uri),
        resource,
    )
}

#[must_use]
pub fn refresh_token_request(
    client: &ClientAuthentication,
    refresh_token: &str,
    scopes: &[String],
    resource: &str,
) -> TokenRequest {
    token_request(
        vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh_token.to_owned()),
        ],
        scopes,
        client,
        None,
        resource,
    )
}

#[must_use]
pub fn revocation_request(
    client: &ClientAuthentication,
    token: &str,
    token_type_hint: Option<&str>,
) -> TokenRequest {
    let mut params = vec![("token", token.to_owned())];
    if let Some(hint) = token_type_hint {
        params.push(("token_type_hint", hint.to_owned()));
    }
    token_request(params, &[], client, None, "")
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(from = "ReceivedTokenResponse")]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: Option<u64>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
}

#[derive(Deserialize)]
struct ReceivedTokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default, deserialize_with = "lifetime")]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    authed_user: Option<AuthedUser>,
}

#[derive(Deserialize)]
struct AuthedUser {
    #[serde(default)]
    scope: Option<String>,
}

impl From<ReceivedTokenResponse> for TokenResponse {
    fn from(received: ReceivedTokenResponse) -> Self {
        Self {
            access_token: received.access_token,
            token_type: received.token_type,
            expires_in: received.expires_in,
            refresh_token: received.refresh_token,
            scope: received
                .scope
                .or_else(|| received.authed_user.and_then(|user| user.scope)),
        }
    }
}

fn lifetime<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seconds {
        Number(u64),
        Text(String),
    }
    match Option::<Seconds>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Seconds::Number(seconds)) => Ok(Some(seconds)),
        Some(Seconds::Text(seconds)) => seconds.parse().map(Some).map_err(serde::de::Error::custom),
    }
}

impl TokenResponse {
    #[must_use]
    pub fn scopes(&self) -> Option<Vec<String>> {
        self.scope.as_deref().map(|scope| {
            scope
                .split(' ')
                .filter(|scope| !scope.is_empty())
                .map(str::to_owned)
                .collect()
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TokenErrorResponse {
    pub error: String,
    #[serde(default)]
    pub error_description: Option<String>,
    #[serde(default)]
    pub error_uri: Option<String>,
}

impl std::fmt::Display for TokenErrorResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)?;
        if let Some(description) = &self.error_description {
            write!(f, ": {description}")?;
        }
        if let Some(uri) = &self.error_uri {
            write!(f, " ({uri})")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    Rejected(TokenErrorResponse),
    Server(TokenErrorResponse),
    Invalid(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(error) | Self::Server(error) => {
                write!(f, "Server returned error response: {error}")
            }
            Self::Invalid(error) => f.write_str(error),
        }
    }
}

const REJECTED_ERRORS: &[&str] = &[
    "invalid_grant",
    "invalid_client",
    "unauthorized_client",
    "bad_refresh_token",
];

impl From<TokenErrorResponse> for TokenError {
    fn from(error: TokenErrorResponse) -> Self {
        if REJECTED_ERRORS.contains(&error.error.as_str()) {
            Self::Rejected(error)
        } else {
            Self::Server(error)
        }
    }
}

fn error_response(status: u16, body: &[u8]) -> TokenError {
    match serde_json::from_slice::<TokenErrorResponse>(body) {
        Ok(error) => error.into(),
        Err(_) if status == 401 => TokenError::Rejected(TokenErrorResponse {
            error: "invalid_client".to_owned(),
            error_description: Some("HTTP 401 without an OAuth error response".to_owned()),
            error_uri: None,
        }),
        Err(_) if body.is_empty() => TokenError::Invalid(format!(
            "Other error: server returned empty error response with HTTP {status}"
        )),
        Err(error) => TokenError::Invalid(format!(
            "Failed to parse server response with HTTP {status}: {error}"
        )),
    }
}

pub fn parse_revocation_response(status: u16, body: &[u8]) -> Result<(), TokenError> {
    if status == 200 {
        return Ok(());
    }
    Err(error_response(status, body))
}

pub fn parse_token_response(
    status: u16,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<TokenResponse, TokenError> {
    if status != 200 {
        return Err(error_response(status, body));
    }
    if let Some(content_type) = content_type
        && !content_type
            .to_ascii_lowercase()
            .starts_with("application/json")
    {
        return Err(TokenError::Invalid(format!(
            "Other error: unexpected response Content-Type: {content_type:?}, should be `application/json`"
        )));
    }
    if body.is_empty() {
        return Err(TokenError::Invalid(
            "Other error: server returned empty response body".to_owned(),
        ));
    }
    if let Ok(error) = serde_json::from_slice::<TokenErrorResponse>(body) {
        return Err(error.into());
    }
    serde_json::from_slice(body)
        .map_err(|error| TokenError::Invalid(format!("Failed to parse server response: {error}")))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ClientRegistrationRequest {
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub grant_types: Vec<String>,
    pub token_endpoint_auth_method: String,
    pub response_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_type: Option<String>,
}

impl ClientRegistrationRequest {
    #[must_use]
    pub fn public_client(client_name: &str, redirect_uri: &str, scopes: &[String]) -> Self {
        Self {
            client_name: client_name.to_owned(),
            redirect_uris: vec![redirect_uri.to_owned()],
            grant_types: vec!["authorization_code".to_owned(), "refresh_token".to_owned()],
            token_endpoint_auth_method: "none".to_owned(),
            response_types: vec!["code".to_owned()],
            scope: (!scopes.is_empty()).then(|| scopes.join(" ")),
            application_type: Some(APPLICATION_TYPE.to_owned()),
        }
    }

    #[must_use]
    pub fn web_client(
        client_name: &str,
        redirect_uri: &str,
        scopes: &[String],
        token_endpoint_auth_method: &str,
        refresh_tokens: bool,
    ) -> Self {
        let mut grant_types = vec!["authorization_code".to_owned()];
        if refresh_tokens {
            grant_types.push("refresh_token".to_owned());
        }
        Self {
            client_name: client_name.to_owned(),
            redirect_uris: vec![redirect_uri.to_owned()],
            grant_types,
            token_endpoint_auth_method: token_endpoint_auth_method.to_owned(),
            response_types: vec!["code".to_owned()],
            scope: (!scopes.is_empty()).then(|| scopes.join(" ")),
            application_type: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ClientRegistrationResponse {
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub token_endpoint_auth_method: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationCallback {
    pub code: String,
    pub state: String,
    pub issuer: Option<String>,
}

impl AuthorizationCallback {
    pub fn from_url(url: &Url) -> Result<Self, String> {
        let mut code = None;
        let mut state = None;
        let mut issuer = None;
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "iss" => issuer = Some(value.into_owned()),
                _ => {}
            }
        }
        Ok(Self {
            code: code.ok_or_else(|| {
                "OAuth authorization failed: Authorization callback missing code".to_owned()
            })?,
            state: state.ok_or_else(|| {
                "OAuth authorization failed: Authorization callback missing state".to_owned()
            })?,
            issuer,
        })
    }

    pub fn validate_issuer(
        &self,
        expected_issuer: Option<&str>,
        require_issuer: bool,
    ) -> Result<(), String> {
        let Some(expected) = expected_issuer else {
            if self.issuer.is_some() || require_issuer {
                return Err("OAuth authorization failed: Authorization callback issuer cannot be validated because expected issuer was not recorded".to_owned());
            }
            return Ok(());
        };
        let Some(received) = self.issuer.as_deref() else {
            if require_issuer {
                return Err(format!(
                    "Authorization server response missing required issuer: expected {expected}"
                ));
            }
            return Ok(());
        };
        if received != expected {
            return Err(format!(
                "Authorization server issuer mismatch: expected {expected}, received {received}"
            ));
        }
        Ok(())
    }
}

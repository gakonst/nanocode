#[cfg(not(target_family = "wasm"))]
pub mod oauth;
pub mod streamable_http;

#[derive(Clone)]
pub enum SecretSource {
    Value(String),
    Environment(String),
}

impl SecretSource {
    pub fn resolve(&self) -> Result<String, String> {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Environment(variable) => std::env::var(variable).map_err(|error| {
                format!("environment variable `{variable}` is unavailable: {error}")
            }),
        }
    }
}

pub fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

#[cfg(not(target_family = "wasm"))]
#[must_use]
pub fn same_origin_redirect_policy(
    replays_plaintext_proxy_credentials: bool,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let Some(original) = attempt.previous().first() else {
            return attempt.error("MCP HTTP redirect has no original request URL");
        };
        if attempt.url().origin() != original.origin() {
            return attempt.error("MCP HTTP redirect to a different origin is not allowed");
        }
        if attempt.previous().len() > 10 {
            return attempt.error("MCP HTTP request exceeded the redirect limit");
        }
        let secure_destination = attempt.url().scheme() == "https"
            || match attempt.url().host() {
                Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => true,
                Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
                None => false,
            };
        if !secure_destination {
            return attempt.error("MCP HTTP redirects for non-loopback hostnames require HTTPS");
        }
        if replays_plaintext_proxy_credentials && original.scheme() != "https" {
            return attempt
                .error("MCP HTTP redirects cannot replay proxy credentials over plaintext HTTP");
        }
        attempt.follow()
    })
}

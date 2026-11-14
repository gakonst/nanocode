use std::collections::HashSet;
use std::fmt::{self, Display};
use std::future::Future;

use nanocode_http::header::{AUTHORIZATION, HOST, HeaderName, HeaderValue};
use nanocode_http::{Method, Request, Response, StatusCode, Url};

use super::error::Fault;
use super::events::{Events, PaymentResponse};
use super::protocol::{
    self, ACCEPT_PAYMENT_HEADER, Challenge, Credential, INTENT_CHARGE, INTENT_SESSION, METHOD_NAME,
    PAYMENT_RECEIPT_HEADER, Receipt,
};
use super::provider::{PaymentContext, PaymentProvider, PendingPayment};

pub const DEFAULT_MAX_PAYMENT_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub enum Policy {
    Always,
    Origins(Vec<String>),
}

impl Policy {
    pub(crate) fn allows(&self, url: &Url) -> bool {
        match self {
            Self::Always => true,
            Self::Origins(patterns) => patterns.iter().any(|pattern| matches_origin(url, pattern)),
        }
    }
}

fn matches_origin(url: &Url, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
            return false;
        };
        let suffix = suffix.to_ascii_lowercase();
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }
    Url::parse(pattern).is_ok_and(|pattern| url.origin() == pattern.origin())
}

#[derive(Debug)]
pub enum HttpFault {
    MissingChallenge,
    NoSupportedChallenge(String),
    InvalidCredential(String),
    CloneFailed,
    CrossOriginRedirect,
    RedirectedRequest(String),
    CrossRealmProof,
    UnsafeCredentialHeader,
    Payment(Fault),
    Request(String),
}

impl Display for HttpFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingChallenge => f.write_str("402 response missing WWW-Authenticate header"),
            Self::NoSupportedChallenge(message) => f.write_str(message),
            Self::InvalidCredential(message) => write!(f, "invalid credential: {message}"),
            Self::CloneFailed => f.write_str("request could not be cloned for retry"),
            Self::CrossOriginRedirect => f.write_str("Refusing to send payment credential across redirect"),
            Self::RedirectedRequest(method) => {
                write!(f, "Cannot automatically pay a redirected {method} request")
            }
            Self::CrossRealmProof => f.write_str("Refusing to sign Tempo proof for a different HTTP realm"),
            Self::UnsafeCredentialHeader => f.write_str(
                "Payment-Authorization requires a redirect-safe client; refusing to create a credential",
            ),
            Self::Payment(error) => write!(f, "payment failed: {error}"),
            Self::Request(error) => write!(f, "HTTP request failed: {error}"),
        }
    }
}

impl From<Fault> for HttpFault {
    fn from(error: Fault) -> Self {
        Self::Payment(error)
    }
}

#[derive(Debug)]
pub enum FlowError<E> {
    Payment(HttpFault),
    Send(E),
}

impl<E> From<HttpFault> for FlowError<E> {
    fn from(error: HttpFault) -> Self {
        Self::Payment(error)
    }
}

pub trait Exchange: Send {
    type Error: Display + Send;

    fn check_challenge(&self, challenge: &Challenge) -> Result<(), HttpFault> {
        match challenge.credential_header() == "Authorization" {
            true => Ok(()),
            false => Err(HttpFault::UnsafeCredentialHeader),
        }
    }

    fn http_client(&self) -> Option<nanocode_http::Client>;

    fn send(
        &mut self,
        request: Request,
    ) -> impl Future<Output = Result<Response, Self::Error>> + Send;
}

impl Exchange for nanocode_http::Client {
    type Error = HttpFault;

    fn http_client(&self) -> Option<nanocode_http::Client> {
        Some(self.clone())
    }

    async fn send(&mut self, request: Request) -> Result<Response, HttpFault> {
        self.execute(request)
            .await
            .map(nanocode_http::decode_flate_response)
            .map_err(|error| HttpFault::Request(error.without_url().to_string()))
    }
}

enum Prepaid {
    Answered(Response),
    Unpaid(Response),
}

pub(crate) enum Selection {
    Chosen(Challenge),
    Expired(Challenge),
    None(String),
}

pub(crate) fn select(
    challenges: &[Challenge],
    ranking: Option<&str>,
    supports: impl Fn(&Challenge) -> bool,
    choose: impl for<'a> Fn(&[&'a Challenge]) -> Option<&'a Challenge>,
) -> Selection {
    let preferences = ranking.and_then(protocol::preferences);
    let rank = |candidates| ranked(candidates, preferences.as_deref());
    let supported: Vec<&Challenge> = challenges
        .iter()
        .filter(|challenge| supports(challenge))
        .collect();
    let payable = rank(
        supported
            .iter()
            .copied()
            .filter(|challenge| !challenge.is_expired())
            .collect(),
    );
    if !payable.is_empty() {
        return match choose(&payable) {
            Some(challenge) => Selection::Chosen(challenge.clone()),
            None => Selection::None(no_method_found(challenges)),
        };
    }
    let supported = rank(supported);
    match choose(&supported) {
        Some(challenge) => Selection::Expired(challenge.clone()),
        None => Selection::None(no_method_found(challenges)),
    }
}

fn ranked<'a>(
    candidates: Vec<&'a Challenge>,
    preferences: Option<&[protocol::Preference]>,
) -> Vec<&'a Challenge> {
    match preferences {
        Some(preferences) => protocol::rank(&candidates, preferences),
        None => candidates,
    }
}

fn no_method_found(challenges: &[Challenge]) -> String {
    let offered: Vec<String> = challenges
        .iter()
        .map(|challenge| format!("{}.{}", challenge.method, challenge.intent))
        .collect();
    format!("No method found for challenges: {}", offered.join(", "))
}

fn with_available_methods(message: String, accept: Option<&str>) -> String {
    let available: Vec<String> = accept
        .and_then(protocol::preferences)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| format!("{}.{}", entry.method, entry.intent))
        .collect();
    format!("{message}. Available: {}", available.join(", "))
}

fn same_resource(a: &Url, b: &Url) -> bool {
    a.as_str().split('#').next() == b.as_str().split('#').next()
}

pub(crate) fn cross_realm_proof(
    challenge: &Challenge,
    url: &Url,
    headers: &nanocode_http::header::HeaderMap,
) -> bool {
    let requested = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Url::parse(&format!("http://{value}")).ok())
        .and_then(|host| host.host_str().map(str::to_owned))
        .or_else(|| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    challenge.method == METHOD_NAME
        && challenge.intent == INTENT_CHARGE
        && super::charge::requested_amount(challenge) == Some(0)
        && !challenge.realm.eq_ignore_ascii_case(&requested)
}

fn credential_header(challenge: &Challenge) -> HeaderName {
    HeaderName::from_bytes(challenge.credential_header().as_bytes()).unwrap_or(AUTHORIZATION)
}

fn authorization_value(credential: &Credential) -> Result<HeaderValue, HttpFault> {
    let header = credential
        .authorization()
        .map_err(|error| HttpFault::InvalidCredential(error.to_string()))?;
    HeaderValue::from_str(&header).map_err(|error| HttpFault::InvalidCredential(error.to_string()))
}

fn receipt_header(response: &Response) -> Option<Receipt> {
    response
        .headers()
        .get(PAYMENT_RECEIPT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Receipt::parse(value).ok())
}

fn paid_receipt(
    response: &Response,
    challenge: &Challenge,
    credential: &Credential,
) -> Option<Receipt> {
    let receipt = receipt_header(response)?;
    if challenge.intent != INTENT_SESSION {
        return Some(receipt);
    }
    (receipt.is_success()
        && receipt.method == challenge.method
        && receipt.extension("challengeId") == Some(challenge.id.as_str())
        && credential
            .payload_str("channelId")
            .is_some_and(|id| receipt.reference.eq_ignore_ascii_case(id)))
    .then_some(receipt)
}

fn session_origin_mismatch() -> HttpFault {
    HttpFault::Payment(Fault::config("session response origin mismatch"))
}

pub async fn send_with_exchange<P: PaymentProvider, X: Exchange>(
    exchange: &mut X,
    request: Request,
    provider: &P,
    policy: &Policy,
    events: &Events,
    max_payment_retries: usize,
) -> Result<Response, FlowError<X::Error>> {
    Flow {
        provider,
        policy,
        events,
        max_payment_retries,
    }
    .run(exchange, request)
    .await
}

struct Flow<'a, P> {
    provider: &'a P,
    policy: &'a Policy,
    events: &'a Events,
    max_payment_retries: usize,
}

impl<P: PaymentProvider> Flow<'_, P> {
    fn supported(&self, challenge: &Challenge) -> bool {
        self.provider.supports(&challenge.method, &challenge.intent)
    }

    fn check<X: Exchange>(
        &self,
        exchange: &X,
        challenge: &Challenge,
        ranking: Option<&str>,
    ) -> Result<(), HttpFault> {
        exchange.check_challenge(challenge)?;
        match select(
            std::slice::from_ref(challenge),
            ranking,
            |challenge| self.supported(challenge),
            |candidates| candidates.first().copied(),
        ) {
            Selection::Chosen(_) => Ok(()),
            Selection::Expired(challenge) => {
                Err(HttpFault::Payment(Fault::Expired(challenge.expires)))
            }
            Selection::None(message) => Err(HttpFault::NoSupportedChallenge(message)),
        }
    }

    fn charged(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: Option<&Receipt>,
    ) -> Option<u128> {
        if challenge.intent == INTENT_SESSION {
            return receipt
                .and_then(|receipt| self.provider.record_receipt(context, challenge, receipt));
        }
        challenge.amount()
    }

    fn responded(&self, context: &PaymentContext, challenge: Challenge, receipt: Option<Receipt>) {
        let amount = self.charged(context, &challenge, receipt.as_ref());
        let funding_currency = self.provider.funding_currency(&challenge);
        self.events.payment_response(&PaymentResponse {
            challenge,
            url: context.url.clone(),
            receipt,
            amount,
            funding_currency,
        });
    }

    async fn run<X: Exchange>(
        &self,
        exchange: &mut X,
        mut request: Request,
    ) -> Result<Response, FlowError<X::Error>> {
        let caller_accept = request
            .headers()
            .get(ACCEPT_PAYMENT_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(String::from);
        let payment_allowed = match request.headers().contains_key(ACCEPT_PAYMENT_HEADER) {
            true => caller_accept.as_deref().is_some_and(|value| {
                protocol::preferences(value).is_some_and(|entries| !entries.is_empty())
            }),
            false => self.policy.allows(request.url()),
        };
        let provider_accept = self.provider.accept_payment_header();
        let mut injected = false;
        if caller_accept.is_none()
            && self.policy.allows(request.url())
            && let Some(value) = provider_accept
                .as_deref()
                .and_then(|header| HeaderValue::from_str(header).ok())
        {
            request.headers_mut().append(ACCEPT_PAYMENT_HEADER, value);
            injected = true;
        }
        let ranking = caller_accept.or(provider_accept);
        let ranking = ranking.as_deref();
        let retry = request.try_clone();
        let context = retry.as_ref().map(|retry| {
            let mut headers = retry.headers().clone();
            if injected {
                headers.remove(ACCEPT_PAYMENT_HEADER);
            }
            PaymentContext {
                http_client: exchange.http_client(),
                url: retry.url().clone(),
                headers,
            }
        });
        let prepaid = match &context {
            Some(context) if payment_allowed => {
                self.provider.prepaid_credential(context.clone()).await
            }
            _ => None,
        };
        let prepaid = prepaid.filter(|(challenge, credential)| {
            let usable = self.check(exchange, challenge, ranking).is_ok();
            if !usable {
                self.provider.abandon_payment(challenge, credential);
            }
            usable
        });
        let mut response = match (prepaid, &context) {
            (Some((challenge, credential)), Some(context)) => {
                match self
                    .send_prepaid(
                        exchange,
                        request,
                        retry.as_ref(),
                        context,
                        challenge,
                        credential,
                    )
                    .await?
                {
                    Prepaid::Answered(response) => return Ok(response),
                    Prepaid::Unpaid(response) => response,
                }
            }
            _ => exchange.send(request).await.map_err(FlowError::Send)?,
        };
        if response.status() != StatusCode::PAYMENT_REQUIRED || !payment_allowed {
            return Ok(response);
        }
        let (Some(retry), Some(context)) = (retry, context) else {
            return Err(HttpFault::CloneFailed.into());
        };
        let mut paid_ids = HashSet::new();
        let mut retried_stale_session = false;
        let mut attempt = 0;
        while attempt < self.max_payment_retries {
            if response.status() != StatusCode::PAYMENT_REQUIRED {
                return Ok(response);
            }
            if context.url.origin() != response.url().origin() {
                return Err(HttpFault::CrossOriginRedirect.into());
            }
            let method = retry.method();
            if *method != Method::GET
                && *method != Method::HEAD
                && !same_resource(&context.url, response.url())
            {
                if attempt > 0 {
                    return Ok(response);
                }
                return Err(HttpFault::RedirectedRequest(method.to_string()).into());
            }
            let values: Vec<&[u8]> = response
                .headers()
                .get_all(nanocode_http::header::WWW_AUTHENTICATE)
                .iter()
                .map(HeaderValue::as_bytes)
                .collect();
            if values.is_empty() {
                if !paid_ids.is_empty() {
                    return Ok(response);
                }
                return Err(HttpFault::MissingChallenge.into());
            }
            let challenges = protocol::challenges(values);
            let safe = challenges.iter().any(|challenge| {
                exchange.check_challenge(challenge).is_ok() && self.supported(challenge)
            });
            if !safe {
                for challenge in challenges
                    .iter()
                    .filter(|challenge| self.supported(challenge))
                {
                    exchange.check_challenge(challenge)?;
                }
            }
            let challenge = match select(
                &challenges,
                ranking,
                |challenge| {
                    exchange.check_challenge(challenge).is_ok() && self.supported(challenge)
                },
                |candidates| self.provider.select_challenge(candidates),
            ) {
                Selection::Chosen(challenge) => challenge,
                Selection::Expired(challenge) => {
                    return Err(HttpFault::Payment(Fault::Expired(challenge.expires)).into());
                }
                Selection::None(message) => {
                    if !paid_ids.is_empty() {
                        return Ok(response);
                    }
                    let message = with_available_methods(
                        message,
                        self.provider.accept_payment_header().as_deref(),
                    );
                    return Err(HttpFault::NoSupportedChallenge(message).into());
                }
            };
            self.check(exchange, &challenge, ranking)?;
            if cross_realm_proof(&challenge, &context.url, &context.headers) {
                return Err(HttpFault::CrossRealmProof.into());
            }
            attempt += 1;
            if !paid_ids.insert(challenge.id.clone()) {
                return Ok(response);
            }
            let credential = self
                .provider
                .pay_with_context(&challenge, context.clone())
                .await
                .map_err(HttpFault::Payment)?;
            let pending =
                PendingPayment::new(self.provider.clone(), challenge.clone(), credential.clone());
            let header = match authorization_value(&credential) {
                Ok(header) => header,
                Err(error) => {
                    pending.rollback().await.map_err(HttpFault::Payment)?;
                    return Err(error.into());
                }
            };
            let Some(mut paid) = retry.try_clone() else {
                pending.rollback().await.map_err(HttpFault::Payment)?;
                return Err(HttpFault::CloneFailed.into());
            };
            *paid.url_mut() = response.url().clone();
            paid.headers_mut()
                .insert(credential_header(&challenge), header);
            response = exchange.send(paid).await.map_err(FlowError::Send)?;
            let session = challenge.intent == INTENT_SESSION;
            if session && response.url().origin() != context.url.origin() {
                drop(pending);
                return Err(session_origin_mismatch().into());
            }
            let status = response.status();
            let receipt = paid_receipt(&response, &challenge, &credential);
            let receipt_valid = !session || receipt.is_some();
            if status.is_success() {
                self.responded(&context, challenge, receipt);
                match receipt_valid {
                    true => pending.commit().await.map_err(HttpFault::Payment)?,
                    false => drop(pending),
                }
                return Ok(response);
            }
            if status == StatusCode::GONE && session {
                pending.invalidate().await.map_err(HttpFault::Payment)?;
                if retried_stale_session {
                    return Ok(response);
                }
                retried_stale_session = true;
                paid_ids.clear();
                response = exchange
                    .send(retry.try_clone().ok_or(HttpFault::CloneFailed)?)
                    .await
                    .map_err(FlowError::Send)?;
                continue;
            }
            let has_receipt = response.headers().contains_key(PAYMENT_RECEIPT_HEADER);
            if status.is_server_error() {
                match has_receipt && receipt_valid {
                    true => pending.commit().await.map_err(HttpFault::Payment)?,
                    false => drop(pending),
                }
                return Ok(response);
            }
            match (has_receipt, receipt_valid) {
                (true, true) => pending.commit().await.map_err(HttpFault::Payment)?,
                (true, false) => drop(pending),
                (false, _) => pending.rollback().await.map_err(HttpFault::Payment)?,
            }
            if credential.is_hash() {
                return Ok(response);
            }
            if status != StatusCode::PAYMENT_REQUIRED || attempt == self.max_payment_retries {
                return Ok(response);
            }
        }
        Ok(response)
    }

    async fn send_prepaid<X: Exchange>(
        &self,
        exchange: &mut X,
        mut request: Request,
        retry: Option<&Request>,
        context: &PaymentContext,
        challenge: Challenge,
        credential: Credential,
    ) -> Result<Prepaid, FlowError<X::Error>> {
        let Ok(header) = authorization_value(&credential) else {
            return exchange
                .send(request)
                .await
                .map(Prepaid::Unpaid)
                .map_err(FlowError::Send);
        };
        request
            .headers_mut()
            .insert(credential_header(&challenge), header);
        let response = exchange.send(request).await.map_err(FlowError::Send)?;
        if challenge.intent == INTENT_SESSION && response.url().origin() != context.url.origin() {
            return Err(session_origin_mismatch().into());
        }
        let status = response.status();
        if status.is_server_error() {
            return Ok(Prepaid::Answered(response));
        }
        if status == StatusCode::PAYMENT_REQUIRED || status == StatusCode::GONE {
            if status == StatusCode::GONE {
                self.provider
                    .invalidate_payment(&challenge, &credential)
                    .await
                    .map_err(HttpFault::Payment)?;
            }
            let retry = retry
                .and_then(Request::try_clone)
                .ok_or(HttpFault::CloneFailed)?;
            return exchange
                .send(retry)
                .await
                .map(Prepaid::Unpaid)
                .map_err(FlowError::Send);
        }
        if status.is_success() {
            let receipt = paid_receipt(&response, &challenge, &credential);
            self.responded(context, challenge, receipt);
        }
        Ok(Prepaid::Answered(response))
    }
}

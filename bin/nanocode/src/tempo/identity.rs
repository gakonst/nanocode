use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use nanocode_sync::{Atom, SingleFlight, Slot};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Request, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::flow::{Exchange, HttpFault};
use super::protocol::{Challenge, decode_url, now_seconds};
use super::{Payments, notify};

pub(crate) const ATTESTATION_HEADER: &str = "identity-attestation";
pub(crate) const REQUIRED_HEADER: &str = "identity-attestation-required";
pub(crate) const DEFAULT_ISSUER: &str = "https://identity.mpp.tempo.xyz";
const EMAIL: &str = "email";
const OTP_PATH: &str = "/email/otp";
const VERIFY_PATH: &str = "/email/verify";
const CODE_ATTEMPTS: u32 = 3;
const CLOCK_TOLERANCE: i64 = 60;
const REFRESH_MARGIN: i64 = 300;
const MAX_LENGTH: usize = 8_192;
const PASSES_DIRECTORY: &str = "mpp-passes";
const PASS_EXTENSION: &str = "json";
const TEMPORARY_EXTENSION: &str = "tmp";
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const UNASKED: &str = "nanocode exec asks for the code only on a terminal and with --identity-email: run nanocode and answer with /pass, run exec in a terminal with --identity-email <address>, or give --identity-attestation <pass>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asker {
    Pass,
    Stdin,
    Nobody,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
    received: i64,
    email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Requirement {
    pub(crate) kind: String,
    pub(crate) issuer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Claims {
    pub(crate) audience: String,
    pub(crate) issuer: String,
    pub(crate) kind: String,
    pub(crate) issued: i64,
    pub(crate) expires: i64,
}

impl Claims {
    fn key(&self) -> String {
        key(&self.audience, &self.issuer, &self.kind)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Held {
    token: String,
    claims: Claims,
    received: i64,
}

impl Held {
    pub(crate) fn decode(token: &str, received: Option<i64>) -> Result<Self, String> {
        let claims = claims(token).ok_or("the identity attestation is malformed")?;
        Ok(Self {
            token: token.to_owned(),
            received: received.unwrap_or(claims.issued),
            claims,
        })
    }

    fn margin(&self) -> i64 {
        REFRESH_MARGIN.min((self.claims.expires - self.received) / 2)
    }

    pub(crate) fn usable(&self, now: i64, paid: bool) -> bool {
        self.claims.expires > now - CLOCK_TOLERANCE
            && (paid || self.claims.expires - now > self.margin())
    }
}

#[derive(Debug)]
enum Answer {
    Email(String),
    Code(String),
    Decline,
}

impl Answer {
    fn parse(text: &str) -> Self {
        let text = text.trim();
        let text = text.strip_prefix("/pass").map_or(text, str::trim);
        match text {
            "" => Self::Decline,
            email if email.contains('@') => Self::Email(email.to_owned()),
            code => Self::Code(code.to_owned()),
        }
    }
}

pub(crate) struct Rejected {
    audience: String,
    groups: Vec<Vec<Requirement>>,
    sent: Vec<String>,
}

pub(crate) struct Identity {
    issuers: Vec<String>,
    email: Atom<Option<String>>,
    held: Atom<BTreeMap<String, Held>>,
    flight: SingleFlight<(String, Result<(), String>)>,
    answers: UnboundedSender<Answer>,
    inbox: Slot<UnboundedReceiver<Answer>>,
    payments: Payments,
    asker: Asker,
    passes: Option<PathBuf>,
}

struct Lent<'a> {
    inbox: Option<UnboundedReceiver<Answer>>,
    slot: &'a Slot<UnboundedReceiver<Answer>>,
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        if let Some(inbox) = self.inbox.take() {
            drop(self.slot.put(inbox));
        }
    }
}

impl Identity {
    pub(crate) fn new(
        payments: Payments,
        issuers: &[String],
        email: Option<String>,
        attestations: &[String],
        asker: Asker,
    ) -> Result<Self, String> {
        let issuers = match issuers.is_empty() {
            true => vec![DEFAULT_ISSUER.to_owned()],
            false => issuers
                .iter()
                .map(|issuer| issuer.trim().trim_end_matches('/').to_owned())
                .collect(),
        };
        if let Some(issuer) = issuers.iter().find(|issuer| !secure_text(issuer)) {
            return Err(format!(
                "identity issuer {issuer} must be an https URL (http only on localhost)"
            ));
        }
        let email = email.filter(|email| !email.trim().is_empty());
        let passes = passes_directory();
        let mut held: BTreeMap<String, Held> = passes
            .as_deref()
            .map(|directory| load(directory, &issuers, email.as_deref(), now_seconds()))
            .unwrap_or_default()
            .into_iter()
            .map(|entry| (entry.claims.key(), entry))
            .collect();
        for token in attestations.iter().flat_map(|value| tokens(value)) {
            let entry = Held::decode(&token, None)?;
            held.insert(entry.claims.key(), entry);
        }
        let (answers, inbox) = mpsc::unbounded_channel();
        Ok(Self {
            issuers,
            email: Atom::new(email),
            held: Atom::new(held),
            flight: SingleFlight::default(),
            answers,
            inbox: Slot::new(Some(inbox)),
            payments,
            asker,
            passes,
        })
    }

    pub(crate) fn answer(&self, text: &str) {
        let answer = Answer::parse(text);
        if let Answer::Email(email) = &answer {
            drop(self.email.set(Some(email.clone())));
        }
        drop(self.answers.send(answer));
    }

    fn notice(&self, text: String) {
        notify(&self.payments.listeners, Some(text));
    }

    pub(crate) fn prove(&self, request: &mut Request) {
        let url = request.url();
        if !secure(url) {
            return;
        }
        let origin = url.origin().ascii_serialization();
        let paid = paid(request.headers());
        let now = now_seconds();
        let tokens: Vec<String> = self.held.read(|held| {
            held.values()
                .filter(|entry| entry.claims.audience == origin && entry.usable(now, paid))
                .map(|entry| entry.token.clone())
                .collect()
        });
        if tokens.is_empty() {
            return;
        }
        if let Ok(value) = HeaderValue::from_str(&tokens.join(", ")) {
            request.headers_mut().insert(ATTESTATION_HEADER, value);
        }
    }

    pub(crate) fn rejected(&self, response: &Response, sent: &Request) -> Option<Rejected> {
        if response.status() != StatusCode::UNAUTHORIZED
            && response.status() != StatusCode::FORBIDDEN
        {
            return None;
        }
        if response.url().origin() != sent.url().origin() || !secure(sent.url()) {
            return None;
        }
        let groups = requirements(response.headers().get(REQUIRED_HEADER)?.to_str().ok()?);
        Some(Rejected {
            audience: sent.url().origin().ascii_serialization(),
            groups,
            sent: sent
                .headers()
                .get(ATTESTATION_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(tokens)
                .unwrap_or_default(),
        })
    }

    pub(crate) async fn retry(&self, rejected: Rejected) -> Option<HeaderValue> {
        let Rejected {
            audience,
            groups,
            sent,
        } = rejected;
        let mut obtained: Vec<(String, String)> = Vec::new();
        let mut failure = None;
        for group in groups {
            let alternatives = self.trusted(&group, &audience);
            if alternatives
                .iter()
                .any(|(key, _)| obtained.iter().any(|(got, _)| got == key))
            {
                continue;
            }
            for (key, issuer) in alternatives {
                match self.obtain(&key, &issuer, &audience, &sent).await {
                    Ok(Some(token)) => {
                        obtained.push((key, token));
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
        }
        if obtained.is_empty() {
            if let Some(error) = failure {
                self.notice(format!("email pass for {audience}: {error}"));
            }
            return None;
        }
        let mut value: Vec<String> = sent
            .into_iter()
            .filter(|token| {
                claims(token)
                    .is_none_or(|claims| !obtained.iter().any(|(key, _)| *key == claims.key()))
            })
            .collect();
        value.extend(obtained.into_iter().map(|(_, token)| token));
        HeaderValue::from_str(&value.join(", ")).ok()
    }

    fn trusted(&self, group: &[Requirement], audience: &str) -> Vec<(String, String)> {
        let mut trusted: Vec<(String, String)> = Vec::new();
        for requirement in group {
            let Some(issuer) = &requirement.issuer else {
                continue;
            };
            if requirement.kind != EMAIL || !self.issuers.contains(issuer) {
                continue;
            }
            let key = key(audience, issuer, &requirement.kind);
            if !trusted.iter().any(|(known, _)| *known == key) {
                trusted.push((key, issuer.clone()));
            }
        }
        trusted
    }

    async fn obtain(
        &self,
        key: &str,
        issuer: &str,
        audience: &str,
        sent: &[String],
    ) -> Result<Option<String>, String> {
        let now = now_seconds();
        let rejected = match self.held(key) {
            Some(entry)
                if entry.usable(now, true)
                    && !sent.contains(&entry.token)
                    && (!sent.is_empty() || entry.usable(now, false)) =>
            {
                return Ok(Some(entry.token));
            }
            Some(entry) if !entry.usable(now, true) || sent.contains(&entry.token) => {
                self.held.update(|held| {
                    if held.get(key).is_some_and(|held| held.token == entry.token) {
                        held.remove(key);
                    }
                });
                self.forget(key, &entry.token).await;
                Some(entry.token)
            }
            _ => None,
        };
        loop {
            let (ran, outcome) = self
                .flight
                .run(|| async {
                    let outcome = self
                        .acquire(key, issuer, audience, rejected.as_deref())
                        .await;
                    (key.to_owned(), outcome)
                })
                .await;
            if let Some(entry) = self
                .held(key)
                .filter(|entry| Some(&entry.token) != rejected.as_ref())
            {
                return Ok(Some(entry.token));
            }
            if ran == key {
                return outcome.map(|()| None);
            }
        }
    }

    fn held(&self, key: &str) -> Option<Held> {
        self.held.read(|held| held.get(key).cloned())
    }

    async fn acquire(
        &self,
        key: &str,
        issuer: &str,
        audience: &str,
        rejected: Option<&str>,
    ) -> Result<(), String> {
        if let Some(entry) = self.recall(key, rejected).await {
            self.held
                .update(|held| held.insert(key.to_owned(), entry.clone()));
            return Ok(());
        }
        let mut lent = Lent {
            inbox: self.inbox.take(),
            slot: &self.inbox,
        };
        let inbox = lent
            .inbox
            .as_mut()
            .ok_or("another email pass is being asked for")?;
        while inbox.try_recv().is_ok() {}
        let email = match (self.email.read(Clone::clone), self.asker) {
            (_, Asker::Nobody) | (None, Asker::Stdin) => return Err(UNASKED.to_owned()),
            (Some(email), Asker::Pass | Asker::Stdin) => email,
            (None, Asker::Pass) => {
                self.notice(format!(
                    "{audience} asks for an email pass from {issuer}: /pass <email> to get a code, or /pass alone to decline"
                ));
                loop {
                    match inbox.recv().await {
                        Some(Answer::Email(email)) => break email,
                        Some(Answer::Code(_)) => self.notice(
                            "an email pass needs an email address first: /pass <email>".to_owned(),
                        ),
                        Some(Answer::Decline) | None => return Ok(()),
                    }
                }
            }
        };
        let client = self.payments.client()?;
        let id = post(
            &client,
            issuer,
            OTP_PATH,
            serde_json::json!({ "email": email, "audience": audience }),
        )
        .await?;
        let id = id["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("the issuer returned a malformed code request")?
            .to_owned();
        let mut failure = String::new();
        for attempt in 1..=CODE_ATTEMPTS {
            self.notice(match self.asker {
                Asker::Stdin => format!(
                    "{issuer} sent a code to {email} for {audience}: type it and press Enter (attempt {attempt} of {CODE_ATTEMPTS}), or an empty line to decline{failure}"
                ),
                Asker::Pass | Asker::Nobody => format!(
                    "{issuer} sent a code to {email} for {audience}: /pass <code> (attempt {attempt} of {CODE_ATTEMPTS}), or /pass alone to decline{failure}"
                ),
            });
            let code = loop {
                match inbox.recv().await {
                    Some(Answer::Code(code)) => break code,
                    Some(Answer::Email(_)) => self.notice(format!(
                        "the code went to {email}: /pass <code>; the new address is used for the next email pass"
                    )),
                    Some(Answer::Decline) | None => return Ok(()),
                }
            };
            let verified = post(
                &client,
                issuer,
                VERIFY_PATH,
                serde_json::json!({ "id": id, "code": code }),
            )
            .await;
            let token = match verified {
                Ok(body) => body["attestation"]
                    .as_str()
                    .filter(|token| !token.is_empty())
                    .ok_or("the issuer returned a malformed attestation")?
                    .to_owned(),
                Err(error) if error.starts_with("400 ") => {
                    failure = format!("; {}", &error[4..]);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let entry = Held::decode(&token, Some(now_seconds()))?;
            if entry.claims.key() != key {
                return Err("the issuer returned an attestation for another audience".to_owned());
            }
            self.held
                .update(|held| held.insert(key.to_owned(), entry.clone()));
            match self.remember(key, &entry, &email).await {
                Ok(()) => self.notice(format!("email pass for {audience} verified")),
                Err(error) => self.notice(format!(
                    "email pass for {audience} verified, kept for this session only: {error}"
                )),
            }
            return Ok(());
        }
        Err(format!(
            "the code was rejected {CODE_ATTEMPTS} times{failure}"
        ))
    }

    async fn recall(&self, key: &str, rejected: Option<&str>) -> Option<Held> {
        let directory = self.passes.clone()?;
        let (issuers, email) = (self.issuers.clone(), self.email.read(Clone::clone));
        let path = pass_path(&directory, key);
        let entry = tokio::task::spawn_blocking(move || read_pass(&path))
            .await
            .ok()?
            .ok()?;
        let (entry, owner) = entry;
        let fresh = entry.usable(now_seconds(), false) && Some(entry.token.as_str()) != rejected;
        (fresh && admitted(&entry, &owner, &issuers, email.as_deref())).then_some(entry)
    }

    async fn remember(&self, key: &str, entry: &Held, email: &str) -> Result<(), String> {
        let Some(directory) = self.passes.clone() else {
            return Err("nanocode found no home directory to keep it in".to_owned());
        };
        let (key, entry, email) = (key.to_owned(), entry.clone(), email.to_owned());
        tokio::task::spawn_blocking(move || save(&directory, &key, &entry, &email))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
    }

    async fn forget(&self, key: &str, token: &str) {
        let Some(directory) = self.passes.clone() else {
            return;
        };
        let (key, token) = (key.to_owned(), token.to_owned());
        let forgotten = tokio::task::spawn_blocking(move || forget(&directory, &key, &token)).await;
        if let Ok(Err(error)) = forgotten {
            tracing::warn!(target: "nanocode", error = %error, "a refused email pass stays on disk");
        }
    }
}

fn passes_directory() -> Option<PathBuf> {
    super::store::default_channel_database_path()
        .ok()
        .map(|path| path.with_file_name(PASSES_DIRECTORY))
}

pub(crate) fn pass_path(directory: &Path, key: &str) -> PathBuf {
    let name: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    directory.join(format!("{name}.{PASS_EXTENSION}"))
}

fn admitted(entry: &Held, owner: &str, issuers: &[String], email: Option<&str>) -> bool {
    entry.claims.kind == EMAIL
        && issuers.contains(&entry.claims.issuer)
        && email.is_none_or(|email| email == owner)
}

fn read_pass(path: &Path) -> Result<(Held, String), String> {
    let bytes = fs::read(path).map_err(|error| error.kind().to_string())?;
    let stored: Stored =
        serde_json::from_slice(&bytes).map_err(|_| "the stored email pass is malformed")?;
    let entry = Held::decode(&stored.token, Some(stored.received))?;
    let directory = path
        .parent()
        .ok_or("the stored email pass has no directory")?;
    if pass_path(directory, &entry.claims.key()) != path {
        return Err("the stored email pass names another audience or issuer".to_owned());
    }
    Ok((entry, stored.email))
}

pub(crate) fn load(
    directory: &Path,
    issuers: &[String],
    email: Option<&str>,
    now: i64,
) -> Vec<Held> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut held = Vec::new();
    for path in entries.filter_map(|entry| Some(entry.ok()?.path())) {
        if path
            .extension()
            .is_none_or(|extension| extension != PASS_EXTENSION)
        {
            continue;
        }
        let (entry, owner) = match read_pass(&path) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(target: "nanocode", path = %path.display(), error = %error, "skipped a stored email pass");
                continue;
            }
        };
        if !entry.usable(now, true) {
            if let Err(error) = fs::remove_file(&path) {
                tracing::warn!(target: "nanocode", path = %path.display(), error = %error, "an expired email pass stays on disk");
            }
            continue;
        }
        if admitted(&entry, &owner, issuers, email) {
            held.push(entry);
        }
    }
    held
}

pub(crate) fn save(directory: &Path, key: &str, entry: &Held, email: &str) -> std::io::Result<()> {
    assert_eq!(entry.claims.key(), key);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(PRIVATE_DIRECTORY_MODE);
    }
    builder.create(directory)?;
    let bytes = serde_json::to_vec(&Stored {
        token: entry.token.clone(),
        received: entry.received,
        email: email.to_owned(),
    })?;
    let path = pass_path(directory, key);
    let temporary = path.with_extension(format!("{}.{TEMPORARY_EXTENSION}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_FILE_MODE);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &path)
}

pub(crate) fn forget(directory: &Path, key: &str, token: &str) -> std::io::Result<()> {
    let path = pass_path(directory, key);
    match read_pass(&path) {
        Ok((entry, _)) if entry.token == token => fs::remove_file(&path),
        _ => Ok(()),
    }
}

async fn post(
    client: &reqwest::Client,
    issuer: &str,
    path: &str,
    body: Value,
) -> Result<Value, String> {
    let url = Url::parse(&format!("{issuer}{path}")).map_err(|error| error.to_string())?;
    if !secure(&url) {
        return Err("the issuer must be an https URL (http only on localhost)".to_owned());
    }
    let response = client
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    match status.is_success() {
        true => Ok(body),
        false => Err(format!(
            "{} {}",
            status.as_u16(),
            body["error"]
                .as_str()
                .map_or_else(|| format!("the issuer answered {status}"), str::to_owned)
        )),
    }
}

pub(crate) struct Proved<'a, X> {
    pub(crate) exchange: X,
    pub(crate) identity: Option<&'a Identity>,
}

impl<X: Exchange> Exchange for Proved<'_, X> {
    type Error = X::Error;

    fn check_challenge(&self, challenge: &Challenge) -> Result<(), HttpFault> {
        self.exchange.check_challenge(challenge)
    }

    fn http_client(&self) -> Option<reqwest::Client> {
        self.exchange.http_client()
    }

    async fn send(&mut self, mut request: Request) -> Result<Response, X::Error> {
        let Some(identity) = self.identity else {
            return self.exchange.send(request).await;
        };
        identity.prove(&mut request);
        let resend = request.try_clone();
        let response = self.exchange.send(request).await?;
        let Some(mut resend) = resend else {
            return Ok(response);
        };
        let Some(rejected) = identity.rejected(&response, &resend) else {
            return Ok(response);
        };
        match identity.retry(rejected).await {
            Some(value) => {
                resend.headers_mut().insert(ATTESTATION_HEADER, value);
                self.exchange.send(resend).await
            }
            None => Ok(response),
        }
    }
}

fn key(audience: &str, issuer: &str, kind: &str) -> String {
    format!("{audience} {issuer} {kind}")
}

fn paid(headers: &HeaderMap) -> bool {
    headers.values().any(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.get(..8))
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("payment "))
    })
}

pub(crate) fn secure(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
        _ => false,
    }
}

fn secure_text(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| secure(&url))
}

pub(crate) fn tokens(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(crate) fn claims(token: &str) -> Option<Claims> {
    if token.len() > MAX_LENGTH {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let payload: Value = serde_json::from_slice(&decode_url(payload).ok()?).ok()?;
    Some(Claims {
        audience: payload["aud"].as_str()?.to_owned(),
        issuer: payload["iss"].as_str()?.to_owned(),
        kind: payload["type"].as_str()?.to_owned(),
        issued: payload["iat"].as_i64()?,
        expires: payload["exp"].as_i64()?,
    })
}

pub(crate) fn requirements(value: &str) -> Vec<Vec<Requirement>> {
    let Some(members) = Fields::new(value).list() else {
        return Vec::new();
    };
    members
        .into_iter()
        .map(|items| {
            items
                .into_iter()
                .filter_map(|(item, issuer)| match (item, issuer) {
                    (Bare::Token(kind), None) => Some(Requirement { kind, issuer: None }),
                    (Bare::Token(kind), Some(Bare::String(issuer))) if secure_text(&issuer) => {
                        Some(Requirement {
                            kind,
                            issuer: Some(issuer),
                        })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
enum Bare {
    Token(String),
    String(String),
    Other,
}

type Item = (Bare, Option<Bare>);

struct Fields<'a> {
    input: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    const fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            at: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    fn skip(&mut self, spaces: &[u8]) {
        while self.peek().is_some_and(|byte| spaces.contains(&byte)) {
            self.at += 1;
        }
    }

    fn list(mut self) -> Option<Vec<Vec<Item>>> {
        let mut members = Vec::new();
        self.skip(b" ");
        if self.peek().is_none() {
            return Some(members);
        }
        loop {
            members.push(match self.peek()? {
                b'(' => self.inner()?,
                _ => vec![self.item()?],
            });
            self.skip(b" \t");
            match self.peek() {
                None => return Some(members),
                Some(b',') => self.at += 1,
                Some(_) => return None,
            }
            self.skip(b" \t");
            self.peek()?;
        }
    }

    fn inner(&mut self) -> Option<Vec<Item>> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip(b" ");
            if self.peek()? == b')' {
                self.at += 1;
                self.parameters()?;
                return Some(items);
            }
            items.push(self.item()?);
            if !matches!(self.peek()?, b' ' | b')') {
                return None;
            }
        }
    }

    fn item(&mut self) -> Option<Item> {
        let bare = self.bare()?;
        Some((bare, self.parameters()?))
    }

    fn parameters(&mut self) -> Option<Option<Bare>> {
        let mut issuer = None;
        while self.peek() == Some(b';') {
            self.at += 1;
            self.skip(b" ");
            let key = self.key()?;
            let value = match self.peek() {
                Some(b'=') => {
                    self.at += 1;
                    self.bare()?
                }
                _ => Bare::Other,
            };
            if key == "issuer" {
                issuer = Some(value);
            }
        }
        Some(issuer)
    }

    fn key(&mut self) -> Option<&'a str> {
        let start = self.at;
        if !self
            .peek()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte == b'*')
        {
            return None;
        }
        while self.peek().is_some_and(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.*".contains(&byte)
        }) {
            self.at += 1;
        }
        std::str::from_utf8(&self.input[start..self.at]).ok()
    }

    fn bare(&mut self) -> Option<Bare> {
        match self.peek()? {
            b'"' => self.string(),
            byte if byte.is_ascii_alphabetic() || byte == b'*' => self.token(),
            byte if byte.is_ascii_digit() || byte == b'-' => {
                self.at += 1;
                self.skip(b"0123456789.");
                Some(Bare::Other)
            }
            b'?' => {
                self.at += 1;
                matches!(self.peek()?, b'0' | b'1').then(|| self.at += 1)?;
                Some(Bare::Other)
            }
            b':' => {
                self.at += 1;
                while self.peek()? != b':' {
                    let byte = self.peek()?;
                    if !(byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)) {
                        return None;
                    }
                    self.at += 1;
                }
                self.at += 1;
                Some(Bare::Other)
            }
            _ => None,
        }
    }

    fn token(&mut self) -> Option<Bare> {
        let start = self.at;
        while self.peek().is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~:/".contains(&byte)
        }) {
            self.at += 1;
        }
        Some(Bare::Token(
            std::str::from_utf8(&self.input[start..self.at])
                .ok()?
                .to_owned(),
        ))
    }

    fn string(&mut self) -> Option<Bare> {
        self.at += 1;
        let mut text = String::new();
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(Bare::String(text));
                }
                b'\\' => {
                    self.at += 1;
                    let escaped = self.peek()?;
                    if escaped != b'"' && escaped != b'\\' {
                        return None;
                    }
                    text.push(char::from(escaped));
                }
                byte if (0x20..0x7f).contains(&byte) => text.push(char::from(byte)),
                _ => return None,
            }
            self.at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jws(payload: &str) -> String {
        let encode = |bytes: &[u8]| nanocode_tempo::base64::encode_url(bytes);
        format!(
            "{}.{}.{}",
            encode(br#"{"alg":"EdDSA","typ":"mpp-attestation+jwt"}"#),
            encode(payload.as_bytes()),
            encode(b"signature")
        )
    }

    fn attestation(audience: &str, issuer: &str, iat: i64, exp: i64) -> String {
        jws(&format!(
            r#"{{"iss":"{issuer}","aud":"{audience}","sub":"a@example.com","type":"email","iat":{iat},"exp":{exp},"jti":"j"}}"#
        ))
    }

    fn email(issuer: &str) -> Requirement {
        Requirement {
            kind: "email".to_owned(),
            issuer: Some(issuer.to_owned()),
        }
    }

    #[test]
    fn parses_a_single_requirement() {
        assert_eq!(
            requirements(r#"email;issuer="https://identity.mpp.tempo.xyz""#),
            vec![vec![email("https://identity.mpp.tempo.xyz")]]
        );
    }

    #[test]
    fn parses_alternatives_and_groups() {
        assert_eq!(
            requirements(
                r#"(email;issuer="https://a.example" email;issuer="https://b.example"), phone;issuer="https://c.example""#
            ),
            vec![
                vec![email("https://a.example"), email("https://b.example")],
                vec![Requirement {
                    kind: "phone".to_owned(),
                    issuer: Some("https://c.example".to_owned()),
                }],
            ]
        );
    }

    #[test]
    fn skips_insecure_or_malformed_members() {
        assert_eq!(
            requirements(
                r#"email;issuer="http://evil.example", email;issuer=1, email;issuer="http://localhost:8787""#
            ),
            vec![vec![email("http://localhost:8787")]]
        );
        assert!(requirements(r#"email;issuer="https://a.example","#).is_empty());
        assert!(requirements(r#"email;issuer="unterminated"#).is_empty());
        assert!(requirements("").is_empty());
    }

    #[test]
    fn keeps_other_parameters_and_escapes() {
        assert_eq!(
            requirements(r#"email;v=1;issuer="https://a.example";x=?1;y=:YWJj:"#),
            vec![vec![email("https://a.example")]]
        );
        assert_eq!(
            requirements("email"),
            vec![vec![Requirement {
                kind: "email".to_owned(),
                issuer: None,
            }]]
        );
    }

    #[test]
    fn decodes_claims() {
        let token = attestation("https://api.example", "https://issuer.example", 100, 200);
        assert_eq!(
            claims(&token),
            Some(Claims {
                audience: "https://api.example".to_owned(),
                issuer: "https://issuer.example".to_owned(),
                kind: "email".to_owned(),
                issued: 100,
                expires: 200,
            })
        );
        assert_eq!(claims("a.b"), None);
        assert_eq!(claims(&format!("{token}.x")), None);
        assert_eq!(claims(&"a".repeat(MAX_LENGTH + 1)), None);
    }

    #[test]
    fn usable_until_the_margin_unpaid_and_the_tolerance_paid() {
        let held =
            Held::decode(&attestation("https://a", "https://i", 0, 86_400), Some(0)).unwrap();
        assert!(held.usable(86_400 - 301, false));
        assert!(!held.usable(86_400 - 300, false));
        assert!(held.usable(86_400 - 300, true));
        assert!(held.usable(86_400 + 59, true));
        assert!(!held.usable(86_400 + 60, true));
        let short = Held::decode(&attestation("https://a", "https://i", 0, 100), Some(0)).unwrap();
        assert_eq!(short.margin(), 50);
    }

    #[test]
    fn only_https_or_localhost_is_secure() {
        for (url, secure_url) in [
            ("https://api.example", true),
            ("http://localhost:4000", true),
            ("http://127.0.0.1:4000", true),
            ("http://api.example", false),
            ("http://localhost.evil.example", false),
        ] {
            assert_eq!(secure(&Url::parse(url).unwrap()), secure_url, "{url}");
        }
    }

    #[test]
    fn a_payment_credential_marks_a_paid_attempt() {
        let mut headers = HeaderMap::new();
        assert!(!paid(&headers));
        headers.insert("authorization", HeaderValue::from_static("Bearer x"));
        assert!(!paid(&headers));
        headers.insert("authorization", HeaderValue::from_static("Payment eyJ"));
        assert!(paid(&headers));
    }

    #[test]
    fn answers() {
        assert!(matches!(Answer::parse(" /pass "), Answer::Decline));
        assert!(matches!(Answer::parse("a@b.c"), Answer::Email(email) if email == "a@b.c"));
        assert!(
            matches!(Answer::parse("/pass 12345678"), Answer::Code(code) if code == "12345678")
        );
    }

    const ISSUER: &str = "https://issuer.example";
    const AUDIENCE: &str = "https://api.example";

    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("nanocode-passes-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        directory
    }

    fn stored(directory: &Path, audience: &str, issued: i64, expires: i64, email: &str) -> Held {
        let entry = Held::decode(
            &attestation(audience, ISSUER, issued, expires),
            Some(issued),
        )
        .unwrap();
        save(directory, &entry.claims.key(), &entry, email).unwrap();
        entry
    }

    #[test]
    fn a_saved_pass_loads_with_its_received_time_and_private_modes() {
        let directory = directory("roundtrip");
        let now = now_seconds();
        let entry = stored(&directory, AUDIENCE, now, now + 3_600, "a@example.com");
        let loaded = load(&directory, &[ISSUER.to_owned()], None, now);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].token, entry.token);
        assert_eq!(loaded[0].received, now);
        assert_eq!(loaded[0].claims.audience, AUDIENCE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = pass_path(&directory, &entry.claims.key());
            let file = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            let parent = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
            assert_eq!(file, PRIVATE_FILE_MODE);
            assert_eq!(parent, PRIVATE_DIRECTORY_MODE);
        }
        let names: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn an_expired_pass_is_deleted_on_load() {
        let directory = directory("expired");
        let now = now_seconds();
        let entry = stored(
            &directory,
            AUDIENCE,
            now - 7_200,
            now - CLOCK_TOLERANCE - 1,
            "a@example.com",
        );
        assert!(load(&directory, &[ISSUER.to_owned()], None, now).is_empty());
        assert!(!pass_path(&directory, &entry.claims.key()).exists());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_pass_is_kept_per_audience_and_issuer() {
        let directory = directory("audience");
        let now = now_seconds();
        let first = stored(&directory, AUDIENCE, now, now + 3_600, "a@example.com");
        let second = stored(
            &directory,
            "https://other.example",
            now,
            now + 3_600,
            "a@example.com",
        );
        assert_ne!(
            pass_path(&directory, &first.claims.key()),
            pass_path(&directory, &second.claims.key())
        );
        let mut audiences: Vec<String> = load(&directory, &[ISSUER.to_owned()], None, now)
            .into_iter()
            .map(|entry| entry.claims.audience)
            .collect();
        audiences.sort();
        assert_eq!(audiences, ["https://api.example", "https://other.example"]);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_pass_from_an_untrusted_issuer_or_another_address_is_not_loaded() {
        let directory = directory("trust");
        let now = now_seconds();
        let entry = stored(&directory, AUDIENCE, now, now + 3_600, "a@example.com");
        assert!(load(&directory, &["https://else.example".to_owned()], None, now).is_empty());
        assert!(load(&directory, &[ISSUER.to_owned()], Some("b@example.com"), now).is_empty());
        assert_eq!(
            load(&directory, &[ISSUER.to_owned()], Some("a@example.com"), now).len(),
            1
        );
        assert!(pass_path(&directory, &entry.claims.key()).exists());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_file_under_another_key_is_skipped() {
        let directory = directory("renamed");
        let now = now_seconds();
        let entry = stored(&directory, AUDIENCE, now, now + 3_600, "a@example.com");
        fs::rename(
            pass_path(&directory, &entry.claims.key()),
            pass_path(&directory, &key("https://other.example", ISSUER, EMAIL)),
        )
        .unwrap();
        assert!(load(&directory, &[ISSUER.to_owned()], None, now).is_empty());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_refused_pass_is_forgotten_but_a_newer_one_is_kept() {
        let directory = directory("forget");
        let now = now_seconds();
        let old = Held::decode(
            &attestation(AUDIENCE, ISSUER, now - 10, now + 3_600),
            Some(now - 10),
        )
        .unwrap();
        let entry = stored(&directory, AUDIENCE, now, now + 3_600, "a@example.com");
        let path = pass_path(&directory, &entry.claims.key());
        forget(&directory, &entry.claims.key(), &old.token).unwrap();
        assert!(path.exists());
        forget(&directory, &entry.claims.key(), &entry.token).unwrap();
        assert!(!path.exists());
        forget(&directory, &entry.claims.key(), &entry.token).unwrap();
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn nothing_loads_from_a_missing_directory() {
        let directory = directory("missing");
        assert!(load(&directory, &[ISSUER.to_owned()], None, now_seconds()).is_empty());
    }
}

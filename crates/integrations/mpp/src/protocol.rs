use std::borrow::Cow;
use std::collections::HashMap;

use nanocode_tempo::base64;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::Fault;

pub const METHOD_NAME: &str = "tempo";
pub const INTENT_CHARGE: &str = "charge";
pub const INTENT_SESSION: &str = "session";
pub(crate) const PAYMENT_SCHEME: &str = "Payment";
pub(crate) const PAYMENT_AUTHORIZATION_HEADER: &str = "Payment-Authorization";
pub(crate) const PAYMENT_RECEIPT_HEADER: &str = "payment-receipt";
pub(crate) const ACCEPT_PAYMENT_HEADER: &str = "Accept-Payment";
const INVALID_METHOD_NAME: &str = "invalid method name: must start with a lowercase ASCII letter and contain only lowercase letters, digits, colons, underscores, or hyphens";
const MAX_TOKEN_LEN: usize = 16 * 1024;
const DIGEST_PREFIX: &str = "sha-256=";
const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_MINUTE: i64 = 60;
const LEAP_SECOND: u32 = 60;
const SURROGATES: std::ops::RangeInclusive<u16> = 0xD800..=0xDFFF;
const HIGH_SURROGATES: std::ops::RangeInclusive<u16> = 0xD800..=0xDBFF;
const LOW_SURROGATES: std::ops::RangeInclusive<u16> = 0xDC00..=0xDFFF;
const SUPPLEMENTARY_BASE: u32 = 0x1_0000;
const SURROGATE_BITS: u32 = 10;
const ESCAPE_DIGITS: usize = 4;
const Q_DIGITS: usize = 3;
const Q_SCALE: f32 = 1000.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Encoded(String);

impl Encoded {
    pub(crate) fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub(crate) fn raw(&self) -> &str {
        &self.0
    }

    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, Fault> {
        let bytes = decode_url(&self.0)?;
        serde_json::from_slice(&bytes).map_err(|error| {
            Fault::InvalidChallenge(Some(format!("Failed to decode base64url JSON: {error}")))
        })
    }
}

pub(crate) fn decode_url(input: &str) -> Result<Vec<u8>, Fault> {
    let normalized: String = input
        .chars()
        .filter(|character| *character != '=')
        .map(|character| match character {
            '+' => '-',
            '/' => '_',
            other => other,
        })
        .collect();
    base64::decode_url(&normalized)
        .map_err(|error| Fault::Config(format!("Invalid base64url: {error}")))
}

#[derive(Debug, Clone)]
pub struct Challenge {
    pub(crate) id: String,
    pub(crate) realm: String,
    pub(crate) method: String,
    pub intent: String,
    pub request: Encoded,
    pub(crate) expires: Option<String>,
    pub description: Option<String>,
    pub(crate) digest: Option<String>,
    pub(crate) opaque: Option<Encoded>,
    pub(crate) header: Option<String>,
}

#[derive(Deserialize)]
struct WireChallenge {
    id: String,
    realm: String,
    method: String,
    intent: String,
    request: Encoded,
    expires: Option<String>,
    description: Option<String>,
    digest: Option<String>,
    opaque: Option<Encoded>,
    header: Option<String>,
}

impl<'de> Deserialize<'de> for Challenge {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireChallenge::deserialize(deserializer)?;
        if !valid_method(&wire.method) {
            return Err(serde::de::Error::custom(INVALID_METHOD_NAME));
        }
        validate_fields(
            &wire.id,
            &wire.intent,
            wire.request.raw(),
            wire.expires.as_deref(),
            wire.digest.as_deref(),
            wire.opaque.as_ref().map(Encoded::raw),
        )
        .and_then(|()| advertised_header(wire.header.as_deref()))
        .map(|header| Self {
            id: wire.id,
            realm: wire.realm,
            method: wire.method,
            intent: wire.intent,
            request: wire.request,
            expires: wire.expires,
            description: wire.description,
            digest: wire.digest,
            opaque: wire.opaque,
            header,
        })
        .map_err(serde::de::Error::custom)
    }
}

impl Challenge {
    pub(crate) fn credential_header(&self) -> &str {
        match self.header.as_deref() {
            Some(header) if header.eq_ignore_ascii_case(PAYMENT_AUTHORIZATION_HEADER) => header,
            _ => "Authorization",
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.expires
            .as_deref()
            .is_some_and(|expires| unix_seconds(expires).is_none_or(|at| at <= now_seconds()))
    }

    pub(crate) fn expires_at(&self) -> Option<i64> {
        unix_seconds(self.expires.as_deref()?)
    }

    pub(crate) fn to_echo(&self) -> ChallengeEcho {
        ChallengeEcho {
            id: self.id.clone(),
            realm: self.realm.clone(),
            method: self.method.clone(),
            intent: self.intent.clone(),
            request: self.request.clone(),
            expires: self.expires.clone(),
            description: self.description.clone(),
            digest: self.digest.clone(),
            opaque: self.opaque.clone(),
            header: self.header.clone(),
        }
    }

    pub(crate) fn request_value(&self) -> Option<Value> {
        self.request.decode::<Value>().ok()
    }

    pub fn amount(&self) -> Option<u128> {
        self.request_value()?.get("amount")?.as_str()?.parse().ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChallengeEcho {
    pub(crate) id: String,
    pub(crate) realm: String,
    pub(crate) method: String,
    pub(crate) intent: String,
    pub(crate) request: Encoded,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) expires: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) opaque: Option<Encoded>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) header: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub(crate) challenge: ChallengeEcho,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    pub(crate) payload: Value,
}

impl Credential {
    pub(crate) const fn with_source(
        challenge: ChallengeEcho,
        source: String,
        payload: Value,
    ) -> Self {
        Self {
            challenge,
            source: Some(source),
            payload,
        }
    }

    pub(crate) fn did(chain_id: u64, address: &str) -> String {
        format!("did:pkh:eip155:{chain_id}:{address}")
    }

    pub(crate) fn payload_str(&self, field: &str) -> Option<&str> {
        self.payload.get(field)?.as_str()
    }

    pub(crate) fn is_hash(&self) -> bool {
        self.payload_str("type") == Some("hash") && self.payload_str("hash").is_some()
    }

    pub(crate) fn authorization(&self) -> Result<String, Fault> {
        let json = serde_json::to_string(self)?;
        Ok(format!(
            "{PAYMENT_SCHEME} {}",
            base64::encode_url(json.as_bytes())
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub(crate) status: String,
    pub(crate) method: String,
    pub(crate) timestamp: String,
    pub reference: String,
    #[serde(rename = "externalId", skip_serializing_if = "Option::is_none")]
    pub(crate) external_id: Option<String>,
    #[serde(rename = "subscriptionId", skip_serializing_if = "Option::is_none")]
    pub(crate) subscription_id: Option<String>,
    #[serde(
        rename = "fundingCurrency",
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) funding_currency: Option<String>,
    #[serde(flatten)]
    pub(crate) extensions: serde_json::Map<String, Value>,
}

fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReceiptStatus {
    Success,
}

impl Receipt {
    pub(crate) fn is_success(&self) -> bool {
        self.status == "success"
    }

    pub(crate) fn parse(header: &str) -> Result<Self, Fault> {
        let decoded = decode_url(header.trim())?;
        let receipt: Self = serde_json::from_slice(&decoded)
            .map_err(|error| Fault::InvalidReceipt(format!("Invalid receipt JSON: {error}")))?;
        serde_json::from_value::<ReceiptStatus>(Value::from(receipt.status.clone()))
            .map_err(|error| Fault::InvalidReceipt(format!("Invalid receipt JSON: {error}")))?;
        if unix_seconds(&receipt.timestamp).is_none() {
            return Err(Fault::InvalidReceipt(
                "Invalid timestamp format: expected ISO 8601".to_owned(),
            ));
        }
        Ok(receipt)
    }

    pub fn from_value(value: Value) -> Option<Self> {
        let receipt: Self = serde_json::from_value(value).ok()?;
        serde_json::from_value::<ReceiptStatus>(Value::from(receipt.status.clone())).ok()?;
        Some(receipt)
    }

    pub fn extension(&self, key: &str) -> Option<&str> {
        self.extensions.get(key)?.as_str()
    }
}

pub(crate) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn digits(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

const fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(crate) fn unix_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let separator = *bytes.get(10)?;
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(separator, b'T' | b't' | b' ')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let year = i64::from(digits(text.get(0..4)?)?);
    let month = digits(text.get(5..7)?)?;
    let day = digits(text.get(8..10)?)?;
    let hour = digits(text.get(11..13)?)?;
    let minute = digits(text.get(14..16)?)?;
    let second = digits(text.get(17..19)?)?;
    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > LEAP_SECOND
    {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let length = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if length == 0 {
            return None;
        }
        rest = &fraction[length..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours = digits(&rest[1..3])?;
            let minutes = digits(&rest[4..6])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (i64::from(hours) * SECONDS_PER_HOUR + i64::from(minutes) * SECONDS_PER_MINUTE)
        }
    };
    let second = second.min(LEAP_SECOND - 1);
    Some(
        days_from_civil(year, i64::from(month), i64::from(day)) * SECONDS_PER_DAY
            + i64::from(hour) * SECONDS_PER_HOUR
            + i64::from(minute) * SECONDS_PER_MINUTE
            + i64::from(second)
            - offset,
    )
}

fn invalid(reason: impl Into<String>) -> Fault {
    Fault::InvalidChallenge(Some(reason.into()))
}

fn advertised_header(header: Option<&str>) -> Result<Option<String>, Fault> {
    match header {
        None => Ok(None),
        Some(name) if name.is_empty() || name.eq_ignore_ascii_case("Authorization") => Ok(None),
        Some(name) if name.eq_ignore_ascii_case(PAYMENT_AUTHORIZATION_HEADER) => {
            Ok(Some(name.to_owned()))
        }
        Some(_) => Err(invalid(
            "Unsupported credential header: must be Payment-Authorization",
        )),
    }
}

fn valid_digest(digest: &str) -> bool {
    let Some(value) = digest.strip_prefix(DIGEST_PREFIX) else {
        return false;
    };
    let value = value
        .strip_prefix(':')
        .and_then(|value| value.strip_suffix(':'))
        .unwrap_or(value);
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_' | b'=')
        })
}

fn valid_method(method: &str) -> bool {
    let mut characters = method.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, ':' | '_' | '-')
        })
}

fn validate_fields(
    id: &str,
    intent: &str,
    request: &str,
    expires: Option<&str>,
    digest: Option<&str>,
    opaque: Option<&str>,
) -> Result<(), Fault> {
    if id.is_empty() {
        return Err(invalid("Empty 'id' parameter"));
    }
    if intent.is_empty() || intent.contains('|') {
        return Err(invalid(format!("Invalid intent: \"{intent}\".")));
    }
    let request = decode_url(request)?;
    serde_json::from_slice::<serde_json::Map<String, Value>>(&request)
        .map_err(|error| invalid(format!("Invalid JSON in request field: {error}")))?;
    if digest.is_some_and(|digest| !valid_digest(digest)) {
        return Err(invalid("Invalid digest format"));
    }
    if expires.is_some_and(|expires| unix_seconds(expires).is_none()) {
        return Err(invalid("Invalid expires timestamp"));
    }
    if opaque.is_some_and(|opaque| decode_url(opaque).is_err()) {
        return Err(invalid("Invalid opaque: expected base64url"));
    }
    Ok(())
}

enum Token<'a> {
    Scheme(&'a str),
    Param { name: &'a str, value: Cow<'a, str> },
    Word,
}

impl Token<'_> {
    const fn is_payment_scheme(&self) -> bool {
        matches!(self, Self::Scheme(scheme) if scheme.eq_ignore_ascii_case(PAYMENT_SCHEME))
    }
}

struct Tokenizer<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> Tokenizer<'a> {
    const fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn param(&mut self) -> Result<Option<(&'a str, Cow<'a, str>)>, Fault> {
        let position = self.position;
        match self.next().transpose()? {
            Some(Token::Param { name, value }) => Ok(Some((name, value))),
            _ => {
                self.position = position;
                Ok(None)
            }
        }
    }

    fn seek(&self, from: usize, stop: impl Fn(u8) -> bool) -> usize {
        self.input.as_bytes()[from..]
            .iter()
            .position(|byte| stop(*byte))
            .map_or(self.input.len(), |offset| from + offset)
    }

    fn value(&mut self) -> Result<Cow<'a, str>, Fault> {
        let bytes = self.input.as_bytes();
        let start = self.seek(self.position, |byte| !byte.is_ascii_whitespace());
        if bytes.get(start) != Some(&b'"') {
            self.position = self.seek(start, |byte| byte == b',' || byte.is_ascii_whitespace());
            return Ok(Cow::Borrowed(&self.input[start..self.position]));
        }
        let start = start + 1;
        let mut unescaped = String::new();
        let mut segment = start;
        let mut index = start;
        loop {
            match bytes.get(index) {
                Some(b'"') => break,
                Some(b'\\') => {
                    unescaped.push_str(&self.input[segment..index]);
                    index += 1;
                    if let Some((decoded, next)) = unicode_escape(self.input, index) {
                        unescaped.push(decoded);
                        index = next;
                    } else if let Some(escaped) = self.input[index..].chars().next() {
                        unescaped.push(escaped);
                        index += escaped.len_utf8();
                    }
                    segment = index;
                }
                Some(_) => index += 1,
                None => {
                    self.position = bytes.len();
                    return Err(invalid("Unterminated quoted-string"));
                }
            }
        }
        self.position = index + 1;
        let tail = &self.input[segment..index];
        if segment == start {
            return Ok(Cow::Borrowed(tail));
        }
        unescaped.push_str(tail);
        Ok(Cow::Owned(unescaped))
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Result<Token<'a>, Fault>;

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.input.as_bytes();
        let first = self.position == 0;
        let mut element_start = first;
        while let Some(&byte) = bytes.get(self.position) {
            if byte == b',' {
                element_start = true;
            } else if !byte.is_ascii_whitespace() {
                break;
            }
            self.position += 1;
        }
        if self.position == bytes.len() {
            return None;
        }
        let start = self.position;
        let separator = |at: usize| matches!(bytes.get(at), Some(b' ' | b'\t'));
        let end = self.seek(start, |byte| byte == b'=' || byte.is_ascii_whitespace());
        let equals = self.seek(end, |byte| !byte.is_ascii_whitespace());
        if bytes.get(equals) == Some(&b'=') && !(first && separator(end)) {
            let name = &self.input[start..end];
            self.position = equals + 1;
            return Some(self.value().map(|value| Token::Param { name, value }));
        }
        let end = bytes[start..end]
            .iter()
            .position(|byte| *byte == b',')
            .map_or(end, |offset| start + offset);
        let word = &self.input[start..end];
        self.position = end;
        Some(Ok(match element_start && separator(end) {
            true => Token::Scheme(word),
            false => Token::Word,
        }))
    }
}

fn unicode_escape(input: &str, at: usize) -> Option<(char, usize)> {
    let (unit, next) = escaped_unit(input, at)?;
    if !SURROGATES.contains(&unit) {
        return char::from_u32(u32::from(unit)).map(|decoded| (decoded, next));
    }
    if HIGH_SURROGATES.contains(&unit)
        && input.as_bytes().get(next) == Some(&b'\\')
        && let Some((low, after)) = escaped_unit(input, next + 1)
        && LOW_SURROGATES.contains(&low)
    {
        let code = SUPPLEMENTARY_BASE
            + ((u32::from(unit) - u32::from(*HIGH_SURROGATES.start())) << SURROGATE_BITS)
            + (u32::from(low) - u32::from(*LOW_SURROGATES.start()));
        return char::from_u32(code).map(|decoded| (decoded, after));
    }
    Some((char::REPLACEMENT_CHARACTER, next))
}

fn escaped_unit(input: &str, at: usize) -> Option<(u16, usize)> {
    if input.as_bytes().get(at) != Some(&b'u') {
        return None;
    }
    let digits = input.get(at + 1..at + 1 + ESCAPE_DIGITS)?;
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(digits, 16)
        .ok()
        .map(|unit| (unit, at + 1 + ESCAPE_DIGITS))
}

fn read_challenge(tokens: &mut Tokenizer<'_>) -> Result<Challenge, Fault> {
    let mut params = HashMap::new();
    while let Some((name, value)) = tokens.param()? {
        let key = name.to_ascii_lowercase();
        if key == "request" && value.len() > MAX_TOKEN_LEN {
            return Err(invalid(format!(
                "Request parameter exceeds maximum length of {MAX_TOKEN_LEN} bytes"
            )));
        }
        if params.contains_key(&key) {
            return Err(invalid(format!("Duplicate parameter: {name}")));
        }
        params.insert(key, value.into_owned());
    }
    let required = |key: &str| {
        params
            .get(key)
            .cloned()
            .ok_or_else(|| invalid(format!("Missing '{key}' field")))
    };
    let id = required("id")?;
    let realm = required("realm")?;
    let method = required("method")?;
    if !valid_method(&method) {
        return Err(invalid(format!(
            "Invalid method: \"{method}\". Must match method-name ABNF."
        )));
    }
    let intent = required("intent")?;
    let request = Encoded::from_raw(required("request")?);
    let digest = params.get("digest").cloned();
    let expires = params.get("expires").cloned();
    let opaque = params.get("opaque").cloned().map(Encoded::from_raw);
    validate_fields(
        &id,
        &intent,
        request.raw(),
        expires.as_deref(),
        digest.as_deref(),
        opaque.as_ref().map(Encoded::raw),
    )?;
    Ok(Challenge {
        id,
        realm,
        method,
        intent,
        request,
        expires,
        description: params.get("description").cloned(),
        digest,
        opaque,
        header: advertised_header(params.get("header").map(String::as_str))?,
    })
}

fn latin1(value: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(value) {
        Ok(ascii) if ascii.is_ascii() => Cow::Borrowed(ascii),
        _ => Cow::Owned(value.iter().copied().map(char::from).collect()),
    }
}

pub(crate) fn challenges<'a>(values: impl IntoIterator<Item = &'a [u8]>) -> Vec<Challenge> {
    let mut found = Vec::new();
    for value in values {
        let text = latin1(value);
        let mut tokens = Tokenizer::new(&text);
        while let Some(Ok(token)) = tokens.next() {
            if token.is_payment_scheme() {
                found.extend(read_challenge(&mut tokens).ok());
            }
        }
    }
    found
}

pub(crate) fn response_challenges(headers: &nanocode_http::header::HeaderMap) -> Vec<Challenge> {
    challenges(
        headers
            .get_all(nanocode_http::header::WWW_AUTHENTICATE)
            .iter()
            .map(nanocode_http::header::HeaderValue::as_bytes),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Preference {
    pub(crate) method: String,
    pub(crate) intent: String,
    pub(crate) q: f32,
    pub(crate) index: usize,
}

fn preference_token(token: &str, method: bool) -> bool {
    token == "*"
        || token.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '-'
                || (method && matches!(character, ':' | '_'))
        })
}

fn q_value(value: &str) -> Option<f32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let valid = fraction.len() <= Q_DIGITS
        && match whole {
            "0" => fraction.bytes().all(|byte| byte.is_ascii_digit()),
            "1" => fraction.bytes().all(|byte| byte == b'0'),
            _ => false,
        };
    if !valid {
        return None;
    }
    if whole == "1" {
        return Some(1.0);
    }
    let thousandths = (0..Q_DIGITS).fold(0u16, |total, index| {
        total * 10 + u16::from(fraction.as_bytes().get(index).map_or(0, |byte| byte - b'0'))
    });
    Some(f32::from(thousandths) / Q_SCALE)
}

fn preference(part: &str, index: usize) -> Option<Preference> {
    let (token, params) = match part.find(';') {
        Some(at) => (part[..at].trim(), Some(part[at + 1..].trim())),
        None => (part.trim(), None),
    };
    let (method, intent) = token.split_once('/')?;
    if method.is_empty()
        || intent.is_empty()
        || !preference_token(method, true)
        || !preference_token(intent, false)
    {
        return None;
    }
    let mut q = 1.0;
    for param in params.into_iter().flat_map(|params| params.split(';')) {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        let (name, value) = param
            .split_once('=')
            .map(|(name, value)| (name.trim(), value.trim()))
            .filter(|(name, value)| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    && !value.is_empty()
                    && !value.contains(char::is_whitespace)
            })?;
        if name.eq_ignore_ascii_case("q") {
            q = q_value(value)?;
        }
    }
    Some(Preference {
        method: method.to_owned(),
        intent: intent.to_owned(),
        q,
        index,
    })
}

pub(crate) fn preferences(header: &str) -> Option<Vec<Preference>> {
    let parts: Vec<&str> = header
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts
        .iter()
        .enumerate()
        .map(|(index, part)| preference(part, index))
        .collect()
}

fn specificity(entry: &Preference) -> u8 {
    u8::from(entry.method != "*") + u8::from(entry.intent != "*")
}

fn best(challenge: &Challenge, preferences: &[Preference]) -> Option<(f32, u8, usize)> {
    let mut best: Option<(f32, u8, usize)> = None;
    for entry in preferences {
        if !((entry.method == "*" || entry.method == challenge.method)
            && (entry.intent == "*" || entry.intent == challenge.intent))
        {
            continue;
        }
        let candidate = (entry.q, specificity(entry), entry.index);
        let wins = match best {
            None => true,
            Some((q, specific, index)) => {
                candidate.1 > specific
                    || (candidate.1 == specific && candidate.0 > q)
                    || (candidate.1 == specific
                        && (candidate.0 - q).abs() < f32::EPSILON
                        && candidate.2 < index)
            }
        };
        if wins {
            best = Some(candidate);
        }
    }
    best
}

pub(crate) fn rank<'a>(
    challenges: &[&'a Challenge],
    preferences: &[Preference],
) -> Vec<&'a Challenge> {
    let mut scored: Vec<(usize, f32, &Challenge)> = challenges
        .iter()
        .enumerate()
        .filter_map(|(index, challenge)| {
            let (q, _, _) = best(challenge, preferences)?;
            (q > 0.0).then_some((index, q, *challenge))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored
        .into_iter()
        .map(|(_, _, challenge)| challenge)
        .collect()
}

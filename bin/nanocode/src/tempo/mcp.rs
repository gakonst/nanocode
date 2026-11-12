use std::cmp::Ordering;

use nanocode_tempo::base64;
use serde_json::Value;

use super::error::Fault;
use super::flow::{self, Selection};
use super::protocol::{self, Challenge, Credential, Encoded, Receipt};
use super::provider::{ManagementTransport, PaymentContext, PaymentProvider, PendingPayment};

const ACCEPT_PAYMENT: &str = "accept-payment";
const SCIENTIFIC_UPPER: i32 = 21;
const SCIENTIFIC_LOWER: i32 = -6;

pub(crate) fn extract_challenges_from_data(payment_required: &Value) -> Option<Vec<Challenge>> {
    let challenges: Vec<Challenge> = payment_required
        .get("challenges")?
        .as_array()?
        .iter()
        .cloned()
        .filter_map(wire_challenge)
        .collect();
    (!challenges.is_empty()).then_some(challenges)
}

fn wire_challenge(mut challenge: Value) -> Option<Challenge> {
    let object = challenge.as_object_mut()?;
    let request = object.get("request")?;
    if !request.is_string() {
        let request = canonical_encoded(request);
        object.insert("request".to_owned(), Value::String(request));
    }
    if !object.contains_key("opaque")
        && let Some(meta) = object.get("meta")
    {
        let opaque = canonical_encoded(meta);
        object.insert("opaque".to_owned(), Value::String(opaque));
    }
    serde_json::from_value(challenge).ok()
}

fn decoded(encoded: &Encoded) -> Result<Value, Fault> {
    let bytes = protocol::decode_url(encoded.raw())?;
    serde_json::from_slice(&bytes).map_err(|error| {
        Fault::InvalidChallenge(Some(format!("Invalid JSON in base64url: {error}")))
    })
}

pub(crate) fn credential_value(credential: &Credential) -> Result<Value, Fault> {
    let mut value =
        serde_json::to_value(credential).map_err(|error| Fault::Config(error.to_string()))?;
    let challenge = value
        .get_mut("challenge")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Fault::config("credential challenge is invalid"))?;
    challenge.insert(
        "request".to_owned(),
        decoded(&credential.challenge.request)?,
    );
    if let Some(opaque) = &credential.challenge.opaque {
        challenge.insert("meta".to_owned(), decoded(opaque)?);
    }
    Ok(value)
}

fn canonical_encoded(value: &Value) -> String {
    let mut text = String::new();
    canonical(value, &mut text);
    base64::encode_url(text.as_bytes())
}

fn utf16_order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn canonical_number(number: &serde_json::Number, text: &mut String) {
    let Some(float) = number.as_f64().filter(|float| float.is_finite()) else {
        text.push_str(&number.to_string());
        return;
    };
    if float == 0.0 {
        text.push('0');
        return;
    }
    let scientific = format!("{:e}", float.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or_default();
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let count = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    if float < 0.0 {
        text.push('-');
    }
    let point = exponent + 1;
    if (count..=SCIENTIFIC_UPPER).contains(&point) {
        text.push_str(&digits);
        text.extend(std::iter::repeat_n(
            '0',
            usize::try_from(point - count).unwrap_or(0),
        ));
    } else if 0 < point && point <= SCIENTIFIC_UPPER {
        let split = usize::try_from(point).unwrap_or(0);
        text.push_str(&digits[..split]);
        text.push('.');
        text.push_str(&digits[split..]);
    } else if SCIENTIFIC_LOWER < point && point <= 0 {
        text.push_str("0.");
        text.extend(std::iter::repeat_n(
            '0',
            usize::try_from(-point).unwrap_or(0),
        ));
        text.push_str(&digits);
    } else {
        text.push_str(&digits[..1]);
        if digits.len() > 1 {
            text.push('.');
            text.push_str(&digits[1..]);
        }
        text.push('e');
        text.push(if exponent < 0 { '-' } else { '+' });
        text.push_str(&exponent.abs().to_string());
    }
}

fn canonical(value: &Value, text: &mut String) {
    match value {
        Value::Null => text.push_str("null"),
        Value::Bool(flag) => text.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => canonical_number(number, text),
        Value::String(string) => text.push_str(&Value::String(string.clone()).to_string()),
        Value::Array(items) => {
            text.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                canonical(item, text);
            }
            text.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| utf16_order(a.0, b.0));
            text.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    text.push(',');
                }
                text.push_str(&Value::String(key.clone()).to_string());
                text.push(':');
                canonical(item, text);
            }
            text.push('}');
        }
    }
}

#[derive(Clone)]
pub(crate) struct McpPayment<P> {
    provider: P,
    context: PaymentContext,
}

impl<P: PaymentProvider> McpPayment<P> {
    pub(crate) const fn new(provider: P, context: PaymentContext) -> Self {
        Self { provider, context }
    }

    pub(crate) const fn provider(&self) -> &P {
        &self.provider
    }

    pub(crate) async fn prepare(
        &self,
        challenges: &[Challenge],
    ) -> Result<Option<PendingPayment<P>>, Fault> {
        self.prepare_inner(challenges, None).await
    }

    pub(crate) async fn prepare_with_management(
        &self,
        challenges: &[Challenge],
        management: &dyn ManagementTransport,
    ) -> Result<Option<PendingPayment<P>>, Fault> {
        self.prepare_inner(challenges, Some(management)).await
    }

    pub(crate) fn record_receipt(&self, challenge: &Challenge, receipt: &Receipt) -> Option<u128> {
        self.provider
            .record_receipt(&self.context, challenge, receipt)
    }

    async fn prepare_inner(
        &self,
        challenges: &[Challenge],
        management: Option<&dyn ManagementTransport>,
    ) -> Result<Option<PendingPayment<P>>, Fault> {
        let mut values = self.context.headers.get_all(ACCEPT_PAYMENT).iter();
        let first = values.next();
        if values.next().is_some() {
            return Ok(None);
        }
        let ranking = match first {
            Some(header) => {
                let value = header
                    .to_str()
                    .map_err(|_| Fault::config("invalid caller Accept-Payment header"))?;
                if value.split(',').all(|part| part.trim().is_empty()) {
                    return Ok(None);
                }
                protocol::preferences(value)
                    .ok_or_else(|| Fault::config("invalid caller Accept-Payment header"))?;
                Some(value.to_owned())
            }
            None => self.provider.accept_payment_header(),
        };
        let mut remaining = challenges.to_vec();
        let mut failure = None;
        loop {
            let challenge = match flow::select(
                &remaining,
                ranking.as_deref(),
                |challenge| self.provider.supports(&challenge.method, &challenge.intent),
                |candidates| self.provider.select_challenge(candidates),
            ) {
                Selection::Chosen(challenge) => challenge,
                Selection::Expired(challenge) => {
                    return Err(failure.unwrap_or(Fault::Expired(challenge.expires)));
                }
                Selection::None(_) => return failure.map_or(Ok(None), Err),
            };
            if flow::cross_realm_proof(&challenge, &self.context.url, &self.context.headers) {
                return Err(Fault::config(
                    "refusing to sign Tempo proof for a different HTTP realm",
                ));
            }
            let paid = match management {
                Some(management) => {
                    self.provider
                        .pay_with_management(&challenge, self.context.clone(), management)
                        .await
                }
                None => {
                    self.provider
                        .pay_with_context(&challenge, self.context.clone())
                        .await
                }
            };
            match paid {
                Ok(credential) => {
                    return Ok(Some(PendingPayment::new(
                        self.provider.clone(),
                        challenge,
                        credential,
                    )));
                }
                Err(
                    error @ (Fault::OutcomeUnknown { .. } | Fault::SessionOutcomeUnknown { .. }),
                ) => {
                    return Err(error);
                }
                Err(error) => {
                    failure.get_or_insert(error);
                    remaining.retain(|candidate| {
                        candidate.id != challenge.id
                            || candidate.method != challenge.method
                            || candidate.intent != challenge.intent
                    });
                }
            }
        }
    }
}

use std::time::{SystemTime, UNIX_EPOCH};

use nanocode_tempo::{Address, Call, U256, hex, keccak256};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::error::Fault;
use super::keys::{Accounts, AuthorizationReservation};
use super::protocol::{Challenge, Credential, INTENT_CHARGE, INTENT_SESSION, METHOD_NAME};
use super::rpc::{Rpc, RpcFault};
use super::signing::{
    KeychainVersion, TempoSigningMode, build_tempo_signature_primitive, effective_signing_hash,
    encode_signed_fee_payer_envelope,
};
use super::tx::{
    self, CHAIN_ID, FEE_PAYER_MIN_REMAINING_VALIDITY_SECS, FEE_PAYER_PLACEHOLDER, MAX_FEE_PER_GAS,
    MAX_PRIORITY_FEE_PER_GAS, OUSD, PATH_USD, STABLECOIN_DEX_ADDRESS, TempoNetwork,
    TempoTransactionRequest, USDC,
};

const EXPIRING_NONCE_KEY: U256 = U256::MAX;
const EXPIRING_NONCE_VALID_BEFORE_SECS: u64 = 25;
const SPONSORED_GAS_LIMIT: u64 = 1_000_000;
const MAX_SPLITS: usize = 10;
const MEMO_BYTES: usize = 32;
const TAG_BYTES: usize = 4;
const FINGERPRINT_BYTES: usize = 10;
const NONCE_BYTES: usize = 7;
const ATTRIBUTION_TAG: &[u8] = b"mpp";
const ATTRIBUTION_VERSION: u8 = 0x01;
const SERVER_OFFSET: usize = TAG_BYTES + 1;
const CLIENT_OFFSET: usize = SERVER_OFFSET + FINGERPRINT_BYTES;
const NONCE_OFFSET: usize = CLIENT_OFFSET + FINGERPRINT_BYTES;
const PROOF_DOMAIN_NAME: &str = "MPP";
const PROOF_DOMAIN_VERSION: &str = "3";
const EIP712_DOMAIN_TYPE: &str = "EIP712Domain(string name,string version,uint256 chainId)";
const PROOF_TYPE: &str = "Proof(address account,string challengeId,string realm)";
const EIP712_PREFIX: [u8; 2] = [0x19, 0x01];
const MAX_SLIPPAGE_BPS: u16 = 5_000;
pub(crate) const DEFAULT_SLIPPAGE_BPS: u16 = 100;
const BASIS_POINTS: u128 = 10_000;
const PUSH_REFUSED_CODE: i64 = -32602;
const NANOS_PER_SECOND: i128 = 1_000_000_000;
const HEX_PREFIX: &str = "0x";
const RECEIPT_SUCCESS: &str = "0x1";
const RECEIPT_REVERTED: &str = "0x0";
const ADDRESS_BYTES: usize = nanocode_tempo::ADDRESS_BYTES;
pub(crate) const DEFAULT_AUTOSWAP_CURRENCIES: [Address; 3] = [PATH_USD, USDC, OUSD];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct ChargeRequest {
    pub(crate) amount: String,
    pub(crate) currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(rename = "externalId", skip_serializing_if = "Option::is_none")]
    pub(crate) external_id: Option<String>,
    #[serde(rename = "methodDetails", skip_serializing_if = "Option::is_none")]
    pub(crate) method_details: Option<Value>,
    #[serde(rename = "_mppx_scope", skip_serializing_if = "Option::is_none")]
    pub(crate) mppx_scope: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct SessionCurrency {
    #[serde(rename = "amount")]
    _amount: String,
    currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Split {
    pub(crate) amount: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) memo: Option<String>,
    pub(crate) recipient: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct TempoMethodDetails {
    #[serde(
        rename = "machineTokenEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) machine_token_enabled: Option<bool>,
    #[serde(rename = "chainId", skip_serializing_if = "Option::is_none")]
    pub(crate) chain_id: Option<u64>,
    #[serde(rename = "feePayer", skip_serializing_if = "Option::is_none")]
    pub(crate) fee_payer: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) splits: Option<Vec<Split>>,
}

fn base_unit_digits(amount: &str) -> Option<&str> {
    (!amount.is_empty() && amount.bytes().all(|byte| byte.is_ascii_digit())).then_some(amount)
}

impl ChargeRequest {
    fn amount_u256(&self) -> Result<U256, Fault> {
        parse_amount(&self.amount)
    }

    fn recipient_address(&self) -> Result<Address, Fault> {
        let recipient = self
            .recipient
            .as_ref()
            .ok_or_else(|| Fault::InvalidChallenge(Some("No recipient specified".to_owned())))?;
        parse_address(recipient)
    }

    fn currency_address(&self) -> Result<Address, Fault> {
        parse_address(&self.currency)
    }

    pub(crate) fn chain_id(&self) -> Option<u64> {
        self.method_details.as_ref()?.get("chainId")?.as_u64()
    }

    fn tempo_method_details(&self) -> Result<TempoMethodDetails, Fault> {
        match &self.method_details {
            Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
                Fault::InvalidChallenge(Some(format!("Invalid Tempo method details: {error}")))
            }),
            None => Ok(TempoMethodDetails::default()),
        }
    }

    pub(crate) fn machine_token_enabled(&self) -> bool {
        self.method_details
            .as_ref()
            .and_then(|details| details.get("machineTokenEnabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
}

pub(crate) fn requested_amount(challenge: &Challenge) -> Option<u128> {
    let request = challenge.request_value()?;
    let amount = request.get("amount")?.as_str()?;
    base_unit_digits(amount).and_then(|amount| amount.parse().ok())
}

fn hex_error(text: &str) -> String {
    let digits = text.strip_prefix(HEX_PREFIX).unwrap_or(text);
    if !digits.len().is_multiple_of(2) {
        return "odd number of digits".to_owned();
    }
    if digits.len() / 2 != ADDRESS_BYTES {
        return "invalid string length".to_owned();
    }
    match digits
        .chars()
        .enumerate()
        .find(|(_, character)| !character.is_ascii_hexdigit())
    {
        Some((index, character)) => format!("invalid character {character:?} at position {index}"),
        None => "invalid string length".to_owned(),
    }
}

pub(crate) fn parse_address(text: &str) -> Result<Address, Fault> {
    text.parse::<Address>()
        .map_err(|_| Fault::config(format!("Invalid EVM address '{text}': {}", hex_error(text))))
}

pub(crate) fn parse_amount(text: &str) -> Result<U256, Fault> {
    let digits = base_unit_digits(text).ok_or_else(|| {
        Fault::InvalidAmount(format!(
            "Invalid U256 amount '{text}': expected base-10 digits"
        ))
    })?;
    U256::from_str_radix(digits, u64::from(DECIMAL_RADIX)).map_err(|_| {
        Fault::InvalidAmount(format!(
            "Invalid U256 amount '{text}': the value is too large to fit the target type"
        ))
    })
}

const DECIMAL_RADIX: u8 = 10;
const SECONDS_END: usize = 19;

fn parse_memo(memo: Option<&str>, context: &str) -> Result<Option<[u8; MEMO_BYTES]>, Fault> {
    let Some(memo) = memo.filter(|memo| !memo.is_empty()) else {
        return Ok(None);
    };
    let digits = memo.strip_prefix(HEX_PREFIX).unwrap_or(memo);
    let bytes = decode_hex_strict(digits)
        .map_err(|error| Fault::InvalidChallenge(Some(format!("Invalid {context}: {error}"))))?;
    if bytes.len() != MEMO_BYTES {
        return Err(Fault::InvalidChallenge(Some(format!(
            "Invalid {context}: expected 32 bytes, got {}",
            bytes.len()
        ))));
    }
    let mut memo = [0u8; MEMO_BYTES];
    memo.copy_from_slice(&bytes);
    Ok(Some(memo))
}

fn decode_hex_strict(digits: &str) -> Result<Vec<u8>, String> {
    if !digits.len().is_multiple_of(2) {
        return Err("Odd number of digits".to_owned());
    }
    if let Some((index, character)) = digits
        .chars()
        .enumerate()
        .find(|(_, character)| !character.is_ascii_hexdigit())
    {
        return Err(format!(
            "Invalid character {character:?} at position {index}"
        ));
    }
    hex::decode(digits).map_err(|error| error.to_string())
}

pub(crate) fn attribution(
    challenge_id: &str,
    server_id: &str,
    client_id: Option<&str>,
) -> [u8; MEMO_BYTES] {
    let fingerprint = |value: &str| {
        let hash = keccak256(value.as_bytes());
        let mut fingerprint = [0u8; FINGERPRINT_BYTES];
        fingerprint.copy_from_slice(&hash[..FINGERPRINT_BYTES]);
        fingerprint
    };
    let mut memo = [0u8; MEMO_BYTES];
    memo[..TAG_BYTES].copy_from_slice(&keccak256(ATTRIBUTION_TAG)[..TAG_BYTES]);
    memo[TAG_BYTES] = ATTRIBUTION_VERSION;
    memo[SERVER_OFFSET..CLIENT_OFFSET].copy_from_slice(&fingerprint(server_id));
    if let Some(client_id) = client_id {
        memo[CLIENT_OFFSET..NONCE_OFFSET].copy_from_slice(&fingerprint(client_id));
    }
    memo[NONCE_OFFSET..].copy_from_slice(&keccak256(challenge_id.as_bytes())[..NONCE_BYTES]);
    memo
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transfer {
    pub(crate) amount: U256,
    pub(crate) recipient: Address,
    pub(crate) memo: Option<[u8; MEMO_BYTES]>,
}

pub(crate) fn get_transfers(
    total_amount: U256,
    primary_recipient: Address,
    primary_memo: Option<[u8; MEMO_BYTES]>,
    splits: Option<&[Split]>,
) -> Result<Vec<Transfer>, Fault> {
    let invalid = |reason: String| Fault::InvalidChallenge(Some(reason));
    let splits = match splits {
        Some([]) => return Err(invalid("Splits must not be empty".to_owned())),
        Some(splits) => splits,
        None => {
            return Ok(vec![Transfer {
                amount: total_amount,
                recipient: primary_recipient,
                memo: primary_memo,
            }]);
        }
    };
    if splits.len() > MAX_SPLITS {
        return Err(invalid(format!(
            "Too many splits: {} (max {MAX_SPLITS})",
            splits.len()
        )));
    }
    let mut split_sum = U256::ZERO;
    let mut split_transfers = Vec::with_capacity(splits.len());
    for split in splits {
        let amount = parse_amount(&split.amount)
            .map_err(|_| invalid(format!("Invalid split amount: {}", split.amount)))?;
        if amount.is_zero() {
            return Err(invalid("Split amount must be greater than zero".to_owned()));
        }
        let recipient = parse_address(&split.recipient)?;
        let memo = parse_memo(split.memo.as_deref(), "split memo")?;
        split_sum = split_sum
            .checked_add(&amount)
            .ok_or_else(|| invalid("Split amounts overflow".to_owned()))?;
        split_transfers.push(Transfer {
            amount,
            recipient,
            memo,
        });
    }
    if split_sum >= total_amount {
        return Err(invalid(format!(
            "Sum of splits ({split_sum}) must be less than total amount ({total_amount})"
        )));
    }
    let primary_amount = total_amount.saturating_sub(&split_sum);
    let mut transfers = Vec::with_capacity(1 + split_transfers.len());
    transfers.push(Transfer {
        amount: primary_amount,
        recipient: primary_recipient,
        memo: primary_memo,
    });
    transfers.extend(split_transfers);
    Ok(transfers)
}

fn word(value: &[u8]) -> [u8; MEMO_BYTES] {
    keccak256(value)
}

pub(crate) fn proof_signing_hash(
    account: Address,
    chain_id: u64,
    challenge_id: &str,
    realm: &str,
) -> [u8; MEMO_BYTES] {
    let mut domain = Vec::new();
    domain.extend_from_slice(&word(EIP712_DOMAIN_TYPE.as_bytes()));
    domain.extend_from_slice(&word(PROOF_DOMAIN_NAME.as_bytes()));
    domain.extend_from_slice(&word(PROOF_DOMAIN_VERSION.as_bytes()));
    domain.extend_from_slice(&U256::from_u64(chain_id).to_be_bytes());
    let separator = keccak256(&domain);
    let mut proof = Vec::new();
    proof.extend_from_slice(&word(PROOF_TYPE.as_bytes()));
    proof.extend_from_slice(&account.word());
    proof.extend_from_slice(&word(challenge_id.as_bytes()));
    proof.extend_from_slice(&word(realm.as_bytes()));
    let structure = keccak256(&proof);
    let mut message = Vec::new();
    message.extend_from_slice(&EIP712_PREFIX);
    message.extend_from_slice(&separator);
    message.extend_from_slice(&structure);
    keccak256(&message)
}

pub(crate) fn proof_source(address: Address, chain_id: u64) -> String {
    format!("did:pkh:eip155:{chain_id}:{address}")
}

fn machine_route(chain_id: u64, currency: Address, transfers: &[Transfer]) -> Option<[Call; 2]> {
    let token = tx::machine_token(chain_id)?;
    let swapper = tx::machine_swapper(chain_id)?;
    let transfer = match transfers {
        [transfer] if transfer.memo.is_some() => transfer,
        _ => return None,
    };
    let memo = transfer.memo?;
    Some([
        Call {
            to: token,
            value: U256::ZERO,
            input: tx::approve_call(swapper, transfer.amount),
        },
        Call {
            to: swapper,
            value: U256::ZERO,
            input: tx::swap_to_call(token, transfer.amount, currency, transfer.recipient, memo),
        },
    ])
}

async fn find_machine_route(
    rpc: &Rpc,
    account: Address,
    chain_id: u64,
    currency: Address,
    transfers: &[Transfer],
) -> Option<[Call; 2]> {
    let calls = machine_route(chain_id, currency, transfers)?;
    let amount = transfers.first()?.amount;
    let token = calls[0].to;
    let request = TempoTransactionRequest {
        calls: calls.to_vec(),
        from: Some(account),
        chain_id: Some(chain_id),
        ..TempoTransactionRequest::default()
    };
    let (balance, simulation) = tokio::join!(
        tx::balance_of(rpc, token, account),
        tx::eth_call_pending(rpc, &request)
    );
    (balance.ok()? >= amount && simulation.is_ok()).then_some(calls)
}

fn can_pay_fees(token: Address) -> bool {
    token != OUSD
}

#[derive(Debug, Clone)]
pub(crate) struct AutoswapConfig {
    pub(crate) token_in: Address,
    pub(crate) slippage_bps: u16,
    pub(crate) max_amount_in: Option<u128>,
    pub(crate) fallback: bool,
}

impl AutoswapConfig {
    pub(crate) const fn new(token_in: Address, slippage_bps: u16) -> Self {
        Self {
            token_in,
            slippage_bps,
            max_amount_in: None,
            fallback: true,
        }
    }

    pub(crate) fn candidates(&self) -> Vec<Address> {
        let mut candidates = vec![self.token_in];
        if self.fallback {
            for token in DEFAULT_AUTOSWAP_CURRENCIES {
                if !candidates.contains(&token) {
                    candidates.push(token);
                }
            }
        }
        candidates
    }

    pub(crate) fn fee_token(&self) -> Option<Address> {
        Some(self.token_in).filter(|token| can_pay_fees(*token))
    }

    pub(crate) fn fee_token_for(
        &self,
        route: Option<&AutoswapRoute>,
        currency: Address,
    ) -> Option<Address> {
        match route {
            Some(route) => route.fee_token(self),
            None if can_pay_fees(currency) => None,
            None => self.fee_token(),
        }
    }
}

impl Default for AutoswapConfig {
    fn default() -> Self {
        Self::new(PATH_USD, DEFAULT_SLIPPAGE_BPS)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AutoswapRoute {
    pub(crate) token_in: Address,
    pub(crate) calls: Vec<Call>,
}

impl AutoswapRoute {
    pub(crate) fn fee_token(&self, config: &AutoswapConfig) -> Option<Address> {
        Some(self.token_in)
            .filter(|token| can_pay_fees(*token))
            .or_else(|| config.fee_token())
    }
}

pub(crate) async fn check_balance_deficit(
    rpc: &Rpc,
    owner: Address,
    currency: Address,
    amount: U256,
) -> Result<Option<U256>, Fault> {
    let balance = tx::balance_of(rpc, currency, owner)
        .await
        .map_err(|error| Fault::http(format!("failed to query balance: {error}")))?;
    Ok(amount
        .checked_sub(&balance)
        .filter(|deficit| !deficit.is_zero()))
}

pub(crate) async fn quote_swap(
    rpc: &Rpc,
    token_in: Address,
    token_out: Address,
    amount_out: u128,
) -> Result<u128, Fault> {
    tx::quote_swap_exact_amount_out(rpc, token_in, token_out, amount_out)
        .await
        .map_err(|error| Fault::http(format!("DEX quote failed: {error}")))
}

pub(crate) fn build_approve_call(token_in: Address, max_amount_in: u128) -> Call {
    Call {
        to: token_in,
        value: U256::ZERO,
        input: tx::approve_call(STABLECOIN_DEX_ADDRESS, U256::from_u128(max_amount_in)),
    }
}

fn with_slippage(amount: u128, slippage_bps: u16) -> U256 {
    let scaled = U256::from_u128(amount)
        .checked_mul_small(BASIS_POINTS as u64 + u64::from(slippage_bps))
        .unwrap_or(U256::MAX);
    divide(scaled, BASIS_POINTS)
}

fn divide(value: U256, divisor: u128) -> U256 {
    let mut quotient = U256::ZERO;
    let mut remainder = 0u128;
    for byte in value.to_be_bytes() {
        remainder = (remainder << u8::BITS) | u128::from(byte);
        let digit = remainder / divisor;
        remainder %= divisor;
        quotient = quotient
            .checked_mul_small(1 << u8::BITS)
            .and_then(|shifted| shifted.checked_add(&U256::from_u128(digit)))
            .unwrap_or(U256::MAX);
    }
    quotient
}

fn saturate_u128(value: U256) -> u128 {
    value.to_u128().unwrap_or(u128::MAX)
}

pub(crate) fn build_swap_call(
    token_in: Address,
    token_out: Address,
    amount_out: u128,
    quoted_amount_in: u128,
    slippage_bps: u16,
) -> Call {
    let max_amount_in = saturate_u128(with_slippage(quoted_amount_in, slippage_bps));
    Call {
        to: STABLECOIN_DEX_ADDRESS,
        value: U256::ZERO,
        input: tx::swap_exact_amount_out_call(token_in, token_out, amount_out, max_amount_in),
    }
}

fn bounded_swap_input(
    amount_out: u128,
    quoted: u128,
    config: &AutoswapConfig,
) -> Result<u128, Fault> {
    let limit = config
        .max_amount_in
        .map(U256::from_u128)
        .unwrap_or_else(|| with_slippage(amount_out, config.slippage_bps));
    if quoted == 0 || U256::from_u128(quoted) > limit {
        return Err(Fault::InvalidAmount(
            "autoswap quote exceeds spend limit or is zero".to_owned(),
        ));
    }
    with_slippage(quoted, config.slippage_bps)
        .min(limit)
        .to_u128()
        .ok_or_else(|| Fault::InvalidAmount("autoswap maximum input exceeds u128".to_owned()))
}

async fn swap_from(
    rpc: &Rpc,
    owner: Address,
    token_in: Address,
    currency: Address,
    amount_out: u128,
    config: &AutoswapConfig,
    single: bool,
) -> Result<Option<Vec<Call>>, Fault> {
    let balance = tx::balance_of(rpc, token_in, owner)
        .await
        .map_err(|error| Fault::http(format!("failed to query balance: {error}")))?;
    if balance.is_zero() && !single {
        return Ok(None);
    }
    let quoted_amount_in = quote_swap(rpc, token_in, currency, amount_out).await?;
    let max_amount_in = bounded_swap_input(amount_out, quoted_amount_in, config)?;
    if balance < U256::from_u128(max_amount_in) {
        return Err(Fault::TokenBalance {
            token: token_in.to_string(),
            available: balance.to_string(),
            required: max_amount_in.to_string(),
        });
    }
    Ok(Some(vec![
        build_approve_call(token_in, max_amount_in),
        build_swap_call(token_in, currency, amount_out, max_amount_in, 0),
    ]))
}

pub(crate) async fn resolve_autoswap_route(
    rpc: &Rpc,
    owner: Address,
    currency: Address,
    amount: U256,
    config: &AutoswapConfig,
) -> Result<Option<AutoswapRoute>, Fault> {
    if config.slippage_bps > MAX_SLIPPAGE_BPS {
        return Err(Fault::config(format!(
            "autoswap slippage {}bps exceeds maximum {MAX_SLIPPAGE_BPS}bps",
            config.slippage_bps
        )));
    }
    let candidates: Vec<Address> = config
        .candidates()
        .into_iter()
        .filter(|token| *token != currency)
        .collect();
    if candidates.is_empty() {
        return Ok(None);
    }
    let Some(deficit) = check_balance_deficit(rpc, owner, currency, amount).await? else {
        return Ok(None);
    };
    let amount_out = deficit
        .to_u128()
        .ok_or_else(|| Fault::InvalidAmount(format!("deficit {deficit} exceeds u128")))?;
    let single = candidates.len() == 1;
    for token_in in candidates {
        match swap_from(rpc, owner, token_in, currency, amount_out, config, single).await {
            Ok(Some(calls)) => return Ok(Some(AutoswapRoute { token_in, calls })),
            Ok(None) => continue,
            Err(error) if single => return Err(error),
            Err(_) => continue,
        }
    }
    Err(Fault::InsufficientBalance(Some(format!(
        "no balance in {currency} and no viable swap route from fallback currencies"
    ))))
}

fn resolve_push_mode(request: &ChargeRequest) -> Result<bool, Fault> {
    let Some(modes) = request
        .method_details
        .as_ref()
        .and_then(|details| details.get("supportedModes"))
        .filter(|modes| !modes.is_null())
    else {
        return Ok(false);
    };
    let supports = |wanted: &str| {
        modes
            .as_array()
            .is_some_and(|modes| modes.iter().any(|mode| mode.as_str() == Some(wanted)))
    };
    if supports("pull") {
        Ok(false)
    } else if supports("push") {
        Ok(true)
    } else {
        Err(Fault::Unsupported(
            "challenge supports neither pull nor push mode".to_owned(),
        ))
    }
}

fn now_nanos() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i128::try_from(elapsed.as_nanos()).unwrap_or(i128::MAX)
        })
}

fn fraction_nanos(text: &str) -> i128 {
    let Some(fraction) = text
        .get(SECONDS_END..)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return 0;
    };
    let digits: String = fraction.chars().take_while(char::is_ascii_digit).collect();
    let mut nanos = 0i128;
    let mut scale = NANOS_PER_SECOND;
    for digit in digits.chars() {
        scale /= i128::from(DECIMAL_RADIX);
        nanos += i128::from(digit.to_digit(u32::from(DECIMAL_RADIX)).unwrap_or(0)) * scale;
    }
    nanos
}

fn expires_nanos(challenge: &Challenge) -> Option<i128> {
    let expires = challenge.expires.as_deref()?;
    let seconds = super::protocol::unix_seconds(expires)?;
    Some(i128::from(seconds) * NANOS_PER_SECOND + fraction_nanos(expires))
}

fn resolve_valid_before(challenge: &Challenge, requested: Option<u64>) -> u64 {
    let now = u64::try_from(super::protocol::now_seconds()).unwrap_or(0);
    let valid_before =
        requested.unwrap_or_else(|| now.saturating_add(EXPIRING_NONCE_VALID_BEFORE_SECS));
    let Some(expires) = challenge.expires_at() else {
        return valid_before;
    };
    let expires = u64::try_from(expires).unwrap_or(0).saturating_add(1);
    match valid_before {
        0 => expires,
        valid_before => valid_before.min(expires),
    }
}

fn ensure_sponsored_expiry_floor(challenge: &Challenge, now: i128) -> Result<(), Fault> {
    let Some(expires) = expires_nanos(challenge) else {
        return Ok(());
    };
    let remaining = expires - now;
    let floor = i128::from(FEE_PAYER_MIN_REMAINING_VALIDITY_SECS) * NANOS_PER_SECOND;
    if remaining < floor {
        let expires_in = remaining / NANOS_PER_SECOND;
        return Err(Fault::BadRequest(format!(
            "fee-sponsored challenge expires in {expires_in}s, less than the {FEE_PAYER_MIN_REMAINING_VALIDITY_SECS}s the server needs to co-sign and broadcast; request a new challenge"
        )));
    }
    Ok(())
}

fn ensure_sponsorable_before_signing(
    challenge: &Challenge,
    valid_before: Option<u64>,
    now: i128,
) -> Result<(), Fault> {
    ensure_sponsored_expiry_floor(challenge, now)?;
    let Some(valid_before) = valid_before.filter(|valid_before| *valid_before != 0) else {
        return Ok(());
    };
    let now_secs =
        now.div_euclid(NANOS_PER_SECOND) + i128::from(now.rem_euclid(NANOS_PER_SECOND) > 0);
    if i128::from(valid_before) < now_secs + i128::from(FEE_PAYER_MIN_REMAINING_VALIDITY_SECS) {
        return Err(Fault::BadRequest(format!(
            "sponsored transaction preparation took too long: less than {FEE_PAYER_MIN_REMAINING_VALIDITY_SECS}s of its validity window remain, so the fee payer would reject it; request a new challenge"
        )));
    }
    Ok(())
}

fn validate_for_charge(challenge: &Challenge) -> Result<(), Fault> {
    if !challenge.method.eq_ignore_ascii_case(METHOD_NAME) {
        return Err(Fault::Unsupported(format!(
            "Payment method '{}' is not supported. Supported methods: {METHOD_NAME}",
            challenge.method
        )));
    }
    if !challenge.intent.eq_ignore_ascii_case(INTENT_CHARGE) {
        return Err(Fault::ChallengeRejected {
            id: challenge.id.clone(),
            reason: format!(
                "Only 'charge' intent is supported, got: {}",
                challenge.intent
            ),
        });
    }
    if challenge.is_expired() {
        return Err(Fault::Expired(challenge.expires.clone()));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct TempoCharge {
    challenge: Challenge,
    recipient: Address,
    currency: Address,
    amount: U256,
    memo: Option<[u8; MEMO_BYTES]>,
    chain_id: u64,
    fee_payer: bool,
    machine_token_enabled: bool,
    splits: Option<Vec<Split>>,
    calls: Option<Vec<Call>>,
    push: bool,
}

impl TempoCharge {
    pub(crate) fn from_challenge(challenge: &Challenge) -> Result<Self, Fault> {
        validate_for_charge(challenge)?;
        let request: ChargeRequest = challenge.request.decode()?;
        let details = request.tempo_method_details()?;
        let recipient = request.recipient_address()?;
        let currency = request.currency_address()?;
        let amount = request.amount_u256()?;
        let push = !amount.is_zero() && resolve_push_mode(&request)?;
        let memo = Some(attribution(&challenge.id, &challenge.realm, None));
        let chain_id = details.chain_id.unwrap_or(CHAIN_ID);
        let fee_payer = details.fee_payer.unwrap_or(false) && !push;
        let machine_token_enabled = details.machine_token_enabled.unwrap_or(false);
        let splits = details.splits;
        get_transfers(amount, recipient, memo, splits.as_deref())?;
        Ok(Self {
            challenge: challenge.clone(),
            recipient,
            currency,
            amount,
            memo,
            chain_id,
            fee_payer,
            machine_token_enabled,
            splits,
            calls: None,
            push,
        })
    }

    pub(crate) const fn chain_id(&self) -> u64 {
        self.chain_id
    }

    const fn with_chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    const fn with_memo(mut self, memo: [u8; MEMO_BYTES]) -> Self {
        self.memo = Some(memo);
        self
    }

    fn pays_in_machine_token(&self) -> bool {
        let Some(mach) = tx::machine_token(self.chain_id) else {
            return false;
        };
        self.currency == mach
            || self
                .calls
                .as_ref()
                .and_then(|calls| calls.first())
                .is_some_and(|call| call.to == mach)
    }

    fn transfers(&self) -> Result<Vec<Transfer>, Fault> {
        get_transfers(
            self.amount,
            self.recipient,
            self.memo,
            self.splits.as_deref(),
        )
    }

    fn build_transfer_calls(&self) -> Result<Vec<Call>, Fault> {
        Ok(self
            .transfers()?
            .into_iter()
            .map(|transfer| Call {
                to: self.currency,
                value: U256::ZERO,
                input: match transfer.memo {
                    Some(memo) => {
                        tx::transfer_with_memo_call(transfer.recipient, transfer.amount, memo)
                    }
                    None => tx::transfer_call(transfer.recipient, transfer.amount),
                },
            })
            .collect())
    }

    fn with_prepended_call(mut self, call: Call) -> Result<Self, Fault> {
        let mut calls = match self.calls.take() {
            Some(calls) => calls,
            None => self.build_transfer_calls()?,
        };
        calls.insert(0, call);
        self.calls = Some(calls);
        Ok(self)
    }

    fn with_calls(mut self, calls: Vec<Call>) -> Self {
        self.calls = Some(calls);
        self
    }

    pub(crate) async fn sign_with_accounts(
        self,
        accounts: &Accounts,
        rpc: &Rpc,
        from: Address,
        fee_token: Option<Address>,
    ) -> Result<SignedCharge, Fault> {
        if self.amount.is_zero() {
            return self.prove(accounts, from);
        }
        if self.fee_payer {
            ensure_sponsored_expiry_floor(&self.challenge, now_nanos())?;
        }
        let machine_token = self.pays_in_machine_token();
        let calls = match self.calls.clone() {
            Some(calls) => calls,
            None => self.build_transfer_calls()?,
        };
        let fee_token = resolve_fee_token(
            fee_token,
            self.currency,
            machine_token,
            self.fee_payer,
            self.chain_id,
            rpc,
            from,
        )
        .await;
        let valid_before = Some(resolve_valid_before(&self.challenge, None));
        let explicit_gas = self.fee_payer.then_some(SPONSORED_GAS_LIMIT);
        let mut request = TempoTransactionRequest {
            calls,
            nonce_key: Some(EXPIRING_NONCE_KEY),
            ..TempoTransactionRequest::default()
        };
        match self.fee_payer {
            true => request.fee_payer_signature = Some(FEE_PAYER_PLACEHOLDER),
            false => request.fee_token = Some(fee_token),
        }
        if let Some(valid_before) = valid_before {
            request = request.with_valid_before(valid_before);
        }
        request.from = Some(from);
        request.chain_id = Some(self.chain_id);
        request.nonce = Some(0);
        request.gas = explicit_gas;
        request.max_fee_per_gas = Some(MAX_FEE_PER_GAS);
        request.max_priority_fee_per_gas = Some(MAX_PRIORITY_FEE_PER_GAS);
        let access_key = accounts
            .prepare_request(rpc, &mut request)
            .await
            .map_err(|error| {
                Fault::http(format!(
                    "failed to prepare Tempo Accounts access key: {error}"
                ))
            })?;
        let from = access_key.account();
        request.gas = Some(match explicit_gas {
            Some(gas_limit) => gas_limit,
            None => tx::estimate_gas(rpc, request.clone()).await?,
        });
        if self.fee_payer {
            ensure_sponsorable_before_signing(&self.challenge, valid_before, now_nanos())?;
        }
        let signed = access_key
            .sign_request(request)
            .map_err(|error| Fault::http(format!("failed to sign Tempo transaction: {error}")))?;
        let authorization_reservation = access_key.authorization_reservation();
        let encoded = match self.fee_payer {
            true => ensure_sponsorable_before_signing(&self.challenge, valid_before, now_nanos())
                .and_then(|()| encode_signed_fee_payer_envelope(signed, from)),
            false => Ok(signed.encoded_2718()),
        };
        let tx_bytes = match encoded {
            Ok(tx_bytes) => tx_bytes,
            Err(error) => {
                if let Some(reservation) = authorization_reservation {
                    access_key.release_authorization(reservation);
                }
                return Err(error);
            }
        };
        let credential = build_charge_credential(&self.challenge, &tx_bytes, self.chain_id, from);
        let signed = SignedCharge {
            credential,
            tx_bytes: Some(tx_bytes),
            chain_id: self.chain_id,
            from,
            authorization_reservation,
        };
        if !self.push {
            return Ok(signed);
        }
        let tx_hash = keccak256(signed.tx_bytes());
        match signed.push(&self.challenge, rpc).await {
            Ok(mut pushed) => {
                pushed.authorization_reservation = None;
                Ok(pushed)
            }
            Err(PushError { error, rejected }) => {
                if let Some(reservation) = authorization_reservation
                    && (rejected || authorization_unused(rpc, tx_hash, reservation).await)
                {
                    access_key.release_authorization(reservation);
                }
                Err(error)
            }
        }
    }

    fn prove(&self, accounts: &Accounts, from: Address) -> Result<SignedCharge, Fault> {
        let access_key = accounts
            .clone()
            .with_chain_id(self.chain_id)
            .active_access_key()
            .map_err(|error| {
                Fault::config(format!(
                    "failed to select Tempo Accounts access key: {error}"
                ))
            })?;
        if access_key.account() != from {
            return Err(Fault::config(
                "the active Tempo account changed while preparing the charge",
            ));
        }
        if access_key.chain_id() != self.chain_id {
            return Err(Fault::ChainIdMismatch {
                expected: access_key.chain_id(),
                got: self.chain_id,
            });
        }
        let mode = TempoSigningMode::Keychain {
            wallet: from,
            key_authorization: None,
            version: KeychainVersion::V2,
        };
        let hash = effective_signing_hash(
            proof_signing_hash(
                from,
                self.chain_id,
                &self.challenge.id,
                &self.challenge.realm,
            ),
            &mode,
        );
        let inner = access_key
            .sign_hash(&hash)
            .map_err(|error| Fault::http(format!("failed to sign proof: {error}")))?;
        let signature =
            hex::encode_prefixed(build_tempo_signature_primitive(inner, &mode).to_bytes());
        let credential = Credential::with_source(
            self.challenge.to_echo(),
            proof_source(from, self.chain_id),
            json!({"type": "proof", "signature": signature}),
        );
        Ok(SignedCharge {
            credential,
            tx_bytes: None,
            chain_id: self.chain_id,
            from,
            authorization_reservation: None,
        })
    }
}

async fn resolve_fee_token(
    explicit: Option<Address>,
    currency: Address,
    machine_token: bool,
    fee_payer: bool,
    chain_id: u64,
    rpc: &Rpc,
    from: Address,
) -> Address {
    if let Some(token) = explicit {
        return token;
    }
    if fee_payer || !machine_token {
        return currency;
    }
    let mut candidates: Vec<Address> = Vec::new();
    let defaults = [
        Some(PATH_USD),
        TempoNetwork::from_chain_id(chain_id).map(TempoNetwork::default_currency),
    ];
    for token in defaults.into_iter().flatten() {
        if !candidates.contains(&token) {
            candidates.push(token);
        }
    }
    for token in &candidates {
        let balance = tx::balance_of(rpc, *token, from).await;
        if balance.is_ok_and(|balance| !balance.is_zero()) {
            return *token;
        }
    }
    candidates.first().copied().unwrap_or(currency)
}

pub(crate) fn build_charge_credential(
    challenge: &Challenge,
    tx_bytes: &[u8],
    chain_id: u64,
    from: Address,
) -> Credential {
    Credential::with_source(
        challenge.to_echo(),
        Credential::did(chain_id, &from.to_string()),
        json!({"type": "transaction", "signature": hex::encode_prefixed(tx_bytes)}),
    )
}

#[derive(Debug)]
pub(crate) struct SignedCharge {
    credential: Credential,
    tx_bytes: Option<Vec<u8>>,
    chain_id: u64,
    from: Address,
    authorization_reservation: Option<AuthorizationReservation>,
}

struct PushError {
    error: Fault,
    rejected: bool,
}

fn push_refused(error: &RpcFault) -> bool {
    error.code() == Some(PUSH_REFUSED_CODE)
}

async fn authorization_unused(
    rpc: &Rpc,
    tx_hash: [u8; MEMO_BYTES],
    reservation: AuthorizationReservation,
) -> bool {
    for method in ["eth_getTransactionReceipt", "eth_getTransactionByHash"] {
        let found = tx::raw_request(rpc, method, json!([hex::encode_prefixed(tx_hash)])).await;
        if !matches!(found, Ok(Value::Null)) {
            return false;
        }
    }
    tx::get_key(rpc, reservation.account(), reservation.key_id())
        .await
        .is_ok_and(|key| key.key_id == Address::ZERO)
}

impl SignedCharge {
    async fn push(mut self, challenge: &Challenge, rpc: &Rpc) -> Result<Self, PushError> {
        let Some(tx_bytes) = self.tx_bytes.clone() else {
            return Ok(self);
        };
        if let Some(expires) = challenge.expires.as_deref() {
            let expired = expires_nanos(challenge).is_none_or(|at| at <= now_nanos());
            if expired {
                return Err(PushError {
                    rejected: true,
                    error: Fault::config(format!(
                        "challenge expired at {expires} before the push payment was broadcast"
                    )),
                });
            }
        }
        let unknown = || Fault::OutcomeUnknown {
            transaction_hash: hex::encode_prefixed(keccak256(&tx_bytes)),
        };
        let receipt = tx::send_raw_transaction_sync(rpc, &tx_bytes)
            .await
            .map_err(|error| {
                let rejected = push_refused(&error);
                PushError {
                    rejected,
                    error: match rejected {
                        true => Fault::http(format!("failed to broadcast push payment: {error}")),
                        false => unknown(),
                    },
                }
            })?;
        let hash = receipt
            .get("transactionHash")
            .and_then(Value::as_str)
            .ok_or_else(|| PushError {
                rejected: false,
                error: unknown(),
            })?;
        let status = receipt.get("status").and_then(Value::as_str);
        if status != Some(RECEIPT_SUCCESS) {
            return Err(PushError {
                rejected: false,
                error: match status == Some(RECEIPT_REVERTED) {
                    true => Fault::http(format!("push payment transaction {hash} reverted")),
                    false => unknown(),
                },
            });
        }
        self.credential = Credential::with_source(
            challenge.to_echo(),
            Credential::did(self.chain_id, &self.from.to_string()),
            json!({"type": "hash", "hash": hash}),
        );
        Ok(self)
    }

    pub(crate) fn into_credential(self) -> Credential {
        self.credential
    }

    pub(crate) fn tx_bytes(&self) -> &[u8] {
        self.tx_bytes.as_deref().unwrap_or(&[])
    }

    pub(crate) const fn authorization_reservation(&self) -> Option<AuthorizationReservation> {
        self.authorization_reservation
    }
}

pub(crate) fn prepare_charge_request(
    challenge: &Challenge,
    expected_chain_id: Option<u64>,
    client_id: Option<&str>,
) -> Result<TempoCharge, Fault> {
    let mut charge = TempoCharge::from_challenge(challenge)?;
    if let Some(expected) = expected_chain_id {
        match challenge.request.decode::<ChargeRequest>()?.chain_id() {
            Some(got) if got != expected => return Err(Fault::ChainIdMismatch { expected, got }),
            None => charge = charge.with_chain_id(expected),
            Some(_) => {}
        }
    }
    Ok(charge.with_memo(attribution(&challenge.id, &challenge.realm, client_id)))
}

pub(crate) struct Funded {
    pub(crate) charge: TempoCharge,
    pub(crate) funding: Address,
    pub(crate) fee_token: Option<Address>,
}

pub(crate) async fn fund(
    mut charge: TempoCharge,
    autoswap: Option<&AutoswapConfig>,
    rpc: &Rpc,
    from: Address,
) -> Result<Funded, Fault> {
    if charge.machine_token_enabled {
        let transfers = charge.transfers()?;
        if let Some(calls) =
            find_machine_route(rpc, from, charge.chain_id, charge.currency, &transfers).await
        {
            let funding = calls[0].to;
            return Ok(Funded {
                charge: charge.with_calls(calls.to_vec()),
                funding,
                fee_token: None,
            });
        }
    }
    let currency = charge.currency;
    let mut funding = currency;
    let mut fee_token = None;
    if let Some(config) = autoswap {
        let route = resolve_autoswap_route(rpc, from, currency, charge.amount, config).await?;
        fee_token = config.fee_token_for(route.as_ref(), currency);
        if let Some(route) = route {
            funding = route.token_in;
            for call in route.calls.into_iter().rev() {
                charge = charge.with_prepended_call(call)?;
            }
        }
    }
    Ok(Funded {
        charge,
        funding,
        fee_token,
    })
}

#[derive(Debug)]
pub(crate) struct Charged {
    pub(crate) credential: Credential,
    pub(crate) reservation: Option<AuthorizationReservation>,
}

pub(crate) async fn pay(
    accounts: &Accounts,
    rpc: &Rpc,
    challenge: &Challenge,
    expected_chain_id: Option<u64>,
    client_id: Option<&str>,
    autoswap: Option<&AutoswapConfig>,
    on_funded: impl FnOnce(Address),
) -> Result<Charged, Fault> {
    let from = accounts.active_account().map_err(|error| {
        Fault::config(format!("failed to read the active Tempo account: {error}"))
    })?;
    let charge = prepare_charge_request(challenge, expected_chain_id, client_id)?;
    let funded = fund(charge, autoswap, rpc, from).await?;
    on_funded(funded.funding);
    let signed = funded
        .charge
        .sign_with_accounts(accounts, rpc, from, funded.fee_token)
        .await?;
    Ok(Charged {
        reservation: signed.authorization_reservation(),
        credential: signed.into_credential(),
    })
}

pub(crate) fn funded_by(challenge: &Challenge, currency: Address) -> bool {
    if challenge.method != METHOD_NAME {
        return false;
    }
    match challenge.intent.as_str() {
        INTENT_CHARGE => challenge
            .request
            .decode::<ChargeRequest>()
            .is_ok_and(|request| {
                request.currency.parse::<Address>().ok() == Some(currency)
                    || (request.machine_token_enabled()
                        && request.chain_id().and_then(tx::machine_token) == Some(currency))
            }),
        INTENT_SESSION => {
            challenge
                .request
                .decode::<SessionCurrency>()
                .ok()
                .and_then(|request| request.currency.parse::<Address>().ok())
                == Some(currency)
        }
        _ => false,
    }
}

pub(crate) fn settlement_rpc_url(chain_id: u64) -> Result<reqwest::Url, Fault> {
    tx::settlement_rpc_url(chain_id)
}

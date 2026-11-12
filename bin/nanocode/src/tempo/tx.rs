use std::fmt;

use nanocode_tempo::{
    Address, Call, PrimitiveSignature, Signature, SignatureType, SignedKeyAuthorization,
    TempoTransaction, U256,
    abi::{self, Token},
    hex,
};
use serde_json::{Map, Value, json};

use super::error::Fault;
use super::rpc::{Rpc, RpcFault};

pub(crate) const CHAIN_ID: u64 = 4217;
pub(crate) const MODERATO_CHAIN_ID: u64 = 42431;
pub(crate) const DEFAULT_RPC_URL: &str = "https://rpc.tempo.xyz";
pub(crate) const MODERATO_RPC_URL: &str = "https://rpc.moderato.tempo.xyz";
pub(crate) const PATH_USD: Address = address("20c0000000000000000000000000000000000000");
pub(crate) const USDC: Address = address("20C000000000000000000000b9537d11c60E8b50");
pub(crate) const OUSD: Address = address("20c0000000000000000000006a37DA5C996874BE");
pub(crate) const USDT0: Address = address("20c00000000000000000000014f22ca97301eb73");
pub(crate) const DEFAULT_CURRENCY_MAINNET: Address = USDC;
pub(crate) const DEFAULT_CURRENCY_TESTNET: Address = PATH_USD;
pub(crate) const MACH: Address = address("20c000000000000000000000f37de3740ADec032");
pub(crate) const MACH_SWAPPER_MAINNET: Address =
    address("F72E5107c32C655ffA7539a3C8e97B7C3cE16A3F");
pub(crate) const MACH_SWAPPER_TESTNET: Address =
    address("d05f8EdFBB54Da0d765C9fE9b2B3f7d2E3a8C466");
pub(crate) const ACCOUNT_KEYCHAIN_ADDRESS: Address =
    address("aAAAaaAA00000000000000000000000000000000");
pub(crate) const STABLECOIN_DEX_ADDRESS: Address =
    address("dec0000000000000000000000000000000000000");
pub(crate) const BASE_FEE_PER_GAS: u128 = 20_000_000_000;
pub(crate) const MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;
pub(crate) const MAX_FEE_PER_GAS: u128 = BASE_FEE_PER_GAS * 2 + MAX_PRIORITY_FEE_PER_GAS;
pub(crate) const FEE_PAYER_MIN_REMAINING_VALIDITY_SECS: u64 = 15;
pub(crate) const GAS_ESTIMATION_CAP: u64 = 3_000_000;
pub(crate) const GAS_ESTIMATE_BUFFER: u64 = 5_000;

const LATEST: &str = "latest";
const PENDING: &str = "pending";
const ZERO_DATA_SUFFIX: &str =
    "returned no data (\"0x\"); the called address might not be a contract";
const OVERRUN: &str = "buffer overrun while deserializing";
const SIGNATURE_BALANCE_OF: &str = "balanceOf(address)";
const SIGNATURE_TRANSFER: &str = "transfer(address,uint256)";
const SIGNATURE_TRANSFER_WITH_MEMO: &str = "transferWithMemo(address,uint256,bytes32)";
const SIGNATURE_APPROVE: &str = "approve(address,uint256)";
const SIGNATURE_GET_KEY: &str = "getKey(address,address)";
const SIGNATURE_QUOTE_SWAP: &str = "quoteSwapExactAmountOut(address,address,uint128)";
const SIGNATURE_SWAP: &str = "swapExactAmountOut(address,address,uint128,uint128)";
const SIGNATURE_SWAP_TO: &str = "swapTo(address,uint256,address,address,bytes32)";
const ERROR_INSUFFICIENT_BALANCE: &str = "InsufficientBalance(uint256,uint256,address)";
const ERROR_SPENDING_LIMIT_EXCEEDED: &str = "SpendingLimitExceeded()";
const KEY_INFO_WORDS: usize = 5;
const HEX_RADIX: u32 = 16;
const U64_BYTES: usize = 8;
const U128_BYTES: usize = 16;
const DECIMAL_DIGITS: u8 = 10;
const NIBBLE_BITS: u32 = 4;

const fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + DECIMAL_DIGITS,
        b'A'..=b'F' => digit - b'A' + DECIMAL_DIGITS,
        _ => panic!("invalid hex digit in address constant"),
    }
}

const fn address(text: &str) -> Address {
    let digits = text.as_bytes();
    let mut bytes = [0u8; nanocode_tempo::ADDRESS_BYTES];
    let mut index = 0;
    while index < nanocode_tempo::ADDRESS_BYTES {
        bytes[index] = (nibble(digits[2 * index]) << NIBBLE_BITS) | nibble(digits[2 * index + 1]);
        index += 1;
    }
    Address(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TempoNetwork {
    Mainnet,
    Moderato,
}

impl TempoNetwork {
    pub(crate) const fn from_chain_id(chain_id: u64) -> Option<Self> {
        match chain_id {
            CHAIN_ID => Some(Self::Mainnet),
            MODERATO_CHAIN_ID => Some(Self::Moderato),
            _ => None,
        }
    }

    pub(crate) const fn default_rpc_url(self) -> &'static str {
        match self {
            Self::Mainnet => DEFAULT_RPC_URL,
            Self::Moderato => MODERATO_RPC_URL,
        }
    }

    pub(crate) const fn default_currency(self) -> Address {
        match self {
            Self::Mainnet => DEFAULT_CURRENCY_MAINNET,
            Self::Moderato => DEFAULT_CURRENCY_TESTNET,
        }
    }
}

pub(crate) const fn machine_token(chain_id: u64) -> Option<Address> {
    match TempoNetwork::from_chain_id(chain_id) {
        Some(_) => Some(MACH),
        None => None,
    }
}

pub(crate) const fn machine_swapper(chain_id: u64) -> Option<Address> {
    match TempoNetwork::from_chain_id(chain_id) {
        Some(TempoNetwork::Mainnet) => Some(MACH_SWAPPER_MAINNET),
        Some(TempoNetwork::Moderato) => Some(MACH_SWAPPER_TESTNET),
        None => None,
    }
}

pub(crate) fn settlement_rpc_url(chain_id: u64) -> Result<reqwest::Url, Fault> {
    let network = TempoNetwork::from_chain_id(chain_id).ok_or_else(|| {
        Fault::config(format!(
            "unknown chain ID {chain_id}: configure an explicit settlement RPC URL"
        ))
    })?;
    network
        .default_rpc_url()
        .parse()
        .map_err(|error| Fault::config(format!("invalid default Tempo RPC URL: {error}")))
}

pub(crate) fn quantity(value: u128) -> Value {
    Value::from(format!("{value:#x}"))
}

pub(crate) fn address_json(address: &Address) -> Value {
    Value::from(format!("{address:#x}"))
}

pub(crate) fn bytes_json(bytes: &[u8]) -> Value {
    Value::from(hex::encode_prefixed(bytes))
}

fn word_quantity(word: &[u8; 32]) -> Value {
    Value::from(U256::from_be_bytes(*word).to_hex_quantity())
}

pub(crate) const fn signature_type_name(kind: SignatureType) -> &'static str {
    match kind {
        SignatureType::Secp256k1 => "secp256k1",
        SignatureType::P256 => "p256",
        SignatureType::WebAuthn => "webAuthn",
    }
}

fn recoverable_json(signature: &Signature) -> Map<String, Value> {
    let parity = quantity(u128::from(signature.y_parity));
    let mut map = Map::new();
    map.insert("r".into(), word_quantity(&signature.r));
    map.insert("s".into(), word_quantity(&signature.s));
    map.insert("yParity".into(), parity.clone());
    map.insert("v".into(), parity);
    map
}

pub(crate) fn primitive_signature_json(signature: &PrimitiveSignature) -> Value {
    let mut map = Map::new();
    map.insert(
        "type".into(),
        signature_type_name(signature.signature_type()).into(),
    );
    match signature {
        PrimitiveSignature::Secp256k1(signature) => map.extend(recoverable_json(signature)),
        PrimitiveSignature::P256(signature) => {
            map.insert("r".into(), bytes_json(&signature.r));
            map.insert("s".into(), bytes_json(&signature.s));
            map.insert("pubKeyX".into(), bytes_json(&signature.public_x));
            map.insert("pubKeyY".into(), bytes_json(&signature.public_y));
            map.insert("preHash".into(), signature.pre_hash.into());
        }
        PrimitiveSignature::WebAuthn(signature) => {
            map.insert("r".into(), bytes_json(&signature.r));
            map.insert("s".into(), bytes_json(&signature.s));
            map.insert("pubKeyX".into(), bytes_json(&signature.public_x));
            map.insert("pubKeyY".into(), bytes_json(&signature.public_y));
            map.insert("webauthnData".into(), bytes_json(&signature.data));
        }
    }
    Value::Object(map)
}

pub(crate) fn key_authorization_json(signed: &SignedKeyAuthorization) -> Value {
    let authorization = &signed.authorization;
    let limits = authorization.limits.as_ref().map(|limits| {
        limits
            .iter()
            .map(|limit| {
                json!({
                    "token": address_json(&limit.token),
                    "limit": limit.limit.to_hex_quantity(),
                    "period": quantity(u128::from(limit.period)),
                })
            })
            .collect::<Vec<_>>()
    });
    let allowed_calls = authorization.allowed_calls.as_ref().map(|scopes| {
        scopes
            .iter()
            .map(|scope| {
                let mut entry = Map::new();
                entry.insert("target".into(), address_json(&scope.target));
                if !scope.selector_rules.is_empty() {
                    let rules = scope
                        .selector_rules
                        .iter()
                        .map(|rule| {
                            let mut item = Map::new();
                            item.insert("selector".into(), bytes_json(&rule.selector));
                            if !rule.recipients.is_empty() {
                                item.insert(
                                    "recipients".into(),
                                    rule.recipients.iter().map(address_json).collect(),
                                );
                            }
                            Value::Object(item)
                        })
                        .collect();
                    entry.insert("selectorRules".into(), Value::Array(rules));
                }
                Value::Object(entry)
            })
            .collect::<Vec<_>>()
    });
    let mut map = Map::new();
    map.insert(
        "chainId".into(),
        quantity(u128::from(authorization.chain_id)),
    );
    map.insert(
        "keyType".into(),
        signature_type_name(authorization.key_type).into(),
    );
    map.insert("keyId".into(), address_json(&authorization.key_id));
    map.insert(
        "expiry".into(),
        authorization
            .expiry
            .map_or(Value::Null, |expiry| quantity(u128::from(expiry))),
    );
    map.insert("limits".into(), limits.map_or(Value::Null, Value::from));
    map.insert(
        "allowedCalls".into(),
        allowed_calls.map_or(Value::Null, Value::from),
    );
    map.insert(
        "witness".into(),
        authorization
            .witness
            .map_or(Value::Null, |witness| bytes_json(&witness)),
    );
    map.insert("isAdmin".into(), authorization.is_admin.into());
    map.insert(
        "account".into(),
        authorization
            .account
            .as_ref()
            .map_or(Value::Null, address_json),
    );
    map.insert(
        "signature".into(),
        primitive_signature_json(&signed.signature),
    );
    Value::Object(map)
}

pub(crate) fn call_json(call: &Call) -> Value {
    json!({
        "to": address_json(&call.to),
        "value": call.value.to_hex_quantity(),
        "input": bytes_json(&call.input),
        "data": Value::Null,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TempoTransactionRequest {
    pub(crate) from: Option<Address>,
    pub(crate) to: Option<Address>,
    pub(crate) max_fee_per_gas: Option<u128>,
    pub(crate) max_priority_fee_per_gas: Option<u128>,
    pub(crate) gas: Option<u64>,
    pub(crate) value: Option<U256>,
    pub(crate) input: Option<Vec<u8>>,
    pub(crate) nonce: Option<u64>,
    pub(crate) chain_id: Option<u64>,
    pub(crate) fee_token: Option<Address>,
    pub(crate) nonce_key: Option<U256>,
    pub(crate) calls: Vec<Call>,
    pub(crate) key_type: Option<SignatureType>,
    pub(crate) key_id: Option<Address>,
    pub(crate) key_authorization: Option<SignedKeyAuthorization>,
    pub(crate) valid_before: Option<u64>,
    pub(crate) valid_after: Option<u64>,
    pub(crate) fee_payer_signature: Option<Signature>,
}

impl TempoTransactionRequest {
    pub(crate) fn with_valid_before(mut self, valid_before: u64) -> Self {
        self.valid_before = (valid_before != 0).then_some(valid_before);
        self
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut map = Map::new();
        if let Some(from) = &self.from {
            map.insert("from".into(), address_json(from));
        }
        if let Some(to) = &self.to {
            map.insert("to".into(), address_json(to));
        }
        if let Some(fee) = self.max_fee_per_gas {
            map.insert("maxFeePerGas".into(), quantity(fee));
        }
        if let Some(fee) = self.max_priority_fee_per_gas {
            map.insert("maxPriorityFeePerGas".into(), quantity(fee));
        }
        if let Some(gas) = self.gas {
            map.insert("gas".into(), quantity(u128::from(gas)));
        }
        if let Some(value) = &self.value {
            map.insert("value".into(), value.to_hex_quantity().into());
        }
        if let Some(input) = &self.input {
            map.insert("input".into(), bytes_json(input));
        }
        if let Some(nonce) = self.nonce {
            map.insert("nonce".into(), quantity(u128::from(nonce)));
        }
        if let Some(chain_id) = self.chain_id {
            map.insert("chainId".into(), quantity(u128::from(chain_id)));
        }
        map.insert(
            "feeToken".into(),
            self.fee_token.as_ref().map_or(Value::Null, address_json),
        );
        if let Some(nonce_key) = &self.nonce_key {
            map.insert("nonceKey".into(), nonce_key.to_hex_quantity().into());
        }
        map.insert("calls".into(), self.calls.iter().map(call_json).collect());
        map.insert(
            "keyType".into(),
            self.key_type
                .map_or(Value::Null, |kind| signature_type_name(kind).into()),
        );
        map.insert("keyData".into(), Value::Null);
        if let Some(key_id) = &self.key_id {
            map.insert("keyId".into(), address_json(key_id));
        }
        if let Some(authorization) = &self.key_authorization {
            map.insert(
                "keyAuthorization".into(),
                key_authorization_json(authorization),
            );
        }
        if let Some(valid_before) = self.valid_before {
            map.insert("validBefore".into(), quantity(u128::from(valid_before)));
        }
        if let Some(valid_after) = self.valid_after {
            map.insert("validAfter".into(), quantity(u128::from(valid_after)));
        }
        if let Some(signature) = &self.fee_payer_signature {
            map.insert(
                "feePayerSignature".into(),
                Value::Object(recoverable_json(signature)),
            );
        }
        Value::Object(map)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TempoTxOptions {
    pub(crate) calls: Vec<Call>,
    pub(crate) chain_id: u64,
    pub(crate) fee_token: Address,
    pub(crate) nonce: u64,
    pub(crate) nonce_key: U256,
    pub(crate) gas_limit: u64,
    pub(crate) max_fee_per_gas: u128,
    pub(crate) max_priority_fee_per_gas: u128,
    pub(crate) fee_payer: bool,
    pub(crate) valid_before: Option<u64>,
    pub(crate) key_authorization: Option<SignedKeyAuthorization>,
}

pub(crate) const FEE_PAYER_PLACEHOLDER: Signature = Signature {
    r: [0; 32],
    s: [0; 32],
    y_parity: false,
};

pub(crate) fn build_tempo_tx(options: TempoTxOptions) -> TempoTransaction {
    TempoTransaction {
        chain_id: options.chain_id,
        fee_token: (!options.fee_payer).then_some(options.fee_token),
        max_priority_fee_per_gas: options.max_priority_fee_per_gas,
        max_fee_per_gas: options.max_fee_per_gas,
        gas_limit: options.gas_limit,
        calls: options.calls,
        nonce_key: options.nonce_key,
        nonce: options.nonce,
        fee_payer_signature: options.fee_payer.then_some(FEE_PAYER_PLACEHOLDER),
        valid_before: options
            .valid_before
            .filter(|valid_before| *valid_before != 0),
        valid_after: None,
        key_authorization: options.key_authorization,
    }
}

pub(crate) fn transaction_from_request(
    request: &TempoTransactionRequest,
) -> Result<TempoTransaction, &'static str> {
    if request.calls.is_empty() && request.to.is_none() {
        return Err("Missing 'calls' or 'to' field for Tempo transaction.");
    }
    let nonce = request
        .nonce
        .ok_or("Missing 'nonce' field for Tempo transaction.")?;
    let gas_limit = request
        .gas
        .ok_or("Missing 'gas_limit' field for Tempo transaction.")?;
    let max_fee_per_gas = request
        .max_fee_per_gas
        .ok_or("Missing 'max_fee_per_gas' field for Tempo transaction.")?;
    let max_priority_fee_per_gas = request
        .max_priority_fee_per_gas
        .ok_or("Missing 'max_priority_fee_per_gas' field for Tempo transaction.")?;
    let mut calls = request.calls.clone();
    if let Some(to) = request.to {
        calls.push(Call {
            to,
            value: request.value.unwrap_or_default(),
            input: request.input.clone().unwrap_or_default(),
        });
    }
    Ok(TempoTransaction {
        chain_id: request.chain_id.unwrap_or(CHAIN_ID),
        fee_token: request.fee_token,
        max_priority_fee_per_gas,
        max_fee_per_gas,
        gas_limit,
        calls,
        nonce_key: request.nonce_key.unwrap_or_default(),
        nonce,
        fee_payer_signature: request.fee_payer_signature,
        valid_before: request.valid_before,
        valid_after: request.valid_after,
        key_authorization: request.key_authorization.clone(),
    })
}

fn extract_field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("{key}: ");
    let start = raw.find(&needle)? + needle.len();
    let rest = &raw[start..];
    let end = rest.find([',', '}', ')']).unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn spelunk_revert(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(text) => hex::decode(text).ok(),
        Value::Object(map) => map.values().find_map(spelunk_revert),
        _ => None,
    }
}

fn revert_data(error: &RpcFault) -> Option<Vec<u8>> {
    let message = error.message()?;
    if !message.contains("revert") {
        return None;
    }
    spelunk_revert(error.data()?)
}

pub(crate) fn classify_transport_error(error: &RpcFault) -> Option<Fault> {
    if let Some(data) = revert_data(error) {
        let selector = abi::selector(ERROR_INSUFFICIENT_BALANCE);
        if let Some(arguments) = data.strip_prefix(selector.as_slice()) {
            let mut decoder = abi::Decoder::new(arguments);
            if let (Ok(available), Ok(required), Ok(token)) =
                (decoder.uint(), decoder.uint(), decoder.word())
            {
                let token =
                    Address::from_slice(&token[abi::WORD - nanocode_tempo::ADDRESS_BYTES..])
                        .unwrap_or_default();
                return Some(Fault::TokenBalance {
                    token: token.to_string(),
                    available: available.to_string(),
                    required: required.to_string(),
                });
            }
        }
        if data.starts_with(&abi::selector(ERROR_SPENDING_LIMIT_EXCEEDED)) {
            let message = error.to_string();
            return Some(Fault::SpendingLimitExceeded {
                token: extract_field(&message, "token").unwrap_or_default(),
                limit: extract_field(&message, "limit").unwrap_or_default(),
                required: extract_field(&message, "required").unwrap_or(message),
            });
        }
    }
    classify_rpc_error(error.to_string())
}

pub(crate) fn classify_rpc_error(message: String) -> Option<Fault> {
    let lower = message.to_lowercase();
    if lower.contains("not provisioned") {
        return Some(Fault::AccessKeyNotProvisioned);
    }
    if lower.contains("spendinglimitexceeded") || lower.contains("spending limit") {
        return Some(Fault::SpendingLimitExceeded {
            token: extract_field(&message, "token").unwrap_or_default(),
            limit: extract_field(&message, "limit").unwrap_or_default(),
            required: extract_field(&message, "required").unwrap_or(message),
        });
    }
    if lower.contains("insufficientbalance")
        || lower.contains("transfer amount exceeds balance")
        || (lower.contains("insufficient") && lower.contains("balance"))
    {
        return Some(Fault::TokenBalance {
            token: extract_field(&message, "token").unwrap_or_default(),
            available: extract_field(&message, "available").unwrap_or_default(),
            required: extract_field(&message, "required").unwrap_or(message),
        });
    }
    if lower.contains("revert") || lower.contains("execution reverted") {
        return Some(Fault::Reverted(message));
    }
    None
}

fn parse_quantity(value: &Value) -> Result<u64, RpcFault> {
    let text = value.as_str().ok_or_else(|| {
        RpcFault::Decode(format!("invalid type: {value}, expected a hex quantity"))
    })?;
    let digits = text
        .strip_prefix("0x")
        .ok_or_else(|| RpcFault::Decode(format!("missing 0x prefix in {text}")))?;
    u64::from_str_radix(digits, HEX_RADIX).map_err(|error| RpcFault::Decode(error.to_string()))
}

pub(crate) async fn estimate_gas(
    rpc: &Rpc,
    mut request: TempoTransactionRequest,
) -> Result<u64, Fault> {
    request.gas = Some(GAS_ESTIMATION_CAP);
    let estimated = match rpc
        .call("eth_estimateGas", json!([request.to_json(), PENDING]))
        .await
    {
        Ok(value) => parse_quantity(&value),
        Err(error) => Err(error),
    };
    estimated
        .map(|gas| gas.saturating_add(GAS_ESTIMATE_BUFFER))
        .map_err(|error| {
            classify_transport_error(&error)
                .unwrap_or_else(|| Fault::http(format!("gas estimation failed: {error}")))
        })
}

pub(crate) async fn chain_id(rpc: &Rpc) -> Result<u64, RpcFault> {
    let value = rpc.call("eth_chainId", json!([])).await?;
    parse_quantity(&value)
}

pub(crate) async fn eth_call(
    rpc: &Rpc,
    request: &TempoTransactionRequest,
) -> Result<Vec<u8>, RpcFault> {
    eth_call_at(rpc, request, LATEST).await
}

#[derive(Debug, Clone)]
pub(crate) enum CallError {
    Rpc(RpcFault),
    ZeroData(&'static str),
    Overrun,
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc(error) => write!(f, "{error}"),
            Self::ZeroData(function) => {
                write!(f, "contract call to `{function}` {ZERO_DATA_SUFFIX}")
            }
            Self::Overrun => f.write_str(OVERRUN),
        }
    }
}

impl std::error::Error for CallError {}

async fn contract_call(
    rpc: &Rpc,
    to: Address,
    input: Vec<u8>,
    function: &'static str,
    words: usize,
) -> Result<Vec<u8>, CallError> {
    let request = TempoTransactionRequest {
        to: Some(to),
        input: Some(input),
        ..TempoTransactionRequest::default()
    };
    let output = eth_call(rpc, &request).await.map_err(CallError::Rpc)?;
    if output.is_empty() {
        return Err(CallError::ZeroData(function));
    }
    if output.len() < words * abi::WORD {
        return Err(CallError::Overrun);
    }
    Ok(output)
}

pub(crate) fn balance_of_call(owner: Address) -> Vec<u8> {
    abi::call(SIGNATURE_BALANCE_OF, &[Token::Address(owner)])
}

pub(crate) async fn balance_of(
    rpc: &Rpc,
    token: Address,
    owner: Address,
) -> Result<U256, CallError> {
    let output = contract_call(rpc, token, balance_of_call(owner), "balanceOf", 1).await?;
    abi::Decoder::new(&output)
        .uint()
        .map_err(|_| CallError::Overrun)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyInfo {
    pub(crate) signature_type: u8,
    pub(crate) key_id: Address,
    pub(crate) expiry: u64,
    pub(crate) enforce_limits: bool,
    pub(crate) is_revoked: bool,
}

fn low_u64(word: &[u8; 32]) -> u64 {
    let mut bytes = [0u8; U64_BYTES];
    bytes.copy_from_slice(&word[abi::WORD - U64_BYTES..]);
    u64::from_be_bytes(bytes)
}

pub(crate) async fn get_key(
    rpc: &Rpc,
    account: Address,
    key_id: Address,
) -> Result<KeyInfo, CallError> {
    let input = abi::call(
        SIGNATURE_GET_KEY,
        &[Token::Address(account), Token::Address(key_id)],
    );
    let output = contract_call(
        rpc,
        ACCOUNT_KEYCHAIN_ADDRESS,
        input,
        "getKey",
        KEY_INFO_WORDS,
    )
    .await?;
    let mut decoder = abi::Decoder::new(&output);
    let words: [[u8; 32]; KEY_INFO_WORDS] = [
        decoder.word().map_err(|_| CallError::Overrun)?,
        decoder.word().map_err(|_| CallError::Overrun)?,
        decoder.word().map_err(|_| CallError::Overrun)?,
        decoder.word().map_err(|_| CallError::Overrun)?,
        decoder.word().map_err(|_| CallError::Overrun)?,
    ];
    Ok(KeyInfo {
        signature_type: words[0][abi::WORD - 1],
        key_id: Address::from_slice(&words[1][abi::WORD - nanocode_tempo::ADDRESS_BYTES..])
            .unwrap_or_default(),
        expiry: low_u64(&words[2]),
        enforce_limits: words[3][abi::WORD - 1] != 0,
        is_revoked: words[4][abi::WORD - 1] != 0,
    })
}

pub(crate) async fn quote_swap_exact_amount_out(
    rpc: &Rpc,
    token_in: Address,
    token_out: Address,
    amount_out: u128,
) -> Result<u128, CallError> {
    let input = abi::call(
        SIGNATURE_QUOTE_SWAP,
        &[
            Token::Address(token_in),
            Token::Address(token_out),
            Token::Uint(U256::from_u128(amount_out)),
        ],
    );
    let output = contract_call(
        rpc,
        STABLECOIN_DEX_ADDRESS,
        input,
        "quoteSwapExactAmountOut",
        1,
    )
    .await?;
    let word = abi::Decoder::new(&output)
        .word()
        .map_err(|_| CallError::Overrun)?;
    let mut bytes = [0u8; U128_BYTES];
    bytes.copy_from_slice(&word[abi::WORD - U128_BYTES..]);
    Ok(u128::from_be_bytes(bytes))
}

pub(crate) fn transfer_call(to: Address, amount: U256) -> Vec<u8> {
    abi::call(
        SIGNATURE_TRANSFER,
        &[Token::Address(to), Token::Uint(amount)],
    )
}

pub(crate) fn transfer_with_memo_call(to: Address, amount: U256, memo: [u8; 32]) -> Vec<u8> {
    abi::call(
        SIGNATURE_TRANSFER_WITH_MEMO,
        &[
            Token::Address(to),
            Token::Uint(amount),
            Token::FixedBytes(memo),
        ],
    )
}

pub(crate) fn approve_call(spender: Address, amount: U256) -> Vec<u8> {
    abi::call(
        SIGNATURE_APPROVE,
        &[Token::Address(spender), Token::Uint(amount)],
    )
}

pub(crate) fn swap_exact_amount_out_call(
    token_in: Address,
    token_out: Address,
    amount_out: u128,
    max_amount_in: u128,
) -> Vec<u8> {
    abi::call(
        SIGNATURE_SWAP,
        &[
            Token::Address(token_in),
            Token::Address(token_out),
            Token::Uint(U256::from_u128(amount_out)),
            Token::Uint(U256::from_u128(max_amount_in)),
        ],
    )
}

pub(crate) fn swap_to_call(
    input_token: Address,
    amount: U256,
    target_token: Address,
    recipient: Address,
    memo: [u8; 32],
) -> Vec<u8> {
    abi::call(
        SIGNATURE_SWAP_TO,
        &[
            Token::Address(input_token),
            Token::Uint(amount),
            Token::Address(target_token),
            Token::Address(recipient),
            Token::FixedBytes(memo),
        ],
    )
}

pub(crate) async fn send_raw_transaction_sync(
    rpc: &Rpc,
    transaction: &[u8],
) -> Result<Value, RpcFault> {
    rpc.call(
        "eth_sendRawTransactionSync",
        json!([hex::encode_prefixed(transaction)]),
    )
    .await
}

pub(crate) async fn raw_request(rpc: &Rpc, method: &str, params: Value) -> Result<Value, RpcFault> {
    rpc.call(method, params).await
}

pub(crate) async fn gas_price(rpc: &Rpc) -> Result<u128, RpcFault> {
    let value = rpc.call_without_params("eth_gasPrice").await?;
    let text = value.as_str().ok_or_else(|| {
        RpcFault::Decode(format!("invalid type: {value}, expected a hex quantity"))
    })?;
    let digits = text
        .strip_prefix("0x")
        .ok_or_else(|| RpcFault::Decode(format!("missing 0x prefix in {text}")))?;
    u128::from_str_radix(digits, HEX_RADIX).map_err(|error| RpcFault::Decode(error.to_string()))
}

pub(crate) async fn eth_call_pending(
    rpc: &Rpc,
    request: &TempoTransactionRequest,
) -> Result<Vec<u8>, RpcFault> {
    eth_call_at(rpc, request, PENDING).await
}

async fn eth_call_at(
    rpc: &Rpc,
    request: &TempoTransactionRequest,
    block: &str,
) -> Result<Vec<u8>, RpcFault> {
    let value = rpc
        .call("eth_call", json!([request.to_json(), block]))
        .await?;
    let text = value
        .as_str()
        .ok_or_else(|| RpcFault::Decode(format!("invalid type: {value}, expected hex bytes")))?;
    hex::decode(text).map_err(|error| RpcFault::Decode(error.to_string()))
}

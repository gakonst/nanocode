use nanocode_tempo::abi::{self, Token};
use nanocode_tempo::{Address, U256, hex, keccak256};
use serde_json::{Value, json};

use super::super::error::Fault;
use super::super::protocol::Challenge;
use super::super::rpc::Rpc;
use super::wire::{
    ChannelDescriptor, SESSION_PROTOCOL_LEGACY, SESSION_PROTOCOL_TIP1034, SessionRequest,
};

pub(crate) type Hash = [u8; 32];

pub(crate) const TIP20_CHANNEL_RESERVE_ADDRESS: Address = Address([
    0x4d, 0x50, 0x50, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
pub(crate) const PRECOMPILE_MAX_CUMULATIVE_AMOUNT: u128 = (1 << 96) - 1;
pub(crate) const DEFAULT_CHAIN_ID: u64 = 4217;
const TESTNET_CHAIN_ID: u64 = 42431;
const MAINNET_LEGACY_ESCROW: &str = "0x33b901018174DDabE4841042ab76ba85D4e24f25";
const TESTNET_LEGACY_ESCROW: &str = "0xe1c4d3dce17bc111181ddf716f75bae49e61a336";
const DOMAIN_NAME: &str = "TIP20 Channel Reserve";
const DOMAIN_VERSION: &str = "1";
const DOMAIN_TYPE: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";
const VOUCHER_TYPE: &str = "Voucher(bytes32 channelId,uint96 cumulativeAmount)";
const CLOSE_AUTHORIZATION_TYPE: &str =
    "CloseAuthorization(bytes32 channelId,uint96 cumulativeAmount)";
const EIP712_PREFIX: [u8; 2] = [0x19, 0x01];
const OPEN_SIGNATURE: &str = "open(address,address,address,uint96,bytes32,address)";
const TOP_UP_SIGNATURE: &str =
    "topUp((address,address,address,address,bytes32,address,bytes32),uint96)";
const GET_CHANNEL_STATE_SIGNATURE: &str = "getChannelState(bytes32)";
const UINT96_BYTES: usize = 12;
const UINT32_BYTES: usize = 4;
const STATE_WORDS: usize = 3;
const PENDING_BLOCK: &str = "pending";

pub(crate) fn hash_text(hash: &Hash) -> String {
    hex::encode_prefixed(hash)
}

pub(crate) fn parse_hash(text: &str) -> Result<Hash, nanocode_tempo::Error> {
    hex::decode_array(text)
}

pub(crate) fn random_hash() -> Result<Hash, Fault> {
    let mut hash = [0u8; 32];
    getrandom::fill(&mut hash)
        .map_err(|error| Fault::Internal(format!("randomness unavailable: {error}")))?;
    Ok(hash)
}

pub(crate) fn is_precompile_escrow(address: Address) -> bool {
    address == TIP20_CHANNEL_RESERVE_ADDRESS
}

pub(crate) fn resolve_chain_id(challenge: &Challenge) -> u64 {
    challenge
        .request
        .decode::<SessionRequest>()
        .ok()
        .and_then(|request| request.chain_id())
        .unwrap_or(DEFAULT_CHAIN_ID)
}

fn default_escrow_contract(chain_id: u64) -> Option<Address> {
    match chain_id {
        DEFAULT_CHAIN_ID => MAINNET_LEGACY_ESCROW.parse().ok(),
        TESTNET_CHAIN_ID => TESTNET_LEGACY_ESCROW.parse().ok(),
        _ => None,
    }
}

pub(crate) fn resolve_escrow(challenge: &Challenge, chain_id: u64) -> Result<Address, Fault> {
    let request = challenge.request.decode::<SessionRequest>().ok();
    let advertised = request.as_ref().and_then(SessionRequest::escrow);
    let tip1034 = match request.as_ref().and_then(SessionRequest::session_protocol) {
        Some(SESSION_PROTOCOL_TIP1034) => true,
        Some(SESSION_PROTOCOL_LEGACY) => false,
        Some(other) => {
            return Err(Fault::config(format!(
                "unsupported Tempo session protocol {other:?}"
            )));
        }
        None => advertised == Some(TIP20_CHANNEL_RESERVE_ADDRESS),
    };
    let trusted = match tip1034 {
        true => Some(TIP20_CHANNEL_RESERVE_ADDRESS),
        false => default_escrow_contract(chain_id),
    };
    match (advertised, trusted) {
        (Some(advertised), Some(trusted)) if advertised == trusted => Ok(trusted),
        (Some(advertised), Some(trusted)) => Err(Fault::config(format!(
            "Tempo session escrow {advertised} does not match client escrow {trusted}"
        ))),
        (None, Some(trusted)) => Ok(trusted),
        (_, None) => Err(Fault::config(
            "No trusted escrowContract available. Provide it in parameters or use a supported chain.",
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Descriptor {
    pub(crate) payer: Address,
    pub(crate) payee: Address,
    pub(crate) operator: Address,
    pub(crate) token: Address,
    pub(crate) salt: Hash,
    pub(crate) authorized_signer: Address,
    pub(crate) expiring_nonce_hash: Hash,
}

fn descriptor_field<T>(field: &str, parsed: Result<T, nanocode_tempo::Error>) -> Result<T, Fault> {
    parsed.map_err(|error| Fault::config(format!("invalid TIP-1034 descriptor {field}: {error}")))
}

impl Descriptor {
    pub(crate) fn from_wire(descriptor: &ChannelDescriptor) -> Result<Self, Fault> {
        Ok(Self {
            payer: descriptor_field("payer", descriptor.payer.parse())?,
            payee: descriptor_field("payee", descriptor.payee.parse())?,
            operator: descriptor_field("operator", descriptor.operator.parse())?,
            token: descriptor_field("token", descriptor.token.parse())?,
            salt: descriptor_field("salt", parse_hash(&descriptor.salt))?,
            authorized_signer: descriptor_field(
                "authorizedSigner",
                descriptor.authorized_signer.parse(),
            )?,
            expiring_nonce_hash: descriptor_field(
                "expiringNonceHash",
                parse_hash(&descriptor.expiring_nonce_hash),
            )?,
        })
    }

    pub(crate) fn to_wire(self) -> ChannelDescriptor {
        ChannelDescriptor {
            payer: self.payer.to_string(),
            payee: self.payee.to_string(),
            operator: self.operator.to_string(),
            token: self.token.to_string(),
            salt: hash_text(&self.salt),
            authorized_signer: self.authorized_signer.to_string(),
            expiring_nonce_hash: hash_text(&self.expiring_nonce_hash),
        }
    }

    fn tokens(self) -> Vec<Token> {
        vec![
            Token::Address(self.payer),
            Token::Address(self.payee),
            Token::Address(self.operator),
            Token::Address(self.token),
            Token::FixedBytes(self.salt),
            Token::Address(self.authorized_signer),
            Token::FixedBytes(self.expiring_nonce_hash),
        ]
    }

    pub(crate) fn channel_id(self, escrow: Address, chain_id: u64) -> Hash {
        let mut tokens = self.tokens();
        tokens.push(Token::Address(escrow));
        tokens.push(Token::Uint(U256::from_u64(chain_id)));
        keccak256(abi::encode(&tokens))
    }
}

pub(crate) fn wire_channel_id(
    descriptor: &ChannelDescriptor,
    escrow: Address,
    chain_id: u64,
) -> Result<Hash, Fault> {
    Descriptor::from_wire(descriptor).map(|descriptor| descriptor.channel_id(escrow, chain_id))
}

fn precompile_amount(value: u128, label: &str) -> Result<U256, Fault> {
    if value > PRECOMPILE_MAX_CUMULATIVE_AMOUNT {
        return Err(Fault::config(format!(
            "{label} {value} exceeds precompile uint96 max"
        )));
    }
    Ok(U256::from_u128(value))
}

fn domain_separator(escrow: Address, chain_id: u64) -> Hash {
    keccak256(abi::encode(&[
        Token::FixedBytes(keccak256(DOMAIN_TYPE.as_bytes())),
        Token::FixedBytes(keccak256(DOMAIN_NAME.as_bytes())),
        Token::FixedBytes(keccak256(DOMAIN_VERSION.as_bytes())),
        Token::Uint(U256::from_u64(chain_id)),
        Token::Address(escrow),
    ]))
}

fn typed_hash(
    struct_type: &str,
    channel_id: &Hash,
    cumulative_amount: u128,
    escrow: Address,
    chain_id: u64,
) -> Result<Hash, Fault> {
    let amount = precompile_amount(cumulative_amount, "cumulative_amount")?;
    let struct_hash = keccak256(abi::encode(&[
        Token::FixedBytes(keccak256(struct_type.as_bytes())),
        Token::FixedBytes(*channel_id),
        Token::Uint(amount),
    ]));
    let mut message = Vec::with_capacity(EIP712_PREFIX.len() + 2 * struct_hash.len());
    message.extend_from_slice(&EIP712_PREFIX);
    message.extend_from_slice(&domain_separator(escrow, chain_id));
    message.extend_from_slice(&struct_hash);
    Ok(keccak256(message))
}

pub(crate) fn voucher_signing_hash(
    channel_id: &Hash,
    cumulative_amount: u128,
    escrow: Address,
    chain_id: u64,
) -> Result<Hash, Fault> {
    typed_hash(
        VOUCHER_TYPE,
        channel_id,
        cumulative_amount,
        escrow,
        chain_id,
    )
}

pub(crate) fn close_authorization_signing_hash(
    channel_id: &Hash,
    cumulative_amount: u128,
    escrow: Address,
    chain_id: u64,
) -> Result<Hash, Fault> {
    typed_hash(
        CLOSE_AUTHORIZATION_TYPE,
        channel_id,
        cumulative_amount,
        escrow,
        chain_id,
    )
}

pub(crate) fn verify_voucher_signature(
    signature: &[u8],
    expected_signer: Address,
    channel_id: &Hash,
    cumulative_amount: u128,
    escrow: Address,
    chain_id: u64,
) -> Result<bool, Fault> {
    let hash = voucher_signing_hash(channel_id, cumulative_amount, escrow, chain_id)?;
    let primitive = nanocode_tempo::PrimitiveSignature::from_bytes(signature)
        .map_err(|error| Fault::config(format!("invalid snapshot voucher signature: {error}")))?;
    if primitive.to_bytes() != signature {
        return Ok(false);
    }
    Ok(primitive
        .recover(&hash)
        .is_ok_and(|address| address == expected_signer))
}

pub(crate) fn open_call(
    payee: Address,
    operator: Address,
    token: Address,
    deposit: u128,
    salt: Hash,
    authorized_signer: Address,
) -> Result<Vec<u8>, Fault> {
    Ok(abi::call(
        OPEN_SIGNATURE,
        &[
            Token::Address(payee),
            Token::Address(operator),
            Token::Address(token),
            Token::Uint(precompile_amount(deposit, "deposit")?),
            Token::FixedBytes(salt),
            Token::Address(authorized_signer),
        ],
    ))
}

pub(crate) fn top_up_call(
    descriptor: Descriptor,
    additional_deposit: u128,
) -> Result<Vec<u8>, Fault> {
    Ok(abi::call(
        TOP_UP_SIGNATURE,
        &[
            Token::Tuple(descriptor.tokens()),
            Token::Uint(precompile_amount(additional_deposit, "additional_deposit")?),
        ],
    ))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OnChainChannelState {
    pub(crate) settled: u128,
    pub(crate) deposit: u128,
    pub(crate) close_requested_at: u64,
}

fn low_bytes(word: &[u8; abi::WORD], bytes: usize) -> u128 {
    word[abi::WORD - bytes..]
        .iter()
        .fold(0u128, |value, byte| (value << u8::BITS) | u128::from(*byte))
}

fn low_u32(word: &[u8; abi::WORD]) -> u64 {
    let mut bytes = [0u8; UINT32_BYTES];
    bytes.copy_from_slice(&word[abi::WORD - UINT32_BYTES..]);
    u64::from(u32::from_be_bytes(bytes))
}

fn decode_channel_state(data: &[u8]) -> Result<OnChainChannelState, Fault> {
    let mut decoder = abi::Decoder::new(data);
    let words: Vec<[u8; abi::WORD]> = (0..STATE_WORDS)
        .map(|_| decoder.word())
        .collect::<Result<_, _>>()
        .map_err(|error| {
            Fault::http(format!("failed to decode TIP-1034 channel state: {error}"))
        })?;
    Ok(OnChainChannelState {
        settled: low_bytes(&words[0], UINT96_BYTES),
        deposit: low_bytes(&words[1], UINT96_BYTES),
        close_requested_at: low_u32(&words[2]),
    })
}

pub(crate) fn call_request(to: Address, input: &[u8]) -> Value {
    json!({
        "to": format!("{to:#x}"),
        "input": hex::encode_prefixed(input),
        "feeToken": null,
        "calls": [],
        "keyType": null,
        "keyData": null,
    })
}

pub(crate) async fn read_channel_state(
    rpc: &Rpc,
    channel_id: &Hash,
    block: Option<u64>,
) -> Result<OnChainChannelState, Fault> {
    let input = abi::call(
        GET_CHANNEL_STATE_SIGNATURE,
        &[Token::FixedBytes(*channel_id)],
    );
    let block = match block {
        Some(number) => Value::from(format!("{number:#x}")),
        None => Value::from(PENDING_BLOCK),
    };
    let result: String = rpc
        .call_as(
            "eth_call",
            json!([call_request(TIP20_CHANNEL_RESERVE_ADDRESS, &input), block]),
        )
        .await
        .map_err(|error| Fault::http(format!("failed to read TIP-1034 channel state: {error}")))?;
    let data = hex::decode(&result)
        .map_err(|error| Fault::http(format!("failed to read TIP-1034 channel state: {error}")))?;
    decode_channel_state(&data)
}

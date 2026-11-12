use crate::{
    Address, U256,
    ecdsa::Signature,
    keccak::keccak256,
    rlp,
    signature::{SignedKeyAuthorization, TempoSignature},
};

pub const TEMPO_TRANSACTION_TYPE: u8 = 0x76;
pub const FEE_PAYER_SIGNATURE_TYPE: u8 = 0x78;
const FEE_PAYER_PLACEHOLDER: u8 = 0x00;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub to: Address,
    pub value: U256,
    pub input: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TempoTransaction {
    pub chain_id: u64,
    pub fee_token: Option<Address>,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub calls: Vec<Call>,
    pub nonce_key: U256,
    pub nonce: u64,
    pub fee_payer_signature: Option<Signature>,
    pub valid_before: Option<u64>,
    pub valid_after: Option<u64>,
    pub key_authorization: Option<SignedKeyAuthorization>,
}

enum FeePayerSlot<'a> {
    Placeholder,
    Sender(&'a Address),
    Full,
    Zero,
}

impl TempoTransaction {
    fn fields(&self, out: &mut Vec<u8>, slot: FeePayerSlot<'_>, skip_fee_token: bool) {
        rlp::encode_u64(out, self.chain_id);
        rlp::encode_u128(out, self.max_priority_fee_per_gas);
        rlp::encode_u128(out, self.max_fee_per_gas);
        rlp::encode_u64(out, self.gas_limit);
        let mut calls = Vec::new();
        for call in &self.calls {
            let mut item = Vec::new();
            rlp::encode_bytes(&mut item, call.to.as_bytes());
            rlp::encode_uint(&mut item, &call.value.to_be_bytes());
            rlp::encode_bytes(&mut item, &call.input);
            rlp::encode_list(&mut calls, &item);
        }
        rlp::encode_list(out, &calls);
        out.push(rlp::EMPTY_LIST);
        rlp::encode_uint(out, &self.nonce_key.to_be_bytes());
        rlp::encode_u64(out, self.nonce);
        for bound in [self.valid_before, self.valid_after] {
            match bound {
                Some(bound) => rlp::encode_u64(out, bound),
                None => out.push(rlp::EMPTY_STRING),
            }
        }
        match (skip_fee_token, &self.fee_token) {
            (false, Some(token)) => rlp::encode_bytes(out, token.as_bytes()),
            _ => out.push(rlp::EMPTY_STRING),
        }
        match (slot, &self.fee_payer_signature) {
            (FeePayerSlot::Sender(sender), _) => rlp::encode_bytes(out, sender.as_bytes()),
            (FeePayerSlot::Zero, _) => out.push(FEE_PAYER_PLACEHOLDER),
            (FeePayerSlot::Placeholder, Some(_)) => out.push(FEE_PAYER_PLACEHOLDER),
            (FeePayerSlot::Full, Some(signature)) => {
                let mut payload = Vec::new();
                rlp::encode_u64(&mut payload, signature.y_parity as u64);
                rlp::encode_uint(&mut payload, &signature.r);
                rlp::encode_uint(&mut payload, &signature.s);
                rlp::encode_list(out, &payload);
            }
            (FeePayerSlot::Placeholder | FeePayerSlot::Full, None) => out.push(rlp::EMPTY_STRING),
        }
        out.push(rlp::EMPTY_LIST);
        if let Some(authorization) = &self.key_authorization {
            authorization.encode(out);
        }
    }

    pub fn encode_for_signing(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.fields(
            &mut payload,
            FeePayerSlot::Placeholder,
            self.fee_payer_signature.is_some(),
        );
        let mut out = vec![TEMPO_TRANSACTION_TYPE];
        rlp::encode_list(&mut out, &payload);
        out
    }

    pub fn signature_hash(&self) -> [u8; 32] {
        keccak256(self.encode_for_signing())
    }

    pub fn fee_payer_signature_hash(&self, sender: &Address) -> [u8; 32] {
        let mut payload = Vec::new();
        self.fields(&mut payload, FeePayerSlot::Sender(sender), false);
        let mut out = vec![FEE_PAYER_SIGNATURE_TYPE];
        rlp::encode_list(&mut out, &payload);
        keccak256(out)
    }

    pub fn encode_for_fee_payer_service(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        self.fields(&mut payload, FeePayerSlot::Zero, true);
        let mut out = vec![TEMPO_TRANSACTION_TYPE];
        rlp::encode_list(&mut out, &payload);
        out
    }

    pub fn encode_signed(&self, signature: &TempoSignature) -> Vec<u8> {
        let mut payload = Vec::new();
        self.fields(&mut payload, FeePayerSlot::Full, false);
        rlp::encode_bytes(&mut payload, &signature.to_bytes());
        let mut out = vec![TEMPO_TRANSACTION_TYPE];
        rlp::encode_list(&mut out, &payload);
        out
    }

    pub fn signed_hash(&self, signature: &TempoSignature) -> [u8; 32] {
        keccak256(self.encode_signed(signature))
    }
}

impl TempoTransaction {
    pub fn encode_fee_payer_envelope(
        &self,
        sender: &Address,
        signature: &TempoSignature,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        self.fields(&mut payload, FeePayerSlot::Sender(sender), false);
        rlp::encode_bytes(&mut payload, &signature.to_bytes());
        let mut out = vec![FEE_PAYER_SIGNATURE_TYPE];
        rlp::encode_list(&mut out, &payload);
        out
    }
}

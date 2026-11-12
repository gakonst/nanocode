use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::error::Fault;

pub(crate) const SESSION_PROTOCOL_LEGACY: &str = "v1";
pub(crate) const SESSION_PROTOCOL_TIP1034: &str = "v2";
pub(crate) const TRANSACTION: &str = "transaction";

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SessionRequest {
    pub(crate) amount: String,
    #[serde(rename = "unitType")]
    _unit_type: Option<String>,
    pub(crate) currency: String,
    pub(crate) recipient: Option<String>,
    #[serde(rename = "suggestedDeposit")]
    pub(crate) suggested_deposit: Option<String>,
    #[serde(rename = "methodDetails")]
    pub(crate) method_details: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct TempoSessionMethodDetails {
    escrow_contract: String,
    channel_id: Option<String>,
    min_voucher_delta: Option<String>,
    chain_id: Option<u64>,
    fee_payer: Option<bool>,
    machine_token_enabled: Option<bool>,
    fee_token: Option<String>,
    settlement_adapter: Option<String>,
    settlement_recipient: Option<String>,
    settlement_token: Option<String>,
    operator: Option<String>,
    session_protocol: Option<String>,
    session_snapshot: Option<SessionSnapshot>,
}

impl SessionRequest {
    pub(crate) fn parse_amount(&self) -> Result<u128, Fault> {
        (!self.amount.is_empty() && self.amount.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| self.amount.parse().ok())
            .flatten()
            .ok_or_else(|| Fault::InvalidAmount(format!("Invalid amount: {}", self.amount)))
    }

    fn detail(&self, key: &str) -> Option<&Value> {
        self.method_details.as_ref()?.get(key)
    }

    pub(crate) fn chain_id(&self) -> Option<u64> {
        self.detail("chainId")?.as_u64()
    }

    pub(crate) fn fee_payer(&self) -> bool {
        self.detail("feePayer")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub(crate) fn machine_token_enabled(&self) -> bool {
        self.detail("machineTokenEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub(crate) fn operator(&self) -> Option<&Value> {
        self.detail("operator")
    }

    pub(crate) fn session_protocol(&self) -> Option<&str> {
        self.detail("sessionProtocol")?.as_str()
    }

    pub(crate) fn escrow(&self) -> Option<nanocode_tempo::Address> {
        ["escrowContract", "escrow"].into_iter().find_map(|key| {
            self.detail(key)
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
    }

    pub(crate) fn session_snapshot(&self) -> Option<SessionSnapshot> {
        serde_json::from_value(self.detail("sessionSnapshot")?.clone()).ok()
    }

    pub(crate) fn advertised_fee_token(&self) -> Option<String> {
        let details: TempoSessionMethodDetails =
            serde_json::from_value(self.method_details.clone()?).ok()?;
        details.fee_token
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelDescriptor {
    pub(crate) payer: String,
    pub(crate) payee: String,
    pub(crate) operator: String,
    pub(crate) token: String,
    pub(crate) salt: String,
    pub(crate) authorized_signer: String,
    pub(crate) expiring_nonce_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettlementRoute {
    pub(crate) adapter: String,
    pub(crate) recipient: String,
    pub(crate) target_token: String,
    pub(crate) route_salt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionSnapshot {
    pub(crate) accepted_cumulative: String,
    pub(crate) chain_id: u64,
    pub(crate) channel_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) close_requested_at: Option<String>,
    pub(crate) deposit: String,
    pub(crate) descriptor: ChannelDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) settlement_route: Option<SettlementRoute>,
    pub(crate) escrow: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) highest_voucher: Option<SnapshotVoucher>,
    pub(crate) required_cumulative: String,
    pub(crate) settled: String,
    pub(crate) spent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) units: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotVoucher {
    pub(crate) channel_id: String,
    pub(crate) cumulative_amount: String,
    pub(crate) signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub(crate) enum SessionCredentialPayload {
    #[serde(rename = "open")]
    Open {
        #[serde(rename = "type")]
        payload_type: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        transaction: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        descriptor: Option<ChannelDescriptor>,
        #[serde(rename = "authorizedSigner", skip_serializing_if = "Option::is_none")]
        authorized_signer: Option<String>,
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        signature: String,
    },
    #[serde(rename = "topUp")]
    TopUp {
        #[serde(rename = "type")]
        payload_type: String,
        #[serde(rename = "channelId")]
        channel_id: String,
        transaction: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        descriptor: Option<ChannelDescriptor>,
        #[serde(rename = "additionalDeposit")]
        additional_deposit: String,
    },
    #[serde(rename = "voucher")]
    Voucher {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        descriptor: Option<ChannelDescriptor>,
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        signature: String,
    },
    #[serde(rename = "close")]
    Close {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        descriptor: Option<ChannelDescriptor>,
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        signature: String,
        #[serde(rename = "closeSignature", skip_serializing_if = "Option::is_none")]
        close_signature: Option<String>,
    },
}

impl SessionCredentialPayload {
    pub(crate) fn channel_id(payload: &Value) -> Option<String> {
        let payload: Self = serde_json::from_value(payload.clone()).ok()?;
        Some(match payload {
            Self::Open {
                payload_type,
                channel_id,
                ..
            }
            | Self::TopUp {
                payload_type,
                channel_id,
                ..
            } => (payload_type == TRANSACTION).then_some(channel_id)?,
            Self::Voucher { channel_id, .. } | Self::Close { channel_id, .. } => channel_id,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct SessionReceipt {
    method: String,
    intent: String,
    status: String,
    timestamp: String,
    reference: String,
    #[serde(rename = "challengeId")]
    challenge_id: String,
    #[serde(rename = "channelId")]
    pub(crate) channel_id: String,
    #[serde(rename = "acceptedCumulative")]
    pub(crate) accepted_cumulative: String,
    pub(crate) spent: String,
    units: Option<u64>,
    #[serde(rename = "txHash")]
    tx_hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NeedVoucherEvent {
    pub(crate) channel_id: String,
    pub(crate) required_cumulative: String,
    pub(crate) accepted_cumulative: String,
}

use nanocode_tempo::Address;

use super::super::error::Fault;
use super::super::store::StoredChannelEntry;
use super::channel::{
    Hash, OnChainChannelState, parse_hash, verify_voucher_signature, wire_channel_id,
};
use super::wire::{ChannelDescriptor, SessionSnapshot};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RecoveryScope {
    pub(crate) payer: Address,
    pub(crate) authorized_signer: Address,
    pub(crate) payee: Address,
    pub(crate) operator: Address,
    pub(crate) token: Address,
    pub(crate) escrow: Address,
    pub(crate) chain_id: u64,
}

fn invalid(message: impl Into<String>) -> Fault {
    Fault::Config(message.into())
}

fn parse_address(label: &str, value: &str) -> Result<Address, Fault> {
    value
        .parse()
        .map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn parse_b256(label: &str, value: &str) -> Result<Hash, Fault> {
    parse_hash(value).map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn parse_amount(label: &str, value: &str) -> Result<u128, Fault> {
    value
        .parse()
        .map_err(|error| invalid(format!("invalid {label}: {error}")))
}

fn descriptor_authority(descriptor: &ChannelDescriptor) -> Result<Address, Fault> {
    let authority = parse_address("descriptor authorizedSigner", &descriptor.authorized_signer)?;
    match authority == Address::ZERO {
        true => parse_address("descriptor payer", &descriptor.payer),
        false => Ok(authority),
    }
}

pub(crate) fn can_sign_descriptor(
    descriptor: &ChannelDescriptor,
    payer: Address,
    authorized_signer: Address,
) -> Result<bool, Fault> {
    Ok(
        parse_address("descriptor payer", &descriptor.payer)? == payer
            && descriptor_authority(descriptor)? == authorized_signer,
    )
}

fn validate_descriptor(
    descriptor: &ChannelDescriptor,
    channel_id: &Hash,
    scope: RecoveryScope,
) -> Result<(), Fault> {
    let payer = parse_address("descriptor payer", &descriptor.payer)?;
    let payee = parse_address("descriptor payee", &descriptor.payee)?;
    let operator = parse_address("descriptor operator", &descriptor.operator)?;
    let token = parse_address("descriptor token", &descriptor.token)?;
    let authority = descriptor_authority(descriptor)?;
    if payer != scope.payer
        || payee != scope.payee
        || operator != scope.operator
        || token != scope.token
        || authority != scope.authorized_signer
    {
        return Err(invalid(
            "channel descriptor identity does not match the challenge",
        ));
    }
    if wire_channel_id(descriptor, scope.escrow, scope.chain_id)? != *channel_id {
        return Err(invalid("channelId does not match its descriptor"));
    }
    Ok(())
}

pub(crate) fn recover_stored_channel(
    mut entry: StoredChannelEntry,
    scope: RecoveryScope,
    state: OnChainChannelState,
) -> Result<StoredChannelEntry, Fault> {
    validate_descriptor(&entry.descriptor, &entry.channel_id, scope)?;
    if entry.escrow != scope.escrow || entry.chain_id != scope.chain_id {
        return Err(invalid("stored channel scope does not match the challenge"));
    }
    if state.deposit == 0 || state.close_requested_at != 0 {
        return Err(invalid("stored channel is absent or closing on-chain"));
    }
    let cumulative = entry.cumulative_amount.max(state.settled);
    if cumulative > state.deposit {
        return Err(invalid(
            "stored cumulative amount exceeds the on-chain channel deposit",
        ));
    }
    if state.settled != 0 && state.settled >= entry.cumulative_amount {
        entry.pending_open = None;
    }
    entry.cumulative_amount = cumulative;
    entry.deposit = state.deposit;
    entry.opened = true;
    Ok(entry)
}

pub(crate) fn hydrate_session_snapshot(
    snapshot: &SessionSnapshot,
    scope: RecoveryScope,
    state: OnChainChannelState,
) -> Result<StoredChannelEntry, Fault> {
    let channel_id = parse_b256("snapshot channelId", &snapshot.channel_id)?;
    let escrow = parse_address("snapshot escrow", &snapshot.escrow)?;
    if snapshot.chain_id != scope.chain_id || escrow != scope.escrow {
        return Err(invalid(
            "snapshot chain or escrow does not match the challenge",
        ));
    }
    validate_descriptor(&snapshot.descriptor, &channel_id, scope)?;
    let signed = snapshot
        .highest_voucher
        .as_ref()
        .ok_or_else(|| invalid("session snapshot is missing its highest signed voucher"))?;
    if parse_b256("snapshot voucher channelId", &signed.channel_id)? != channel_id {
        return Err(invalid(
            "snapshot voucher channelId does not match snapshot channelId",
        ));
    }
    let accepted = parse_amount("snapshot acceptedCumulative", &snapshot.accepted_cumulative)?;
    let voucher_cumulative = parse_amount(
        "snapshot voucher cumulativeAmount",
        &signed.cumulative_amount,
    )?;
    let required = parse_amount("snapshot requiredCumulative", &snapshot.required_cumulative)?;
    let snapshot_deposit = parse_amount("snapshot deposit", &snapshot.deposit)?;
    let snapshot_settled = parse_amount("snapshot settled", &snapshot.settled)?;
    let spent = parse_amount("snapshot spent", &snapshot.spent)?;
    if voucher_cumulative != accepted {
        return Err(invalid(
            "snapshot voucher amount does not match acceptedCumulative",
        ));
    }
    if spent > accepted || snapshot_settled > snapshot_deposit {
        return Err(invalid("snapshot amounts are inconsistent"));
    }
    if state.deposit == 0 || state.close_requested_at != 0 {
        return Err(invalid("snapshot channel is absent or closing on-chain"));
    }
    let signature = nanocode_tempo::hex::decode(signed.signature.trim_start_matches("0x"))
        .map_err(|error| invalid(format!("invalid snapshot voucher signature: {error}")))?;
    let authority = descriptor_authority(&snapshot.descriptor)?;
    if !verify_voucher_signature(
        &signature,
        authority,
        &channel_id,
        voucher_cumulative,
        escrow,
        snapshot.chain_id,
    )? {
        return Err(invalid("snapshot highest voucher signature is invalid"));
    }
    let cumulative_amount = accepted.max(required).max(state.settled);
    if cumulative_amount > state.deposit {
        return Err(invalid(
            "recovered cumulative amount exceeds the on-chain channel deposit",
        ));
    }
    Ok(StoredChannelEntry {
        channel_id,
        cumulative_amount,
        deposit: state.deposit,
        descriptor: snapshot.descriptor.clone(),
        settlement_route: snapshot.settlement_route.clone(),
        escrow,
        chain_id: snapshot.chain_id,
        opened: true,
        pending_open: None,
        origin: None,
    })
}

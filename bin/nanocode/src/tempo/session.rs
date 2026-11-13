use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use nanocode_sync::Atom;
use nanocode_tempo::{Address, Call, SignedKeyAuthorization, U256, hex, keccak256};
use reqwest::Url;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::charge::{AutoswapConfig, AutoswapRoute, resolve_autoswap_route};
use super::error::Fault;
use super::events::{Deposit, Events, PaymentUpdate};
use super::keys::AccessKey;
use super::protocol::{
    self, Challenge, Credential, INTENT_SESSION, METHOD_NAME, PAYMENT_RECEIPT_HEADER, Receipt,
};
use super::provider::{ManagementTransport, PaymentContext, PaymentEventFuture, PaymentProvider};
use super::rpc::Rpc;
use super::signing::{
    PrimitiveSigner, TempoSigningMode, resolve_key_authorization,
    sign_and_encode_fee_payer_envelope_primitive_async, sign_and_encode_primitive_async,
};
use super::store::{ChannelStore, StoreLease, StoredChannelEntry, channel_key};
use super::tx::{self, TempoTransactionRequest, TempoTxOptions, build_tempo_tx};

pub(crate) mod channel;
mod recovery;
pub(crate) mod wire;

use channel::{
    Descriptor, Hash, PRECOMPILE_MAX_CUMULATIVE_AMOUNT, TIP20_CHANNEL_RESERVE_ADDRESS,
    close_authorization_signing_hash, hash_text, is_precompile_escrow, parse_hash, random_hash,
    read_channel_state, resolve_chain_id, resolve_escrow, voucher_signing_hash, wire_channel_id,
};
use recovery::{
    RecoveryScope, can_sign_descriptor, hydrate_session_snapshot, recover_stored_channel,
};
use wire::{
    ChannelDescriptor, NeedVoucherEvent, SessionCredentialPayload, SessionReceipt, SessionRequest,
    SessionSnapshot, TRANSACTION,
};

const MANAGEMENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const EXPIRING_NONCE_VALID_BEFORE_SECS: u64 = 25;
const VALID_AFTER_PAST_SECS: u64 = 60;
const SESSION_GAS_LIMIT: u64 = 2_000_000;
const VALID_AFTER_PREFIX_BYTES: usize = 8;
const SESSION_ACCEPT_PAYMENT: &str = "tempo/session";
const LATEST_BLOCK: &str = "latest";
const MANAGEMENT_DROPPED_HEADERS: [&str; 9] = [
    "connection",
    "proxy-authorization",
    "proxy-authenticate",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionSpend {
    pub(crate) spent: u128,
    pub(crate) last: Option<u128>,
    pub(crate) remaining: u128,
}

#[derive(Clone, Debug, Default)]
struct SpendLedger {
    channels: HashMap<String, u128>,
    spent: u128,
    last: Option<u128>,
}

impl SpendLedger {
    fn record(&mut self, channel_id: &str, spent: u128, quoted: Option<u128>) -> u128 {
        let cost = match self.channels.get(channel_id) {
            Some(previous) => spent.saturating_sub(*previous),
            None => quoted.unwrap_or_default(),
        };
        self.channels.insert(channel_id.to_owned(), spent);
        self.spent += cost;
        self.last = Some(cost);
        cost
    }

    fn update(&mut self, channel_id: &str, spent: u128) -> Option<i128> {
        let previous = self.channels.insert(channel_id.to_owned(), spent)?;
        let change = i128::try_from(spent).ok()? - i128::try_from(previous).ok()?;
        if change == 0 {
            return None;
        }
        self.spent = self.spent.saturating_add_signed(change);
        self.last = self.last.map(|last| last.saturating_add_signed(change));
        Some(change)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ChannelEntry {
    pub(crate) channel_id: Hash,
    pub(crate) cumulative_amount: u128,
    pub(crate) deposit: u128,
    pub(crate) descriptor: ChannelDescriptor,
    pub(crate) settlement_route: Option<wire::SettlementRoute>,
    pub(crate) escrow_contract: Address,
    pub(crate) chain_id: u64,
    pub(crate) opened: bool,
}

impl ChannelEntry {
    fn id(&self) -> String {
        hash_text(&self.channel_id)
    }
}

#[derive(Clone, Debug)]
struct PendingOpen {
    payload: Option<(SessionCredentialPayload, Option<u64>)>,
    challenge_id: String,
    store_key: Option<String>,
}

#[derive(Clone, Debug)]
struct PrepaidVoucher {
    challenge: Challenge,
    channel_id: String,
    spent: u128,
    price: u128,
}

struct PaymentLease {
    channel_id: Option<String>,
    _process: OwnedSemaphorePermit,
    _store: StoreLease,
}

struct PreparedTopUp {
    key: String,
    channel_id: Hash,
    chain_id: u64,
    cumulative_amount: u128,
    credential: Credential,
    headers: HeaderMap,
    new_deposit: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PaymentHold {
    LeaseOnChannel,
    Lease,
    Channel,
}

enum TopUpRoute<'a> {
    Http {
        client: &'a reqwest::Client,
        url: &'a str,
        headers: HeaderMap,
    },
    Management {
        transport: &'a dyn ManagementTransport,
        url: &'a str,
    },
}

struct Shared {
    channels: Atom<HashMap<String, ChannelEntry>>,
    channel_keys: Atom<HashMap<String, String>>,
    pending_opens: Atom<HashMap<String, PendingOpen>>,
    last_challenge: Atom<Option<Challenge>>,
    payment_lock: Arc<Semaphore>,
    funding_locks: Atom<HashMap<String, Arc<Semaphore>>>,
    leases: Atom<HashMap<String, Arc<PaymentLease>>>,
    prepaid: Atom<HashMap<String, PrepaidVoucher>>,
    topping_up: Atom<HashSet<String>>,
    spend: Atom<SpendLedger>,
    server_spent_saves: Arc<Semaphore>,
    uncertain_top_ups: Atom<HashSet<Hash>>,
    origin: Atom<Option<String>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            channels: Atom::default(),
            channel_keys: Atom::default(),
            pending_opens: Atom::default(),
            last_challenge: Atom::default(),
            payment_lock: Arc::new(Semaphore::new(1)),
            funding_locks: Atom::default(),
            leases: Atom::default(),
            prepaid: Atom::default(),
            topping_up: Atom::default(),
            spend: Atom::default(),
            server_spent_saves: Arc::new(Semaphore::new(1)),
            uncertain_top_ups: Atom::default(),
            origin: Atom::default(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct SessionProvider {
    signer: PrimitiveSigner,
    rpc: Rpc,
    authorized_signer: Option<Address>,
    signing_mode: TempoSigningMode,
    default_deposit: Option<u128>,
    top_up_amount: Option<u128>,
    autoswap: Option<AutoswapConfig>,
    fee_token: Option<Address>,
    channel_store: ChannelStore,
    prepaid_requests: u32,
    events: Events,
    management_timeout: Duration,
    shared: Arc<Shared>,
}

async fn with_management_timeout<T>(
    timeout: Duration,
    context: &str,
    request: impl Future<Output = Result<T, Fault>>,
) -> Result<T, Fault> {
    tokio::time::timeout(timeout, request)
        .await
        .unwrap_or_else(|_| {
            Err(Fault::SessionOutcomeUnknown {
                context: format!("{context}: session management request timed out"),
            })
        })
}

fn ensure_redirect_safe_credential_header(challenge: &Challenge) -> Result<(), Fault> {
    if challenge.credential_header() != "Authorization" {
        return Err(Fault::config(
            "Payment-Authorization requires a redirect-safe client; refusing to create a credential",
        ));
    }
    Ok(())
}

fn credential_header_name(challenge: &Challenge) -> HeaderName {
    HeaderName::from_bytes(challenge.credential_header().as_bytes())
        .unwrap_or(reqwest::header::AUTHORIZATION)
}

fn authorization_value(credential: &Credential, context: &str) -> Result<HeaderValue, Fault> {
    credential
        .authorization()?
        .parse()
        .map_err(|error| Fault::config(format!("{context}: {error}")))
}

fn validate_for_session(challenge: &Challenge) -> Result<(), Fault> {
    if !challenge.method.eq_ignore_ascii_case(METHOD_NAME) {
        return Err(Fault::Unsupported(format!(
            "Payment method '{}' is not supported. Supported methods: {METHOD_NAME}",
            challenge.method
        )));
    }
    if challenge.intent != INTENT_SESSION {
        return Err(Fault::InvalidChallenge(Some(format!(
            "Expected 'session' intent, got: {}",
            challenge.intent
        ))));
    }
    if challenge.is_expired() {
        return Err(Fault::Expired(challenge.expires.clone()));
    }
    Ok(())
}

fn session_request(challenge: &Challenge) -> Result<SessionRequest, Fault> {
    challenge
        .request
        .decode()
        .map_err(|error| Fault::config(format!("failed to decode session request: {error}")))
}

fn build_credential(
    challenge: &Challenge,
    payload: &SessionCredentialPayload,
    chain_id: u64,
    payer: Address,
) -> Result<Credential, Fault> {
    Ok(Credential::with_source(
        challenge.to_echo(),
        Credential::did(chain_id, &payer.to_string()),
        serde_json::to_value(payload)?,
    ))
}

fn session_challenge(response: &reqwest::Response) -> Option<Challenge> {
    protocol::response_challenges(response.headers())
        .into_iter()
        .find(|challenge| challenge.method == METHOD_NAME && challenge.intent == INTENT_SESSION)
}

fn response_receipt(response: &reqwest::Response) -> Option<Receipt> {
    response
        .headers()
        .get(PAYMENT_RECEIPT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Receipt::parse(value).ok())
}

fn management_headers(mut headers: HeaderMap) -> HeaderMap {
    let connection: Vec<String> = headers
        .get_all(reqwest::header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(',').map(|name| name.trim().to_owned()))
        .collect();
    for name in connection {
        headers.remove(name.as_str());
    }
    for name in MANAGEMENT_DROPPED_HEADERS {
        headers.remove(name);
    }
    headers
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn session_nonce_fields() -> (u64, U256, Option<u64>) {
    (
        0,
        U256::MAX,
        Some(unix_seconds().saturating_add(EXPIRING_NONCE_VALID_BEFORE_SECS)),
    )
}

fn random_past_valid_after() -> Result<Option<u64>, Fault> {
    let latest = unix_seconds().saturating_sub(VALID_AFTER_PAST_SECS);
    if latest == 0 {
        return Ok(None);
    }
    let random = random_hash()?;
    let mut prefix = [0u8; VALID_AFTER_PREFIX_BYTES];
    prefix.copy_from_slice(&random[..VALID_AFTER_PREFIX_BYTES]);
    Ok(Some((u64::from_be_bytes(prefix) % latest).max(1)))
}

fn expiring_nonce_hash(transaction: &nanocode_tempo::TempoTransaction, sender: Address) -> Hash {
    let mut preimage = transaction.encode_for_signing();
    preimage.extend_from_slice(sender.as_bytes());
    keccak256(preimage)
}

fn amount(value: &str) -> Result<u128, Fault> {
    value
        .parse::<u128>()
        .map_err(|error| Fault::config(format!("invalid session amount: {error}")))
}

fn http(context: &str) -> impl Fn(reqwest::Error) -> Fault + '_ {
    move |error| Fault::http(format!("{context}: {error}"))
}

impl SessionProvider {
    pub(crate) fn new(key: AccessKey, rpc_url: &str) -> Result<Self, Fault> {
        let url: Url = rpc_url
            .parse()
            .map_err(|error| Fault::config(format!("invalid RPC URL: {error}")))?;
        Ok(Self {
            signer: PrimitiveSigner::from(key),
            rpc: Rpc::new(None, url),
            authorized_signer: None,
            signing_mode: TempoSigningMode::Direct,
            default_deposit: None,
            top_up_amount: None,
            autoswap: None,
            fee_token: None,
            channel_store: ChannelStore::default(),
            prepaid_requests: 0,
            events: Events::default(),
            management_timeout: MANAGEMENT_REQUEST_TIMEOUT,
            shared: Arc::default(),
        })
    }

    pub(crate) fn with_signing_mode(mut self, mode: TempoSigningMode) -> Self {
        self.signing_mode = mode;
        self
    }

    pub(crate) const fn with_authorized_signer(mut self, address: Address) -> Self {
        self.authorized_signer = Some(address);
        self
    }

    pub(crate) fn with_channel_store(mut self, store: impl Into<ChannelStore>) -> Self {
        self.channel_store = store.into();
        self
    }

    pub(crate) const fn with_autoswap(mut self, config: AutoswapConfig) -> Self {
        self.autoswap = Some(config);
        self
    }

    pub(crate) const fn with_default_deposit(mut self, amount: u128) -> Self {
        self.default_deposit = Some(amount);
        self
    }

    pub(crate) const fn with_top_up_amount(mut self, amount: u128) -> Self {
        self.top_up_amount = Some(amount);
        self
    }

    pub(crate) const fn with_fee_token(mut self, token: Address) -> Self {
        self.fee_token = Some(token);
        self
    }

    pub(crate) const fn with_prepaid_requests(mut self, requests: u32) -> Self {
        self.prepaid_requests = requests;
        self
    }

    pub(crate) fn with_events(mut self, events: Events) -> Self {
        self.events = events;
        self
    }

    fn payer(&self) -> Address {
        self.signing_mode.from_address(self.signer.address())
    }

    fn voucher_signer(&self) -> Address {
        self.authorized_signer.unwrap_or(self.signer.address())
    }

    pub(crate) fn spend(&self) -> SessionSpend {
        let ledger = self.shared.spend.get();
        let remaining = self.shared.channels.read(|channels| {
            channels
                .values()
                .filter_map(|entry| {
                    let spent = ledger.channels.get(&entry.id())?;
                    Some(entry.deposit.saturating_sub(*spent))
                })
                .sum()
        });
        SessionSpend {
            spent: ledger.spent,
            last: ledger.last,
            remaining,
        }
    }

    pub(crate) fn bind_origin(&self, url: &str) -> Result<(), Fault> {
        let url = Url::parse(url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .ok_or_else(|| Fault::config("session payments require an HTTP origin"))?;
        let origin = url.origin().ascii_serialization();
        self.shared.origin.modify(|bound| match bound.as_ref() {
            Some(bound) if *bound != origin => {
                Err(Fault::config("session payment origin mismatch"))
            }
            Some(_) => Ok(()),
            None => {
                *bound = Some(origin.clone());
                Ok(())
            }
        })
    }

    pub(crate) fn bind_http_origin(&self, url: &str) -> Result<(), Fault> {
        match Url::parse(url) {
            Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => self.bind_origin(url),
            _ => Ok(()),
        }
    }

    fn holds_channel_hex(&self, channel_id: &str) -> bool {
        parse_hash(channel_id).is_ok_and(|id| self.knows_channel(&hash_text(&id)))
    }

    fn knows_channel(&self, channel_id: &str) -> bool {
        self.shared
            .channel_keys
            .read(|keys| keys.contains_key(channel_id))
    }

    fn is_bound_to(&self, url: &str) -> bool {
        let Some(url) = Url::parse(url)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
        else {
            return false;
        };
        let origin = url.origin().ascii_serialization();
        self.shared
            .origin
            .read(|bound| bound.as_deref() == Some(origin.as_str()))
    }

    pub(crate) fn payment_hold(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Option<PaymentHold> {
        let channel_id = SessionCredentialPayload::channel_id(&credential.payload);
        let knows = channel_id
            .as_deref()
            .is_some_and(|id| self.knows_channel(id));
        self.shared
            .leases
            .read(|leases| match leases.get(&challenge.id) {
                Some(lease) => {
                    let on_channel = match (&lease.channel_id, &channel_id) {
                        (Some(leased), Some(paid)) => leased.eq_ignore_ascii_case(paid),
                        (None, Some(_)) => knows,
                        _ => false,
                    };
                    Some(match on_channel {
                        true => PaymentHold::LeaseOnChannel,
                        false => PaymentHold::Lease,
                    })
                }
                None => knows.then_some(PaymentHold::Channel),
            })
    }

    fn hint_client() -> Result<reqwest::Client, Fault> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(nanocode_http::accept_encoding(
                reqwest::header::HeaderMap::new(),
            ))
            .build()
            .map_err(http("failed to build the session hint client"))
    }

    pub(crate) async fn send_hint(
        &self,
        request: impl FnOnce(&reqwest::Client) -> reqwest::RequestBuilder,
        context: &str,
    ) -> Result<reqwest::Response, Fault> {
        let client = Self::hint_client()?;
        with_management_timeout(self.management_timeout, context, async {
            request(&client)
                .send()
                .await
                .map(nanocode_http::decode_flate_response)
                .map_err(http(context))
        })
        .await
    }

    fn report_deposit(&self, url: &str, entry: &ChannelEntry, amount: u128, opened: bool) {
        let Ok(url) = url.parse() else {
            return;
        };
        self.events.deposit(&Deposit {
            url,
            channel_id: entry.id(),
            currency: Some(entry.descriptor.token.clone()),
            amount,
            deposit: entry.deposit,
            opened,
        });
    }

    async fn autoswap_calls(
        &self,
        payer: Address,
        currency: Address,
        amount: u128,
    ) -> Result<(Vec<Call>, Option<AutoswapRoute>), Fault> {
        let Some(config) = &self.autoswap else {
            return Ok((Vec::new(), None));
        };
        let route =
            resolve_autoswap_route(&self.rpc, payer, currency, U256::from_u128(amount), config)
                .await?;
        let calls = route
            .as_ref()
            .map(|route| route.calls.clone())
            .unwrap_or_default();
        Ok((calls, route))
    }

    fn default_fee_token(&self, currency: Address, route: Option<&AutoswapRoute>) -> Address {
        self.fee_token
            .or_else(|| {
                self.autoswap
                    .as_ref()
                    .and_then(|config| config.fee_token_for(route, currency))
            })
            .unwrap_or(currency)
    }

    fn session_fee_token(
        &self,
        request: &SessionRequest,
        currency: Address,
        route: Option<&AutoswapRoute>,
    ) -> Result<Address, Fault> {
        let advertised = request
            .advertised_fee_token()
            .map(|token| token.parse::<Address>())
            .transpose()
            .map_err(|error| Fault::config(format!("invalid session feeToken: {error}")))?;
        Ok(match advertised {
            Some(token) if request.fee_payer() => token,
            advertised => self
                .fee_token
                .or(advertised)
                .unwrap_or_else(|| self.default_fee_token(currency, route)),
        })
    }

    fn required_top_up(
        &self,
        required: u128,
        deposit: u128,
        suggested: Option<&str>,
    ) -> Result<Option<u128>, Fault> {
        let Some(shortfall) = required
            .checked_sub(deposit)
            .filter(|shortfall| *shortfall > 0)
        else {
            return Ok(None);
        };
        let suggested = suggested
            .map(str::parse::<u128>)
            .transpose()
            .map_err(|error| Fault::config(format!("invalid suggestedDeposit: {error}")))?
            .unwrap_or_default();
        Ok(Some(shortfall.max(self.top_up_amount.unwrap_or(suggested))))
    }

    fn stored_entry(entry: &ChannelEntry) -> StoredChannelEntry {
        StoredChannelEntry {
            channel_id: entry.channel_id,
            cumulative_amount: entry.cumulative_amount,
            deposit: entry.deposit,
            descriptor: entry.descriptor.clone(),
            settlement_route: entry.settlement_route.clone(),
            escrow: entry.escrow_contract,
            chain_id: entry.chain_id,
            opened: entry.opened,
            pending_open: None,
            origin: None,
        }
    }

    fn store_key(entry: &ChannelEntry) -> Option<String> {
        is_precompile_escrow(entry.escrow_contract).then(|| Self::stored_entry(entry).key())
    }

    async fn store_get(&self, key: &str) -> Result<Option<StoredChannelEntry>, Fault> {
        let entry = self.channel_store.get(key).await?;
        if let Some(stored) = entry.as_ref().and_then(|entry| entry.origin.clone()) {
            self.shared.origin.modify(|bound| match bound.as_ref() {
                Some(bound) if *bound != stored => Err(super::store::StoreFault::InvalidEntry(
                    "stored channel origin mismatch".into(),
                )),
                Some(_) => Ok(()),
                None => {
                    *bound = Some(stored.clone());
                    Ok(())
                }
            })?;
        }
        Ok(entry)
    }

    async fn store_set(&self, entry: &StoredChannelEntry) -> Result<(), Fault> {
        let bound = self.shared.origin.get();
        let mut entry = entry.clone();
        match (&entry.origin, bound.as_ref()) {
            (Some(origin), Some(bound)) if origin != bound => {
                return Err(super::store::StoreFault::InvalidEntry(
                    "stored channel origin mismatch".into(),
                )
                .into());
            }
            (_, Some(bound)) => entry.origin = Some(bound.clone()),
            (_, None) => {}
        }
        Ok(self.channel_store.set(&entry).await?)
    }

    async fn store_delete(&self, key: &str) -> Result<(), Fault> {
        Ok(self.channel_store.delete(key).await?)
    }

    async fn merge_stored_channel(&self, key: &str, entry: &mut ChannelEntry) -> Result<(), Fault> {
        if let Some(stored) = self
            .store_get(key)
            .await?
            .filter(|stored| stored.channel_id == entry.channel_id)
        {
            entry.cumulative_amount = entry.cumulative_amount.max(stored.cumulative_amount);
            entry.deposit = entry.deposit.max(stored.deposit);
        }
        Ok(())
    }

    fn channel(&self, key: &str) -> Option<ChannelEntry> {
        self.shared
            .channels
            .read(|channels| channels.get(key).cloned())
    }

    fn channel_key_of(&self, channel_id: &str) -> Option<String> {
        self.shared
            .channel_keys
            .read(|keys| keys.get(channel_id).cloned())
    }

    fn channel_by_id(&self, channel_id: &str) -> Option<ChannelEntry> {
        self.channel_key_of(channel_id)
            .and_then(|key| self.channel(&key))
    }

    fn track(&self, key: &str, entry: &ChannelEntry) {
        let id = entry.id();
        self.shared
            .channel_keys
            .update(|keys| keys.insert(id.clone(), key.to_owned()).map(drop));
        self.shared
            .channels
            .update(|channels| channels.insert(key.to_owned(), entry.clone()).map(drop));
    }

    fn forget(&self, key: &str, channel_id: &str) {
        self.shared
            .pending_opens
            .update(|pending| pending.remove(channel_id).map(drop));
        self.shared
            .channel_keys
            .update(|keys| keys.remove(channel_id).map(drop));
        self.shared
            .channels
            .update(|channels| channels.remove(key).map(drop));
    }

    fn pending_open(&self, channel_id: &str) -> Option<PendingOpen> {
        self.shared
            .pending_opens
            .read(|pending| pending.get(channel_id).cloned())
    }

    fn is_pending_open(&self, channel_id: &str) -> bool {
        self.shared
            .pending_opens
            .read(|pending| pending.contains_key(channel_id))
    }

    fn settle_pending_payload(&self, channel_id: &str) {
        self.shared.pending_opens.update(|pending| {
            if let Some(open) = pending.get_mut(channel_id) {
                open.payload = None;
            }
        });
    }

    async fn latest_block(&self) -> Result<(u64, u64), Fault> {
        let block = self
            .rpc
            .call("eth_getBlockByNumber", json!([LATEST_BLOCK, false]))
            .await
            .map_err(|error| Fault::http(format!("failed to reconcile opening expiry: {error}")))?;
        let field = |name: &str| {
            block
                .get(name)
                .and_then(Value::as_str)
                .and_then(|text| u64::from_str_radix(text.trim_start_matches("0x"), 16).ok())
        };
        match (field("timestamp"), field("number")) {
            (Some(timestamp), Some(number)) => Ok((timestamp, number)),
            _ => Err(Fault::http("opening expiry block unavailable")),
        }
    }

    async fn reconcile_expired_open(
        &self,
        key: &str,
        entry: &ChannelEntry,
        valid_before: u64,
    ) -> Result<bool, Fault> {
        async {
            let (timestamp, number) = self.latest_block().await?;
            if timestamp <= valid_before {
                return Ok(false);
            }
            let state = read_channel_state(&self.rpc, &entry.channel_id, Some(number)).await?;
            if state.deposit != 0 || state.settled != 0 || state.close_requested_at != 0 {
                self.settle_pending_payload(&entry.id());
                return Ok(false);
            }
            if let Some(store_key) = Self::store_key(entry) {
                self.store_delete(&store_key).await?;
            }
            self.forget(key, &entry.id());
            Ok(true)
        }
        .await
        .map_err(|_: Fault| Fault::SessionOutcomeUnknown {
            context: "pending opening expiry could not be reconciled".into(),
        })
    }

    async fn persist_channel(&self, entry: &ChannelEntry) -> Result<(), Fault> {
        if !is_precompile_escrow(entry.escrow_contract) {
            return Ok(());
        }
        let mut stored = Self::stored_entry(entry);
        stored.pending_open = self
            .pending_open(&entry.id())
            .map(|pending| pending.challenge_id);
        self.store_set(&stored).await
    }

    fn commit_referenced_open(&self, channel_id: &str) {
        let current = self
            .channel_by_id(channel_id)
            .is_some_and(|entry| entry.id() == channel_id);
        if current {
            self.shared
                .pending_opens
                .update(|pending| pending.remove(channel_id).map(drop));
        }
    }

    async fn rollback_credential(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        let Some(channel_id) = SessionCredentialPayload::channel_id(&credential.payload) else {
            return Ok(());
        };
        let pending = self
            .pending_open(&channel_id)
            .is_some_and(|pending| pending.challenge_id == challenge.id);
        if !pending {
            return Ok(());
        }
        self.invalidate_credential(challenge, credential).await
    }

    async fn invalidate_credential(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        let Some(channel_id) = SessionCredentialPayload::channel_id(&credential.payload) else {
            return Ok(());
        };
        let entry = self.shared.channels.read(|channels| {
            channels
                .values()
                .find(|entry| entry.id() == channel_id)
                .cloned()
        });
        if let Some(entry) = entry.filter(|entry| is_precompile_escrow(entry.escrow_contract)) {
            let state = read_channel_state(&self.rpc, &entry.channel_id, None).await?;
            if state.deposit != 0 {
                return Err(Fault::config(
                    "server rejected a funded channel; retained descriptor for recovery",
                ));
            }
        }
        let pending = self.pending_open(&channel_id);
        if pending
            .as_ref()
            .is_some_and(|pending| pending.challenge_id != challenge.id)
        {
            return Ok(());
        }
        let pending_store_key = match pending {
            Some(_) => self
                .shared
                .pending_opens
                .update(|pending| pending.remove(&channel_id))
                .and_then(|pending| pending.store_key),
            None => None,
        };
        let runtime_key = self
            .shared
            .channel_keys
            .update(|keys| keys.remove(&channel_id));
        let runtime_store_key = runtime_key.and_then(|runtime_key| {
            self.shared
                .channels
                .update(|channels| {
                    match channels
                        .get(&runtime_key)
                        .is_some_and(|entry| entry.id() == channel_id)
                    {
                        true => channels.remove(&runtime_key),
                        false => None,
                    }
                })
                .and_then(|entry| Self::store_key(&entry))
        });
        if let Some(store_key) = pending_store_key.or(runtime_store_key) {
            self.store_delete(&store_key).await?;
        }
        Ok(())
    }

    fn channel_entry(entry: StoredChannelEntry) -> Result<ChannelEntry, Fault> {
        parse_hash(&entry.descriptor.salt)
            .map_err(|error| Fault::config(format!("invalid stored channel salt: {error}")))?;
        Ok(ChannelEntry {
            channel_id: entry.channel_id,
            cumulative_amount: entry.cumulative_amount,
            deposit: entry.deposit,
            descriptor: entry.descriptor,
            settlement_route: entry.settlement_route,
            escrow_contract: entry.escrow,
            chain_id: entry.chain_id,
            opened: entry.opened,
        })
    }

    fn parse_operator(request: &SessionRequest) -> Result<Address, Fault> {
        match request.operator() {
            None => Ok(Address::ZERO),
            Some(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| Fault::config("methodDetails.operator must be a string"))?;
                text.parse()
                    .map_err(|_| Fault::config(format!("invalid operator address: {text}")))
            }
        }
    }

    fn payment_route(request: &SessionRequest) -> Result<(Address, Address, Address), Fault> {
        let merchant: Address = request
            .recipient
            .as_deref()
            .ok_or_else(|| Fault::config("session challenge missing recipient"))?
            .parse()
            .map_err(|_| Fault::config("invalid recipient address"))?;
        let target: Address = request
            .currency
            .parse()
            .map_err(|_| Fault::config("invalid currency address"))?;
        if request.machine_token_enabled() {
            return Err(Fault::config(
                "machine-token session settlement (machineTokenEnabled) is not supported by this client",
            ));
        }
        Ok((merchant, Self::parse_operator(request)?, target))
    }

    fn scope_key(payee: Address, token: Address, escrow: Address, chain_id: u64) -> String {
        channel_key(
            &format!("{payee:#x}"),
            &format!("{token:#x}"),
            escrow,
            chain_id,
        )
    }

    fn expected_channel_key(&self, challenge: &Challenge) -> Result<(String, u64), Fault> {
        let chain_id = resolve_chain_id(challenge);
        let escrow = resolve_escrow(challenge, chain_id)?;
        let request = session_request(challenge)?;
        let (payee, _, currency) = Self::payment_route(&request)?;
        Ok((Self::scope_key(payee, currency, escrow, chain_id), chain_id))
    }

    fn scope(
        &self,
        payee: Address,
        operator: Address,
        token: Address,
        escrow: Address,
        chain_id: u64,
    ) -> RecoveryScope {
        RecoveryScope {
            payer: self.payer(),
            authorized_signer: self.voucher_signer(),
            payee,
            operator,
            token,
            escrow,
            chain_id,
        }
    }

    async fn restore_from_snapshot(
        &self,
        stored: Option<&StoredChannelEntry>,
        scope: RecoveryScope,
        snapshot: &SessionSnapshot,
        request_amount: u128,
    ) -> Result<(ChannelEntry, bool), Fault> {
        let snapshot_channel_id = parse_hash(&snapshot.channel_id)
            .map_err(|error| Fault::config(format!("invalid snapshot channelId: {error}")))?;
        if stored.is_some_and(|entry| {
            entry.pending_open.is_some() && entry.channel_id != snapshot_channel_id
        }) {
            return Err(Fault::config("snapshot does not match pending channel"));
        }
        let state = read_channel_state(&self.rpc, &snapshot_channel_id, None).await?;
        let mut recovered = hydrate_session_snapshot(snapshot, scope, state)?;
        let spent = snapshot
            .spent
            .parse::<u128>()
            .map_err(|error| Fault::config(format!("invalid snapshot spent: {error}")))?;
        let boundary = spent
            .checked_add(request_amount)
            .ok_or_else(|| Fault::config("snapshot request cumulative amount overflowed"))?;
        recovered.cumulative_amount = recovered.cumulative_amount.max(boundary);
        let exceeds =
            || Fault::config("recovered session cumulative amount exceeds channel deposit");
        if recovered.cumulative_amount > recovered.deposit {
            return Err(exceeds());
        }
        if let Some(stored) = stored.filter(|stored| stored.channel_id == recovered.channel_id) {
            recovered.cumulative_amount = recovered.cumulative_amount.max(stored.cumulative_amount);
        }
        if recovered.cumulative_amount > recovered.deposit {
            return Err(exceeds());
        }
        self.store_set(&recovered).await?;
        let id = hash_text(&recovered.channel_id);
        self.shared
            .spend
            .update(|ledger| ledger.channels.insert(id.clone(), spent).map(drop));
        Self::channel_entry(recovered).map(|entry| (entry, true))
    }

    async fn restore_precompile_channel(
        &self,
        store_key: &str,
        scope: RecoveryScope,
        snapshot: Option<&SessionSnapshot>,
        request_amount: u128,
    ) -> Result<Option<(ChannelEntry, bool)>, Fault> {
        let stored = self.store_get(store_key).await?;
        let snapshot = match snapshot {
            Some(snapshot) => {
                can_sign_descriptor(&snapshot.descriptor, scope.payer, scope.authorized_signer)?
                    .then_some(snapshot)
            }
            None => None,
        };
        if let Some(snapshot) = snapshot {
            return self
                .restore_from_snapshot(stored.as_ref(), scope, snapshot, request_amount)
                .await
                .map(Some);
        }
        let Some(stored) = stored else {
            return Ok(None);
        };
        if !can_sign_descriptor(&stored.descriptor, scope.payer, scope.authorized_signer)? {
            return Ok(None);
        }
        let stored_spent = self
            .channel_store
            .server_spent(store_key, &stored.channel_id)
            .await
            .ok()
            .flatten()
            .filter(|spent| *spent <= stored.cumulative_amount);
        let state = read_channel_state(&self.rpc, &stored.channel_id, None).await?;
        if state.close_requested_at != 0 && state.deposit != 0 {
            return Err(Fault::config(
                "stored channel has a pending close; retained descriptor for recovery",
            ));
        }
        if state.deposit == 0 {
            self.store_delete(store_key).await?;
            return Ok(None);
        }
        let recovered = recover_stored_channel(stored, scope, state)?;
        let pending = recovered.pending_open.clone();
        let id = hash_text(&recovered.channel_id);
        if let Some(challenge_id) = &pending {
            let open = PendingOpen {
                payload: None,
                challenge_id: challenge_id.clone(),
                store_key: Some(store_key.to_owned()),
            };
            self.shared
                .pending_opens
                .update(|pending| pending.insert(id.clone(), open.clone()).map(drop));
        }
        self.store_set(&recovered).await?;
        if let Some(spent) = stored_spent {
            self.shared.spend.update(|ledger| {
                ledger.channels.entry(id.clone()).or_insert(spent);
            });
        }
        Self::channel_entry(recovered).map(|entry| Some((entry, pending.is_some())))
    }

    fn top_up_ahead(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        channel_id: &str,
        price: u128,
    ) {
        let (Some(amount), Some(client), Ok(runtime)) = (
            self.top_up_amount,
            context.http_client.clone(),
            tokio::runtime::Handle::try_current(),
        ) else {
            return;
        };
        let Some(entry) = self.channel_by_id(channel_id) else {
            return;
        };
        let reserve = price.saturating_mul(u128::from(self.prepaid_requests) + 1);
        if entry.deposit.saturating_sub(entry.cumulative_amount) >= reserve {
            return;
        }
        if !self
            .shared
            .topping_up
            .update(|topping| topping.insert(channel_id.to_owned()))
        {
            return;
        }
        let provider = self.clone();
        let url = context.url.to_string();
        let headers = context.headers.clone();
        let challenge = challenge.clone();
        let channel_id = channel_id.to_owned();
        runtime.spawn(async move {
            let _ = provider
                .top_up_in_background(
                    &client,
                    &url,
                    headers,
                    &challenge,
                    &channel_id,
                    amount,
                    reserve,
                )
                .await;
            provider
                .shared
                .topping_up
                .update(|topping| topping.remove(&channel_id));
        });
    }

    fn prepay(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        channel_id: String,
        spent: u128,
        quoted: Option<u128>,
    ) {
        if self.prepaid_requests == 0 {
            return;
        }
        let url = context.url.to_string();
        let price = self
            .shared
            .prepaid
            .read(|prepaid| {
                prepaid
                    .get(&url)
                    .filter(|previous| previous.channel_id == channel_id && spent > previous.spent)
                    .map(|previous| spent - previous.spent)
            })
            .or(quoted)
            .filter(|price| *price > 0);
        let Some(price) = price else {
            return;
        };
        let voucher = PrepaidVoucher {
            challenge: challenge.clone(),
            channel_id: channel_id.clone(),
            spent,
            price,
        };
        self.shared
            .prepaid
            .update(|prepaid| prepaid.insert(url.clone(), voucher.clone()).map(drop));
        self.top_up_ahead(context, challenge, &channel_id, price);
    }

    pub(crate) async fn restore(&self, challenge: &Challenge) -> Result<bool, Fault> {
        validate_for_session(challenge)?;
        let chain_id = resolve_chain_id(challenge);
        let escrow = resolve_escrow(challenge, chain_id)?;
        if !is_precompile_escrow(escrow) {
            return Ok(false);
        }
        let request = session_request(challenge)?;
        let (payee, operator, currency) = Self::payment_route(&request)?;
        let key = Self::scope_key(payee, currency, escrow, chain_id);
        if self.channel(&key).is_some() {
            return Ok(true);
        }
        let _process = self.acquire_payment_lock(challenge).await?;
        let _store = self.channel_store.acquire(&key).await?;
        if self.channel(&key).is_some() {
            return Ok(true);
        }
        let restored = self
            .restore_precompile_channel(
                &key,
                self.scope(payee, operator, currency, escrow, chain_id),
                request.session_snapshot().as_ref(),
                request.parse_amount()?,
            )
            .await?;
        let Some((entry, _)) = restored else {
            return Ok(false);
        };
        self.track(&key, &entry);
        Ok(true)
    }

    pub(crate) fn restored_channel_id(&self, challenge: &Challenge) -> Option<String> {
        let (key, _) = self.expected_channel_key(challenge).ok()?;
        self.channel(&key)
            .filter(|entry| entry.opened)
            .map(|entry| entry.id())
    }

    pub(crate) fn prime_prepaid(&self, url: &str, challenge: &Challenge) -> bool {
        if self.prepaid_requests == 0 {
            return false;
        }
        let Ok(request) = challenge.request.decode::<SessionRequest>() else {
            return false;
        };
        let Some(snapshot) = request.session_snapshot() else {
            return false;
        };
        let (Ok(spent), Ok(price)) = (snapshot.spent.parse::<u128>(), request.parse_amount())
        else {
            return false;
        };
        let Ok(channel_id) = parse_hash(&snapshot.channel_id) else {
            return false;
        };
        let channel_id = hash_text(&channel_id);
        if self.restored_channel_id(challenge).as_deref() != Some(channel_id.as_str()) {
            return false;
        }
        if self.bind_origin(url).is_err() {
            return false;
        }
        self.shared
            .spend
            .update(|ledger| ledger.channels.insert(channel_id.clone(), spent).map(drop));
        let voucher = PrepaidVoucher {
            challenge: challenge.clone(),
            channel_id,
            spent,
            price,
        };
        self.shared
            .prepaid
            .update(|prepaid| prepaid.insert(url.to_owned(), voucher.clone()).map(drop));
        true
    }

    fn sign(&self, hash: &Hash, label: &str) -> Result<String, Fault> {
        let signature = self
            .signer
            .sign_hash(hash)
            .map_err(|error| Fault::Internal(format!("failed to sign {label}: {error}")))?;
        Ok(hex::encode_prefixed(signature.to_bytes()))
    }

    fn voucher_payload(
        &self,
        descriptor: ChannelDescriptor,
        cumulative_amount: u128,
        chain_id: u64,
    ) -> Result<SessionCredentialPayload, Fault> {
        let channel_id = wire_channel_id(&descriptor, TIP20_CHANNEL_RESERVE_ADDRESS, chain_id)?;
        let hash = voucher_signing_hash(
            &channel_id,
            cumulative_amount,
            TIP20_CHANNEL_RESERVE_ADDRESS,
            chain_id,
        )?;
        Ok(SessionCredentialPayload::Voucher {
            channel_id: hash_text(&channel_id),
            descriptor: Some(descriptor),
            cumulative_amount: cumulative_amount.to_string(),
            signature: self.sign(&hash, "precompile voucher")?,
        })
    }

    fn close_payload(
        &self,
        channel_id: Hash,
        descriptor: ChannelDescriptor,
        cumulative_amount: u128,
        chain_id: u64,
    ) -> Result<SessionCredentialPayload, Fault> {
        if wire_channel_id(&descriptor, TIP20_CHANNEL_RESERVE_ADDRESS, chain_id)? != channel_id {
            return Err(Fault::config(
                "TIP-1034 close descriptor does not match channel_id",
            ));
        }
        let voucher = voucher_signing_hash(
            &channel_id,
            cumulative_amount,
            TIP20_CHANNEL_RESERVE_ADDRESS,
            chain_id,
        )?;
        let signature = self.sign(&voucher, "precompile voucher")?;
        let close = close_authorization_signing_hash(
            &channel_id,
            cumulative_amount,
            TIP20_CHANNEL_RESERVE_ADDRESS,
            chain_id,
        )?;
        Ok(SessionCredentialPayload::Close {
            channel_id: hash_text(&channel_id),
            descriptor: Some(descriptor),
            cumulative_amount: cumulative_amount.to_string(),
            signature,
            close_signature: Some(self.sign(&close, "close authorization")?),
        })
    }

    fn legacy_unsupported() -> Fault {
        Fault::config("Tempo access keys require the native TIP-1034 precompile")
    }

    fn insert_signed_entry(&self, key: &str, entry: &mut ChannelEntry) {
        let current = self.channel(key);
        if let Some(current) = current.filter(|current| current.channel_id == entry.channel_id) {
            entry.deposit = entry.deposit.max(current.deposit);
        }
        let stored = entry.clone();
        self.shared.channels.update(|channels| {
            let mut stored = stored.clone();
            if let Some(current) = channels
                .get(key)
                .filter(|current| current.channel_id == stored.channel_id)
            {
                stored.deposit = stored.deposit.max(current.deposit);
            }
            channels.insert(key.to_owned(), stored);
        });
    }

    async fn voucher_credential_for_challenge(
        &self,
        challenge: &Challenge,
        channel_id: &str,
        required: u128,
    ) -> Result<Credential, Fault> {
        let key = self
            .channel_key_of(channel_id)
            .ok_or_else(|| Fault::config(format!("no channel found for id {channel_id}")))?;
        let (expected_key, expected_chain_id) = self.expected_channel_key(challenge)?;
        if key != expected_key {
            return Err(Fault::config("channel does not match active session"));
        }
        let mut entry = self
            .channel(&key)
            .ok_or_else(|| Fault::config("channel not found"))?;
        if entry.chain_id != expected_chain_id || entry.id() != channel_id {
            return Err(Fault::config("channel does not match active session"));
        }
        self.merge_stored_channel(&key, &mut entry).await?;
        if required > entry.cumulative_amount && self.is_pending_open(channel_id) {
            return Err(Fault::config(
                "pending channel open requires settlement reconciliation before another payment",
            ));
        }
        entry.cumulative_amount = entry.cumulative_amount.max(required);
        if entry.cumulative_amount > entry.deposit {
            return Err(Fault::config(
                "voucher cumulative amount exceeds channel deposit",
            ));
        }
        if !is_precompile_escrow(entry.escrow_contract) {
            return Err(Self::legacy_unsupported());
        }
        let payload = self.voucher_payload(
            entry.descriptor.clone(),
            entry.cumulative_amount,
            entry.chain_id,
        )?;
        self.persist_channel(&entry).await?;
        self.insert_signed_entry(&key, &mut entry);
        build_credential(challenge, &payload, entry.chain_id, self.payer())
    }

    async fn sign_transaction(
        &self,
        transaction: nanocode_tempo::TempoTransaction,
        fee_payer: bool,
    ) -> Result<String, Fault> {
        let bytes = match fee_payer {
            true => {
                sign_and_encode_fee_payer_envelope_primitive_async(
                    transaction,
                    &self.signer,
                    &self.signing_mode,
                )
                .await?
            }
            false => {
                sign_and_encode_primitive_async(transaction, &self.signer, &self.signing_mode)
                    .await?
            }
        };
        Ok(hex::encode_prefixed(bytes))
    }

    async fn transaction_options(
        &self,
        calls: Vec<Call>,
        chain_id: u64,
        fee_token: Address,
        fee_payer: bool,
    ) -> Result<TempoTxOptions, Fault> {
        let gas_price = tx::gas_price(&self.rpc)
            .await
            .map_err(|error| Fault::http(format!("failed to get gas price: {error}")))?;
        let (nonce, nonce_key, valid_before) = session_nonce_fields();
        let key_authorization: Option<SignedKeyAuthorization> =
            resolve_key_authorization(&self.rpc, &self.signing_mode, self.signer.address()).await?;
        Ok(TempoTxOptions {
            calls,
            chain_id,
            fee_token,
            nonce,
            nonce_key,
            gas_limit: SESSION_GAS_LIMIT,
            max_fee_per_gas: gas_price,
            max_priority_fee_per_gas: gas_price,
            fee_payer,
            valid_before,
            key_authorization,
        })
    }

    async fn estimate(
        &self,
        options: &mut TempoTxOptions,
        valid_after: Option<u64>,
        payer: Address,
    ) -> Result<(), Fault> {
        if options.fee_payer {
            return Ok(());
        }
        let request = TempoTransactionRequest {
            from: Some(payer),
            max_fee_per_gas: Some(options.max_fee_per_gas),
            max_priority_fee_per_gas: Some(options.max_priority_fee_per_gas),
            nonce: Some(options.nonce),
            chain_id: Some(options.chain_id),
            fee_token: Some(options.fee_token),
            nonce_key: Some(options.nonce_key),
            calls: options.calls.clone(),
            key_type: Some(self.signer.signature_type()),
            key_id: matches!(self.signing_mode, TempoSigningMode::Keychain { .. })
                .then(|| self.signer.address()),
            key_authorization: options.key_authorization.clone(),
            valid_before: options
                .valid_before
                .filter(|valid_before| *valid_before != 0),
            valid_after,
            ..TempoTransactionRequest::default()
        };
        options.gas_limit = tx::estimate_gas(&self.rpc, request).await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_payload(
        &self,
        prefix_calls: Vec<Call>,
        operator: Address,
        payee: Address,
        currency: Address,
        fee_token: Address,
        deposit: u128,
        initial_amount: u128,
        chain_id: u64,
        fee_payer: bool,
    ) -> Result<(ChannelEntry, SessionCredentialPayload, Option<u64>), Fault> {
        if deposit > PRECOMPILE_MAX_CUMULATIVE_AMOUNT {
            return Err(Fault::config(format!(
                "deposit {deposit} exceeds precompile uint96 max"
            )));
        }
        if initial_amount > PRECOMPILE_MAX_CUMULATIVE_AMOUNT {
            return Err(Fault::config(format!(
                "initial_amount {initial_amount} exceeds precompile uint96 max"
            )));
        }
        let payer = self.payer();
        let authorized_signer = self.voucher_signer();
        let salt = random_hash()?;
        let mut calls = prefix_calls;
        calls.push(Call {
            to: TIP20_CHANNEL_RESERVE_ADDRESS,
            value: U256::ZERO,
            input: channel::open_call(payee, operator, currency, deposit, salt, authorized_signer)?,
        });
        let mut options = self
            .transaction_options(calls, chain_id, fee_token, fee_payer)
            .await?;
        self.estimate(&mut options, None, payer).await?;
        let valid_before = options.valid_before;
        let unsigned = build_tempo_tx(options);
        let expiring_nonce_hash = expiring_nonce_hash(&unsigned, payer);
        let descriptor = Descriptor {
            payer,
            payee,
            operator,
            token: currency,
            salt,
            authorized_signer,
            expiring_nonce_hash,
        };
        let channel_id = descriptor.channel_id(TIP20_CHANNEL_RESERVE_ADDRESS, chain_id);
        let transaction = self.sign_transaction(unsigned, fee_payer).await?;
        let voucher = voucher_signing_hash(
            &channel_id,
            initial_amount,
            TIP20_CHANNEL_RESERVE_ADDRESS,
            chain_id,
        )?;
        let signature = self.sign(&voucher, "precompile voucher")?;
        let descriptor = descriptor.to_wire();
        let entry = ChannelEntry {
            channel_id,
            cumulative_amount: initial_amount,
            deposit,
            descriptor: descriptor.clone(),
            settlement_route: None,
            escrow_contract: TIP20_CHANNEL_RESERVE_ADDRESS,
            chain_id,
            opened: true,
        };
        let payload = SessionCredentialPayload::Open {
            payload_type: TRANSACTION.to_owned(),
            channel_id: hash_text(&channel_id),
            transaction,
            descriptor: Some(descriptor),
            authorized_signer: Some(authorized_signer.to_string()),
            cumulative_amount: initial_amount.to_string(),
            signature,
        };
        Ok((entry, payload, valid_before))
    }

    async fn top_up_payload(
        &self,
        prefix_calls: Vec<Call>,
        descriptor: &ChannelDescriptor,
        fee_token: Address,
        additional_deposit: u128,
        chain_id: u64,
        fee_payer: bool,
    ) -> Result<SessionCredentialPayload, Fault> {
        if additional_deposit > PRECOMPILE_MAX_CUMULATIVE_AMOUNT {
            return Err(Fault::config(format!(
                "additional_deposit {additional_deposit} exceeds precompile uint96 max"
            )));
        }
        if additional_deposit == 0 {
            return Err(Fault::config("top-up amount must be greater than zero"));
        }
        let typed = Descriptor::from_wire(descriptor)?;
        let mut calls = prefix_calls;
        calls.push(Call {
            to: TIP20_CHANNEL_RESERVE_ADDRESS,
            value: U256::ZERO,
            input: channel::top_up_call(typed, additional_deposit)?,
        });
        let mut options = self
            .transaction_options(calls, chain_id, fee_token, fee_payer)
            .await?;
        let valid_after = random_past_valid_after()?;
        self.estimate(&mut options, valid_after, self.payer())
            .await?;
        let mut unsigned = build_tempo_tx(options);
        unsigned.valid_after = valid_after;
        let transaction = self.sign_transaction(unsigned, fee_payer).await?;
        let channel_id = typed.channel_id(TIP20_CHANNEL_RESERVE_ADDRESS, chain_id);
        Ok(SessionCredentialPayload::TopUp {
            payload_type: TRANSACTION.to_owned(),
            channel_id: hash_text(&channel_id),
            transaction,
            descriptor: Some(descriptor.clone()),
            additional_deposit: additional_deposit.to_string(),
        })
    }

    async fn top_up_credential_for_challenge(
        &self,
        challenge: &Challenge,
        entry: &ChannelEntry,
        additional: u128,
    ) -> Result<Credential, Fault> {
        let request = session_request(challenge)?;
        let payer = self.payer();
        let currency: Address = entry.descriptor.token.parse().map_err(|error| {
            Fault::config(format!("invalid TIP-1034 descriptor token: {error}"))
        })?;
        let (prefix_calls, route) = self.autoswap_calls(payer, currency, additional).await?;
        let fee_token = self.session_fee_token(&request, currency, route.as_ref())?;
        let payload = self
            .top_up_payload(
                prefix_calls,
                &entry.descriptor,
                fee_token,
                additional,
                entry.chain_id,
                request.fee_payer(),
            )
            .await?;
        build_credential(challenge, &payload, entry.chain_id, payer)
    }

    async fn top_up_with_lease(
        &self,
        client: &reqwest::Client,
        url: &str,
        headers: HeaderMap,
        challenge: &Challenge,
        channel_id: &str,
        additional: u128,
    ) -> Result<Option<Receipt>, Fault> {
        let prepared = self
            .prepare_top_up(headers, challenge, channel_id, additional)
            .await?;
        let receipt = self
            .submit_top_up(client, url, challenge, &prepared)
            .await?;
        self.record_top_up(url, &prepared, None).await?;
        Ok(receipt)
    }

    async fn top_up_with_management(
        &self,
        transport: &dyn ManagementTransport,
        url: &str,
        challenge: &Challenge,
        channel_id: &str,
        additional: u128,
    ) -> Result<Option<Receipt>, Fault> {
        let prepared = self
            .prepare_top_up(HeaderMap::new(), challenge, channel_id, additional)
            .await?;
        let uncertain = || {
            let id = prepared.channel_id;
            self.shared
                .uncertain_top_ups
                .update(|uncertain| uncertain.insert(id));
        };
        let Ok(receipt) = tokio::time::timeout(
            self.management_timeout,
            transport.send(challenge, &prepared.credential),
        )
        .await
        else {
            uncertain();
            return Err(Fault::SessionOutcomeUnknown {
                context: "top-up request failed: session management request timed out".into(),
            });
        };
        let receipt = receipt.map_err(|error| match error {
            Fault::Http(_) => {
                uncertain();
                Fault::SessionOutcomeUnknown {
                    context: "top-up management call failed".into(),
                }
            }
            error => error,
        })?;
        self.record_top_up(url, &prepared, None).await?;
        Ok(receipt)
    }

    #[allow(clippy::too_many_arguments)]
    async fn top_up_in_background(
        &self,
        client: &reqwest::Client,
        url: &str,
        headers: HeaderMap,
        challenge: &Challenge,
        channel_id: &str,
        additional: u128,
        reserve: u128,
    ) -> Result<(), Fault> {
        let Some(key) = self.channel_key_of(channel_id) else {
            return Ok(());
        };
        let funding = self.acquire_funding_lock(&key, challenge).await?;
        let covered = self
            .channel(&key)
            .filter(|entry| entry.id() == channel_id)
            .is_none_or(|entry| entry.deposit.saturating_sub(entry.cumulative_amount) >= reserve);
        if covered {
            return Ok(());
        }
        let prepared = self
            .prepare_top_up(headers, challenge, channel_id, additional)
            .await?;
        self.submit_top_up(client, url, challenge, &prepared)
            .await?;
        let previous = self.raise_local_deposit(&prepared, channel_id);
        drop(funding);
        let _process = self.acquire_payment_lock(challenge).await?;
        self.record_top_up(url, &prepared, previous).await
    }

    fn raise_local_deposit(&self, prepared: &PreparedTopUp, channel_id: &str) -> Option<u128> {
        self.shared.channels.update(|channels| {
            let entry = channels
                .get_mut(&prepared.key)
                .filter(|entry| entry.id() == channel_id)?;
            let previous = entry.deposit;
            entry.deposit = entry.deposit.max(prepared.new_deposit);
            Some(previous)
        })
    }

    async fn prepare_top_up(
        &self,
        headers: HeaderMap,
        challenge: &Challenge,
        channel_id: &str,
        additional: u128,
    ) -> Result<PreparedTopUp, Fault> {
        ensure_redirect_safe_credential_header(challenge)?;
        if additional == 0 {
            return Err(Fault::config("top-up amount must be greater than zero"));
        }
        let key = self
            .channel_key_of(channel_id)
            .ok_or_else(|| Fault::config(format!("no channel found for id {channel_id}")))?;
        let (expected_key, expected_chain_id) = self.expected_channel_key(challenge)?;
        if key != expected_key {
            return Err(Fault::config("channel does not match active session"));
        }
        let mut entry = self
            .channel(&key)
            .ok_or_else(|| Fault::config("channel not found"))?;
        if entry.chain_id != expected_chain_id || entry.id() != channel_id {
            return Err(Fault::config("channel does not match active session"));
        }
        if !is_precompile_escrow(entry.escrow_contract) {
            return Err(Fault::config(
                "automatic top-up requires the native TIP-1034 precompile",
            ));
        }
        if self
            .shared
            .uncertain_top_ups
            .read(|uncertain| uncertain.contains(&entry.channel_id))
        {
            return Err(Fault::config(
                "session funding outcome is unknown; reconcile the pending payment before another top-up",
            ));
        }
        if let Some(stored) = self.store_get(&key).await? {
            if stored.channel_id != entry.channel_id {
                return Err(Fault::config("stored channel no longer matches top-up"));
            }
            entry.deposit = entry.deposit.max(stored.deposit);
            entry.cumulative_amount = entry.cumulative_amount.max(stored.cumulative_amount);
        }
        let state = read_channel_state(&self.rpc, &entry.channel_id, None).await?;
        entry.deposit = entry.deposit.max(state.deposit);
        let mut headers = management_headers(headers);
        let new_deposit = entry
            .deposit
            .checked_add(additional)
            .ok_or_else(|| Fault::config("channel deposit overflowed"))?;
        let credential = self
            .top_up_credential_for_challenge(challenge, &entry, additional)
            .await?;
        headers.insert(
            credential_header_name(challenge),
            authorization_value(&credential, "invalid top-up authorization header")?,
        );
        Ok(PreparedTopUp {
            key,
            channel_id: entry.channel_id,
            chain_id: entry.chain_id,
            cumulative_amount: entry.cumulative_amount,
            credential,
            headers,
            new_deposit,
        })
    }

    async fn submit_top_up(
        &self,
        client: &reqwest::Client,
        url: &str,
        challenge: &Challenge,
        prepared: &PreparedTopUp,
    ) -> Result<Option<Receipt>, Fault> {
        let mut headers = prepared.headers.clone();
        let mut response = self
            .send_top_up(
                client.post(url).headers(headers.clone()),
                "top-up POST failed",
                prepared.channel_id,
            )
            .await?;
        if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED
            && let Some(fresh) = session_challenge(&response)
        {
            let (fresh_key, fresh_chain_id) = self.expected_channel_key(&fresh)?;
            if fresh_key != prepared.key || fresh_chain_id != prepared.chain_id {
                return Err(Fault::config(
                    "top-up route challenge does not match active session",
                ));
            }
            let original = session_request(challenge)?;
            let refreshed = fresh.request.decode::<SessionRequest>().map_err(|error| {
                Fault::config(format!(
                    "failed to decode refreshed session request: {error}"
                ))
            })?;
            if original.fee_payer() != refreshed.fee_payer() {
                return Err(Fault::config("top-up route changed fee-payer signing mode"));
            }
            ensure_redirect_safe_credential_header(&fresh)?;
            let mut credential = prepared.credential.clone();
            credential.challenge = fresh.to_echo();
            headers.remove(credential_header_name(challenge));
            headers.insert(
                credential_header_name(&fresh),
                authorization_value(&credential, "invalid refreshed top-up authorization header")?,
            );
            response = self
                .send_top_up(
                    client.post(url).headers(headers),
                    "refreshed top-up POST failed",
                    prepared.channel_id,
                )
                .await?;
        }
        let status = response.status();
        let receipt = response_receipt(&response);
        if !status.is_success() {
            let body = self.management_body(response).await;
            return Err(Fault::http(format!(
                "top-up POST returned {status}: {body}"
            )));
        }
        Ok(receipt)
    }

    async fn record_top_up(
        &self,
        url: &str,
        prepared: &PreparedTopUp,
        previous: Option<u128>,
    ) -> Result<(), Fault> {
        let mut entry = self
            .channel(&prepared.key)
            .ok_or_else(|| Fault::config("channel not found"))?;
        let previous = previous.unwrap_or(entry.deposit).min(entry.deposit);
        entry.deposit = entry.deposit.max(prepared.new_deposit);
        let amount = prepared.new_deposit.saturating_sub(previous);
        entry.cumulative_amount = entry.cumulative_amount.max(prepared.cumulative_amount);
        entry.opened = true;
        self.persist_channel(&entry).await?;
        let stored = entry.clone();
        self.shared.channels.update(|channels| {
            channels
                .insert(prepared.key.clone(), stored.clone())
                .map(drop)
        });
        if amount > 0 {
            self.report_deposit(url, &entry, amount, false);
        }
        Ok(())
    }

    fn close_credential_for_challenge(
        &self,
        challenge: &Challenge,
        channel_id: &str,
    ) -> Result<Credential, Fault> {
        let key = self
            .channel_key_of(channel_id)
            .ok_or_else(|| Fault::config(format!("no channel found for id {channel_id}")))?;
        let (expected_key, expected_chain_id) = self.expected_channel_key(challenge)?;
        if key != expected_key {
            return Err(Fault::config("channel does not match active session"));
        }
        let entry = self
            .channel(&key)
            .filter(|entry| entry.opened)
            .ok_or_else(|| Fault::config("channel not found"))?;
        if entry.chain_id != expected_chain_id || entry.id() != channel_id {
            return Err(Fault::config("channel does not match active session"));
        }
        if !is_precompile_escrow(entry.escrow_contract) {
            return Err(Self::legacy_unsupported());
        }
        let payload = self.close_payload(
            entry.channel_id,
            entry.descriptor,
            entry.cumulative_amount,
            entry.chain_id,
        )?;
        build_credential(challenge, &payload, entry.chain_id, self.payer())
    }

    async fn send_top_up(
        &self,
        request: reqwest::RequestBuilder,
        context: &str,
        channel_id: Hash,
    ) -> Result<reqwest::Response, Fault> {
        let error = match tokio::time::timeout(self.management_timeout, request.send()).await {
            Ok(Ok(response)) => return Ok(nanocode_http::decode_flate_response(response)),
            Ok(Err(error)) if error.is_connect() => {
                return Err(Fault::http(format!("{context}: {error}")));
            }
            Ok(Err(error)) => format!("{context}: {error}"),
            Err(_) => format!("{context}: session management request timed out"),
        };
        self.shared
            .uncertain_top_ups
            .update(|uncertain| uncertain.insert(channel_id));
        Err(Fault::SessionOutcomeUnknown { context: error })
    }

    async fn management_body(&self, response: reqwest::Response) -> String {
        tokio::time::timeout(self.management_timeout, response.text())
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    async fn post_credential(
        &self,
        client: &reqwest::Client,
        url: &str,
        challenge: &Challenge,
        credential: &Credential,
        context: &str,
    ) -> Result<reqwest::Response, Fault> {
        ensure_redirect_safe_credential_header(challenge)?;
        let request = client.post(url).header(
            credential_header_name(challenge),
            credential.authorization()?,
        );
        with_management_timeout(self.management_timeout, context, async {
            request
                .send()
                .await
                .map(nanocode_http::decode_flate_response)
                .map_err(http(context))
        })
        .await
    }

    fn refreshed_challenge(response: &reqwest::Response) -> Option<Challenge> {
        (response.status() == reqwest::StatusCode::PAYMENT_REQUIRED)
            .then(|| session_challenge(response))
            .flatten()
    }

    fn last_challenge(&self, purpose: &str) -> Result<Challenge, Fault> {
        self.shared
            .last_challenge
            .read(Clone::clone)
            .ok_or_else(|| Fault::config(format!("no challenge available for {purpose}")))
    }

    pub(crate) async fn send_voucher(
        &self,
        client: &reqwest::Client,
        url: &str,
        channel_id: &str,
        required: u128,
    ) -> Result<(), Fault> {
        self.bind_origin(url)?;
        let challenge = self.last_challenge("voucher")?;
        ensure_redirect_safe_credential_header(&challenge)?;
        let credential = self
            .voucher_credential_for_challenge(&challenge, channel_id, required)
            .await?;
        let mut response = self
            .post_credential(client, url, &challenge, &credential, "voucher POST failed")
            .await?;
        let refreshed = Self::refreshed_challenge(&response);
        if let Some(refreshed) = &refreshed {
            ensure_redirect_safe_credential_header(refreshed)?;
            let credential = self
                .voucher_credential_for_challenge(refreshed, channel_id, required)
                .await?;
            response = self
                .post_credential(client, url, refreshed, &credential, "voucher POST failed")
                .await?;
        }
        if !response.status().is_success() {
            return Err(Fault::http(format!(
                "voucher POST returned status {}",
                response.status()
            )));
        }
        if let Some(refreshed) = refreshed {
            self.shared.last_challenge.set(Some(refreshed));
        }
        Ok(())
    }

    async fn record_stream_receipt(&self, url: Option<Url>, channel_id: &str, spent: u128) {
        let Ok(channel_id) = parse_hash(channel_id) else {
            return;
        };
        let channel_id = hash_text(&channel_id);
        let change = self
            .shared
            .spend
            .update(|ledger| ledger.update(&channel_id, spent));
        if let (Some(change), Some(url)) = (change, url) {
            self.events.payment_update(&PaymentUpdate { url, change });
        }
        self.save_server_spent(&channel_id).await;
    }

    async fn save_server_spent(&self, channel_id: &str) {
        let Ok(_save) = self.shared.server_spent_saves.clone().acquire_owned().await else {
            return;
        };
        let (Ok(id), Some(key)) = (parse_hash(channel_id), self.channel_key_of(channel_id)) else {
            return;
        };
        let Some(spent) = self
            .shared
            .spend
            .read(|ledger| ledger.channels.get(channel_id).copied())
        else {
            return;
        };
        let _ = self.channel_store.set_server_spent(&key, &id, spent).await;
    }

    pub(crate) async fn acknowledge_accepted(
        &self,
        channel_id: &str,
        accepted: u128,
    ) -> Result<bool, Fault> {
        let channel_id = parse_hash(channel_id)
            .map(|id| hash_text(&id))
            .map_err(|_| Fault::config("invalid session channel ID"))?;
        if !self.is_pending_open(&channel_id) {
            return Ok(false);
        }
        let Some(entry) = self
            .channel_by_id(&channel_id)
            .filter(|entry| accepted >= entry.cumulative_amount)
        else {
            return Ok(false);
        };
        self.commit_referenced_open(&channel_id);
        self.persist_channel(&entry).await?;
        Ok(true)
    }

    pub(crate) async fn close(
        &self,
        client: &reqwest::Client,
        url: &str,
    ) -> Result<Option<Receipt>, Fault> {
        self.bind_origin(url)?;
        let Some(challenge) = self.shared.last_challenge.read(Clone::clone) else {
            return Ok(None);
        };
        if !self
            .shared
            .channels
            .read(|channels| channels.values().any(|entry| entry.opened))
        {
            return Ok(None);
        }
        let _process = self.acquire_payment_lock(&challenge).await?;
        let (expected_key, expected_chain_id) = self.expected_channel_key(&challenge)?;
        let _store = self.channel_store.acquire(&expected_key).await?;
        let Some(entry) = self.channel(&expected_key).filter(|entry| entry.opened) else {
            return Ok(None);
        };
        if entry.chain_id != expected_chain_id {
            return Err(Fault::config("channel does not match active session"));
        }
        ensure_redirect_safe_credential_header(&challenge)?;
        let channel_id = entry.id();
        let credential = self.close_credential_for_challenge(&challenge, &channel_id)?;
        let mut response = self
            .post_credential(client, url, &challenge, &credential, "close request failed")
            .await?;
        if let Some(refreshed) = Self::refreshed_challenge(&response) {
            ensure_redirect_safe_credential_header(&refreshed)?;
            let credential = self.close_credential_for_challenge(&refreshed, &channel_id)?;
            response = self
                .post_credential(client, url, &refreshed, &credential, "close request failed")
                .await?;
        }
        let status = response.status();
        let receipt = response_receipt(&response);
        let native_gone =
            status == reqwest::StatusCode::GONE && is_precompile_escrow(entry.escrow_contract);
        if !status.is_success() && !native_gone {
            let body = self.management_body(response).await;
            return Err(Fault::http(format!(
                "close request returned {status}: {body}"
            )));
        }
        if receipt.is_some() || native_gone {
            self.forget_closed_channel(&entry, &expected_key).await?;
        }
        Ok(receipt)
    }

    async fn forget_closed_channel(&self, entry: &ChannelEntry, key: &str) -> Result<(), Fault> {
        if !is_precompile_escrow(entry.escrow_contract) {
            return Ok(());
        }
        let Ok(state) = read_channel_state(&self.rpc, &entry.channel_id, None).await else {
            return Ok(());
        };
        if state.deposit != 0 {
            return Ok(());
        }
        self.store_delete(&Self::stored_entry(entry).key()).await?;
        self.shared
            .channels
            .update(|channels| channels.remove(key).map(drop));
        Ok(())
    }

    fn resolve_deposit(&self, suggested: Option<&str>) -> Result<u128, Fault> {
        let suggested = suggested.and_then(|text| text.parse::<u128>().ok());
        suggested.or(self.default_deposit).ok_or_else(|| {
            Fault::config(
                "No deposit amount available. Set \x60default_deposit\x60, \x60max_deposit\x60, or ensure the server challenge includes \x60suggestedDeposit\x60.",
            )
        })
    }

    async fn lock_until_expiry(
        lock: Arc<Semaphore>,
        challenge: &Challenge,
    ) -> Result<OwnedSemaphorePermit, Fault> {
        let closed = |_| Fault::Internal("session payment lock closed".into());
        let Some(expires) = challenge.expires_at() else {
            return lock.acquire_owned().await.map_err(closed);
        };
        let remaining = u64::try_from(expires - protocol::now_seconds()).unwrap_or(0);
        match tokio::time::timeout(Duration::from_secs(remaining), lock.acquire_owned()).await {
            Ok(permit) => permit.map_err(closed),
            Err(_) => Err(Fault::Expired(challenge.expires.clone())),
        }
    }

    async fn acquire_payment_lock(
        &self,
        challenge: &Challenge,
    ) -> Result<OwnedSemaphorePermit, Fault> {
        Self::lock_until_expiry(self.shared.payment_lock.clone(), challenge).await
    }

    async fn acquire_funding_lock(
        &self,
        key: &str,
        challenge: &Challenge,
    ) -> Result<OwnedSemaphorePermit, Fault> {
        let fresh = Arc::new(Semaphore::new(1));
        let lock = self.shared.funding_locks.update(|locks| {
            locks
                .entry(key.to_owned())
                .or_insert_with(|| fresh.clone())
                .clone()
        });
        Self::lock_until_expiry(lock, challenge).await
    }

    pub(crate) async fn pay(&self, challenge: &Challenge) -> Result<Credential, Fault> {
        self.payment_credential(challenge, None).await
    }

    async fn payment_credential(
        &self,
        challenge: &Challenge,
        top_up: Option<TopUpRoute<'_>>,
    ) -> Result<Credential, Fault> {
        validate_for_session(challenge)?;
        let process = self.acquire_payment_lock(challenge).await?;
        let (key, _) = self.expected_channel_key(challenge)?;
        let store = self.channel_store.acquire(&key).await?;
        let credential = self.payment_credential_inner(challenge, top_up).await?;
        let lease = Arc::new(PaymentLease {
            channel_id: SessionCredentialPayload::channel_id(&credential.payload),
            _process: process,
            _store: store,
        });
        let id = challenge.id.clone();
        self.shared
            .leases
            .update(|leases| leases.insert(id.clone(), lease.clone()).map(drop));
        Ok(credential)
    }

    async fn reconcile_pending_expiry(
        &self,
        key: &str,
        entry: &ChannelEntry,
    ) -> Result<bool, Fault> {
        let valid_before = self
            .pending_open(&entry.id())
            .and_then(|pending| pending.payload)
            .and_then(|(_, valid_before)| valid_before);
        let Some(valid_before) = valid_before else {
            return Ok(false);
        };
        let now = unix_seconds();
        Ok(now > valid_before
            && self
                .reconcile_expired_open(key, entry, valid_before)
                .await?)
    }

    async fn existing_channel(
        &self,
        key: &str,
        scope: RecoveryScope,
        request: &SessionRequest,
        request_amount: u128,
    ) -> Result<Option<(ChannelEntry, bool)>, Fault> {
        let mut existing = self.channel(key).map(|entry| (entry, false));
        if let Some((entry, _)) = existing.as_mut() {
            self.merge_stored_channel(key, entry).await?;
        }
        if let Some((entry, _)) = &existing
            && self.reconcile_pending_expiry(key, entry).await?
        {
            existing = None;
        }
        let snapshot = request.session_snapshot();
        let reconcile_pending = snapshot.is_some()
            && existing
                .as_ref()
                .is_some_and(|(entry, _)| self.is_pending_open(&entry.id()));
        if let (true, Some(snapshot), Some((entry, _))) =
            (reconcile_pending, snapshot.as_ref(), existing.as_ref())
        {
            if parse_hash(&snapshot.channel_id).ok() != Some(entry.channel_id) {
                return Err(Fault::config("snapshot does not match pending channel"));
            }
            if !can_sign_descriptor(&snapshot.descriptor, scope.payer, scope.authorized_signer)? {
                return Err(Fault::config(
                    "pending channel snapshot does not match the signer",
                ));
            }
            existing = None;
        }
        if existing.is_none() {
            existing = self
                .restore_precompile_channel(key, scope, snapshot.as_ref(), request_amount)
                .await?;
            if let Some((entry, _)) = &existing {
                self.track(key, entry);
                if reconcile_pending {
                    self.commit_referenced_open(&entry.id());
                }
            }
        }
        if reconcile_pending && existing.is_none() {
            return Err(Fault::config(
                "pending channel open could not be reconciled",
            ));
        }
        Ok(existing)
    }

    async fn pending_retry(
        &self,
        entry: &ChannelEntry,
        includes_request: bool,
    ) -> Result<Option<SessionCredentialPayload>, Fault> {
        let id = entry.id();
        let mut pending = self.is_pending_open(&id);
        if pending && !includes_request {
            let state = read_channel_state(&self.rpc, &entry.channel_id, None)
                .await
                .map_err(|_| Fault::SessionOutcomeUnknown {
                    context: "pending opening funding could not be reconciled".into(),
                })?;
            if state.deposit != 0 || state.settled != 0 || state.close_requested_at != 0 {
                self.settle_pending_payload(&id);
            }
            if state.settled != 0 && state.settled >= entry.cumulative_amount {
                self.commit_referenced_open(&id);
                self.persist_channel(entry).await?;
                pending = false;
            }
        }
        if !pending {
            return Ok(None);
        }
        let payload = self
            .pending_open(&id)
            .and_then(|pending| pending.payload)
            .map(|(payload, _)| payload);
        Ok(payload)
    }

    #[allow(clippy::too_many_arguments)]
    async fn pay_existing(
        &self,
        challenge: &Challenge,
        key: &str,
        mut entry: ChannelEntry,
        includes_request: bool,
        request: &SessionRequest,
        amount: u128,
        chain_id: u64,
        top_up: Option<TopUpRoute<'_>>,
    ) -> Result<Credential, Fault> {
        let payer = self.payer();
        if let Some(payload) = self.pending_retry(&entry, includes_request).await? {
            return build_credential(challenge, &payload, chain_id, payer);
        }
        let pending = self.is_pending_open(&entry.id());
        if !includes_request && !pending {
            entry.cumulative_amount = entry
                .cumulative_amount
                .checked_add(amount)
                .ok_or_else(|| Fault::config("session cumulative amount overflowed"))?;
        }
        if entry.cumulative_amount > entry.deposit {
            let Some(top_up) = top_up else {
                return Err(Fault::config(
                    "session cumulative amount exceeds channel deposit",
                ));
            };
            let _funding = self.acquire_funding_lock(key, challenge).await?;
            if let Some(current) = self
                .channel(key)
                .filter(|current| current.channel_id == entry.channel_id)
            {
                entry.deposit = entry.deposit.max(current.deposit);
            }
            if entry.cumulative_amount > entry.deposit {
                let additional = self
                    .required_top_up(
                        entry.cumulative_amount,
                        entry.deposit,
                        request.suggested_deposit.as_deref(),
                    )?
                    .ok_or_else(|| Fault::config("session top-up amount underflowed"))?;
                let channel_id = entry.id();
                match top_up {
                    TopUpRoute::Http {
                        client,
                        url,
                        headers,
                    } => {
                        self.top_up_with_lease(
                            client,
                            url,
                            headers,
                            challenge,
                            &channel_id,
                            additional,
                        )
                        .await?;
                    }
                    TopUpRoute::Management { transport, url } => {
                        self.top_up_with_management(
                            transport,
                            url,
                            challenge,
                            &channel_id,
                            additional,
                        )
                        .await?;
                    }
                }
                entry.deposit = self
                    .channel(key)
                    .ok_or_else(|| Fault::config("channel disappeared after top-up"))?
                    .deposit;
            }
        }
        let payload =
            self.voucher_payload(entry.descriptor.clone(), entry.cumulative_amount, chain_id)?;
        self.persist_channel(&entry).await?;
        self.insert_signed_entry(key, &mut entry);
        build_credential(challenge, &payload, chain_id, payer)
    }

    async fn payment_credential_inner(
        &self,
        challenge: &Challenge,
        top_up: Option<TopUpRoute<'_>>,
    ) -> Result<Credential, Fault> {
        validate_for_session(challenge)?;
        self.shared.last_challenge.set(Some(challenge.clone()));
        let chain_id = resolve_chain_id(challenge);
        let escrow = resolve_escrow(challenge, chain_id)?;
        let request = session_request(challenge)?;
        let (payee, operator, currency) = Self::payment_route(&request)?;
        let amount = request.parse_amount()?;
        if !is_precompile_escrow(escrow) {
            return Err(Self::legacy_unsupported());
        }
        let key = Self::scope_key(payee, currency, escrow, chain_id);
        let scope = self.scope(payee, operator, currency, escrow, chain_id);
        let existing = self.existing_channel(&key, scope, &request, amount).await?;
        if let Some((entry, includes_request)) = existing.filter(|(entry, _)| entry.opened) {
            return self
                .pay_existing(
                    challenge,
                    &key,
                    entry,
                    includes_request,
                    &request,
                    amount,
                    chain_id,
                    top_up,
                )
                .await;
        }
        let deposit = self.resolve_deposit(request.suggested_deposit.as_deref())?;
        if deposit < amount {
            return Err(Fault::config(format!(
                "opening deposit {deposit} below request amount {amount}"
            )));
        }
        let payer = self.payer();
        let (prefix_calls, route) = self.autoswap_calls(payer, currency, deposit).await?;
        let fee_token = self.session_fee_token(&request, currency, route.as_ref())?;
        let (entry, payload, valid_before) = self
            .open_payload(
                prefix_calls,
                operator,
                payee,
                currency,
                fee_token,
                deposit,
                amount,
                chain_id,
                request.fee_payer(),
            )
            .await?;
        let id = entry.id();
        let pending = PendingOpen {
            payload: Some((payload.clone(), valid_before)),
            challenge_id: challenge.id.clone(),
            store_key: Self::store_key(&entry),
        };
        self.shared
            .pending_opens
            .update(|pending_opens| pending_opens.insert(id.clone(), pending.clone()).map(drop));
        if let Err(error) = self.persist_channel(&entry).await {
            self.shared
                .pending_opens
                .update(|pending_opens| pending_opens.remove(&id).map(drop));
            return Err(error);
        }
        self.track(&key, &entry);
        build_credential(challenge, &payload, chain_id, payer)
    }

    pub(crate) async fn pay_with_context(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
    ) -> Result<Credential, Fault> {
        let Some(client) = context.http_client else {
            self.bind_http_origin(context.url.as_str())?;
            return self.pay(challenge).await;
        };
        let url = context.url.to_string();
        self.bind_origin(&url)?;
        self.payment_credential(
            challenge,
            Some(TopUpRoute::Http {
                client: &client,
                url: &url,
                headers: context.headers,
            }),
        )
        .await
    }

    pub(crate) async fn pay_with_management(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> Result<Credential, Fault> {
        self.bind_http_origin(context.url.as_str())?;
        let url = context.url.to_string();
        self.payment_credential(
            challenge,
            Some(TopUpRoute::Management {
                transport: management,
                url: &url,
            }),
        )
        .await
    }

    fn take_lease(&self, challenge: &Challenge) -> Option<Arc<PaymentLease>> {
        self.shared
            .leases
            .update(|leases| leases.remove(&challenge.id))
    }

    pub(crate) async fn commit_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        let lease = self.take_lease(challenge);
        if let Some(id) = SessionCredentialPayload::channel_id(&credential.payload)
            && let Some(entry) = self
                .channel_by_id(&id)
                .filter(|entry| is_precompile_escrow(entry.escrow_contract))
        {
            self.store_set(&Self::stored_entry(&entry)).await?;
            self.save_server_spent(&id).await;
        }
        if let Some(id) = SessionCredentialPayload::channel_id(&credential.payload) {
            self.commit_referenced_open(&id);
        }
        drop(lease);
        Ok(())
    }

    pub(crate) async fn rollback_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        let lease = self.take_lease(challenge);
        let result = self.rollback_credential(challenge, credential).await;
        drop(lease);
        result
    }

    pub(crate) async fn invalidate_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        let lease = self.take_lease(challenge);
        let result = self.invalidate_credential(challenge, credential).await;
        drop(lease);
        result
    }

    pub(crate) fn abandon_payment(&self, challenge: &Challenge, _credential: &Credential) {
        drop(self.take_lease(challenge));
    }

    pub(crate) async fn prepaid_credential(
        &self,
        context: PaymentContext,
    ) -> Option<(Challenge, Credential)> {
        if self.prepaid_requests == 0 {
            return None;
        }
        let url = context.url.to_string();
        let state = self
            .shared
            .prepaid
            .read(|prepaid| prepaid.get(&url).cloned())?;
        if !self.is_bound_to(&url) {
            return None;
        }
        if state
            .challenge
            .expires_at()
            .is_some_and(|expires| expires <= protocol::now_seconds())
        {
            self.shared
                .prepaid
                .update(|prepaid| prepaid.remove(&url).map(drop));
            return None;
        }
        if let Some(header) = context.headers.get(protocol::ACCEPT_PAYMENT_HEADER) {
            let preferences = protocol::preferences(header.to_str().ok()?)?;
            if protocol::rank(&[&state.challenge], &preferences).is_empty() {
                return None;
            }
        }
        let _lock = self.acquire_payment_lock(&state.challenge).await.ok()?;
        let key = self.channel_key_of(&state.channel_id)?;
        let mut entry = self.channel(&key)?;
        self.merge_stored_channel(&key, &mut entry).await.ok()?;
        let target = state
            .price
            .checked_mul(u128::from(self.prepaid_requests))
            .and_then(|headroom| state.spent.checked_add(headroom))?;
        let cumulative = target.max(entry.cumulative_amount.checked_add(1)?);
        if cumulative > entry.deposit {
            return None;
        }
        let credential = self
            .voucher_credential_for_challenge(&state.challenge, &state.channel_id, cumulative)
            .await
            .ok()?;
        self.shared
            .last_challenge
            .set(Some(state.challenge.clone()));
        Some((state.challenge, credential))
    }

    pub(crate) fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: &Receipt,
    ) -> Option<u128> {
        if challenge.intent != INTENT_SESSION {
            return None;
        }
        let spent = receipt
            .extensions
            .get("spent")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u128>().ok())?;
        let channel_id = hash_text(&parse_hash(&receipt.reference).ok()?);
        let quoted = challenge
            .request
            .decode::<SessionRequest>()
            .ok()
            .and_then(|request| request.parse_amount().ok());
        let cost = self
            .shared
            .spend
            .update(|ledger| ledger.record(&channel_id, spent, quoted));
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let provider = self.clone();
            let id = channel_id.clone();
            runtime.spawn(async move { provider.save_server_spent(&id).await });
        }
        if self.is_pending_open(&channel_id)
            && let Some(entry) = self.channel_by_id(&channel_id)
        {
            self.report_deposit(context.url.as_str(), &entry, entry.deposit, true);
        }
        self.prepay(context, challenge, channel_id, spent, quoted);
        Some(cost)
    }

    pub(crate) fn stream_event(
        &self,
        client: &reqwest::Client,
        url: &str,
        name: &str,
        data: &str,
    ) -> Option<PaymentEventFuture> {
        let session = self.clone();
        match name {
            "payment-need-voucher" => {
                let parsed = serde_json::from_str::<NeedVoucherEvent>(data).map_err(|error| {
                    Fault::config(format!("invalid payment-need-voucher event: {error}"))
                });
                let bound = match &parsed {
                    Ok(event) if self.holds_channel_hex(&event.channel_id) => self.bind_origin(url),
                    Ok(_) => Err(Fault::config(
                        "payment-need-voucher event for an unknown session channel",
                    )),
                    Err(_) => Ok(()),
                };
                let (client, url) = (client.clone(), url.to_owned());
                Some(Box::pin(async move {
                    let event = parsed?;
                    bound?;
                    session
                        .acknowledge_accepted(
                            &event.channel_id,
                            amount(&event.accepted_cumulative)?,
                        )
                        .await?;
                    session
                        .send_voucher(
                            &client,
                            &url,
                            &event.channel_id,
                            amount(&event.required_cumulative)?,
                        )
                        .await
                }))
            }
            "payment-receipt" => {
                let parsed = serde_json::from_str::<SessionReceipt>(data).map_err(|error| {
                    Fault::config(format!("invalid payment-receipt event: {error}"))
                });
                let known =
                    matches!(&parsed, Ok(receipt) if self.holds_channel_hex(&receipt.channel_id));
                let bound = match known {
                    true => self.bind_origin(url),
                    false => Ok(()),
                };
                let url = url.parse::<Url>().ok();
                Some(Box::pin(async move {
                    let receipt = parsed?;
                    bound?;
                    if !known {
                        return Ok(());
                    }
                    session
                        .acknowledge_accepted(
                            &receipt.channel_id,
                            amount(&receipt.accepted_cumulative)?,
                        )
                        .await?;
                    session
                        .record_stream_receipt(url, &receipt.channel_id, amount(&receipt.spent)?)
                        .await;
                    Ok(())
                }))
            }
            _ => None,
        }
    }

    pub(crate) fn accept_payment_header(&self) -> Option<String> {
        Some(SESSION_ACCEPT_PAYMENT.to_owned())
    }
}

impl PaymentProvider for SessionProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == METHOD_NAME && intent == INTENT_SESSION
    }

    fn select_challenge<'a>(&self, challenges: &[&'a Challenge]) -> Option<&'a Challenge> {
        challenges.first().copied()
    }

    async fn pay_with_context(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
    ) -> Result<Credential, Fault> {
        Self::pay_with_context(self, challenge, context).await
    }

    async fn pay_with_management(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> Result<Credential, Fault> {
        Self::pay_with_management(self, challenge, context, management).await
    }

    async fn commit_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        Self::commit_payment(self, challenge, credential).await
    }

    async fn rollback_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        Self::rollback_payment(self, challenge, credential).await
    }

    async fn invalidate_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        Self::invalidate_payment(self, challenge, credential).await
    }

    fn abandon_payment(&self, challenge: &Challenge, credential: &Credential) {
        Self::abandon_payment(self, challenge, credential);
    }

    async fn prepaid_credential(&self, context: PaymentContext) -> Option<(Challenge, Credential)> {
        Self::prepaid_credential(self, context).await
    }

    fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: &Receipt,
    ) -> Option<u128> {
        Self::record_receipt(self, context, challenge, receipt)
    }

    fn funding_currency(&self, _challenge: &Challenge) -> Option<String> {
        None
    }

    fn stream_event(
        &self,
        client: &reqwest::Client,
        url: &str,
        name: &str,
        data: &str,
    ) -> Option<PaymentEventFuture> {
        Self::stream_event(self, client, url, name, data)
    }

    fn accept_payment_header(&self) -> Option<String> {
        Self::accept_payment_header(self)
    }
}

use std::collections::HashMap;
use std::sync::Arc;

use nanocode_sync::Atom;
use nanocode_tempo::Address;

use super::charge::{self, AutoswapConfig};
use super::error::Fault;
use super::events::Events;
use super::keys::{Accounts, AuthorizationReservation};
use super::protocol::{
    self, Challenge, Credential, INTENT_CHARGE, INTENT_SESSION, METHOD_NAME, Receipt,
};
use super::provider::{ManagementTransport, PaymentContext, PaymentEventFuture, PaymentProvider};
use super::rpc::Rpc;
use super::session::SessionProvider;
use super::session::channel::resolve_chain_id;
use super::signing::{KeychainVersion, TempoSigningMode};
use super::store::ChannelStore;

type SessionKey = (u64, Option<String>);

#[derive(Clone)]
struct AccountsSession {
    store: ChannelStore,
    providers: Arc<Atom<HashMap<SessionKey, SessionProvider>>>,
}

#[derive(Clone, Default)]
struct SessionSettings {
    default_deposit: Option<u128>,
    top_up_amount: Option<u128>,
    fee_token: Option<Address>,
    prepaid_requests: Option<u32>,
    events: Option<Events>,
}

#[derive(Clone)]
pub struct Payer {
    accounts: Accounts,
    autoswap: Option<AutoswapConfig>,
    preferred_currency: Option<Address>,
    expected_chain_id: Option<u64>,
    pending_authorizations: Arc<Atom<HashMap<String, AuthorizationReservation>>>,
    funding: Arc<Atom<HashMap<String, Address>>>,
    session: Option<AccountsSession>,
    settings: SessionSettings,
    rpcs: Arc<Atom<HashMap<String, Rpc>>>,
}

fn http_origin(url: &str) -> Option<String> {
    nanocode_http::Url::parse(url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
}

impl Payer {
    pub fn from_default_store() -> Result<Self, Fault> {
        let accounts = Accounts::from_default_store().map_err(|error| {
            Fault::Config(format!("failed to open Tempo Accounts store: {error}"))
        })?;
        Ok(Self {
            accounts,
            autoswap: None,
            preferred_currency: None,
            expected_chain_id: None,
            pending_authorizations: Arc::default(),
            funding: Arc::default(),
            session: None,
            settings: SessionSettings::default(),
            rpcs: Arc::default(),
        })
    }

    pub const fn with_expected_chain_id(mut self, chain_id: u64) -> Self {
        self.expected_chain_id = Some(chain_id);
        self
    }

    pub const fn with_preferred_currency(mut self, currency: Address) -> Self {
        self.preferred_currency = Some(currency);
        self
    }

    pub const fn with_autoswap(mut self, config: AutoswapConfig) -> Self {
        self.autoswap = Some(config);
        self
    }

    pub fn with_session_store(mut self, store: impl Into<ChannelStore>) -> Self {
        self.session = Some(AccountsSession {
            store: store.into(),
            providers: Arc::default(),
        });
        self
    }

    pub const fn with_session_default_deposit(mut self, amount: u128) -> Self {
        self.settings.default_deposit = Some(amount);
        self
    }

    pub const fn with_session_top_up_amount(mut self, amount: u128) -> Self {
        self.settings.top_up_amount = Some(amount);
        self
    }

    pub const fn with_session_prepaid_requests(mut self, requests: u32) -> Self {
        self.settings.prepaid_requests = Some(requests);
        self
    }

    pub fn with_session_events(mut self, events: Events) -> Self {
        self.settings.events = Some(events);
        self
    }

    pub const fn with_session_fee_token(mut self, token: Address) -> Self {
        self.settings.fee_token = Some(token);
        self
    }

    fn session_provider(&self, store: ChannelStore) -> Result<SessionProvider, Fault> {
        let chain_id = self.expected_chain_id.ok_or_else(|| {
            Fault::config("Tempo Accounts session provider requires an expected chain ID")
        })?;
        let key = self
            .accounts
            .clone()
            .with_chain_id(chain_id)
            .active_access_key()
            .map_err(|error| {
                Fault::Config(format!(
                    "failed to select a Tempo Accounts session key: {error}"
                ))
            })?;
        let account = key.account();
        let access_key = key.address();
        let key_authorization = key.key_authorization().cloned().map(Box::new);
        let rpc_url = charge::settlement_rpc_url(chain_id)?;
        let mut provider = SessionProvider::new(key, rpc_url.as_str())?
            .with_signing_mode(TempoSigningMode::Keychain {
                wallet: account,
                key_authorization,
                version: KeychainVersion::V2,
            })
            .with_authorized_signer(access_key)
            .with_channel_store(store);
        if let Some(config) = &self.autoswap {
            provider = provider.with_autoswap(config.clone());
        }
        Ok(provider)
    }

    pub fn session_for(&self, url: &str) -> Result<SessionProvider, Fault> {
        self.configured_session_provider(Some(url))
    }

    fn check_expected_chain(&self, challenge: &Challenge) -> Result<(), Fault> {
        if let Some(expected) = self.expected_chain_id {
            let got = resolve_chain_id(challenge);
            if got != expected {
                return Err(Fault::ChainIdMismatch { expected, got });
            }
        }
        Ok(())
    }

    fn session_provider_for(
        &self,
        challenge: &Challenge,
        url: Option<&str>,
    ) -> Result<SessionProvider, Fault> {
        self.check_expected_chain(challenge)?;
        self.configured_session_provider(url)
    }

    fn session_provider_holding(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<SessionProvider, Fault> {
        if let (Some(session), Some(chain_id)) = (self.session.as_ref(), self.expected_chain_id) {
            let holder = session.providers.read(|providers| {
                providers
                    .iter()
                    .filter(|((chain, _), _)| *chain == chain_id)
                    .filter_map(|((_, origin), provider)| {
                        provider
                            .payment_hold(challenge, credential)
                            .map(|hold| (hold, origin.clone(), provider.clone()))
                    })
                    .min_by(|(a, a_origin, _), (b, b_origin, _)| {
                        a.cmp(b).then_with(|| a_origin.cmp(b_origin))
                    })
                    .map(|(_, _, provider)| provider)
            });
            if let Some(provider) = holder {
                return Ok(provider);
            }
        }
        self.configured_session_provider(None)
    }

    fn configured_session_provider(&self, url: Option<&str>) -> Result<SessionProvider, Fault> {
        let session = self
            .session
            .as_ref()
            .ok_or_else(|| Fault::Unsupported("tempo.session is not configured".to_owned()))?;
        let origin = url.and_then(http_origin);
        if let Some(provider) = self.expected_chain_id.and_then(|chain_id| {
            session
                .providers
                .read(|providers| providers.get(&(chain_id, origin.clone())).cloned())
        }) {
            return Ok(provider);
        }
        let settings = &self.settings;
        let mut provider = self.session_provider(session.store.clone())?;
        if let Some(amount) = settings.default_deposit {
            provider = provider.with_default_deposit(amount);
        }
        if let Some(amount) = settings.top_up_amount {
            provider = provider.with_top_up_amount(amount);
        }
        if let Some(token) = settings.fee_token {
            provider = provider.with_fee_token(token);
        }
        if let Some(requests) = settings.prepaid_requests {
            provider = provider.with_prepaid_requests(requests);
        }
        if let Some(events) = &settings.events {
            provider = provider.with_events(events.clone());
        }
        if let Some(origin) = &origin {
            provider.bind_origin(origin)?;
        }
        if let Some(chain_id) = self.expected_chain_id {
            let key = (chain_id, origin);
            return Ok(session.providers.update(|providers| {
                providers
                    .entry(key.clone())
                    .or_insert_with(|| provider.clone())
                    .clone()
            }));
        }
        Ok(provider)
    }

    pub async fn preconnect(
        &self,
        client: &nanocode_http::Client,
        url: &str,
        body: String,
    ) -> Result<(), Fault> {
        let challenges = self.probe(client, url, body.clone(), None).await?;
        let candidates: Vec<&Challenge> = challenges.iter().collect();
        let Some(challenge) = self
            .select_challenge(&candidates)
            .filter(|challenge| challenge.intent == INTENT_SESSION)
        else {
            return Ok(());
        };
        let session = self.session_provider_for(challenge, Some(url))?;
        session.bind_origin(url)?;
        if !session.restore(challenge).await? {
            return Ok(());
        }
        let Some(channel_id) = session.restored_channel_id(challenge) else {
            return Ok(());
        };
        let described = self
            .probe(client, url, body, Some((&channel_id, &session)))
            .await?;
        for challenge in &described {
            if self.check_expected_chain(challenge).is_ok() && session.prime_prepaid(url, challenge)
            {
                break;
            }
        }
        Ok(())
    }

    async fn probe(
        &self,
        client: &nanocode_http::Client,
        url: &str,
        body: String,
        hint: Option<(&str, &SessionProvider)>,
    ) -> Result<Vec<Challenge>, Fault> {
        let post = |client: &nanocode_http::Client| {
            client
                .post(url)
                .header(nanocode_http::header::CONTENT_TYPE, "application/json")
                .body(body)
        };
        let response = match hint {
            Some((channel_id, session)) => {
                session
                    .send_hint(
                        |client| {
                            post(client)
                                .header("payment-session", channel_id)
                                .header(protocol::ACCEPT_PAYMENT_HEADER, "tempo/session")
                        },
                        "preconnect probe failed",
                    )
                    .await?
            }
            None => post(client)
                .send()
                .await
                .map(nanocode_http::decode_flate_response)
                .map_err(|error| Fault::Http(format!("preconnect probe failed: {error}")))?,
        };
        Ok(protocol::response_challenges(response.headers())
            .into_iter()
            .filter(|challenge| self.supports(&challenge.method, &challenge.intent))
            .collect())
    }

    fn settlement_rpc(&self, chain_id: u64) -> Result<Rpc, Fault> {
        let url = charge::settlement_rpc_url(chain_id)?;
        let key = url.to_string();
        if let Some(rpc) = self.rpcs.read(|rpcs| rpcs.get(&key).cloned()) {
            return Ok(rpc);
        }
        let rpc = Rpc::new(None, url);
        Ok(self.rpcs.update(|rpcs| {
            rpcs.entry(key.clone())
                .or_insert_with(|| rpc.clone())
                .clone()
        }))
    }

    fn forget_funding(&self, challenge: &Challenge) {
        self.funding.update(|funding| {
            funding.remove(&challenge.id);
        });
    }

    fn remember_authorization(
        &self,
        challenge_id: &str,
        reservation: AuthorizationReservation,
    ) -> Result<(), Fault> {
        let accepted =
            self.pending_authorizations
                .update(|pending| match pending.get(challenge_id) {
                    None => {
                        pending.insert(challenge_id.to_owned(), reservation);
                        true
                    }
                    Some(existing) => *existing == reservation,
                });
        match accepted {
            true => Ok(()),
            false => Err(Fault::Config(format!(
                "challenge {challenge_id} already has a different Tempo authorization in flight"
            ))),
        }
    }

    fn take_authorization(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<Option<AuthorizationReservation>, Fault> {
        if credential.challenge.id != challenge.id {
            return Err(Fault::config(
                "payment lifecycle credential does not match its challenge",
            ));
        }
        Ok(self
            .pending_authorizations
            .update(|pending| pending.remove(&challenge.id)))
    }

    async fn pay_charge(&self, challenge: &Challenge) -> Result<Credential, Fault> {
        let request = charge::prepare_charge_request(challenge, self.expected_chain_id, None)?;
        let rpc = self.settlement_rpc(request.chain_id())?;
        let funding = self.funding.clone();
        let id = challenge.id.clone();
        let charged = charge::pay(
            &self.accounts,
            &rpc,
            challenge,
            self.expected_chain_id,
            None,
            self.autoswap.as_ref(),
            move |currency| {
                funding.update(|funding| {
                    funding.insert(id.clone(), currency);
                });
            },
        )
        .await?;
        if let Some(reservation) = charged.reservation
            && let Err(error) = self.remember_authorization(&challenge.id, reservation)
        {
            self.accounts.release_authorization(reservation);
            return Err(error);
        }
        Ok(charged.credential)
    }
}

fn funded(challenge: &Challenge, currency: Address) -> bool {
    challenge.method == METHOD_NAME && charge::funded_by(challenge, currency)
}

impl PaymentProvider for Payer {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == METHOD_NAME
            && (intent == INTENT_CHARGE || (intent == INTENT_SESSION && self.session.is_some()))
    }

    fn select_challenge<'a>(&self, challenges: &[&'a Challenge]) -> Option<&'a Challenge> {
        let first = challenges.first().copied()?;
        let Some(currency) = self.preferred_currency else {
            return Some(first);
        };
        challenges
            .iter()
            .copied()
            .find(|challenge| funded(challenge, currency))
            .or(Some(first))
    }

    async fn pay_with_context(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
    ) -> Result<Credential, Fault> {
        if challenge.intent == INTENT_SESSION {
            return self
                .session_provider_for(challenge, Some(context.url.as_str()))?
                .pay_with_context(challenge, context)
                .await;
        }
        self.pay_charge(challenge).await
    }

    async fn pay_with_management(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> Result<Credential, Fault> {
        if challenge.intent == INTENT_SESSION {
            return self
                .session_provider_for(challenge, Some(context.url.as_str()))?
                .pay_with_management(challenge, context, management)
                .await;
        }
        self.pay_charge(challenge).await
    }

    async fn commit_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        if challenge.intent == INTENT_SESSION {
            return self
                .session_provider_holding(challenge, credential)?
                .commit_payment(challenge, credential)
                .await;
        }
        self.forget_funding(challenge);
        self.take_authorization(challenge, credential)?;
        Ok(())
    }

    async fn rollback_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        if challenge.intent == INTENT_SESSION {
            return self
                .session_provider_holding(challenge, credential)?
                .rollback_payment(challenge, credential)
                .await;
        }
        self.forget_funding(challenge);
        if let Some(reservation) = self.take_authorization(challenge, credential)? {
            self.accounts.release_authorization(reservation);
        }
        Ok(())
    }

    async fn invalidate_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        if challenge.intent == INTENT_SESSION {
            return self
                .session_provider_holding(challenge, credential)?
                .invalidate_payment(challenge, credential)
                .await;
        }
        self.rollback_payment(challenge, credential).await
    }

    fn abandon_payment(&self, challenge: &Challenge, credential: &Credential) {
        if challenge.intent == INTENT_SESSION
            && let Ok(provider) = self.session_provider_holding(challenge, credential)
        {
            provider.abandon_payment(challenge, credential);
        }
    }

    async fn prepaid_credential(&self, context: PaymentContext) -> Option<(Challenge, Credential)> {
        self.session.as_ref()?;
        let provider = self
            .configured_session_provider(Some(context.url.as_str()))
            .ok()?;
        let (challenge, credential) = provider.prepaid_credential(context).await?;
        if self.check_expected_chain(&challenge).is_err() {
            provider.abandon_payment(&challenge, &credential);
            return None;
        }
        Some((challenge, credential))
    }

    fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: &Receipt,
    ) -> Option<u128> {
        if challenge.intent != INTENT_SESSION {
            return None;
        }
        self.configured_session_provider(Some(context.url.as_str()))
            .ok()?
            .record_receipt(context, challenge, receipt)
    }

    fn funding_currency(&self, challenge: &Challenge) -> Option<String> {
        self.funding
            .update(|funding| funding.remove(&challenge.id))
            .map(|token| token.to_string())
    }

    fn stream_event(
        &self,
        client: &nanocode_http::Client,
        url: &str,
        name: &str,
        data: &str,
    ) -> Option<PaymentEventFuture> {
        self.session.as_ref()?;
        self.configured_session_provider(Some(url))
            .ok()?
            .stream_event(client, url, name, data)
    }

    fn accept_payment_header(&self) -> Option<String> {
        Some(match self.session {
            Some(_) => "tempo/session, tempo/charge;q=0.5".to_owned(),
            None => "tempo/charge".to_owned(),
        })
    }
}

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::Stream;
use nanocode::{
    ClaudeModel, Model, ModelSelection,
    http::{self as layer, BoxFuture, Event, EventHook, Layer, Next},
    tools::mcp::{McpPaymentManagement, McpPaymentProvider, McpPendingPayment, McpServer},
};
use nanocode_sync::{Atom, Signal, SingleFlight};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

mod accounts;
mod charge;
mod error;
mod events;
mod flow;
mod identity;
mod keys;
mod mcp;
mod payer;
mod protocol;
mod provider;
mod proxy;
mod rpc;
mod session;
mod signing;
mod store;
mod tx;
mod x509;

use self::{
    charge::AutoswapConfig,
    error::Fault,
    events::{Deposit, Events},
    flow::{DEFAULT_MAX_PAYMENT_RETRIES, Exchange, FlowError, Policy},
    identity::{Asker, Identity, Proved},
    mcp::McpPayment,
    payer::Payer,
    protocol::{Challenge, Credential, INTENT_CHARGE, INTENT_SESSION, METHOD_NAME, Receipt},
    provider::{
        ManagementFuture, ManagementTransport, PaymentContext, PaymentEventFuture, PaymentProvider,
        PendingPayment,
    },
    proxy::ForwardProxy,
    session::SessionProvider,
    store::{FileChannelStore, FileChannelStoreOptions, default_channel_database_path},
    tx::{CHAIN_ID, DEFAULT_RPC_URL, OUSD, PATH_USD, USDC, USDT0, machine_token},
};
use nanocode_tempo::{Address, U256};

use crate::{
    cli::GatewayArgs,
    host::{ModelClients, Panel, Result, Route},
};

const MERCATOR: &str = "https://mercator.sh/mcp";
const GATEWAY: &str = "https://gateway.tempo-dev.workers.dev/mcp";
const TEMPO_GATEWAY: &str = "https://gateway.tempo-dev.workers.dev";
const BOUNDLESS: &str = "https://api.inference.boundless.network/mpp/v1";
const TEMPO_SERVICES: &str = "*.mpp.tempo.xyz";
const OWN_PROXY_SETTINGS: [&str; 10] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
];
const HELD_TOKENS: [&str; 3] = [
    "0x20C000000000000000000000b9537d11c60E8b50",
    "0x20c00000000000000000000014f22ca97301eb73",
    "0x20c0000000000000000000000000000000000000",
];
const MACH: u128 = 1_000_000;
const PREPAID_REQUESTS: u32 = 2;
const SESSION_SLOTS: usize = 64;
const CLOSE_ATTEMPTS: u32 = 12;
const CLOSE_RETRY: Duration = Duration::from_secs(5);
const WATERFALL: Duration = Duration::from_secs(30);
const WATERFALL_COLUMNS: usize = 30;
const LIVE: Duration = Duration::from_secs(2);
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Clone)]
struct Mpp {
    wallet: Wallet,
    probe: (String, Option<&'static str>),
    warm: Arc<Warm>,
    identity: Option<Arc<Identity>>,
}

#[derive(Default)]
struct Warm {
    done: Signal,
    flight: SingleFlight<()>,
}

impl Mpp {
    fn new(wallet: Wallet, probe: (String, Option<&'static str>)) -> Self {
        Self {
            wallet,
            probe,
            warm: Arc::default(),
            identity: None,
        }
    }

    fn identified(self, identity: Arc<Identity>) -> Self {
        Self {
            identity: Some(identity),
            ..self
        }
    }

    async fn preconnect(&self, request: &[u8]) {
        if self.warm.done.is_fired() {
            return;
        }
        self.warm
            .flight
            .run(|| async {
                let (Ok(wallet), Ok(client)) =
                    (self.wallet.get().await, self.wallet.payments.client())
                else {
                    return;
                };
                let (url, model) = &self.probe;
                let model = match model {
                    Some(model) => Value::from(*model),
                    None => match serde_json::from_slice::<Value>(request) {
                        Ok(request) if request["model"].is_string() => request["model"].clone(),
                        _ => return,
                    },
                };
                let body = serde_json::json!({ "model": model }).to_string();
                let _ = wallet.preconnect(&client, url, body).await;
                self.warm.done.fire();
            })
            .await;
    }
}

impl Layer for Mpp {
    fn send<'a>(
        &'a self,
        request: layer::Request,
        next: Next<'a>,
    ) -> BoxFuture<'a, std::result::Result<layer::Response, layer::Error>> {
        Box::pin(async move {
            self.preconnect(&request.body).await;
            let url = nanocode_http::Url::parse(&request.url)
                .map_err(|error| layer::Error::new(error.to_string()).unsent(true))?;
            let mut paid = nanocode_http::Request::new(request.method, url);
            *paid.headers_mut() = request.headers;
            *paid.body_mut() = Some(request.body.into());
            let mut exchange = Proved {
                exchange: NextExchange {
                    next,
                    client: self
                        .wallet
                        .payments
                        .client()
                        .map_err(|error| layer::Error::new(error).unsent(true))?,
                    body: None,
                },
                identity: self.identity.as_deref(),
            };
            let response = flow::send_with_exchange(
                &mut exchange,
                paid,
                &self.wallet,
                &Policy::Always,
                &self.wallet.payments.events,
                DEFAULT_MAX_PAYMENT_RETRIES,
            )
            .await
            .map_err(|error| match error {
                FlowError::Send(error) => error,
                FlowError::Payment(error) => layer::Error::new(error.to_string()),
            })?;
            let body = exchange
                .exchange
                .body
                .ok_or_else(|| layer::Error::new("the payment flow returned no response"))?;
            Ok(
                layer::Response::new(response.status(), response.headers().clone(), body)
                    .with_events(StreamPayments {
                        mpp: self.clone(),
                        url: request.url,
                    }),
            )
        })
    }
}

struct StreamPayments {
    mpp: Mpp,
    url: String,
}

impl EventHook for StreamPayments {
    fn event(
        &mut self,
        event: Event<'_>,
    ) -> Option<BoxFuture<'static, std::result::Result<(), layer::Error>>> {
        let client = self.mpp.wallet.payments.client().ok()?;
        let work = self
            .mpp
            .wallet
            .stream_event(&client, &self.url, event.name?, event.data)?;
        Some(Box::pin(async move {
            work.await
                .map_err(|error| layer::Error::new(format!("MPP session stream: {error}")))
        }))
    }
}

struct NextExchange<'a> {
    next: Next<'a>,
    client: nanocode_http::Client,
    body: Option<layer::Body>,
}

impl Exchange for NextExchange<'_> {
    type Error = layer::Error;

    fn http_client(&self) -> Option<nanocode_http::Client> {
        Some(self.client.clone())
    }

    async fn send(
        &mut self,
        request: nanocode_http::Request,
    ) -> std::result::Result<nanocode_http::Response, layer::Error> {
        let sent = layer::Request {
            method: request.method().clone(),
            url: request.url().to_string(),
            headers: request.headers().clone(),
            body: request
                .body()
                .and_then(nanocode_http::Body::as_bytes)
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
        };
        let (status, headers, body, _) = self.next.send(sent).await?.into_parts();
        self.body = Some(body);
        Ok(nanocode_http::Response::from_stream(
            status,
            headers,
            request.url().clone(),
            futures_util::stream::empty(),
        ))
    }
}

type Listeners = Atom<Vec<UnboundedSender<String>>>;

#[derive(Clone)]
struct Payments {
    client: Arc<Atom<Option<nanocode_http::Client>>>,
    ledger: Arc<Atom<Ledger>>,
    listeners: Arc<Listeners>,
    events: Events,
    failure: Arc<Atom<Option<String>>>,
    session: Option<String>,
    wallets: Arc<Atom<HashMap<String, Wallet>>>,
    slots: Arc<Atom<HashMap<String, String>>>,
    closing: Arc<Atom<Vec<(SessionProvider, String)>>>,
    short: Arc<Atom<bool>>,
}

#[derive(Clone, Default)]
struct Ledger {
    machine: Address,
    models: Vec<String>,
    services: BTreeMap<String, Service>,
    sessions: HashMap<String, SessionProvider>,
}

#[derive(Clone)]
struct Service {
    host: String,
    model: bool,
    symbol: String,
    spent: u128,
    requests: u64,
    last: u128,
    charges: VecDeque<(Instant, i128)>,
}

impl Service {
    fn charged(&mut self, change: i128) {
        let now = Instant::now();
        self.charges.push_back((now, change));
        while self
            .charges
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > WATERFALL)
        {
            self.charges.pop_front();
        }
    }

    fn buckets(&self, now: Instant) -> [i128; WATERFALL_COLUMNS] {
        let mut buckets = [0; WATERFALL_COLUMNS];
        let width = WATERFALL / WATERFALL_COLUMNS as u32;
        for (at, change) in &self.charges {
            let age = now.duration_since(*at);
            if age < WATERFALL {
                let column = (age.as_nanos() / width.as_nanos()) as usize;
                buckets[WATERFALL_COLUMNS - 1 - column] += change;
            }
        }
        buckets
    }

    fn live(&self, now: Instant) -> bool {
        self.charges
            .back()
            .is_some_and(|(at, _)| now.duration_since(*at) < LIVE)
    }
}

struct Paid<'a> {
    url: &'a nanocode_http::Url,
    challenge: &'a Challenge,
    amount: u128,
    funding: Option<&'a str>,
    reference: &'a str,
}

impl Payments {
    fn new(machine: Address, models: Vec<String>, session: Option<String>) -> Self {
        let events = Events::default();
        let ledger = Arc::new(Atom::new(Ledger {
            machine,
            models,
            ..Ledger::default()
        }));
        let listeners = Arc::<Listeners>::default();
        let short = Arc::<Atom<bool>>::default();
        let (paid, updated, funded) = (ledger.clone(), ledger.clone(), short.clone());
        let (told, deposits) = (listeners.clone(), listeners.clone());
        events.on_payment_response(move |context| {
            funded.set(false);
            if let Some(amount) = context.amount {
                let receipt = context.receipt.as_ref();
                let funding = context
                    .funding_currency
                    .clone()
                    .or_else(|| Some(receipt?.extension("fundingCurrency")?.to_owned()));
                let charge = Paid {
                    url: &context.url,
                    challenge: &context.challenge,
                    amount,
                    funding: funding.as_deref(),
                    reference: receipt.map_or("", |receipt| receipt.reference.as_str()),
                };
                notify(&told, paid.update(|ledger| ledger.record(&charge)));
            }
        });
        events.on_deposit(move |context| {
            notify(&deposits, Some(deposit_notice(context, machine)));
        });
        events.on_payment_update(move |context| {
            updated.update(|ledger| ledger.update(&context.url, context.change));
        });
        Self {
            client: Arc::default(),
            ledger,
            listeners,
            events,
            failure: Arc::default(),
            session,
            wallets: Arc::default(),
            slots: Arc::default(),
            closing: Arc::default(),
            short,
        }
    }

    fn client(&self) -> std::result::Result<nanocode_http::Client, String> {
        if let Some(client) = self.client.read(Clone::clone) {
            return Ok(client);
        }
        let client = nanocode_http::Client::builder()
            .redirect(nanocode_http::redirect::Policy::none())
            .default_headers(nanocode_http::accept_encoding(
                nanocode_http::header::HeaderMap::new(),
            ))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(self
            .client
            .update(|slot| slot.get_or_insert_with(|| client.clone()).clone()))
    }

    fn wallet(&self, url: &nanocode_http::Url, terms: &Terms) -> Wallet {
        let origin = url.origin().ascii_serialization();
        self.wallets.update(|wallets| {
            wallets
                .entry(origin.clone())
                .or_insert_with(|| Wallet {
                    url: url.clone(),
                    terms: terms.clone(),
                    payments: self.clone(),
                    loaded: Arc::default(),
                    loading: Arc::default(),
                })
                .clone()
        })
    }

    fn store(&self, origin: String, service: &str) -> Result<FileChannelStore> {
        let namespace = match self.slots.read(|slots| slots.get(&origin).cloned()) {
            Some(namespace) => namespace,
            None => {
                let namespace = match session_slot(&origin, self.session.as_deref())?.as_str() {
                    "0" => origin.clone(),
                    slot => format!("{origin}#{slot}"),
                };
                self.slots.update(|slots| {
                    slots
                        .entry(origin.clone())
                        .or_insert_with(|| namespace.clone())
                        .clone()
                })
            }
        };
        Ok(FileChannelStore::open(FileChannelStoreOptions {
            namespace,
            path: None,
            request_url: Some(service.to_owned()),
        })?)
    }

    fn sessions(
        &self,
        wallet: &Payer,
        service: &nanocode_http::Url,
        terms: &Terms,
    ) -> Result<Payer> {
        let origin = service.origin().ascii_serialization();
        let origin = match &terms.variant {
            Some(variant) => format!("{origin}~{variant}"),
            None => origin,
        };
        let store = self.store(origin, service.as_str())?;
        let wallet = wallet.clone().with_session_store(store);
        let host = service.host_str().unwrap_or_default().to_owned();
        let session = wallet.session_for(service.as_str())?;
        self.ledger.update(|ledger| {
            ledger.sessions.insert(host.clone(), session.clone());
        });
        if terms.closes {
            self.closing.update(|closing| {
                closing.push((session.clone(), format!("{service}/responses")));
            });
        }
        Ok(wallet)
    }

    fn pay_tools(
        &self,
        terms: Terms,
        bypass: Vec<String>,
        policy: impl Future<Output = Policy> + Send + 'static,
        identity: Arc<Identity>,
    ) -> Result<(Vec<(&'static str, String)>, PathBuf)> {
        nanocode::codex::transport::install_default_rustls_crypto_provider();
        let payments = self.clone();
        let proxy = ForwardProxy::bind(move |url| {
            Ok::<_, std::convert::Infallible>(payments.wallet(url, &terms))
        })?
        .bypass(bypass)
        .events(self.events.clone())
        .identity(identity);
        let paying = (proxy.environment(), proxy.certificates().to_path_buf());
        std::thread::Builder::new()
            .name("nanocode-paying-proxy".to_owned())
            .spawn(move || {
                if let Err(error) = proxy.spawn(policy) {
                    tracing::warn!(target: "nanocode", error = %error, "shell commands pay no Tempo services: the paying proxy did not start");
                }
            })?;
        Ok(paying)
    }

    fn mcp(&self, url: &str, terms: &Terms) -> Result<McpServer> {
        let url: nanocode_http::Url = url.parse()?;
        Ok(
            McpServer::http(url.as_str()).payment_provider(Arc::new(McpPayments {
                wallet: self.wallet(&url, terms),
                url,
            })),
        )
    }

    fn notices(&self) -> impl Stream<Item = String> + use<> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        self.listeners
            .update(|listeners| listeners.push(sender.clone()));
        futures_util::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|notice| (notice, receiver))
        })
    }

    fn status(&self) -> Option<String> {
        if let Some(error) = self.failure.read(Clone::clone) {
            return Some(format!("mpp.dev directory unavailable: {error}"));
        }
        self.ledger.read(|ledger| {
            (!ledger.services.is_empty()).then(|| format!("spent {}", ledger.totals()))
        })
    }

    fn panel(&self) -> Option<Panel> {
        self.ledger.read(Ledger::panel)
    }
}

impl Ledger {
    fn panel(&self) -> Option<Panel> {
        let ledger = self;
        let mut services: Vec<_> = ledger.services.iter().collect();
        if services.is_empty() {
            return None;
        }
        services.sort_by_key(|(_, service)| std::cmp::Reverse(service.spent));
        let now = Instant::now();
        let buckets: Vec<_> = services
            .iter()
            .map(|(_, service)| service.buckets(now))
            .collect();
        let peak = buckets.iter().flatten().copied().max().unwrap_or(0).max(1);
        let window: i128 = buckets.iter().flatten().filter(|change| **change > 0).sum();
        let calls: u64 = services.iter().map(|(_, service)| service.requests).sum();
        let rows = services
            .iter()
            .zip(&buckets)
            .map(|((host, service), buckets)| {
                let left = ledger
                    .sessions
                    .get(&service.host)
                    .map(|session| session.spend().remaining)
                    .filter(|left| *left > 0)
                    .map_or_else(String::new, |left| {
                        format!("{} {} left", mach(left), service.symbol)
                    });
                vec![
                    format!("{} {host}", if service.live(now) { '●' } else { ' ' }),
                    if service.model { "model" } else { "tool" }.to_owned(),
                    buckets.iter().map(|change| bar(*change, peak)).collect(),
                    priced(service.spent, &service.symbol),
                    format!("{}×", service.requests),
                    format!("last {}", priced(service.last, &service.symbol)),
                    left,
                ]
            })
            .collect();
        let rate = u128::try_from(window).unwrap_or(0) * 60 / WATERFALL.as_secs() as u128;
        let rate = match services.first() {
            _ if services.iter().all(|(_, service)| dollars(&service.symbol)) => {
                format!(" · ${}/min", mach(rate))
            }
            Some((_, first))
                if services
                    .iter()
                    .all(|(_, service)| service.symbol == first.symbol) =>
            {
                format!(" · {} {}/min", mach(rate), first.symbol)
            }
            _ => String::new(),
        };
        Some(Panel {
            title: format!(
                "⚡ {} streamed · {calls} paid calls · {} services{rate}",
                ledger.totals(),
                services.len(),
            ),
            rows,
            accent: vec![2],
            refresh: buckets.iter().flatten().any(|change| *change != 0),
        })
    }

    fn record(&mut self, paid: &Paid<'_>) -> Option<String> {
        let request = paid.challenge.request.decode::<Value>().ok()?;
        let currency = match paid.funding {
            Some(funding) => funding.to_owned(),
            None => request.get("currency")?.as_str()?.to_owned(),
        };
        let symbol = symbol(&currency, self.machine);
        let host = paid.url.host_str()?.to_owned();
        let model = self
            .models
            .iter()
            .any(|prefix| paid.url.as_str().starts_with(prefix.as_str()));
        let key = match paid.challenge.description.as_deref() {
            Some(description) if !model && !description.trim().is_empty() => {
                format!("{host} · {description}")
            }
            _ => host.clone(),
        };
        let service = self.services.entry(key.clone()).or_insert(Service {
            host,
            model,
            symbol: symbol.clone(),
            spent: 0,
            requests: 0,
            last: 0,
            charges: VecDeque::new(),
        });
        service.spent += paid.amount;
        service.requests += 1;
        service.last = paid.amount;
        service.charged(i128::try_from(paid.amount).unwrap_or(i128::MAX));
        if model {
            return None;
        }
        let (intent, label) = match paid.challenge.intent.as_str() {
            "session" => ("session", "channel"),
            intent => (intent, "tx"),
        };
        let token = match dollars(&symbol) {
            true => format!(" in {symbol}"),
            false => String::new(),
        };
        Some(format!(
            "paid {}{token} to {key} ({intent}, {label} {})",
            priced(paid.amount, &symbol),
            paid.reference
        ))
    }

    fn update(&mut self, url: &nanocode_http::Url, change: i128) {
        let Some(service) = url.host_str().and_then(|host| self.services.get_mut(host)) else {
            return;
        };
        service.spent = service.spent.saturating_add_signed(change);
        service.last = service.last.saturating_add_signed(change);
        service.charged(change);
    }

    fn totals(&self) -> String {
        let mut usd = None::<u128>;
        let mut tokens = BTreeMap::<&str, u128>::new();
        for service in self.services.values() {
            match dollars(&service.symbol) {
                true => *usd.get_or_insert(0) += service.spent,
                false => *tokens.entry(&service.symbol).or_default() += service.spent,
            }
        }
        usd.map(|total| format!("${}", mach(total)))
            .into_iter()
            .chain(
                tokens
                    .into_iter()
                    .map(|(symbol, total)| priced(total, symbol)),
            )
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

struct McpPayments {
    wallet: Wallet,
    url: nanocode_http::Url,
}

#[derive(Clone, Copy)]
enum Gas {
    Challenge,
    Token(Address),
    Held,
}

#[derive(Clone)]
struct Terms {
    currency: Address,
    gas: Gas,
    deposit: Option<u128>,
    variant: Option<String>,
    closes: bool,
}

impl Terms {
    fn mainnet(deposit: Option<u128>) -> Result<Self> {
        Ok(Self {
            currency: machine_token(CHAIN_ID).ok_or("MACH is not deployed on Tempo mainnet")?,
            gas: Gas::Challenge,
            deposit,
            variant: None,
            closes: false,
        })
    }

    fn tempo(args: &GatewayArgs) -> Result<Self> {
        let machine = Self::mainnet(None)?.currency;
        let wanted = args
            .tempo_currency
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let currency: Address = match wanted.as_str() {
            "" | "mach" => machine,
            "usdc.e" | "usdc" => USDC,
            "ousd" => OUSD,
            address if address.starts_with("0x") => address.parse()?,
            other => {
                return Err(format!(
                    "--tempo-currency must be mach, ousd, usdc.e or a token address, not {other}"
                )
                .into());
            }
        };
        Ok(Self {
            currency,
            gas: match args.tempo_fee_token.as_deref() {
                Some(token) => Gas::Token(gas_token(token)?),
                None => Gas::Challenge,
            },
            deposit: Some(args.tempo_deposit.unwrap_or(2 * MACH)),
            variant: (currency != machine).then_some(wanted),
            closes: args.tempo_close_session,
        })
    }
}

#[derive(Clone)]
struct Wallet {
    url: nanocode_http::Url,
    terms: Terms,
    payments: Payments,
    loaded: Arc<Atom<Option<Payer>>>,
    loading: Arc<SingleFlight<std::result::Result<Payer, String>>>,
}

impl Wallet {
    fn ready(&self) -> Option<Payer> {
        self.loaded.read(Clone::clone)
    }

    async fn get(&self) -> std::result::Result<Payer, String> {
        if let Some(wallet) = self.ready() {
            return Ok(wallet);
        }
        self.loading
            .run(|| async {
                if let Some(wallet) = self.ready() {
                    return Ok(wallet);
                }
                let fee = match self.terms.gas {
                    Gas::Challenge => None,
                    Gas::Token(token) => Some(token),
                    Gas::Held => Some(self.held().await?),
                };
                let opening = self.clone();
                let wallet = tokio::task::spawn_blocking(move || opening.open(fee))
                    .await
                    .map_err(|error| error.to_string())??;
                self.loaded.set(Some(wallet.clone()));
                Ok(wallet)
            })
            .await
    }

    async fn load(&self) -> std::result::Result<Payer, Fault> {
        self.get().await.map_err(Fault::Config)
    }

    async fn held(&self) -> std::result::Result<Address, String> {
        let host = self.url.host_str().unwrap_or_default().to_owned();
        let account = tokio::task::spawn_blocking(accounts::default_active_account)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| {
                format!("{error}; sign in to a Tempo wallet with /login tempo to pay {host}")
            })?;
        let client = self.payments.client()?;
        let mut held = (U256::ZERO, None);
        for token in HELD_TOKENS {
            let balance = balance(&client, token, account).await?;
            if balance > held.0 {
                held = (balance, Some(token));
            }
        }
        held.1
            .ok_or_else(|| format!("{host} sponsors no gas, and the Tempo wallet holds no USDC.e, USDT0 or pathUSD to pay it in; --tempo-fee-token names another token"))?
            .parse()
            .map_err(|error| format!("{error}"))
    }

    fn open(&self, fee: Option<Address>) -> std::result::Result<Payer, String> {
        let wallet = Payer::from_default_store()
            .map_err(|error| {
                let error = match error {
                    Fault::Config(message) => message,
                    error => error.to_string(),
                };
                format!(
                    "{error}; sign in to a Tempo wallet with /login tempo to pay {}",
                    self.url.host_str().unwrap_or_default()
                )
            })?
            .with_expected_chain_id(CHAIN_ID)
            .with_preferred_currency(self.terms.currency)
            .with_autoswap(AutoswapConfig::default())
            .with_session_prepaid_requests(PREPAID_REQUESTS)
            .with_session_events(self.payments.events.clone());
        let wallet = match self.terms.deposit {
            Some(deposit) => wallet
                .with_session_default_deposit(deposit)
                .with_session_top_up_amount(deposit),
            None => wallet,
        };
        let wallet = match fee {
            Some(token) => wallet.with_session_fee_token(token),
            None => wallet,
        };
        self.payments
            .sessions(&wallet, &self.url, &self.terms)
            .map_err(|error| error.to_string())
    }

    fn noted<T>(&self, paid: std::result::Result<T, Fault>) -> std::result::Result<T, Fault> {
        if let Err(error) = &paid
            && error.is_insufficient_balance()
            && !*self.payments.short.set(true)
        {
            notify(
                &self.payments.listeners,
                Some(format!(
                    "the Tempo wallet holds too little to pay {} ({error}); /fund buys MACH by card, and nanocode pays with it once it arrives",
                    self.url.host_str().unwrap_or_default()
                )),
            );
        }
        paid
    }
}

impl PaymentProvider for Wallet {
    fn supports(&self, method: &str, intent: &str) -> bool {
        match self.ready() {
            Some(wallet) => wallet.supports(method, intent),
            None => method == METHOD_NAME && [INTENT_CHARGE, INTENT_SESSION].contains(&intent),
        }
    }

    fn select_challenge<'a>(&self, challenges: &[&'a Challenge]) -> Option<&'a Challenge> {
        match self.ready() {
            Some(wallet) => wallet.select_challenge(challenges),
            None => challenges.first().copied(),
        }
    }

    async fn pay_with_context(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
    ) -> std::result::Result<Credential, Fault> {
        let wallet = self.load().await?;
        self.noted(wallet.pay_with_context(challenge, context).await)
    }

    async fn pay_with_management(
        &self,
        challenge: &Challenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> std::result::Result<Credential, Fault> {
        let wallet = self.load().await?;
        self.noted(
            wallet
                .pay_with_management(challenge, context, management)
                .await,
        )
    }

    async fn commit_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> std::result::Result<(), Fault> {
        match self.ready() {
            Some(wallet) => wallet.commit_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    async fn rollback_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> std::result::Result<(), Fault> {
        match self.ready() {
            Some(wallet) => wallet.rollback_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    async fn invalidate_payment(
        &self,
        challenge: &Challenge,
        credential: &Credential,
    ) -> std::result::Result<(), Fault> {
        match self.ready() {
            Some(wallet) => wallet.invalidate_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    fn abandon_payment(&self, challenge: &Challenge, credential: &Credential) {
        if let Some(wallet) = self.ready() {
            wallet.abandon_payment(challenge, credential);
        }
    }

    async fn prepaid_credential(&self, context: PaymentContext) -> Option<(Challenge, Credential)> {
        self.get().await.ok()?.prepaid_credential(context).await
    }

    fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &Challenge,
        receipt: &Receipt,
    ) -> Option<u128> {
        self.ready()?.record_receipt(context, challenge, receipt)
    }

    fn funding_currency(&self, challenge: &Challenge) -> Option<String> {
        self.ready()?.funding_currency(challenge)
    }

    fn stream_event(
        &self,
        client: &nanocode_http::Client,
        url: &str,
        name: &str,
        data: &str,
    ) -> Option<PaymentEventFuture> {
        self.ready()?.stream_event(client, url, name, data)
    }

    fn accept_payment_header(&self) -> Option<String> {
        match self.ready() {
            Some(wallet) => wallet.accept_payment_header(),
            None => Some(format!(
                "{METHOD_NAME}/{INTENT_SESSION}, {METHOD_NAME}/{INTENT_CHARGE};q=0.5"
            )),
        }
    }
}

impl McpPaymentProvider for McpPayments {
    fn prepare<'life0, 'life1, 'async_trait>(
        &'life0 self,
        payment_required: &'life1 Value,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = std::result::Result<Option<Box<dyn McpPendingPayment>>, String>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: std::result::Result<Option<Box<dyn McpPendingPayment>>, String> =
                { self.pay(payment_required, None).await };
            output
        })
    }

    fn prepare_with_management<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        payment_required: &'life1 Value,
        management: &'life2 dyn McpPaymentManagement,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = std::result::Result<Option<Box<dyn McpPendingPayment>>, String>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: std::result::Result<Option<Box<dyn McpPendingPayment>>, String> = {
                self.pay(payment_required, Some(&ToolCall(management)))
                    .await
            };
            output
        })
    }
}

impl McpPayments {
    async fn pay(
        &self,
        payment_required: &Value,
        management: Option<&ToolCall<'_>>,
    ) -> std::result::Result<Option<Box<dyn McpPendingPayment>>, String> {
        let Some(challenges) = mcp::extract_challenges_from_data(payment_required) else {
            return Ok(None);
        };
        self.wallet.get().await?;
        let payment = McpPayment::new(
            self.wallet.clone(),
            PaymentContext {
                url: self.url.clone(),
                headers: Default::default(),
                http_client: None,
            },
        );
        let pending = match management {
            Some(management) => {
                payment
                    .prepare_with_management(&challenges, management)
                    .await
            }
            None => payment.prepare(&challenges).await,
        }
        .map_err(|error| error.to_string())?;
        let Some(pending) = pending else {
            return Ok(None);
        };
        Ok(Some(Box::new(McpPending {
            credential: mcp::credential_value(pending.credential())
                .map_err(|error| error.to_string())?,
            pending,
            payment,
            receipt: None,
            url: self.url.clone(),
        })))
    }
}

struct ToolCall<'a>(&'a dyn McpPaymentManagement);

impl ManagementTransport for ToolCall<'_> {
    fn send<'a>(
        &'a self,
        _challenge: &'a Challenge,
        credential: &'a Credential,
    ) -> ManagementFuture<'a> {
        Box::pin(async move {
            let receipt = self
                .0
                .send(mcp::credential_value(credential)?)
                .await
                .map_err(Fault::Http)?;
            Ok(receipt.and_then(Receipt::from_value))
        })
    }
}

struct McpPending {
    pending: PendingPayment<Wallet>,
    payment: McpPayment<Wallet>,
    credential: Value,
    receipt: Option<Value>,
    url: nanocode_http::Url,
}

impl McpPendingPayment for McpPending {
    fn credential(&self) -> &Value {
        &self.credential
    }

    fn receipt(&mut self, receipt: &Value) {
        self.receipt = Some(receipt.clone());
    }

    fn commit<'async_trait>(
        self: Box<Self>,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = std::result::Result<(), String>> + Send + 'async_trait>,
    >
    where
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: std::result::Result<(), String> = {
                let challenge = self.pending.challenge();
                let charged = self
                    .receipt
                    .clone()
                    .and_then(Receipt::from_value)
                    .and_then(|receipt| self.payment.record_receipt(challenge, &receipt));
                let amount = charged.or_else(|| challenge.amount());
                if let (Some(receipt), Some(amount)) = (&self.receipt, amount) {
                    let wallet = self.payment.provider();
                    let funding = wallet.funding_currency(challenge);
                    let charge = Paid {
                        url: &self.url,
                        challenge,
                        amount,
                        funding: funding.as_deref(),
                        reference: receipt
                            .get("reference")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    };
                    notify(
                        &wallet.payments.listeners,
                        wallet
                            .payments
                            .ledger
                            .update(|ledger| ledger.record(&charge)),
                    );
                }
                self.pending
                    .commit()
                    .await
                    .map_err(|error| error.to_string())
            };
            output
        })
    }

    fn rollback<'async_trait>(
        self: Box<Self>,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = std::result::Result<(), String>> + Send + 'async_trait>,
    >
    where
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: std::result::Result<(), String> = {
                self.pending
                    .rollback()
                    .await
                    .map_err(|error| error.to_string())
            };
            output
        })
    }
}

fn deposit_notice(context: &Deposit, machine: Address) -> String {
    let symbol = context
        .currency
        .as_deref()
        .map(|currency| symbol(currency, machine))
        .unwrap_or_default();
    let host = context.url.host_str().unwrap_or_default();
    match context.opened {
        true => format!(
            "opened a {} {symbol} session with {host} (channel {})",
            mach(context.amount),
            context.channel_id
        ),
        false => format!(
            "topped up the {host} session by {} {symbol} (deposit {} {symbol})",
            mach(context.amount),
            mach(context.deposit)
        ),
    }
}

fn symbol(currency: &str, machine: Address) -> String {
    match currency.parse::<Address>() {
        Ok(currency) if currency == machine => "MACH".to_owned(),
        Ok(currency) if currency == USDC => "USDC.e".to_owned(),
        Ok(currency) if currency == PATH_USD => "pathUSD".to_owned(),
        Ok(currency) if currency == OUSD => "OUSD".to_owned(),
        _ => currency.to_owned(),
    }
}

fn notify(listeners: &Listeners, notice: Option<String>) {
    let Some(notice) = notice else {
        return;
    };
    let gone = listeners.read(|listeners| {
        listeners
            .iter()
            .filter(|listener| listener.send(notice.clone()).is_err())
            .count()
    });
    if gone > 0 {
        listeners.update(|listeners| listeners.retain(|listener| !listener.is_closed()));
    }
}

fn bar(change: i128, peak: i128) -> char {
    if change <= 0 {
        return BARS[0];
    }
    let level = ((change as f64 / peak as f64).sqrt() * (BARS.len() - 1) as f64).ceil() as usize;
    BARS[level.clamp(1, BARS.len() - 1)]
}

fn mach(amount: u128) -> String {
    format!("{}.{:06}", amount / MACH, amount % MACH)
}

fn dollars(symbol: &str) -> bool {
    matches!(symbol, "MACH" | "USDC.e" | "pathUSD" | "OUSD")
}

fn priced(amount: u128, symbol: &str) -> String {
    match dollars(symbol) {
        true => format!("${}", mach(amount)),
        false => format!("{} {symbol}", mach(amount)),
    }
}

type Read = fn(&str, &Value) -> std::result::Result<Vec<Offer>, String>;

pub(crate) struct Listing {
    url: String,
    read: Read,
    state: Atom<Listed>,
    flight: SingleFlight<()>,
}

enum Listed {
    Loading,
    Offers(Vec<Offer>),
    Failed(String),
}

struct Offer {
    model: Model,
    provider: String,
    upstream: String,
    price: Option<String>,
}

impl Offer {
    fn named_for_provider(&self) -> bool {
        self.model
            .as_str()
            .split_once('/')
            .map(|(prefix, _)| prefix)
            == Some(self.provider.as_str())
    }
}

impl Listing {
    fn remote(url: String, read: Read) -> Arc<Self> {
        Arc::new(Self {
            url,
            read,
            state: Atom::new(Listed::Loading),
            flight: SingleFlight::default(),
        })
    }

    async fn load(&self, payments: &Payments) {
        if self.state.read(|state| matches!(state, Listed::Offers(_))) {
            return;
        }
        self.flight
            .run(|| async {
                let listed = match payments.client() {
                    Ok(client) => list(&client, &self.url, self.read).await,
                    Err(error) => Err(error),
                };
                self.state.set(match listed {
                    Ok(offers) => Listed::Offers(offers),
                    Err(error) => Listed::Failed(error),
                });
            })
            .await;
    }

    fn read<T>(&self, rows: impl FnOnce(&[Offer]) -> T) -> std::result::Result<T, String> {
        self.state.read(|state| match state {
            Listed::Loading => Err("listing models…".to_owned()),
            Listed::Failed(error) => Err(error.clone()),
            Listed::Offers(offers) => Ok(rows(offers)),
        })
    }
}

async fn list(
    client: &nanocode_http::Client,
    url: &str,
    read: Read,
) -> std::result::Result<Vec<Offer>, String> {
    let body = async {
        client
            .get(url)
            .send()
            .await
            .map(nanocode_http::decode_flate_response)?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|error| format!("listing models at {url}: {error}"))?;
    let listing: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("{url} did not return a model list: {error}"))?;
    let offers = read(url, &listing)?;
    if offers.is_empty() {
        return Err(format!("{url} lists no models"));
    }
    Ok(offers)
}

fn price(rates: &Value) -> Option<String> {
    match (&rates["input"], &rates["output"]) {
        (Value::Number(input), Value::Number(output)) => Some(format!("${input}/${output}")),
        _ => None,
    }
}

fn boundless_offers(url: &str, listing: &Value) -> std::result::Result<Vec<Offer>, String> {
    let mut offers = Vec::new();
    for entry in listing["data"]
        .as_array()
        .ok_or_else(|| format!("{url} returned no model list"))?
    {
        let id = entry["id"]
            .as_str()
            .ok_or_else(|| format!("{url} listed a model without an id"))?;
        offers.push(Offer {
            model: id.parse::<Model>()?,
            provider: Tempo::BOUNDLESS.to_owned(),
            upstream: id.to_owned(),
            price: price(&entry["pricing"]),
        });
    }
    Ok(offers)
}

fn gateway_offers(url: &str, listing: &Value) -> std::result::Result<Vec<Offer>, String> {
    let mut offers: Vec<Offer> = Vec::new();
    for entry in listing["models"]
        .as_array()
        .ok_or_else(|| format!("{url} returned no model list"))?
    {
        let id = entry["id"]
            .as_str()
            .ok_or_else(|| format!("{url} listed a model without an id"))?;
        if id.starts_with("boundless/") {
            continue;
        }
        let offer = Offer {
            model: id.parse::<Model>()?,
            provider: entry["provider"]
                .as_str()
                .ok_or_else(|| format!("{url} names no provider for {id}"))?
                .to_owned(),
            upstream: entry["upstream"].as_str().unwrap_or(id).to_owned(),
            price: price(&entry["house"]),
        };
        if offers.iter().all(|known| known.model != offer.model) {
            offers.push(offer);
        }
    }
    Ok(offers)
}

type OpenAiSource = Arc<dyn Fn() -> nanocode::codex::OpenAiBuilder + Send + Sync>;
type ClaudeSource = Arc<dyn Fn() -> nanocode::claude::ClaudeClient + Send + Sync>;

pub(crate) type Offered = Vec<(ModelSelection, Option<String>)>;

#[derive(Clone)]
pub(crate) struct Tempo {
    openai: OpenAiSource,
    claude: ClaudeSource,
    boundless: OpenAiSource,
    payments: Payments,
    listing: Arc<Listing>,
    catalog: Arc<Listing>,
}

impl Tempo {
    pub(crate) const NAME: &str = "tempo";
    pub(crate) const OUTPUT_TOKENS: u32 = 32_768;
    pub(crate) const PREFIX: &str = "tempo/";
    pub(crate) const ABOUT: &str = "models the Tempo gateway serves (OpenAI, Anthropic, Workers AI, Vercel, OpenRouter), paid from the Tempo wallet";
    pub(crate) const BOUNDLESS: &str = "boundless";
    pub(crate) const BOUNDLESS_ABOUT: &str = "models Boundless serves at api.inference.boundless.network, paid directly from the Tempo wallet";

    fn new(payments: &Payments, args: &GatewayArgs, identity: Arc<Identity>) -> Result<Self> {
        let terms = Terms::tempo(args)?;
        let wallet = payments.wallet(
            &nanocode_http::Url::parse(&format!("{TEMPO_GATEWAY}/v1"))?,
            &terms,
        );
        let unsponsored = Terms {
            gas: match terms.gas {
                Gas::Challenge => Gas::Held,
                gas => gas,
            },
            ..terms
        };
        let boundless = Mpp::new(
            payments.wallet(&nanocode_http::Url::parse(BOUNDLESS)?, &unsponsored),
            (format!("{BOUNDLESS}/responses"), None),
        )
        .identified(identity.clone());
        let client = payments.client()?;
        let responses = Mpp::new(
            wallet.clone(),
            (format!("{TEMPO_GATEWAY}/v1/responses"), None),
        )
        .identified(identity.clone());
        let messages =
            Mpp::new(wallet, (format!("{TEMPO_GATEWAY}/v1/messages"), None)).identified(identity);
        let (openai_client, claude_client) = (client.clone(), client.clone());
        Ok(Self {
            openai: Arc::new(move || {
                nanocode::OpenAi::builder(nanocode::codex::OpenAiAuth::gateway_managed())
                    .api_base_url(format!("{TEMPO_GATEWAY}/v1"))
                    .transport(nanocode::codex::transport::ResponsesTransport::Https)
                    .http_client(openai_client.clone())
                    .http_layer(responses.clone())
            }),
            claude: Arc::new(move || {
                nanocode::claude::ClaudeClient::with_auth_headers(
                    claude_client.clone(),
                    format!("{TEMPO_GATEWAY}/v1/messages"),
                    Default::default(),
                )
                .with_http_layer(messages.clone())
            }),
            boundless: Arc::new(move || {
                nanocode::OpenAi::builder(nanocode::codex::OpenAiAuth::gateway_managed())
                    .api_base_url(BOUNDLESS)
                    .transport(nanocode::codex::transport::ResponsesTransport::Https)
                    .http_client(client.clone())
                    .http_layer(boundless.clone())
            }),
            payments: payments.clone(),
            listing: Listing::remote(
                format!("{TEMPO_GATEWAY}/api/auction/models"),
                gateway_offers,
            ),
            catalog: Listing::remote(format!("{BOUNDLESS}/models"), boundless_offers),
        })
    }

    pub(crate) fn openai(&self) -> nanocode::codex::OpenAiBuilder {
        (self.openai)()
    }

    pub(crate) fn claude(&self) -> nanocode::claude::ClaudeClient {
        (self.claude)()
    }

    pub(crate) fn boundless(&self) -> nanocode::codex::OpenAiBuilder {
        (self.boundless)()
    }

    pub(crate) async fn load(&self) {
        futures_util::future::join(
            self.listing.load(&self.payments),
            self.catalog.load(&self.payments),
        )
        .await;
    }

    pub(crate) fn offer(&self, subagents: &nanocode::SubagentControl) {
        let catalog = |tempo: &Self| {
            let (groups, note) = tempo.groups();
            [
                (Route::Tempo, Self::NAME, Self::ABOUT),
                (Route::Boundless, Self::BOUNDLESS, Self::BOUNDLESS_ABOUT),
            ]
            .map(|(route, name, about)| nanocode::subagents::SpawnProvider {
                name: name.to_owned(),
                about: about.to_owned(),
                models: groups
                    .iter()
                    .filter(|(_, offered, _)| *offered == route)
                    .flat_map(|(_, _, models)| models.iter().map(|(model, _)| model.clone()))
                    .collect(),
                note: note.clone(),
            })
            .to_vec()
        };
        let providers = catalog(self);
        subagents.update_spawn_policy(|policy| policy.providers = Some(providers.clone()));
        let (tempo, subagents) = (self.clone(), subagents.clone());
        tokio::spawn(async move {
            tempo.load().await;
            let providers = catalog(&tempo);
            subagents.update_spawn_policy(|policy| policy.providers = Some(providers.clone()));
        });
    }

    pub(crate) fn model(id: &str) -> ModelSelection {
        match (Model::known(id), id.parse::<ClaudeModel>()) {
            (Some(model), _) => ModelSelection::Codex(model),
            (None, Ok(model)) => ModelSelection::Claude(model),
            (None, Err(_)) => ModelSelection::Codex(Model::Custom(nanocode::CustomModel::new(id))),
        }
    }

    pub(crate) fn boundless_model(id: &str) -> ModelSelection {
        ModelSelection::Codex(Model::Custom(nanocode::CustomModel::new(id)))
    }

    pub(crate) fn groups(&self) -> (Vec<(String, Route, Offered)>, Option<String>) {
        let mut groups = Vec::new();
        let mut notes = Vec::new();
        match self.catalog.read(|offers| {
            offers
                .iter()
                .map(|offer| {
                    (
                        Self::boundless_model(offer.model.as_str()),
                        offer.price.clone(),
                    )
                })
                .collect::<Offered>()
        }) {
            Ok(models) => groups.push((Self::BOUNDLESS.to_owned(), Route::Boundless, models)),
            Err(note) => notes.push(format!("{}: {note}", Self::BOUNDLESS)),
        }
        match self.listing.read(rows) {
            Ok(rows) => groups.extend(
                rows.into_iter()
                    .map(|(provider, models)| (provider, Route::Tempo, models)),
            ),
            Err(note) => notes.push(note),
        }
        (groups, (!notes.is_empty()).then(|| notes.join("; ")))
    }
}

struct Row<'a> {
    provider: &'a str,
    builtin: Option<ModelSelection>,
    upstream: &'a str,
    model: ModelSelection,
    price: Option<String>,
    preferred: bool,
}

fn rows(offers: &[Offer]) -> Vec<(String, Offered)> {
    let routed = |builtin: &ModelSelection| {
        ["vercel/", "openrouter/"].into_iter().find_map(|prefix| {
            offers.iter().find(|offer| {
                offer.model.as_str().starts_with(prefix)
                    && alias(&offer.upstream).as_ref() == Some(builtin)
            })
        })
    };
    let mut rows: Vec<Row<'_>> = Vec::new();
    for offer in offers {
        let builtin = alias(&offer.upstream);
        let route = builtin
            .as_ref()
            .and_then(|builtin| Some((builtin, routed(builtin)?)))
            .filter(|(_, route)| route.provider == offer.provider);
        let row = match route {
            Some((builtin, route)) => Row {
                provider: &offer.provider,
                builtin: Some(builtin.clone()),
                upstream: &offer.upstream,
                model: builtin.clone(),
                price: route.price.clone(),
                preferred: true,
            },
            None => Row {
                provider: &offer.provider,
                builtin: builtin.clone(),
                upstream: &offer.upstream,
                model: ModelSelection::Codex(offer.model),
                price: offer.price.clone(),
                preferred: offer.named_for_provider(),
            },
        };
        match rows.iter_mut().find(|known| {
            known.provider == row.provider
                && match (&known.builtin, &row.builtin) {
                    (Some(known), Some(builtin)) => known == builtin,
                    _ => known.upstream == row.upstream,
                }
        }) {
            Some(known) if !known.preferred && row.preferred => *known = row,
            Some(_) => {}
            None => rows.push(row),
        }
    }
    let mut groups: Vec<(String, Offered)> = Vec::new();
    for row in rows {
        let offered = (row.model, row.price);
        match groups
            .iter_mut()
            .find(|(provider, _)| provider == row.provider)
        {
            Some((_, models)) => models.push(offered),
            None => groups.push((row.provider.to_owned(), vec![offered])),
        }
    }
    groups
}

fn alias(id: &str) -> Option<ModelSelection> {
    let name = id.rsplit('/').next()?;
    Model::known(name)
        .filter(|model| Model::ALL.contains(model))
        .map(ModelSelection::Codex)
        .or_else(|| {
            name.replace('.', "-")
                .parse::<ClaudeModel>()
                .ok()
                .map(ModelSelection::Claude)
        })
}

pub(crate) struct Closing {
    payments: Payments,
    certificates: Option<PathBuf>,
}

pub(crate) async fn close(closing: Closing) {
    if let Some(certificates) = closing.certificates {
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(certificates)).await;
    }
    let sessions = closing.payments.closing.get();
    if sessions.is_empty() {
        return;
    }
    let client = nanocode_http::client();
    for (session, url) in sessions.iter() {
        close_session(&client, session, url).await;
    }
}

async fn close_session(client: &nanocode_http::Client, session: &SessionProvider, url: &str) {
    let host = nanocode_http::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned());
    for _ in 0..CLOSE_ATTEMPTS {
        match session.close(client, url).await {
            Ok(Some(receipt)) => {
                return eprintln!(
                    "nanocode: closed the {host} session cooperatively (receipt {})",
                    receipt.reference
                );
            }
            Ok(None) => return,
            Err(error) if error.to_string().contains(" 409 ") => {
                tokio::time::sleep(CLOSE_RETRY).await;
            }
            Err(error) => {
                return eprintln!(
                    "nanocode: closing the {host} session: {error}; it stays open for the next run"
                );
            }
        }
    }
    eprintln!("nanocode: the {host} session stayed busy; the next run closes it");
}

async fn balance(
    client: &nanocode_http::Client,
    token: &str,
    account: Address,
) -> std::result::Result<U256, String> {
    let account = format!("{account:x}");
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [
            { "to": token, "data": format!("0x70a08231{:0>64}", account.trim_start_matches("0x")) },
            "latest",
        ],
    });
    let reading = async {
        client
            .post(DEFAULT_RPC_URL)
            .header(nanocode_http::header::CONTENT_TYPE, "application/json")
            .body(call.to_string())
            .send()
            .await
            .map(nanocode_http::decode_flate_response)?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|error| {
        format!("reading the wallet's {token} balance from {DEFAULT_RPC_URL}: {error}")
    })?;
    let answer: Value = serde_json::from_slice(&reading).map_err(|error| error.to_string())?;
    let result = answer["result"]
        .as_str()
        .ok_or_else(|| format!("{DEFAULT_RPC_URL} answered no balance of {token}: {answer}"))?;
    U256::from_str_radix(result.trim_start_matches("0x"), 16).map_err(|error| error.to_string())
}

fn gas_token(token: &str) -> Result<Address> {
    let token = token.trim().to_lowercase();
    let address: Address = match token.as_str() {
        "usdc.e" | "usdc" => USDC,
        "usdt0" => USDT0,
        address if address.starts_with("0x") => address.parse()?,
        other => {
            return Err(format!(
                "--tempo-fee-token must be usdc.e, usdt0 or a token address, not {other}"
            )
            .into());
        }
    };
    if machine_token(CHAIN_ID) == Some(address) || OUSD == address {
        return Err(
            format!("--tempo-fee-token {token} cannot pay gas; MACH and OUSD do not").into(),
        );
    }
    Ok(address)
}

fn session_slot(origin: &str, named: Option<&str>) -> Result<String> {
    if let Some(name) = named.map(str::trim).filter(|name| !name.is_empty()) {
        return Ok(name.to_owned());
    }
    let dir = default_channel_database_path()?.with_file_name("mpp-sessions");
    std::fs::create_dir_all(&dir)?;
    let key: String = origin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    for slot in 0..SESSION_SLOTS {
        let lock = dir.join(format!("{key}-{slot}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock)?;
        if file.try_lock().is_ok() {
            std::mem::forget(file);
            return Ok(slot.to_string());
        }
    }
    Err(format!("{SESSION_SLOTS} agents already pay this endpoint from this wallet").into())
}

fn asker(args: &GatewayArgs, exec: bool) -> Asker {
    use std::io::IsTerminal;
    let email = args
        .identity_email
        .as_deref()
        .is_some_and(|email| !email.trim().is_empty());
    match exec {
        false => Asker::Pass,
        true if email && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
            Asker::Stdin
        }
        true => Asker::Nobody,
    }
}

fn tool_deposit(args: &GatewayArgs) -> u128 {
    args.tempo_tool_deposit.unwrap_or(MACH / 10)
}

fn paid_origins() -> Result<Vec<String>> {
    let mut origins = vec![TEMPO_SERVICES.to_owned()];
    for url in [GATEWAY, MERCATOR] {
        origins.push(
            nanocode_http::Url::parse(url)?
                .origin()
                .ascii_serialization(),
        );
    }
    Ok(origins)
}

fn tools(clients: ModelClients, payments: &Payments, args: &GatewayArgs) -> Result<ModelClients> {
    Ok(clients
        .mcp_server("mercator", payments.mcp(MERCATOR, &Terms::mainnet(None)?)?)
        .mcp_server(
            "gateway",
            payments.mcp(GATEWAY, &Terms::mainnet(Some(tool_deposit(args)))?)?,
        ))
}

fn shell(
    clients: ModelClients,
    payments: &Payments,
    args: &GatewayArgs,
    identity: Arc<Identity>,
) -> Result<(ModelClients, Option<PathBuf>)> {
    if OWN_PROXY_SETTINGS
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return Ok((clients, None));
    }
    let policy = Policy::Origins(paid_origins()?);
    match payments.pay_tools(
        Terms::mainnet(Some(tool_deposit(args)))?,
        Vec::new(),
        std::future::ready(policy),
        identity,
    ) {
        Ok((environment, certificates)) => {
            Ok((clients.tool_environment(environment), Some(certificates)))
        }
        Err(error) => {
            tracing::warn!(target: "nanocode", error = %error, "shell commands pay no Tempo services: the paying proxy did not start");
            Ok((clients, None))
        }
    }
}

pub(crate) fn apply(
    clients: ModelClients,
    args: &GatewayArgs,
    exec: bool,
) -> Result<(ModelClients, Option<Closing>)> {
    let models = !clients.supplies_openai();
    if clients.reports_spend() || !models && args.no_tempo_tools {
        return Ok((clients, None));
    }
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let machine = machine_token(CHAIN_ID).ok_or("MACH is not deployed on Tempo mainnet")?;
    let served = match models {
        true => vec![
            format!("{TEMPO_GATEWAY}/v1/responses"),
            format!("{TEMPO_GATEWAY}/v1/messages"),
            format!("{BOUNDLESS}/responses"),
        ],
        false => Vec::new(),
    };
    let payments = Payments::new(machine, served, args.tempo_session.clone());
    let identity = Arc::new(Identity::new(
        payments.clone(),
        &args.identity_issuer,
        args.identity_email.clone(),
        &args.identity_attestation,
        asker(args, exec),
    )?);
    let mut clients = clients;
    if models {
        clients = clients.tempo(Tempo::new(&payments, args, identity.clone())?);
    }
    let mut certificates = None;
    if !args.no_tempo_tools {
        (clients, certificates) = shell(
            tools(clients, &payments, args)?,
            &payments,
            args,
            identity.clone(),
        )?;
    }
    let (notices, status, panel) = (payments.clone(), payments.clone(), payments.clone());
    let clients = clients
        .notices(move || notices.notices())
        .status(move || status.status())
        .answer(move |text| identity.answer(text))
        .panel(
            "/spend",
            "show or hide what models and tools have spent",
            move || panel.panel(),
        );
    Ok((
        clients,
        Some(Closing {
            payments,
            certificates,
        }),
    ))
}

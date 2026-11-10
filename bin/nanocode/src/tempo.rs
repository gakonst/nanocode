use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::Stream;
use mpp::{
    Address, MppError, PaymentChallenge, PaymentCredential, Receipt,
    client::{
        AcceptPaymentPolicy, AutoswapConfig, ClientEventSubscription, ClientEvents,
        DEFAULT_MAX_PAYMENT_RETRIES, DepositContext, Exchange, FlowError, ForwardProxy,
        ManagementFuture, ManagementTransport, PaymentContext, PaymentEventFuture, PaymentProvider,
        PendingPayment, TempoAccountsProvider, TempoClientError, TempoSessionProvider,
        tempo::session::store::{
            SqliteChannelStore, SqliteChannelStoreOptions, default_channel_database_path,
        },
    },
    mcp::client::McpPayment,
    protocol::{
        intents::{INTENT_CHARGE, INTENT_SESSION},
        methods::tempo::{CHAIN_ID, METHOD_NAME, PATH_USD, USDC, machine_token},
    },
};
use nanocode::{
    ClaudeModel, Model, ModelSelection,
    http::{self as layer, BoxFuture, Event, EventHook, Layer, Next},
    tools::mcp::{McpPaymentManagement, McpPaymentProvider, McpPendingPayment, McpServer},
};
use nanocode_sync::{Atom, Signal, SingleFlight};
use reqwest::ResponseBuilderExt;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    cli::GatewayArgs,
    host::{ModelClients, Panel, Result},
};

const MERCATOR: &str = "https://mercator.sh/mcp";
const GATEWAY: &str = "https://gateway.tempo-dev.workers.dev/mcp";
const TEMPO_GATEWAY: &str = "https://gateway.tempo-dev.workers.dev";
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
const OUSD: &str = "0x20c0000000000000000000006a37DA5C996874BE";
const USDT0: &str = "0x20c00000000000000000000014f22ca97301eb73";
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
            let url = reqwest::Url::parse(&request.url)
                .map_err(|error| layer::Error::new(error.to_string()).unsent(true))?;
            let mut paid = reqwest::Request::new(request.method, url);
            *paid.headers_mut() = request.headers;
            *paid.body_mut() = Some(request.body.into());
            let mut exchange = NextExchange {
                next,
                client: self
                    .wallet
                    .payments
                    .client()
                    .map_err(|error| layer::Error::new(error).unsent(true))?,
                body: None,
            };
            let response = mpp::client::send_with_exchange(
                &mut exchange,
                paid,
                &self.wallet,
                &AcceptPaymentPolicy::Always,
                &self.wallet.payments.events,
                DEFAULT_MAX_PAYMENT_RETRIES,
            )
            .await
            .map_err(|error| match error {
                FlowError::Send(error) => error,
                FlowError::Payment(error) => layer::Error::new(error.to_string()),
            })?;
            let body = exchange
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
    client: reqwest::Client,
    body: Option<layer::Body>,
}

impl Exchange for NextExchange<'_> {
    type Error = layer::Error;

    fn http_client(&self) -> Option<reqwest::Client> {
        Some(self.client.clone())
    }

    async fn send(
        &mut self,
        request: reqwest::Request,
    ) -> std::result::Result<reqwest::Response, layer::Error> {
        let sent = layer::Request {
            method: request.method().clone(),
            url: request.url().to_string(),
            headers: request.headers().clone(),
            body: request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .map(<[u8]>::to_vec)
                .unwrap_or_default(),
        };
        let (status, headers, body, _) = self.next.send(sent).await?.into_parts();
        self.body = Some(body);
        let mut response = http::Response::builder()
            .status(status)
            .url(request.url().clone())
            .body(reqwest::Body::from(Vec::new()))
            .map_err(|error| layer::Error::new(error.to_string()))?;
        *response.headers_mut() = headers;
        Ok(response.into())
    }
}

type Listeners = Atom<Vec<UnboundedSender<String>>>;

#[derive(Clone)]
struct Payments {
    client: Arc<Atom<Option<reqwest::Client>>>,
    ledger: Arc<Atom<Ledger>>,
    listeners: Arc<Listeners>,
    events: ClientEvents,
    failure: Arc<Atom<Option<String>>>,
    session: Option<String>,
    wallets: Arc<Atom<HashMap<String, Wallet>>>,
    slots: Arc<Atom<HashMap<String, String>>>,
    closing: Arc<Atom<Vec<(TempoSessionProvider, String)>>>,
    short: Arc<Atom<bool>>,
    _observers: Arc<[ClientEventSubscription; 3]>,
}

#[derive(Clone, Default)]
struct Ledger {
    machine: Address,
    models: Vec<String>,
    services: BTreeMap<String, Service>,
    sessions: HashMap<String, TempoSessionProvider>,
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
    url: &'a reqwest::Url,
    challenge: &'a PaymentChallenge,
    amount: u128,
    funding: Option<&'a str>,
    reference: &'a str,
}

impl Payments {
    fn new(machine: Address, models: Vec<String>, session: Option<String>) -> Self {
        let events = ClientEvents::default();
        let ledger = Arc::new(Atom::new(Ledger {
            machine,
            models,
            ..Ledger::default()
        }));
        let listeners = Arc::<Listeners>::default();
        let short = Arc::<Atom<bool>>::default();
        let (paid, updated, funded) = (ledger.clone(), ledger.clone(), short.clone());
        let (told, deposits) = (listeners.clone(), listeners.clone());
        let observers = [
            events.on_payment_response(move |context| {
                funded.set(false);
                if let Some(amount) = context.amount {
                    let receipt = context.receipt.as_deref();
                    let funding = context.funding_currency.clone().or_else(|| {
                        Some(
                            receipt?
                                .extensions
                                .get("fundingCurrency")?
                                .as_str()?
                                .to_owned(),
                        )
                    });
                    let charge = Paid {
                        url: &context.url,
                        challenge: &context.challenge,
                        amount,
                        funding: funding.as_deref(),
                        reference: receipt.map_or("", |receipt| receipt.reference.as_str()),
                    };
                    notify(&told, paid.update(|ledger| ledger.record(&charge)));
                }
                async {}
            }),
            events.on_deposit(move |context| {
                notify(&deposits, Some(deposit_notice(&context, machine)));
                async {}
            }),
            events.on_payment_update(move |context| {
                updated.update(|ledger| ledger.update(&context.url, context.change));
                async {}
            }),
        ];
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
            _observers: Arc::new(observers),
        }
    }

    fn client(&self) -> std::result::Result<reqwest::Client, String> {
        if let Some(client) = self.client.read(Clone::clone) {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        Ok(self
            .client
            .update(|slot| slot.get_or_insert_with(|| client.clone()).clone()))
    }

    fn wallet(&self, url: &reqwest::Url, terms: &Terms) -> Wallet {
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

    fn store(&self, origin: String, service: &str) -> Result<SqliteChannelStore> {
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
        Ok(SqliteChannelStore::open(SqliteChannelStoreOptions {
            namespace,
            path: None,
            request_url: Some(service.to_owned()),
        })?)
    }

    fn sessions(
        &self,
        wallet: &TempoAccountsProvider,
        service: &reqwest::Url,
        terms: &Terms,
    ) -> Result<TempoAccountsProvider> {
        let origin = service.origin().ascii_serialization();
        let origin = match &terms.variant {
            Some(variant) => format!("{origin}~{variant}"),
            None => origin,
        };
        let store = self.store(origin, service.as_str())?;
        let wallet = wallet.clone().with_session_store(Arc::new(store));
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
        policy: impl Future<Output = AcceptPaymentPolicy> + Send + 'static,
    ) -> Result<(Vec<(&'static str, String)>, PathBuf)> {
        nanocode::codex::transport::install_default_rustls_crypto_provider();
        let payments = self.clone();
        let proxy = ForwardProxy::bind(move |url| {
            Ok::<_, std::convert::Infallible>(payments.wallet(url, &terms))
        })?
        .bypass(bypass)
        .events(self.events.clone());
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
        let url: reqwest::Url = url.parse()?;
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

    fn update(&mut self, url: &reqwest::Url, change: i128) {
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
    url: reqwest::Url,
}

#[derive(Clone)]
struct Terms {
    currency: Address,
    fee: Option<Address>,
    deposit: Option<u128>,
    variant: Option<String>,
    closes: bool,
}

impl Terms {
    fn mainnet(deposit: Option<u128>) -> Result<Self> {
        Ok(Self {
            currency: machine_token::token(CHAIN_ID)
                .ok_or("MACH is not deployed on Tempo mainnet")?,
            fee: None,
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
            "usdc.e" | "usdc" => USDC.parse()?,
            "ousd" => OUSD.parse()?,
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
            fee: args.tempo_fee_token.as_deref().map(gas_token).transpose()?,
            deposit: Some(args.tempo_deposit.unwrap_or(2 * MACH)),
            variant: (currency != machine).then_some(wanted),
            closes: args.tempo_close_session,
        })
    }
}

#[derive(Clone)]
struct Wallet {
    url: reqwest::Url,
    terms: Terms,
    payments: Payments,
    loaded: Arc<Atom<Option<TempoAccountsProvider>>>,
    loading: Arc<SingleFlight<std::result::Result<TempoAccountsProvider, String>>>,
}

impl Wallet {
    fn ready(&self) -> Option<TempoAccountsProvider> {
        self.loaded.read(Clone::clone)
    }

    async fn get(&self) -> std::result::Result<TempoAccountsProvider, String> {
        if let Some(wallet) = self.ready() {
            return Ok(wallet);
        }
        self.loading
            .run(|| async {
                if let Some(wallet) = self.ready() {
                    return Ok(wallet);
                }
                let opening = self.clone();
                let wallet = tokio::task::spawn_blocking(move || opening.open())
                    .await
                    .map_err(|error| error.to_string())??;
                self.loaded.set(Some(wallet.clone()));
                Ok(wallet)
            })
            .await
    }

    async fn load(&self) -> std::result::Result<TempoAccountsProvider, MppError> {
        self.get().await.map_err(MppError::InvalidConfig)
    }

    fn open(&self) -> std::result::Result<TempoAccountsProvider, String> {
        let wallet = TempoAccountsProvider::from_default_store()
            .map_err(|error| {
                let error = match error {
                    MppError::InvalidConfig(message) => message,
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
        let wallet = match self.terms.fee {
            Some(token) => wallet.with_session_fee_token(token),
            None => wallet,
        };
        self.payments
            .sessions(&wallet, &self.url, &self.terms)
            .map_err(|error| error.to_string())
    }

    fn noted<T>(&self, paid: std::result::Result<T, MppError>) -> std::result::Result<T, MppError> {
        if let Err(error) = &paid
            && matches!(
                error,
                MppError::InsufficientBalance(_)
                    | MppError::Tempo(TempoClientError::InsufficientBalance { .. })
            )
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

    fn select_challenge<'a>(
        &self,
        challenges: &[&'a PaymentChallenge],
    ) -> Option<&'a PaymentChallenge> {
        match self.ready() {
            Some(wallet) => wallet.select_challenge(challenges),
            None => challenges.first().copied(),
        }
    }

    async fn pay(
        &self,
        challenge: &PaymentChallenge,
    ) -> std::result::Result<PaymentCredential, MppError> {
        let wallet = self.load().await?;
        self.noted(wallet.pay(challenge).await)
    }

    async fn pay_with_context(
        &self,
        challenge: &PaymentChallenge,
        context: PaymentContext,
    ) -> std::result::Result<PaymentCredential, MppError> {
        let wallet = self.load().await?;
        self.noted(wallet.pay_with_context(challenge, context).await)
    }

    async fn pay_with_management(
        &self,
        challenge: &PaymentChallenge,
        context: PaymentContext,
        management: &dyn ManagementTransport,
    ) -> std::result::Result<PaymentCredential, MppError> {
        let wallet = self.load().await?;
        self.noted(
            wallet
                .pay_with_management(challenge, context, management)
                .await,
        )
    }

    async fn commit_payment(
        &self,
        challenge: &PaymentChallenge,
        credential: &PaymentCredential,
    ) -> std::result::Result<(), MppError> {
        match self.ready() {
            Some(wallet) => wallet.commit_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    async fn rollback_payment(
        &self,
        challenge: &PaymentChallenge,
        credential: &PaymentCredential,
    ) -> std::result::Result<(), MppError> {
        match self.ready() {
            Some(wallet) => wallet.rollback_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    async fn invalidate_payment(
        &self,
        challenge: &PaymentChallenge,
        credential: &PaymentCredential,
    ) -> std::result::Result<(), MppError> {
        match self.ready() {
            Some(wallet) => wallet.invalidate_payment(challenge, credential).await,
            None => Ok(()),
        }
    }

    fn abandon_payment(&self, challenge: &PaymentChallenge, credential: &PaymentCredential) {
        if let Some(wallet) = self.ready() {
            wallet.abandon_payment(challenge, credential);
        }
    }

    async fn prepaid_credential(
        &self,
        context: PaymentContext,
    ) -> Option<(PaymentChallenge, PaymentCredential)> {
        self.get().await.ok()?.prepaid_credential(context).await
    }

    fn observe_receipt(
        &self,
        context: &PaymentContext,
        challenge: &PaymentChallenge,
        receipt: &Receipt,
    ) {
        if let Some(wallet) = self.ready() {
            wallet.observe_receipt(context, challenge, receipt);
        }
    }

    fn record_receipt(
        &self,
        context: &PaymentContext,
        challenge: &PaymentChallenge,
        receipt: &Receipt,
    ) -> Option<u128> {
        self.ready()?.record_receipt(context, challenge, receipt)
    }

    fn funding_currency(&self, challenge: &PaymentChallenge) -> Option<String> {
        self.ready()?.funding_currency(challenge)
    }

    fn stream_event(
        &self,
        client: &reqwest::Client,
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

#[async_trait]
impl McpPaymentProvider for McpPayments {
    async fn prepare(
        &self,
        payment_required: &Value,
    ) -> std::result::Result<Option<Box<dyn McpPendingPayment>>, String> {
        self.pay(payment_required, None).await
    }

    async fn prepare_with_management(
        &self,
        payment_required: &Value,
        management: &dyn McpPaymentManagement,
    ) -> std::result::Result<Option<Box<dyn McpPendingPayment>>, String> {
        self.pay(payment_required, Some(&ToolCall(management)))
            .await
    }
}

impl McpPayments {
    async fn pay(
        &self,
        payment_required: &Value,
        management: Option<&ToolCall<'_>>,
    ) -> std::result::Result<Option<Box<dyn McpPendingPayment>>, String> {
        let Some(challenges) = mpp::mcp::extract_challenges_from_data(payment_required) else {
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
            credential: mpp::mcp::credential_value(pending.credential())
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
        _challenge: &'a PaymentChallenge,
        credential: &'a PaymentCredential,
    ) -> ManagementFuture<'a> {
        Box::pin(async move {
            let receipt = self
                .0
                .send(mpp::mcp::credential_value(credential)?)
                .await
                .map_err(MppError::Http)?;
            Ok(receipt.and_then(|receipt| serde_json::from_value(receipt).ok()))
        })
    }
}

struct McpPending {
    pending: PendingPayment<Wallet>,
    payment: McpPayment<Wallet>,
    credential: Value,
    receipt: Option<Value>,
    url: reqwest::Url,
}

#[async_trait]
impl McpPendingPayment for McpPending {
    fn credential(&self) -> &Value {
        &self.credential
    }

    fn receipt(&mut self, receipt: &Value) {
        self.receipt = Some(receipt.clone());
    }

    async fn commit(self: Box<Self>) -> std::result::Result<(), String> {
        let challenge = self.pending.challenge();
        let charged = self
            .receipt
            .clone()
            .and_then(|receipt| serde_json::from_value(receipt).ok())
            .and_then(|receipt| self.payment.record_receipt(challenge, &receipt));
        let amount = charged.or_else(|| {
            challenge
                .request
                .decode::<Value>()
                .ok()
                .and_then(|request| request.get("amount")?.as_str()?.parse().ok())
        });
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
    }

    async fn rollback(self: Box<Self>) -> std::result::Result<(), String> {
        self.pending
            .rollback()
            .await
            .map_err(|error| error.to_string())
    }
}

fn deposit_notice(context: &DepositContext, machine: Address) -> String {
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
        Ok(currency) if USDC.parse() == Ok(currency) => "USDC.e".to_owned(),
        Ok(currency) if PATH_USD.parse() == Ok(currency) => "pathUSD".to_owned(),
        Ok(currency) if OUSD.parse() == Ok(currency) => "OUSD".to_owned(),
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

pub(crate) struct Listing {
    url: String,
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
    fn remote(url: String) -> Arc<Self> {
        Arc::new(Self {
            url,
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
                    Ok(client) => list(&client, &self.url).await,
                    Err(error) => Err(error),
                };
                self.state.set(match listed {
                    Ok(offers) => Listed::Offers(offers),
                    Err(error) => Listed::Failed(error),
                });
            })
            .await;
    }
}

async fn list(client: &reqwest::Client, url: &str) -> std::result::Result<Vec<Offer>, String> {
    let body = async {
        client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|error| format!("listing models at {url}: {error}"))?;
    let listing: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("{url} did not return a model list: {error}"))?;
    let mut offers: Vec<Offer> = Vec::new();
    for entry in listing["models"]
        .as_array()
        .ok_or_else(|| format!("{url} returned no model list"))?
    {
        let id = entry["id"]
            .as_str()
            .ok_or_else(|| format!("{url} listed a model without an id"))?;
        let offer = Offer {
            model: id.parse::<Model>()?,
            provider: entry["provider"]
                .as_str()
                .ok_or_else(|| format!("{url} names no provider for {id}"))?
                .to_owned(),
            upstream: entry["upstream"].as_str().unwrap_or(id).to_owned(),
            price: match (&entry["house"]["input"], &entry["house"]["output"]) {
                (Value::Number(input), Value::Number(output)) => {
                    Some(format!("${input}/${output}"))
                }
                _ => None,
            },
        };
        match offers.iter_mut().find(|known| {
            known.model == offer.model
                || known.provider == offer.provider && known.upstream == offer.upstream
        }) {
            Some(known) if !known.named_for_provider() && offer.named_for_provider() => {
                *known = offer;
            }
            Some(_) => {}
            None => offers.push(offer),
        }
    }
    if offers.is_empty() {
        return Err(format!("{url} lists no models"));
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
    payments: Payments,
    listing: Arc<Listing>,
}

impl Tempo {
    pub(crate) const NAME: &str = "tempo";
    pub(crate) const OUTPUT_TOKENS: u32 = 32_768;
    pub(crate) const PREFIX: &str = "tempo/";
    pub(crate) const ABOUT: &str = "models the Tempo gateway serves (OpenAI, Anthropic, Workers AI, Vercel, OpenRouter, Boundless), paid from the Tempo wallet";

    fn new(payments: &Payments, args: &GatewayArgs) -> Result<Self> {
        let wallet = payments.wallet(
            &reqwest::Url::parse(&format!("{TEMPO_GATEWAY}/v1"))?,
            &Terms::tempo(args)?,
        );
        let responses = Mpp::new(
            wallet.clone(),
            (format!("{TEMPO_GATEWAY}/v1/responses"), None),
        );
        let messages = Mpp::new(wallet, (format!("{TEMPO_GATEWAY}/v1/messages"), None));
        Ok(Self {
            openai: Arc::new(move || {
                nanocode::OpenAi::builder(nanocode::codex::OpenAiAuth::gateway_managed())
                    .api_base_url(format!("{TEMPO_GATEWAY}/v1"))
                    .transport(nanocode::codex::transport::ResponsesTransport::Https)
                    .http_layer(responses.clone())
            }),
            claude: Arc::new(move || {
                nanocode::claude::ClaudeClient::with_auth_headers(
                    Default::default(),
                    format!("{TEMPO_GATEWAY}/v1/messages"),
                    Default::default(),
                )
                .with_http_layer(messages.clone())
            }),
            payments: payments.clone(),
            listing: Listing::remote(format!("{TEMPO_GATEWAY}/api/auction/models")),
        })
    }

    pub(crate) fn openai(&self) -> nanocode::codex::OpenAiBuilder {
        (self.openai)()
    }

    pub(crate) fn claude(&self) -> nanocode::claude::ClaudeClient {
        (self.claude)()
    }

    pub(crate) async fn load(&self) {
        self.listing.load(&self.payments).await;
    }

    pub(crate) fn offer(&self, subagents: &nanocode::SubagentControl) {
        let catalog = |tempo: &Self| {
            let (groups, note) = tempo.groups();
            vec![nanocode::subagents::SpawnProvider {
                name: Self::NAME.to_owned(),
                about: Self::ABOUT.to_owned(),
                models: groups
                    .into_iter()
                    .flat_map(|(_, offered)| offered)
                    .map(|(model, _)| model)
                    .collect(),
                note,
            }]
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

    pub(crate) fn groups(&self) -> (Vec<(Option<String>, Offered)>, Option<String>) {
        self.listing.state.read(|state| match state {
            Listed::Loading => (Vec::new(), Some("listing models…".to_owned())),
            Listed::Failed(error) => (Vec::new(), Some(error.clone())),
            Listed::Offers(offers) => {
                let aliases = offers
                    .iter()
                    .filter_map(|offer| alias(offer.model.as_str()))
                    .collect::<Vec<_>>();
                let builtin = Model::ALL
                    .map(ModelSelection::Codex)
                    .into_iter()
                    .chain(ClaudeModel::ALL.map(ModelSelection::Claude))
                    .filter(|model| aliases.contains(model))
                    .map(|model| (model, None))
                    .collect();
                let mut groups: Vec<(Option<String>, Offered)> = vec![(None, builtin)];
                for offer in offers {
                    let offered = (ModelSelection::Codex(offer.model), offer.price.clone());
                    match groups
                        .iter_mut()
                        .find(|(group, _)| group.as_deref() == Some(offer.provider.as_str()))
                    {
                        Some((_, models)) => models.push(offered),
                        None => groups.push((Some(offer.provider.clone()), vec![offered])),
                    }
                }
                (groups, None)
            }
        })
    }
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
    let client = reqwest::Client::new();
    for (session, url) in sessions.iter() {
        close_session(&client, session, url).await;
    }
}

async fn close_session(client: &reqwest::Client, session: &TempoSessionProvider, url: &str) {
    for _ in 0..CLOSE_ATTEMPTS {
        match session.close(client, url).await {
            Ok(Some(receipt)) => {
                return eprintln!(
                    "nanocode: closed the Tempo gateway session cooperatively (receipt {})",
                    receipt.reference
                );
            }
            Ok(None) => return,
            Err(error) if error.to_string().contains(" 409 ") => {
                tokio::time::sleep(CLOSE_RETRY).await;
            }
            Err(error) => {
                return eprintln!(
                    "nanocode: closing the Tempo gateway session: {error}; it stays open for the next run"
                );
            }
        }
    }
    eprintln!("nanocode: the Tempo gateway session stayed busy; the next run closes it");
}

fn gas_token(token: &str) -> Result<Address> {
    let token = token.trim().to_lowercase();
    let address: Address = match token.as_str() {
        "usdc.e" | "usdc" => USDC.parse()?,
        "usdt0" => USDT0.parse()?,
        address if address.starts_with("0x") => address.parse()?,
        other => {
            return Err(format!(
                "--tempo-fee-token must be usdc.e, usdt0 or a token address, not {other}"
            )
            .into());
        }
    };
    if machine_token::token(CHAIN_ID) == Some(address) || OUSD.parse() == Ok(address) {
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

fn tool_deposit(args: &GatewayArgs) -> u128 {
    args.tempo_tool_deposit.unwrap_or(MACH / 10)
}

fn paid_origins() -> Result<Vec<String>> {
    let mut origins = vec![TEMPO_SERVICES.to_owned()];
    for url in [GATEWAY, MERCATOR] {
        origins.push(reqwest::Url::parse(url)?.origin().ascii_serialization());
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
) -> Result<(ModelClients, Option<PathBuf>)> {
    if OWN_PROXY_SETTINGS
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return Ok((clients, None));
    }
    let policy = AcceptPaymentPolicy::Origins(paid_origins()?);
    match payments.pay_tools(
        Terms::mainnet(Some(tool_deposit(args)))?,
        Vec::new(),
        std::future::ready(policy),
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
) -> Result<(ModelClients, Option<Closing>)> {
    let models = !clients.supplies_openai();
    if clients.reports_spend() || !models && args.no_tempo_tools {
        return Ok((clients, None));
    }
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let machine = machine_token::token(CHAIN_ID).ok_or("MACH is not deployed on Tempo mainnet")?;
    let served = match models {
        true => vec![
            format!("{TEMPO_GATEWAY}/v1/responses"),
            format!("{TEMPO_GATEWAY}/v1/messages"),
        ],
        false => Vec::new(),
    };
    let payments = Payments::new(machine, served, args.tempo_session.clone());
    let mut clients = clients;
    if models {
        clients = clients.tempo(Tempo::new(&payments, args)?);
    }
    let mut certificates = None;
    if !args.no_tempo_tools {
        (clients, certificates) = shell(tools(clients, &payments, args)?, &payments, args)?;
    }
    let (notices, status, panel) = (payments.clone(), payments.clone(), payments.clone());
    let clients = clients
        .notices(move || notices.notices())
        .status(move || status.status())
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

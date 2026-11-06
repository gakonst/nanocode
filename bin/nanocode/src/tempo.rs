use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::Stream;
use mpp::{
    Address, MppError, PaymentChallenge, PaymentCredential,
    client::{
        AcceptPaymentPolicy, ClientEventSubscription, ClientEvents, DEFAULT_MAX_PAYMENT_RETRIES,
        DepositContext, Exchange, FlowError, ForwardProxy, ManagementFuture, ManagementTransport,
        PaymentContext, PaymentProvider, PendingPayment, TempoAccountsProvider,
        TempoSessionProvider,
        directory::{self, MPP_DIRECTORY_URL},
    },
    mcp::client::McpPayment,
    protocol::methods::tempo::{CHAIN_ID, PATH_USD, USDC, machine_token},
};
use nanocode::{
    ClaudeModel, Model,
    http::{self as layer, BoxFuture, Event, EventHook, Layer, Next},
    tools::mcp::{McpPaymentManagement, McpPaymentProvider, McpPendingPayment, McpServer},
};
use nanocode_sync::{Atom, Signal, SingleFlight};
use reqwest::ResponseBuilderExt;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    cli::{GatewayArgs, TempoNetworkArg},
    host::{ModelClients, Panel, Result},
};

const OPENAI: &str = "https://openai.mpp.tempo.xyz/v1";
const ANTHROPIC: &str = "https://anthropic.mpp.tempo.xyz/v1/messages";
const MERCATOR: &str = "https://mercator.sh/mcp";
const GATEWAY: &str = "https://gateway.tempo-dev.workers.dev/mcp";
const GATEWAY_ORIGIN: &str = "https://gateway.tempo-dev.workers.dev";
const TEMPO_SERVICES: &str = "*.mpp.tempo.xyz";
const TESTNET_CHAIN_ID: u64 = 42431;
const OUSD: &str = "0x20c0000000000000000000006a37DA5C996874BE";
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
    wallet: TempoAccountsProvider,
    client: reqwest::Client,
    events: ClientEvents,
    probe: (String, &'static str),
    warm: Arc<Warm>,
}

#[derive(Default)]
struct Warm {
    done: Signal,
    flight: SingleFlight<()>,
}

impl Mpp {
    fn new(
        wallet: &TempoAccountsProvider,
        service: &str,
        payments: &Payments,
        probe: (String, &'static str),
    ) -> Result<Self> {
        let wallet = payments.sessions(wallet, &reqwest::Url::parse(service)?)?;
        Ok(Self {
            wallet,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            events: payments.events.clone(),
            probe,
            warm: Arc::default(),
        })
    }

    async fn preconnect(&self) {
        if self.warm.done.is_fired() {
            return;
        }
        self.warm
            .flight
            .run(|| async {
                let (url, model) = &self.probe;
                let body = serde_json::json!({ "model": model }).to_string();
                let _ = self.wallet.preconnect(&self.client, url, body).await;
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
            self.preconnect().await;
            let url = reqwest::Url::parse(&request.url)
                .map_err(|error| layer::Error::new(error.to_string()).unsent(true))?;
            let mut paid = reqwest::Request::new(request.method, url);
            *paid.headers_mut() = request.headers;
            *paid.body_mut() = Some(request.body.into());
            let mut exchange = NextExchange {
                next,
                client: self.client.clone(),
                body: None,
            };
            let response = mpp::client::send_with_exchange(
                &mut exchange,
                paid,
                &self.wallet,
                &AcceptPaymentPolicy::Always,
                &self.events,
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
        let work =
            self.mpp
                .wallet
                .stream_event(&self.mpp.client, &self.url, event.name?, event.data)?;
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
    ledger: Arc<Atom<Ledger>>,
    listeners: Arc<Listeners>,
    events: ClientEvents,
    failure: Arc<Atom<Option<String>>>,
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
    fn new(machine: Address, models: Vec<String>) -> Self {
        let events = ClientEvents::default();
        let ledger = Arc::new(Atom::new(Ledger {
            machine,
            models,
            ..Ledger::default()
        }));
        let listeners = Arc::<Listeners>::default();
        let (paid, updated) = (ledger.clone(), ledger.clone());
        let (told, deposits) = (listeners.clone(), listeners.clone());
        let observers = [
            events.on_payment_response(move |context| {
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
            ledger,
            listeners,
            events,
            failure: Arc::default(),
            _observers: Arc::new(observers),
        }
    }

    fn sessions(
        &self,
        wallet: &TempoAccountsProvider,
        service: &reqwest::Url,
    ) -> Result<TempoAccountsProvider> {
        let wallet = wallet.clone().with_sqlite_session_store(service)?;
        let host = service.host_str().unwrap_or_default().to_owned();
        let session = wallet.session_for(service.as_str())?;
        self.ledger.update(|ledger| {
            ledger.sessions.insert(host.clone(), session.clone());
        });
        Ok(wallet)
    }

    fn pay_tools(
        &self,
        wallet: &TempoAccountsProvider,
        models: Vec<String>,
    ) -> Result<Vec<(&'static str, String)>> {
        let (payments, tools) = (self.clone(), wallet.clone());
        let proxy = ForwardProxy::bind(move |url| payments.sessions(&tools, url))?
            .bypass(models)
            .events(self.events.clone());
        let environment = proxy.environment();
        let (client, failed) = (reqwest::Client::new(), self.failure.clone());
        proxy.spawn(async move {
            let services = directory::service_urls(&client, MPP_DIRECTORY_URL)
                .await
                .unwrap_or_else(|error| {
                    failed.update(|failure| {
                        failure.get_or_insert_with(|| error.to_string());
                    });
                    Vec::new()
                });
            AcceptPaymentPolicy::Origins(
                services
                    .into_iter()
                    .chain([TEMPO_SERVICES.to_owned(), GATEWAY_ORIGIN.to_owned()])
                    .collect(),
            )
        })?;
        Ok(environment)
    }

    fn mcp(&self, wallet: &TempoAccountsProvider, url: &str) -> Result<McpServer> {
        let url: reqwest::Url = url.parse()?;
        Ok(
            McpServer::http(url.as_str()).payment_provider(Arc::new(McpPayments {
                wallet: self.sessions(wallet, &url)?,
                url,
                payments: self.clone(),
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
        let model = self.models.contains(&host);
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
    wallet: TempoAccountsProvider,
    url: reqwest::Url,
    payments: Payments,
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
            wallet: self.wallet.clone(),
            receipt: None,
            url: self.url.clone(),
            payments: self.payments.clone(),
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
    wallet: TempoAccountsProvider,
    pending: PendingPayment<TempoAccountsProvider>,
    payment: McpPayment<TempoAccountsProvider>,
    credential: Value,
    receipt: Option<Value>,
    url: reqwest::Url,
    payments: Payments,
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
            let funding = self.wallet.funding_currency(challenge);
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
                &self.payments.listeners,
                self.payments.ledger.update(|ledger| ledger.record(&charge)),
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

fn endpoint(
    clients: ModelClients,
    url: &str,
    args: &GatewayArgs,
) -> Result<(ModelClients, Option<Closing>)> {
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let url = url.trim_end_matches('/');
    let endpoint = match url.contains("://") {
        true => format!("{url}/v1"),
        false => format!("https://{url}/v1"),
    };
    let (models, chain) = endpoint_terms(&endpoint, args.tempo_network)?;
    let default = match chain {
        CHAIN_ID => USDC,
        TESTNET_CHAIN_ID => PATH_USD,
        other => {
            return Err(format!(
                "{endpoint} takes payment on chain {other}, neither Tempo mainnet nor testnet"
            )
            .into());
        }
    };
    let default: Address = default.parse()?;
    let wanted = args
        .tempo_currency
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let currency: Address = match wanted.as_str() {
        "" => default,
        "mach" => machine_token::token(chain).ok_or("MACH is not deployed on this network")?,
        "usdc.e" | "usdc" if chain == CHAIN_ID => USDC.parse()?,
        "ousd" if chain == CHAIN_ID => OUSD.parse()?,
        "pathusd" if chain != CHAIN_ID => PATH_USD.parse()?,
        address if address.starts_with("0x") => address.parse()?,
        other => {
            return Err(format!(
                "--tempo-currency must be mach, usdc.e or ousd (mainnet), pathusd (testnet) or a token address, not {other}"
            )
            .into());
        }
    };
    let deposit = args.tempo_deposit.unwrap_or(MACH);
    let host = reqwest::Url::parse(&endpoint)?
        .host_str()
        .unwrap_or_default()
        .to_owned();
    let payments = Payments::new(
        machine_token::token(chain).unwrap_or(Address::ZERO),
        vec![host.clone()],
    );
    let wallet = TempoAccountsProvider::from_default_store()
        .map_err(|error| format!("{error}; log in with `tempo wallet login` or /login tempo"))?
        .with_expected_chain_id(chain)
        .with_preferred_currency(currency)
        .with_session_prepaid_requests(PREPAID_REQUESTS)
        .with_session_events(payments.events.clone())
        .with_session_default_deposit(deposit)
        .with_session_top_up_amount(deposit);
    let mut layer = Mpp::new(
        &wallet,
        &endpoint,
        &payments,
        (format!("{endpoint}/responses"), models[0].as_str()),
    )?;
    let origin = reqwest::Url::parse(&endpoint)?
        .origin()
        .ascii_serialization();
    let origin = match currency == default {
        true => origin,
        false => format!("{origin}~{wanted}"),
    };
    let namespace = match session_slot(&origin, args.tempo_session.as_deref())?.as_str() {
        "0" => origin,
        session => format!("{origin}#{session}"),
    };
    let store = mpp::client::tempo::session::store::SqliteChannelStore::open(
        mpp::client::tempo::session::store::SqliteChannelStoreOptions {
            namespace,
            path: None,
            request_url: Some(endpoint.clone()),
        },
    )?;
    layer.wallet = layer.wallet.clone().with_session_store(Arc::new(store));
    let session = layer.wallet.session_for(&endpoint)?;
    payments.ledger.update(|ledger| {
        ledger.sessions.insert(host.clone(), session.clone());
    });
    let closing = (!args.tempo_keep_session).then(|| Closing {
        session,
        url: format!("{endpoint}/responses"),
    });
    let (notices, status) = (payments.clone(), payments.clone());
    let clients = clients
        .openai_endpoint(&endpoint, layer)
        .models(models)
        .notices(move || notices.notices())
        .status(move || status.status())
        .panel("/spend", "show or hide what models have spent", move || {
            payments.panel()
        });
    Ok((clients, closing))
}

pub(crate) struct Closing {
    session: TempoSessionProvider,
    url: String,
}

pub(crate) async fn close(closing: Closing) {
    let client = reqwest::Client::new();
    for _ in 0..CLOSE_ATTEMPTS {
        match closing.session.close(&client, &closing.url).await {
            Ok(Some(receipt)) => {
                return eprintln!(
                    "nanocode: closed the session cooperatively (receipt {})",
                    receipt.reference
                );
            }
            Ok(None) => return,
            Err(error) if error.to_string().contains(" 409 ") => {
                tokio::time::sleep(CLOSE_RETRY).await;
            }
            Err(error) => {
                return eprintln!(
                    "nanocode: closing the session: {error}; --tempo-keep-session keeps it open"
                );
            }
        }
    }
    eprintln!("nanocode: the session stayed busy; the next run closes it");
}

fn endpoint_terms(endpoint: &str, network: Option<TempoNetworkArg>) -> Result<(Vec<Model>, u64)> {
    let pinned = network.map(|network| match network {
        TempoNetworkArg::Mainnet => CHAIN_ID,
        TempoNetworkArg::Testnet => TESTNET_CHAIN_ID,
    });
    let client = reqwest::Client::new();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let models = served_models(&client, endpoint).await?;
            let chain = match pinned {
                Some(chain) => chain,
                None => challenge_chain(&client, endpoint, models[0]).await?,
            };
            Ok((models, chain))
        })
}

async fn challenge_chain(client: &reqwest::Client, endpoint: &str, model: Model) -> Result<u64> {
    let url = format!("{endpoint}/responses");
    let response = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "model": model.as_str() }).to_string())
        .send()
        .await
        .map_err(|error| format!("asking {url} for its payment terms: {error}"))?;
    let challenge = response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| mpp::parse_www_authenticate(value).ok())
        .ok_or_else(|| {
            format!(
                "{url} answered {} without a payment challenge",
                response.status()
            )
        })?;
    challenge.request.decode_value()?["methodDetails"]["chainId"]
        .as_u64()
        .ok_or_else(|| format!("{url}'s payment challenge names no chain").into())
}

async fn served_models(client: &reqwest::Client, endpoint: &str) -> Result<Vec<Model>> {
    let url = format!("{endpoint}/models");
    let body = async {
        client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await
    }
    .await
    .map_err(|error| format!("listing models at {url}: {error}"))?;
    let listing: Value = serde_json::from_slice(&body)?;
    let mut models = Vec::new();
    for entry in listing["data"]
        .as_array()
        .ok_or_else(|| format!("{url} returned no data list"))?
    {
        let model = entry["id"]
            .as_str()
            .ok_or_else(|| format!("{url} listed a model without an id"))?
            .parse::<Model>()?;
        if !models.contains(&model) {
            models.push(model);
        }
    }
    if models.is_empty() {
        return Err(format!("{url} lists no models").into());
    }
    Ok(models)
}

fn session_slot(origin: &str, named: Option<&str>) -> Result<String> {
    if let Some(name) = named.map(str::trim).filter(|name| !name.is_empty()) {
        return Ok(name.to_owned());
    }
    let dir = mpp::client::tempo::session::store::default_channel_database_path()?
        .with_file_name("mpp-sessions");
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

fn proxies(clients: ModelClients, args: &GatewayArgs) -> Result<ModelClients> {
    nanocode::codex::transport::install_default_rustls_crypto_provider();
    let deposit = args.tempo_deposit.unwrap_or(2 * MACH);
    let tool_deposit = args.tempo_tool_deposit.unwrap_or(MACH / 10);
    let machine = machine_token::token(CHAIN_ID).ok_or("MACH is not deployed on Tempo mainnet")?;
    let mut models = Vec::new();
    for url in [OPENAI, ANTHROPIC] {
        models.extend(reqwest::Url::parse(url)?.host_str().map(str::to_owned));
    }
    let payments = Payments::new(machine, models.clone());
    let wallet = TempoAccountsProvider::from_default_store()
        .map_err(|error| format!("{error}; log in with `tempo wallet login` or /login tempo"))?
        .with_expected_chain_id(CHAIN_ID)
        .with_preferred_currency(machine)
        .with_session_prepaid_requests(PREPAID_REQUESTS)
        .with_session_events(payments.events.clone());
    let tool_wallet = wallet
        .clone()
        .with_session_default_deposit(tool_deposit)
        .with_session_top_up_amount(tool_deposit);
    let tool_environment = payments.pay_tools(&tool_wallet, models)?;
    let mercator = payments.mcp(&wallet, MERCATOR)?;
    let gateway = payments.mcp(&tool_wallet, GATEWAY)?;
    let wallet = wallet
        .with_session_default_deposit(deposit)
        .with_session_top_up_amount(deposit);
    let openai = Mpp::new(
        &wallet,
        OPENAI,
        &payments,
        (format!("{OPENAI}/responses"), Model::Luna.as_str()),
    )?;
    let claude = Mpp::new(
        &wallet,
        ANTHROPIC,
        &payments,
        (ANTHROPIC.to_owned(), ClaudeModel::Haiku55.as_str()),
    )?;
    let warm = (openai.clone(), claude.clone());
    let (notices, status) = (payments.clone(), payments.clone());
    Ok(clients
        .openai_endpoint(OPENAI, openai)
        .claude_endpoint(ANTHROPIC, claude)
        .warm_up(move || {
            let (openai, claude) = warm.clone();
            async move {
                futures_util::future::join(openai.preconnect(), claude.preconnect()).await;
            }
        })
        .tool_environment(tool_environment)
        .mcp_server("mercator", mercator)
        .mcp_server("gateway", gateway)
        .notices(move || notices.notices())
        .status(move || status.status())
        .panel(
            "/spend",
            "show or hide what models and tools have spent",
            move || payments.panel(),
        ))
}

pub(crate) fn apply(
    clients: ModelClients,
    args: &GatewayArgs,
) -> Result<(ModelClients, Option<Closing>)> {
    if clients.supplies_openai() {
        return Err(
            "this binary supplies its own Codex client; --gateway tempo is unavailable".into(),
        );
    }
    match &args.gateway_base_url {
        Some(url) => endpoint(clients, url, args),
        None => proxies(clients, args).map(|clients| (clients, None)),
    }
}

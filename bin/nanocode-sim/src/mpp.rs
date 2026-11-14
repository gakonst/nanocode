use std::{cell::Cell, ops::Range, process::ExitCode, sync::Arc};

use bytes::Bytes;
use nanocode_http::{
    Method, Request, Response, StatusCode, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue, WWW_AUTHENTICATE},
};
use nanocode_mpp::{
    error::Fault,
    events::Events,
    flow::{Exchange, FlowError, Policy, send_with_exchange},
    protocol::{Challenge, Credential, Receipt},
    provider::{ManagementTransport, PaymentContext, PaymentEventFuture, PaymentProvider},
};
use nanocode_sync::Atom;
use nanocode_sys::base64::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use crate::{
    executor::{Clock, Ended, Executor, Task},
    rng::{FNV_OFFSET, Rng, fnv},
};

const RETRIES_MAX: u64 = 4;
const CHALLENGES_MAX: u64 = 3;
const AMOUNT_MAX: u64 = 1000;
const BODY_BYTES_MAX: u64 = 64;
const FREE_PPM: u64 = 100_000;
const POST_PPM: u64 = 300_000;
const FOREIGN_METHOD_PPM: u64 = 200_000;
const EXPIRED_PPM: u64 = 100_000;
const PROOF_PPM: u64 = 50_000;
const RECEIPT_PPM: u64 = 500_000;
const PAY_FAILS_PPM: u64 = 50_000;
const REUSE_ID_PPM: u64 = 100_000;
const BYTE_VALUES: u64 = 256;
const REALM: &str = "pay.sim";
const FOREIGN_REALM: &str = "other.sim";
const URL: &str = "https://pay.sim/resource";
const METHOD: &str = "tempo";
const FOREIGN_METHOD: &str = "card";
const INTENT: &str = "charge";
const EXPIRED: &str = "2001-01-01T00:00:00Z";
const RECEIPT_TIME: &str = "2026-11-14T00:00:00Z";
const RECEIPT_HEADER: &str = "payment-receipt";
const SCHEME: &str = "Payment ";
const TOOK_STATUSES: [u16; 2] = [200, 201];
const REFUSED_STATUSES: [u16; 3] = [400, 403, 409];
const SERVER_STATUSES: [u16; 2] = [500, 503];
const PAYMENT_REQUIRED: u16 = 402;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MppMutation {
    None,
    DropAbandon,
    ReceiptOnRefusal,
}

impl MppMutation {
    const ALL: [Self; 3] = [Self::None, Self::DropAbandon, Self::ReceiptOnRefusal];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mutation| mutation.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::DropAbandon => "drop_abandon",
            Self::ReceiptOnRefusal => "receipt_on_refusal",
        }
    }

    pub fn names() -> String {
        Self::ALL.map(Self::name).join("|")
    }
}

#[derive(Clone, Debug)]
struct Offer {
    id: String,
    method: &'static str,
    realm: &'static str,
    amount: u64,
    expired: bool,
}

impl Offer {
    fn payable(&self) -> bool {
        self.method == METHOD && !self.expired && !self.proof()
    }

    fn proof(&self) -> bool {
        self.amount == 0 && self.realm != REALM
    }

    fn request(&self) -> String {
        let request =
            json!({"amount": self.amount.to_string(), "currency": "usd", "nonce": self.id});
        URL_SAFE_NO_PAD.encode(request.to_string())
    }

    fn header(&self) -> String {
        let mut value = format!(
            "Payment id=\"{}\", realm=\"{}\", method=\"{}\", intent=\"{INTENT}\", request=\"{}\"",
            self.id,
            self.realm,
            self.method,
            self.request()
        );
        if self.expired {
            value.push_str(&format!(", expires=\"{EXPIRED}\""));
        }
        value
    }
}

#[derive(Clone, Debug)]
enum Fate {
    Took { status: u16, receipt: bool },
    Again { receipt: bool, offers: Vec<Offer> },
    Refused { status: u16, receipt: bool },
    Server { status: u16, receipt: bool },
    Network,
}

impl Fate {
    const fn settlement(&self, receipt_on_refusal: bool) -> Settled {
        match self {
            Self::Took { .. }
            | Self::Again { receipt: true, .. }
            | Self::Server { receipt: true, .. } => Settled::Commit,
            Self::Refused { receipt, .. } => {
                if *receipt && !receipt_on_refusal {
                    Settled::Commit
                } else {
                    Settled::Rollback
                }
            }
            Self::Again { receipt: false, .. } => Settled::Rollback,
            Self::Server { receipt: false, .. } | Self::Network => Settled::Abandon,
        }
    }
}

struct Script {
    post: bool,
    body: Vec<u8>,
    retries: usize,
    first: Option<Vec<Offer>>,
    fates: Vec<Fate>,
    pay_fails: Option<usize>,
}

fn offers(rng: &mut Rng, next: &mut u64, last: &[Offer]) -> Vec<Offer> {
    (0..rng.range(1, CHALLENGES_MAX))
        .map(|_| {
            if let Some(reused) = (!last.is_empty() && rng.chance(REUSE_ID_PPM))
                .then(|| last[rng.pick(last.len())].clone())
            {
                return reused;
            }
            *next += 1;
            let proof = rng.chance(PROOF_PPM);
            Offer {
                id: format!("c{next}"),
                method: if rng.chance(FOREIGN_METHOD_PPM) {
                    FOREIGN_METHOD
                } else {
                    METHOD
                },
                realm: if proof { FOREIGN_REALM } else { REALM },
                amount: if proof { 0 } else { rng.range(1, AMOUNT_MAX) },
                expired: rng.chance(EXPIRED_PPM),
            }
        })
        .collect()
}

impl Script {
    fn of(seed: u64) -> Self {
        let mut rng = Rng::derive(seed, "mpp-script");
        let mut next = 0;
        let post = rng.chance(POST_PPM);
        let body = if post {
            let length = rng.range(0, BODY_BYTES_MAX);
            (0..length)
                .map(|_| u8::try_from(rng.below(BYTE_VALUES)).expect("a byte value fits in u8"))
                .collect()
        } else {
            Vec::new()
        };
        let retries = usize::try_from(rng.range(1, RETRIES_MAX)).expect("a small count fits");
        let first = (!rng.chance(FREE_PPM)).then(|| offers(&mut rng, &mut next, &[]));
        let mut last = first.clone().unwrap_or_default();
        let fates = (0..retries)
            .map(|_| {
                let receipt = rng.chance(RECEIPT_PPM);
                match rng.below(5) {
                    0 => Fate::Took {
                        status: TOOK_STATUSES[rng.pick(TOOK_STATUSES.len())],
                        receipt,
                    },
                    1 => {
                        let fresh = offers(&mut rng, &mut next, &last);
                        last.clone_from(&fresh);
                        Fate::Again {
                            receipt,
                            offers: fresh,
                        }
                    }
                    2 => Fate::Refused {
                        status: REFUSED_STATUSES[rng.pick(REFUSED_STATUSES.len())],
                        receipt,
                    },
                    3 => Fate::Server {
                        status: SERVER_STATUSES[rng.pick(SERVER_STATUSES.len())],
                        receipt,
                    },
                    _ => Fate::Network,
                }
            })
            .collect();
        let pay_fails = rng.chance(PAY_FAILS_PPM).then(|| rng.pick(retries));
        Self {
            post,
            body,
            retries,
            first,
            fates,
            pay_fails,
        }
    }

    fn describe(&self) -> String {
        let show = |offers: &[Offer]| {
            offers
                .iter()
                .map(|offer| {
                    format!(
                        "{}:{}:{}{}{}",
                        offer.id,
                        offer.method,
                        offer.amount,
                        if offer.expired { ":expired" } else { "" },
                        if offer.proof() { ":proof" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        let fates = self
            .fates
            .iter()
            .map(|fate| match fate {
                Fate::Again { receipt, offers } => {
                    format!("Again{{receipt:{receipt},[{}]}}", show(offers))
                }
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "scenario=mpp {} retries={} first={} pay_fails={:?} fates=[{fates}]",
            if self.post { "POST" } else { "GET" },
            self.retries,
            self.first
                .as_deref()
                .map_or_else(|| "free".to_owned(), show),
            self.pay_fails
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    Commit,
    Rollback,
    Invalidate,
    Abandon,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    Paid(String),
    Settled(String, Settled),
}

#[derive(Clone)]
struct SimProvider {
    ledger: Arc<Atom<Vec<Entry>>>,
    pays: Arc<Atom<usize>>,
    pay_fails: Option<usize>,
    drop_abandon: bool,
}

fn credential_id(credential: &Credential) -> String {
    serde_json::to_value(credential)
        .ok()
        .and_then(|value| value["challenge"]["id"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn nonce(challenge: &Challenge) -> String {
    challenge
        .request
        .decode::<Value>()
        .ok()
        .and_then(|value| value["nonce"].as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl SimProvider {
    fn settle(&self, credential: &Credential, settled: Settled) {
        let id = credential_id(credential);
        self.ledger
            .update(|ledger| ledger.push(Entry::Settled(id.clone(), settled)));
    }
}

impl PaymentProvider for SimProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == METHOD && intent == INTENT
    }

    fn select_challenge<'a>(&self, challenges: &[&'a Challenge]) -> Option<&'a Challenge> {
        challenges.first().copied()
    }

    async fn pay_with_context(
        &self,
        challenge: &Challenge,
        _context: PaymentContext,
    ) -> Result<Credential, Fault> {
        let attempt = self.pays.update(|pays| {
            *pays += 1;
            *pays - 1
        });
        if self.pay_fails == Some(attempt) {
            return Err(Fault::InsufficientBalance(None));
        }
        let id = nonce(challenge);
        let offer_json = json!({
            "challenge": {
                "id": id,
                "realm": REALM,
                "method": METHOD,
                "intent": challenge.intent,
                "request": URL_SAFE_NO_PAD.encode(challenge.request.decode::<Value>().unwrap_or_default().to_string()),
            },
            "payload": {"type": "transaction", "signature": format!("0xsim{id}")},
        });
        let credential: Credential = serde_json::from_value(offer_json)
            .map_err(|error| Fault::Config(format!("sim credential: {error}")))?;
        self.ledger
            .update(|ledger| ledger.push(Entry::Paid(id.clone())));
        Ok(credential)
    }

    async fn pay_with_management(
        &self,
        _challenge: &Challenge,
        _context: PaymentContext,
        _management: &dyn ManagementTransport,
    ) -> Result<Credential, Fault> {
        Err(Fault::Unsupported(
            "the simulator pays only by credential".to_owned(),
        ))
    }

    async fn commit_payment(
        &self,
        _challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        self.settle(credential, Settled::Commit);
        Ok(())
    }

    async fn rollback_payment(
        &self,
        _challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        self.settle(credential, Settled::Rollback);
        Ok(())
    }

    async fn invalidate_payment(
        &self,
        _challenge: &Challenge,
        credential: &Credential,
    ) -> Result<(), Fault> {
        self.settle(credential, Settled::Invalidate);
        Ok(())
    }

    fn abandon_payment(&self, _challenge: &Challenge, credential: &Credential) {
        if !self.drop_abandon {
            self.settle(credential, Settled::Abandon);
        }
    }

    async fn prepaid_credential(
        &self,
        _context: PaymentContext,
    ) -> Option<(Challenge, Credential)> {
        None
    }

    fn record_receipt(
        &self,
        _context: &PaymentContext,
        challenge: &Challenge,
        _receipt: &Receipt,
    ) -> Option<u128> {
        challenge.amount()
    }

    fn funding_currency(&self, _challenge: &Challenge) -> Option<String> {
        None
    }

    fn stream_event(
        &self,
        _client: &nanocode_http::Client,
        _url: &str,
        _name: &str,
        _data: &str,
    ) -> Option<PaymentEventFuture> {
        None
    }

    fn accept_payment_header(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Debug)]
struct Sent {
    credential: Option<String>,
    body: Vec<u8>,
    answered: Option<u16>,
}

struct SimExchange<'a> {
    script: &'a Script,
    sent: Vec<Sent>,
    paid: usize,
    receipt_on_refusal: bool,
}

fn receipt(id: &str) -> String {
    let receipt = json!({"status": "success", "method": METHOD, "timestamp": RECEIPT_TIME, "reference": format!("0xref{id}")});
    URL_SAFE_NO_PAD.encode(receipt.to_string())
}

fn response(url: &Url, status: u16, offers: &[Offer], receipt_for: Option<&str>) -> Response {
    let mut headers = HeaderMap::new();
    for offer in offers {
        if let Ok(value) = HeaderValue::from_str(&offer.header()) {
            headers.append(WWW_AUTHENTICATE, value);
        }
    }
    if let Some(id) = receipt_for
        && let Ok(value) = HeaderValue::from_str(&receipt(id))
    {
        headers.insert(RECEIPT_HEADER, value);
    }
    let status = StatusCode::from_u16(status).expect("the scripted statuses are valid");
    let body = nanocode_sync::stream::iter(Vec::<nanocode_http::Result<Bytes>>::new());
    Response::from_stream(status, headers, url.clone(), body)
}

impl Exchange for SimExchange<'_> {
    type Error = String;

    fn http_client(&self) -> Option<nanocode_http::Client> {
        None
    }

    async fn send(&mut self, request: Request) -> Result<Response, String> {
        let credential = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix(SCHEME))
            .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
            .and_then(|json| serde_json::from_slice::<Value>(&json).ok())
            .map(|value| {
                value["challenge"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            });
        let body = request
            .body()
            .and_then(nanocode_http::Body::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        let url = request.url().clone();
        let (answered, reply) = match &credential {
            None => match &self.script.first {
                None => (Some(200), Ok(response(&url, 200, &[], None))),
                Some(offers) => (
                    Some(PAYMENT_REQUIRED),
                    Ok(response(&url, PAYMENT_REQUIRED, offers, None)),
                ),
            },
            Some(id) => {
                let fate = self
                    .script
                    .fates
                    .get(self.paid)
                    .cloned()
                    .unwrap_or(Fate::Network);
                self.paid += 1;
                let with = |receipt: bool| receipt.then_some(id.as_str());
                match fate {
                    Fate::Took { status, receipt } => {
                        (Some(status), Ok(response(&url, status, &[], with(receipt))))
                    }
                    Fate::Again { receipt, offers } => (
                        Some(PAYMENT_REQUIRED),
                        Ok(response(&url, PAYMENT_REQUIRED, &offers, with(receipt))),
                    ),
                    Fate::Refused { status, receipt } => {
                        let receipt = receipt || self.receipt_on_refusal;
                        (Some(status), Ok(response(&url, status, &[], with(receipt))))
                    }
                    Fate::Server { status, receipt } => {
                        (Some(status), Ok(response(&url, status, &[], with(receipt))))
                    }
                    Fate::Network => (
                        None,
                        Err("sim: connection reset after the request was sent".to_owned()),
                    ),
                }
            }
        };
        self.sent.push(Sent {
            credential,
            body,
            answered,
        });
        reply
    }
}

fn judge(
    script: &Script,
    sent: &[Sent],
    ledger: &[Entry],
    result: &Result<u16, String>,
    mutation: MppMutation,
) -> Result<(), String> {
    let paid: Vec<&str> = ledger
        .iter()
        .filter_map(|entry| match entry {
            Entry::Paid(id) => Some(id.as_str()),
            Entry::Settled(..) => None,
        })
        .collect();
    if paid.len() > script.retries {
        return Err(format!(
            "the client paid {} times where it may retry {} times",
            paid.len(),
            script.retries
        ));
    }
    let all_offers: Vec<&Offer> = script
        .first
        .iter()
        .flatten()
        .chain(script.fates.iter().flat_map(|fate| match fate {
            Fate::Again { offers, .. } => offers.as_slice(),
            _ => &[],
        }))
        .collect();
    for (index, id) in paid.iter().enumerate() {
        if paid[..index].contains(id) {
            return Err(format!("the client paid challenge {id} twice"));
        }
        let Some(offer) = all_offers.iter().find(|offer| offer.id == *id) else {
            return Err(format!("the client paid {id}, which no server offered"));
        };
        if !offer.payable() {
            return Err(format!(
                "the client paid challenge {id}, which it must not pay (method {}, expired {}, proof {})",
                offer.method,
                offer.expired,
                offer.proof()
            ));
        }
        let settlements: Vec<Settled> = ledger
            .iter()
            .filter_map(|entry| match entry {
                Entry::Settled(settled_id, settled) if settled_id == id => Some(*settled),
                _ => None,
            })
            .collect();
        if settlements.len() != 1 {
            return Err(format!(
                "payment for {id} was settled {} times ({settlements:?}) where it must be settled once",
                settlements.len()
            ));
        }
        let presented: Vec<(usize, &Sent)> = sent
            .iter()
            .filter(|sent| sent.credential.as_deref() == Some(*id))
            .enumerate()
            .collect();
        if presented.len() != 1 {
            return Err(format!(
                "the credential for {id} was presented {} times where it was paid once",
                presented.len()
            ));
        }
        let attempt = sent
            .iter()
            .filter(|sent| sent.credential.is_some())
            .position(|sent| sent.credential.as_deref() == Some(*id))
            .expect("a presented credential has a position");
        let fate = script.fates.get(attempt).cloned().unwrap_or(Fate::Network);
        let wanted = fate.settlement(mutation == MppMutation::ReceiptOnRefusal);
        if settlements[0] != wanted {
            return Err(format!(
                "payment for {id} was settled {:?} after the server answered {fate:?}, where it must be {wanted:?}",
                settlements[0]
            ));
        }
    }
    for entry in ledger {
        if let Entry::Settled(id, settled) = entry
            && !paid.contains(&id.as_str())
        {
            return Err(format!(
                "the client settled {settled:?} {id}, which it never paid"
            ));
        }
    }
    for sent in sent {
        if sent.body != script.body {
            return Err(format!(
                "a request carried a body of {} bytes where the caller sent {}",
                sent.body.len(),
                script.body.len()
            ));
        }
    }
    if let Ok(status) = result
        && sent.last().and_then(|sent| sent.answered) != Some(*status)
    {
        return Err(format!(
            "the client returned status {status} where the last answer was {:?}",
            sent.last().and_then(|sent| sent.answered)
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MppStats {
    pub seeds: u64,
    pub pays: u64,
    pub commits: u64,
    pub rollbacks: u64,
    pub abandons: u64,
    pub errors: u64,
}

impl MppStats {
    pub const fn add(&mut self, other: &Self) {
        self.seeds += other.seeds;
        self.pays += other.pays;
        self.commits += other.commits;
        self.rollbacks += other.rollbacks;
        self.abandons += other.abandons;
        self.errors += other.errors;
    }
}

pub struct MppOutcome {
    pub digest: u64,
    pub wrong: Option<String>,
    pub stats: MppStats,
    pub shape: String,
}

fn simulate(seed: u64, mutation: MppMutation) -> MppOutcome {
    let script = Script::of(seed);
    let shape = script.describe();
    let ledger = Arc::new(Atom::new(Vec::new()));
    let provider = SimProvider {
        ledger: Arc::clone(&ledger),
        pays: Arc::new(Atom::new(0)),
        pay_fails: script.pay_fails,
        drop_abandon: mutation == MppMutation::DropAbandon,
    };
    let mut exchange = SimExchange {
        script: &script,
        sent: Vec::new(),
        paid: 0,
        receipt_on_refusal: mutation == MppMutation::ReceiptOnRefusal,
    };
    let url = Url::parse(URL).expect("the simulated URL parses");
    let method = if script.post {
        Method::POST
    } else {
        Method::GET
    };
    let mut request = Request::new(method, url);
    if script.post {
        *request.body_mut() = Some(nanocode_http::Body::from(script.body.clone()));
    }
    let events = Events::default();
    let outcome = Cell::new(None);
    let flow = async {
        let result = send_with_exchange(
            &mut exchange,
            request,
            &provider,
            &Policy::Always,
            &events,
            script.retries,
        )
        .await;
        outcome.set(Some(match result {
            Ok(response) => Ok(response.status().as_u16()),
            Err(FlowError::Send(error)) => Err(format!("send: {error}")),
            Err(FlowError::Payment(error)) => Err(format!("payment: {error}")),
        }));
    };
    let clock = Clock::new();
    crate::host::enter(seed, &clock);
    let tasks: Vec<Task<'_>> = vec![Box::pin(flow)];
    let ended = Executor::new(Arc::clone(&clock), Rng::derive(seed, "scheduler")).run(tasks, None);
    let result = match (ended, outcome.take()) {
        (Ended::Stuck, _) => Err("stuck: every task waits and no timer is set".to_owned()),
        (_, Some(result)) => result,
        (_, None) => Err("the payment flow never finished".to_owned()),
    };
    let entries = ledger.get().as_ref().clone();
    let wrong = judge(&script, &exchange.sent, &entries, &result, mutation).err();
    let count = |wanted: Settled| {
        u64::try_from(
            entries
                .iter()
                .filter(|entry| matches!(entry, Entry::Settled(_, settled) if *settled == wanted))
                .count(),
        )
        .expect("fits")
    };
    let mut digest = fnv(FNV_OFFSET, shape.as_bytes());
    digest = fnv(digest, format!("{entries:?} {result:?}").as_bytes());
    MppOutcome {
        digest,
        wrong,
        stats: MppStats {
            seeds: 1,
            pays: u64::try_from(
                entries
                    .iter()
                    .filter(|entry| matches!(entry, Entry::Paid(_)))
                    .count(),
            )
            .expect("fits"),
            commits: count(Settled::Commit),
            rollbacks: count(Settled::Rollback),
            abandons: count(Settled::Abandon),
            errors: u64::from(result.is_err()),
        },
        shape,
    }
}

pub fn run_seed(seed: u64, mutation: MppMutation) -> MppOutcome {
    std::panic::catch_unwind(|| simulate(seed, mutation)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
            .unwrap_or_default();
        MppOutcome {
            digest: 0,
            wrong: Some(format!("panic: {message}")),
            stats: MppStats::default(),
            shape: String::new(),
        }
    })
}

pub fn judge_seed(
    seed: u64,
    mutation: MppMutation,
    twice: bool,
    verbose: bool,
    totals: &mut MppStats,
) -> Option<String> {
    let first = run_seed(seed, mutation);
    totals.add(&first.stats);
    if verbose {
        println!("seed {seed} digest {:016x} {}", first.digest, first.shape);
    }
    if first.wrong.is_some() || !twice {
        return first
            .wrong
            .map(|message| format!("{} :: {message}", first.shape));
    }
    let second = run_seed(seed, mutation);
    (second.digest != first.digest || second.wrong.is_some()).then(|| {
        format!(
            "{} :: nondeterminism: trace digest {:016x} then {:016x}",
            first.shape, first.digest, second.digest
        )
    })
}

pub fn replay(seed: u64, mutation: MppMutation) -> String {
    let mut command = format!("nanocode-sim --scenario mpp --seed {seed}");
    if mutation != MppMutation::None {
        command.push_str(" --mutation ");
        command.push_str(mutation.name());
    }
    command
}

pub fn report(
    seeds: &Range<u64>,
    passed: u64,
    failed: u64,
    twice: bool,
    mutation: MppMutation,
    totals: &MppStats,
) -> ExitCode {
    println!(
        "RESULT seeds={}..{} scenario=mpp mutation={} passed={passed} failed={failed} twice={twice} stats={totals:?}",
        seeds.start,
        seeds.end,
        mutation.name()
    );
    match mutation {
        MppMutation::None if failed == 0 => ExitCode::SUCCESS,
        MppMutation::None => ExitCode::FAILURE,
        _ if failed > 0 => {
            println!("MUTATION {} caught by {failed} seeds", mutation.name());
            ExitCode::SUCCESS
        }
        _ => {
            println!("MUTATION {} NOT caught", mutation.name());
            ExitCode::FAILURE
        }
    }
}

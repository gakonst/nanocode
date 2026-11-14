use std::{collections::VecDeque, sync::Arc, time::Duration};

use nanocode::claude::transport::{
    HttpError, HttpFuture, HttpRequest, HttpResponse, HttpTransport,
};
use nanocode_sync::Slot;

use crate::{
    rng::{PPM, Rng},
    httpd::WireMutation,
    server::ModelServer,
    world::World,
};

pub const CALM_PPM: u64 = 300_000;
pub const UNSENT_PPM_MAX: u64 = 60_000;
pub const LOST_PPM_MAX: u64 = 40_000;
pub const STATUS_PPM_MAX: u64 = 80_000;
pub const TRUNCATE_PPM_MAX: u64 = 60_000;
pub const DUPLICATE_PPM_MAX: u64 = 40_000;
pub const SLOW_PPM_MAX: u64 = 50_000;
pub const LATENCY_MS_MIN: u64 = 20;
pub const LATENCY_MS_MAX: u64 = 3_000;
pub const SLOW_HEADERS_MS_MAX: u64 = 180_000;
pub const CHUNK_GAP_MS_MAX: u64 = 500;
pub const CHUNKS_MAX: u64 = 6;
pub const RETRY_AFTER_MS_MAX: u64 = 5_000;
const STATUSES: [u16; 3] = [429, 500, 529];

#[derive(Clone, Copy, Debug)]
pub struct NetworkFaults {
    pub unsent_ppm: u64,
    pub lost_ppm: u64,
    pub status_ppm: u64,
    pub truncate_ppm: u64,
    pub duplicate_ppm: u64,
    pub slow_ppm: u64,
    pub latency_ms_max: u64,
}

impl NetworkFaults {
    pub fn of(seed: u64, calm: bool) -> Self {
        let mut rng = Rng::derive(seed, "network-faults");
        let latency_ms_max = rng.range(LATENCY_MS_MIN, LATENCY_MS_MAX);
        if calm || rng.chance(CALM_PPM) {
            return Self {
                unsent_ppm: 0,
                lost_ppm: 0,
                status_ppm: 0,
                truncate_ppm: 0,
                duplicate_ppm: 0,
                slow_ppm: 0,
                latency_ms_max,
            };
        }
        Self {
            unsent_ppm: rng.range(0, UNSENT_PPM_MAX),
            lost_ppm: rng.range(0, LOST_PPM_MAX),
            status_ppm: rng.range(0, STATUS_PPM_MAX),
            truncate_ppm: rng.range(0, TRUNCATE_PPM_MAX),
            duplicate_ppm: rng.range(0, DUPLICATE_PPM_MAX),
            slow_ppm: rng.range(0, SLOW_PPM_MAX),
            latency_ms_max,
        }
    }

    pub const fn calm(&self) -> bool {
        self.unsent_ppm == 0
            && self.lost_ppm == 0
            && self.status_ppm == 0
            && self.truncate_ppm == 0
            && self.duplicate_ppm == 0
            && self.slow_ppm == 0
    }
}

enum Fate {
    Unsent,
    Lost,
    Status(u16, u64),
    Delivered {
        duplicate: bool,
        headers_ms: u64,
        chunks: Vec<(u64, u64)>,
        truncated: Option<bool>,
    },
}

pub struct Network {
    server: ModelServer,
    world: Arc<World>,
    faults: NetworkFaults,
    rng: Slot<Rng>,
}

fn transport(detail: &str, unsent: bool) -> HttpError {
    HttpError::Transport {
        detail: detail.to_owned(),
        retryable: true,
        timeout: false,
        unsent,
    }
}

impl Network {
    pub fn new(seed: u64, world: Arc<World>, faults: NetworkFaults, mutation: WireMutation) -> Self {
        Self {
            server: ModelServer::new(seed, Arc::clone(&world), mutation),
            world,
            faults,
            rng: Slot::new(Some(Rng::derive(seed, "network"))),
        }
    }

    fn draw<R>(&self, use_rng: impl FnOnce(&mut Rng) -> R) -> R {
        let mut rng = self
            .rng
            .take()
            .expect("the simulator polls one task at a time, so the network is never taken twice");
        let result = use_rng(&mut rng);
        assert!(self.rng.put(rng).is_none(), "the network has one home");
        result
    }

    fn fate(&self) -> Fate {
        let faults = self.faults;
        self.draw(|rng| {
            if rng.chance(faults.unsent_ppm) {
                return Fate::Unsent;
            }
            if rng.chance(faults.lost_ppm) {
                return Fate::Lost;
            }
            if rng.chance(faults.status_ppm) {
                let status = STATUSES[rng.pick(STATUSES.len())];
                return Fate::Status(status, rng.range(0, RETRY_AFTER_MS_MAX));
            }
            let headers_ms = if rng.chance(faults.slow_ppm) {
                rng.range(faults.latency_ms_max, SLOW_HEADERS_MS_MAX)
            } else {
                rng.range(0, faults.latency_ms_max)
            };
            let count = rng.range(1, CHUNKS_MAX);
            let mut cuts: Vec<u64> = (1..count).map(|_| rng.below(PPM)).collect();
            cuts.push(PPM);
            cuts.sort_unstable();
            let chunks = cuts
                .into_iter()
                .map(|cut| (rng.range(0, CHUNK_GAP_MS_MAX), cut))
                .collect();
            let truncated = rng
                .chance(faults.truncate_ppm)
                .then(|| rng.chance(PPM / 2));
            Fate::Delivered {
                duplicate: rng.chance(faults.duplicate_ppm),
                headers_ms,
                chunks,
                truncated,
            }
        })
    }

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let fate = self.fate();
        let connect_ms = self.draw(|rng| rng.range(0, self.faults.latency_ms_max));
        nanocode_platform::sleep(Duration::from_millis(connect_ms)).await;
        match fate {
            Fate::Unsent => {
                self.world.count(|counts| counts.faults += 1);
                self.world.trace("net unsent");
                return Err(transport("sim: connection refused", true));
            }
            Fate::Lost => {
                self.world.count(|counts| counts.faults += 1);
                let reply = self.server.respond(&request.body);
                self.world.trace(&format!(
                    "net lost response to {}",
                    reply.as_ref().map_or("?", |reply| reply.label.as_str())
                ));
                return Err(transport("sim: connection reset after the request", false));
            }
            Fate::Status(status, retry_after_ms) => {
                self.world.count(|counts| counts.faults += 1);
                self.world.trace(&format!("net status {status}"));
                let mut headers = http::HeaderMap::new();
                headers.insert(
                    "retry-after-ms",
                    http::HeaderValue::from(retry_after_ms),
                );
                headers.insert(
                    http::header::CONTENT_TYPE,
                    http::HeaderValue::from_static("application/json"),
                );
                let body = format!(
                    "{{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"sim status {status}\"}}}}"
                );
                return Ok(HttpResponse::new(
                    http::StatusCode::from_u16(status).expect("a simulated status is valid"),
                    headers,
                    Box::pin(nanocode_sync::stream::once(
                        async move { Ok(body.into_bytes()) },
                    )),
                ));
            }
            Fate::Delivered { .. } => {}
        }
        let Fate::Delivered {
            duplicate,
            headers_ms,
            chunks,
            truncated,
        } = fate
        else {
            unreachable!("the other fates returned above");
        };
        if duplicate {
            self.world.count(|counts| counts.faults += 1);
            self.world.trace("net duplicate delivery");
            drop(self.server.respond(&request.body));
        }
        let reply = self
            .server
            .respond(&request.body)
            .map_err(|error| HttpError::Transport {
                detail: format!("sim: the model refused the request: {error}"),
                retryable: false,
                timeout: false,
                unsent: false,
            })?;
        nanocode_platform::sleep(Duration::from_millis(headers_ms)).await;
        let bytes = reply.body.into_bytes();
        let mut parts = VecDeque::new();
        let mut from = 0;
        let kept = match truncated {
            Some(_) => chunks.len().saturating_sub(1).max(1),
            None => chunks.len(),
        };
        let length = u64::try_from(bytes.len()).expect("a reply length fits in u64");
        for (gap_ms, cut_ppm) in chunks.into_iter().take(kept) {
            let cut_ppm = match truncated {
                Some(_) if cut_ppm == PPM => PPM / 2,
                _ => cut_ppm,
            };
            let cut = usize::try_from(length * cut_ppm / PPM).expect("a cut inside a reply fits in usize");
            if cut > from {
                parts.push_back((gap_ms, Ok(bytes[from..cut].to_vec())));
                from = cut;
            }
        }
        if let Some(with_error) = truncated {
            self.world.count(|counts| counts.faults += 1);
            self.world.trace(&format!("net truncated {} at {from} of {}", reply.label, bytes.len()));
            if with_error {
                parts.push_back((0, Err(transport("sim: stream reset", false))));
            }
        }
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/event-stream"),
        );
        headers.insert("request-id", http::HeaderValue::from_static("req_sim"));
        let body = nanocode_sync::stream::unfold(parts, |mut parts| async move {
            let (gap_ms, part) = parts.pop_front()?;
            nanocode_platform::sleep(Duration::from_millis(gap_ms)).await;
            Some((part, parts))
        });
        Ok(HttpResponse::new(http::StatusCode::OK, headers, Box::pin(body)))
    }
}

pub struct Transport(pub Arc<Network>);

impl HttpTransport for Transport {
    fn post(&self, request: HttpRequest) -> HttpFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(self.0.send(request))
    }
}

use crate::egress::{
    authority::Fixed,
    lookup::Caching,
    proxy::{Identity, Limits, Log, Proxy, Reach, Rules, Texts},
    redact::Injecting,
    tls::Tls,
    wire::{Credential, Grant, Secret},
};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpListener, UnixListener},
    task::JoinHandle,
};

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

pub struct Allowlist {
    pub hosts: Vec<String>,
    pub private_hosts: Vec<String>,
    pub secrets: Vec<Secret>,
    pub tls: Arc<Tls>,
    pub tailnet: Option<SocketAddr>,
    pub refusal: fn(&str) -> String,
    pub limits: Limits,
}

pub struct Egress {
    port: u16,
    socket: Option<PathBuf>,
    log: Arc<Log>,
    task: JoinHandle<()>,
}

impl Drop for Egress {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(socket) = &self.socket {
            std::fs::remove_file(socket).ok();
        }
    }
}

fn private(host: &str) -> String {
    format!(
        "{host} resolves only to private, loopback or link-local addresses, which this proxy does not reach."
    )
}

fn proxy(allowlist: Allowlist, log: &Arc<Log>) -> Arc<Proxy<Fixed>> {
    let Allowlist {
        hosts,
        private_hosts,
        mut secrets,
        tls,
        tailnet,
        refusal,
        limits,
    } = allowlist;
    secrets.retain(|secret| secret.hosts.iter().any(|host| secret.sets(host)));
    let rules = Rules {
        caching: Caching {
            granted: Duration::ZERO,
            refused: Duration::ZERO,
            recheck: Duration::ZERO,
            gather: Duration::ZERO,
            most: 0,
            flight: Duration::ZERO,
        },
        reporting: None,
        identity: Identity::Fixed(Credential {
            name: "sandbox".into(),
            token: String::new(),
        }),
        reach: Reach::Listed {
            hosts,
            private_hosts,
            tailnet,
        },
        passthrough: Vec::new(),
        injecting: Injecting::Set,
        intercept_port: None,
        texts: Texts {
            client: "sandbox",
            needs_credential: "",
            realm: "",
            unreachable: "",
            stale: "",
            private,
            refused: refusal,
            compressed: "The egress proxy refused a compressed or upgraded response to a request it added a secret to.",
        },
        limits,
    };
    let fixed = Arc::new(Fixed(Grant { secrets }));
    Arc::new(Proxy::new(fixed, tls, rules).logged(Arc::clone(log)))
}

impl Egress {
    pub async fn tcp(allowlist: Allowlist) -> io::Result<Self> {
        let listener = Arc::new(TcpListener::bind(("127.0.0.1", 0)).await?);
        let port = listener.local_addr()?.port();
        let log = Arc::default();
        let proxy = proxy(allowlist, &log);
        let task = tokio::spawn(proxy.accept(move || {
            let listener = Arc::clone(&listener);
            async move {
                let (stream, _) = listener.accept().await.ok()?;
                drop(stream.set_nodelay(true));
                Some((stream, LOCAL))
            }
        }));
        Ok(Self {
            port,
            socket: None,
            log,
            task,
        })
    }

    pub fn unix(path: &Path, allowlist: Allowlist) -> io::Result<Self> {
        std::fs::remove_file(path).ok();
        let listener = Arc::new(UnixListener::bind(path)?);
        let log = Arc::default();
        let proxy = proxy(allowlist, &log);
        let task = tokio::spawn(proxy.accept(move || {
            let listener = Arc::clone(&listener);
            async move { Some((listener.accept().await.ok()?.0, LOCAL)) }
        }));
        Ok(Self {
            port: 0,
            socket: Some(path.to_owned()),
            log,
            task,
        })
    }

    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn denied(&self) -> Vec<String> {
        self.log.denied.get().as_ref().clone()
    }

    #[must_use]
    pub fn unreachable(&self) -> Vec<String> {
        self.log.unreachable.get().as_ref().clone()
    }
}

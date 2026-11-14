use crate::egress::{
    certificate::{self, ExtendedKeyUsagePurpose, Issuer, KeyPair, Validity},
    wire::Ca,
};
use nanocode_sync::Atom;
use rustls::{
    ClientConfig, RootCertStore, ServerConfig,
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, pem::PemObject},
};
use std::{
    collections::HashMap,
    error::Error,
    fmt, io,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

#[derive(Clone)]
pub struct Leaves {
    pub lifetime: Option<Lifetime>,
    pub cached: usize,
    pub usages: Vec<ExtendedKeyUsagePurpose>,
}

impl Default for Leaves {
    fn default() -> Self {
        Self {
            lifetime: None,
            cached: usize::MAX,
            usages: vec![ExtendedKeyUsagePurpose::ServerAuth],
        }
    }
}

#[derive(Clone, Copy)]
pub struct Lifetime {
    pub backdated: Duration,
    pub valid: Duration,
    pub renewed: Duration,
}

type Minted = HashMap<String, (Instant, Arc<ServerConfig>)>;

pub struct Tls {
    issuer: Issuer,
    ca_der: CertificateDer<'static>,
    ca_pem: String,
    upstream: Arc<ClientConfig>,
    leaves: Leaves,
    minted: Atom<Minted>,
}

#[derive(Debug)]
pub struct BadCa {
    what: &'static str,
    cause: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for BadCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.what)
    }
}

impl Error for BadCa {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(nanocode_tls::provider())
}

fn other(error: impl Into<Box<dyn Error + Send + Sync>>) -> io::Error {
    io::Error::other(error)
}

impl Tls {
    #[cfg(all(unix, not(target_os = "android"), not(target_vendor = "apple")))]
    pub fn new(name: &str) -> io::Result<Self> {
        let roots = rustls_platform_verifier::system_roots();
        if roots.is_empty() {
            return Err(other("no trusted root certificates found"));
        }
        Self::with_roots(name, RootCertStore::clone(&roots))
    }

    #[cfg(not(all(unix, not(target_os = "android"), not(target_vendor = "apple"))))]
    pub fn new(name: &str) -> io::Result<Self> {
        let found = nanocode_platform::trust::load_native_certificates();
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(found.certificates);
        if added == 0 {
            return Err(other(format!(
                "no trusted root certificates found ({} errors loading them)",
                found.errors.len()
            )));
        }
        Self::with_roots(name, roots)
    }

    pub fn with_roots(name: &str, roots: RootCertStore) -> io::Result<Self> {
        nanocode_tls::cpu::check()?;
        let (issuer, der) = Issuer::self_signed(name)?;
        let pem = certificate::pem(&der);
        Self::issuing(
            issuer,
            CertificateDer::from(der),
            pem,
            roots,
            Leaves::default(),
        )
    }

    pub fn from_ca(ca: &Ca, roots: RootCertStore, leaves: Leaves) -> Result<Self, BadCa> {
        nanocode_tls::cpu::check().map_err(|cause| BadCa {
            what: "this CPU",
            cause: Box::new(cause),
        })?;
        let key = KeyPair::from_pem(&ca.key).map_err(|cause| BadCa {
            what: "the egress CA's key",
            cause: Box::new(cause),
        })?;
        let bad = |cause: io::Error| BadCa {
            what: "the egress CA",
            cause: Box::new(cause),
        };
        let ca_der = CertificateDer::from_pem_slice(ca.cert.as_bytes())
            .map_err(|cause| bad(other(cause)))?;
        let issuer = Issuer::from_certificate(&ca_der, key).map_err(bad)?;
        Self::issuing(issuer, ca_der, ca.cert.clone(), roots, leaves).map_err(bad)
    }

    pub fn signed_by(
        issuer: Issuer,
        ca_pem: &str,
        roots: RootCertStore,
        leaves: Leaves,
    ) -> io::Result<Self> {
        nanocode_tls::cpu::check()?;
        let ca_der = CertificateDer::from_pem_slice(ca_pem.as_bytes()).map_err(other)?;
        Self::issuing(issuer, ca_der, ca_pem.to_owned(), roots, leaves)
    }

    fn issuing(
        issuer: Issuer,
        ca_der: CertificateDer<'static>,
        ca_pem: String,
        roots: RootCertStore,
        leaves: Leaves,
    ) -> io::Result<Self> {
        let mut upstream = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(other)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        upstream.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self {
            issuer,
            ca_der,
            ca_pem,
            upstream: Arc::new(upstream),
            leaves,
            minted: Atom::new(HashMap::new()),
        })
    }

    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    pub const fn ca_der(&self) -> &CertificateDer<'static> {
        &self.ca_der
    }

    pub fn upstream(&self) -> Arc<ClientConfig> {
        Arc::clone(&self.upstream)
    }

    fn cached(&self, minted: &Minted, host: &str) -> Option<Arc<ServerConfig>> {
        let (made, config) = minted.get(host)?;
        let fresh = self
            .leaves
            .lifetime
            .is_none_or(|lifetime| made.elapsed() < lifetime.renewed);
        fresh.then(|| Arc::clone(config))
    }

    pub fn server(&self, host: &str) -> io::Result<Arc<ServerConfig>> {
        if let Some(config) = self.minted.read(|minted| self.cached(minted, host)) {
            return Ok(config);
        }
        let validity = match self.leaves.lifetime {
            Some(lifetime) => {
                let now = SystemTime::now();
                Some(Validity::between(
                    now - lifetime.backdated,
                    now + lifetime.valid,
                )?)
            }
            None => None,
        };
        let (leaf, key) = self.issuer.leaf(host, &self.leaves.usages, validity)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(other)?
            .with_no_client_auth()
            .with_single_cert(vec![CertificateDer::from(leaf), self.ca_der.clone()], key)
            .map_err(other)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);
        let made = Instant::now();
        Ok(self.minted.update(|minted| {
            if let Some(config) = self.cached(minted, host) {
                return config;
            }
            if minted.len() >= self.leaves.cached {
                minted.clear();
            }
            minted.insert(host.to_owned(), (made, Arc::clone(&config)));
            Arc::clone(&config)
        }))
    }
}

#![cfg(not(target_family = "wasm"))]

use arc_swap::ArcSwapOption;
use nanocode_sync::SingleFlight;
use rustls::{ClientConfig, RootCertStore};
use std::{
    ffi::OsString,
    io,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

const MAX_AGE: Duration = Duration::from_secs(300);
type TrustEnvironment = (Option<OsString>, Option<OsString>);
struct Cached {
    loaded: Instant,
    environment: TrustEnvironment,
    config: Arc<ClientConfig>,
}
impl Cached {
    fn matches(&self, now: Instant, environment: &TrustEnvironment) -> bool {
        now.duration_since(self.loaded) < MAX_AGE && &self.environment == environment
    }
}
static CACHE: ArcSwapOption<Cached> = ArcSwapOption::const_empty();
type Loaded = Result<Arc<ClientConfig>, (io::ErrorKind, String)>;
static LOADING: LazyLock<SingleFlight<Loaded>> = LazyLock::new(SingleFlight::default);

pub async fn native_client_config() -> io::Result<Arc<ClientConfig>> {
    let environment = (
        std::env::var_os("SSL_CERT_FILE"),
        std::env::var_os("SSL_CERT_DIR"),
    );
    loop {
        if let Some(cached) = &*CACHE.load()
            && cached.matches(Instant::now(), &environment)
        {
            return Ok(cached.config.clone());
        }
        let loading = environment.clone();
        LOADING
            .run(|| async move {
                let config = tokio::task::spawn_blocking(load)
                    .await
                    .map_err(io::Error::other)
                    .and_then(|loaded| loaded)
                    .map_err(|error| (error.kind(), error.to_string()))?;
                let config = Arc::new(config);
                CACHE.store(Some(Arc::new(Cached {
                    loaded: Instant::now(),
                    environment: loading,
                    config: Arc::clone(&config),
                })));
                Ok(config)
            })
            .await
            .map_err(|(kind, message)| io::Error::new(kind, message))?;
    }
}

fn load() -> io::Result<ClientConfig> {
    let began = Instant::now();
    install_default_rustls_crypto_provider();
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    if std::env::var_os("SSL_CERT_FILE").is_none() && std::env::var_os("SSL_CERT_DIR").is_none() {
        use rustls_platform_verifier::BuilderVerifierExt;

        let config = ClientConfig::builder()
            .with_platform_verifier()
            .map_err(io::Error::other)?
            .with_no_client_auth();
        tracing::info!(target: "nanocode_tls", stage = "tls.platform_verifier",
            elapsed_ms = began.elapsed().as_secs_f64() * 1000.0);
        return Ok(config);
    }
    let loaded = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (added, rejected) = roots.add_parsable_certificates(loaded.certs);
    tracing::info!(target: "nanocode_tls", stage = "tls.native_roots", elapsed_ms = began.elapsed().as_secs_f64() * 1000.0,
        certificates = added, rejected, load_errors = loaded.errors.len());
    if roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no usable native TLS trust roots",
        ));
    }
    Ok(ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

pub fn install_default_rustls_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let provider = rustls_graviola::default_provider();
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        let provider = rustls::crypto::ring::default_provider();
        drop(provider.install_default());
    }
}

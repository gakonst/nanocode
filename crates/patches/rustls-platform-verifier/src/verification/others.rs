use std::fmt::Debug;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types;
use rustls::{
    crypto::CryptoProvider, CertificateError, DigitallySignedStruct, Error as TlsError, OtherError,
    SignatureScheme,
};

use super::log_server_cert;

/// A TLS certificate verifier that uses the system's root store and WebPKI.
#[derive(Debug)]
pub struct Verifier {
    extra_roots: rustls::RootCertStore,
    crypto_provider: Arc<CryptoProvider>,
    inner: ArcSwapOption<WebPkiServerVerifier>,
}

impl Verifier {
    /// Creates a new verifier whose certificate validation is provided by
    /// WebPKI, using root certificates provided by the platform.
    #[cfg_attr(docsrs, doc(cfg(all())))]
    pub fn new(crypto_provider: Arc<CryptoProvider>) -> Result<Self, TlsError> {
        Self::new_inner([], crypto_provider)
    }

    /// Creates a new verifier whose certificate validation is provided by
    /// WebPKI, using root certificates provided by the platform and augmented by
    /// the provided extra root certificates.
    #[cfg_attr(docsrs, doc(cfg(not(target_os = "android"))))]
    pub fn new_with_extra_roots(
        extra_roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        Self::new_inner(extra_roots, crypto_provider)
    }

    /// Creates a new verifier whose certificate validation is provided by
    /// WebPKI, using root certificates provided by the platform and augmented by
    /// the provided extra root certificates.
    fn new_inner(
        extra_roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let mut root_store = rustls::RootCertStore::empty();

        // While we ignore invalid certificates from the system, we forward errors from
        // parsing the extra roots to the caller.
        for cert in extra_roots {
            root_store.add(cert)?;
        }

        Ok(Self {
            extra_roots: root_store,
            crypto_provider,
            inner: ArcSwapOption::const_empty(),
        })
    }

    fn inner(&self) -> Result<Arc<WebPkiServerVerifier>, TlsError> {
        if let Some(inner) = self.inner.load_full() {
            return Ok(inner);
        }
        let mut root_store = self.extra_roots.clone();

        #[cfg(all(
            unix,
            not(target_os = "android"),
            not(target_vendor = "apple"),
            not(target_arch = "wasm32"),
        ))]
        {
            root_store
                .roots
                .extend(system::roots().roots.iter().cloned());
            if root_store.is_empty() {
                return Err(rustls::Error::General(
                    "No CA certificates were loaded from the system".to_owned(),
                ));
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            root_store.add_parsable_certificates(
                webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().cloned(),
            );
        };

        let inner = WebPkiServerVerifier::builder_with_provider(
            root_store.into(),
            self.crypto_provider.clone(),
        )
        .build()
        .map_err(|e| TlsError::Other(OtherError(Arc::new(e))))?;
        self.inner.store(Some(Arc::clone(&inner)));
        Ok(inner)
    }
}

#[cfg_attr(docsrs, doc(cfg(all())))]
impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &pki_types::CertificateDer<'_>,
        intermediates: &[pki_types::CertificateDer<'_>],
        server_name: &pki_types::ServerName,
        ocsp_response: &[u8],
        now: pki_types::UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        log_server_cert(end_entity);

        self.inner()?
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .map_err(map_webpki_errors)
            // This only contains information from the system or other public
            // bits of the TLS handshake, so it can't leak anything.
            .map_err(|e| {
                log::error!("failed to verify TLS certificate: {}", e);
                e
            })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.crypto_provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.crypto_provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}


fn map_webpki_errors(err: TlsError) -> TlsError {
    match &err {
        TlsError::InvalidCertificate(CertificateError::InvalidPurpose)
        | TlsError::InvalidCertificate(CertificateError::InvalidPurposeContext { .. }) => {
            TlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(
                super::EkuError,
            ))))
        }
        _ => err,
    }
}

#[cfg(all(
    unix,
    not(target_os = "android"),
    not(target_vendor = "apple"),
    not(target_arch = "wasm32"),
))]
pub mod system {
    use arc_swap::ArcSwapOption;
    use std::{
        ffi::OsString,
        sync::Arc,
        time::{Duration, Instant},
    };

    const MAX_AGE: Duration = Duration::from_secs(300);
    type Environment = (Option<OsString>, Option<OsString>);

    struct Loaded {
        at: Instant,
        environment: Environment,
        roots: Arc<rustls::RootCertStore>,
    }

    static ROOTS: ArcSwapOption<Loaded> = ArcSwapOption::const_empty();

    pub fn roots() -> Arc<rustls::RootCertStore> {
        let environment = (
            std::env::var_os("SSL_CERT_FILE"),
            std::env::var_os("SSL_CERT_DIR"),
        );
        if let Some(loaded) = &*ROOTS.load() {
            if loaded.environment == environment && loaded.at.elapsed() < MAX_AGE {
                return Arc::clone(&loaded.roots);
            }
        }
        let result = rustls_native_certs::load_native_certs();
        let mut store = rustls::RootCertStore::empty();
        let (added, ignored) = store.add_parsable_certificates(result.certs);
        if ignored > 0 {
            log::warn!("{ignored} platform CA root certificates were ignored due to errors");
        }
        for error in result.errors {
            log::warn!("Error loading CA root certificate: {error}");
        }
        log::debug!("Loaded {added} CA root certificates from the system");
        let roots = Arc::new(store);
        ROOTS.store(Some(Arc::new(Loaded {
            at: Instant::now(),
            environment,
            roots: Arc::clone(&roots),
        })));
        roots
    }
}

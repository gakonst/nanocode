#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use rustls::{client::WantsClientCert, ClientConfig, ConfigBuilder, WantsVerifier};

mod verification;
pub use verification::Verifier;

// Build the Android module when generating docs so that
// the Android-specific functions are included regardless of
// the host.
#[cfg(any(all(doc, docsrs), target_os = "android"))]
#[cfg_attr(docsrs, doc(cfg(target_os = "android")))]
pub mod android;

/// Extension trait to help configure [`ClientConfig`]s with the platform verifier.
pub trait BuilderVerifierExt {
    /// Configures the `ClientConfig` with the platform verifier.
    fn with_platform_verifier(
        self,
    ) -> Result<ConfigBuilder<ClientConfig, WantsClientCert>, rustls::Error>;
}

impl BuilderVerifierExt for ConfigBuilder<ClientConfig, WantsVerifier> {
    fn with_platform_verifier(
        self,
    ) -> Result<ConfigBuilder<ClientConfig, WantsClientCert>, rustls::Error> {
        let verifier = Verifier::new(self.crypto_provider().clone())?;
        Ok(self
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier)))
    }
}

/// Extension trait to help build a [`ClientConfig`] with the platform verifier.
pub trait ConfigVerifierExt {
    /// Build a [`ClientConfig`] with the platform verifier and the default `CryptoProvider`.
    fn with_platform_verifier() -> Result<ClientConfig, rustls::Error>;
}

impl ConfigVerifierExt for ClientConfig {
    fn with_platform_verifier() -> Result<ClientConfig, rustls::Error> {
        Ok(ClientConfig::builder()
            .with_platform_verifier()?
            .with_no_client_auth())
    }
}

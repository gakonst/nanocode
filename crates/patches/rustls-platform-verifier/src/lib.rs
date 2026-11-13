#![cfg_attr(docsrs, feature(doc_cfg))]

use std::sync::Arc;

use rustls::{client::WantsClientCert, ClientConfig, ConfigBuilder, WantsVerifier};

mod verification;
#[cfg(all(unix, not(target_vendor = "apple"), not(target_arch = "wasm32")))]
pub use verification::system_roots;
pub use verification::Verifier;

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

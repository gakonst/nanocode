use std::sync::Arc;

use nanocode_sys::security::{self, ServerTrust, TrustError};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types;
use rustls::{
    CertificateError, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme,
};

use crate::verification::invalid_certificate;

const ERR_SEC_CREATE_CHAIN_FAILED: isize = -25318;
const ERR_SEC_HOST_NAME_MISMATCH: isize = -67602;
const ERR_SEC_INVALID_EXTENDED_KEY_USAGE: isize = -67609;
const ERR_SEC_CERTIFICATE_REVOKED: isize = -67820;

#[derive(Debug)]
pub struct Verifier {
    extra_roots: Vec<pki_types::CertificateDer<'static>>,
    crypto_provider: Arc<CryptoProvider>,
}

impl Verifier {
    pub fn new(crypto_provider: Arc<CryptoProvider>) -> Result<Self, TlsError> {
        Ok(Self {
            extra_roots: Vec::new(),
            crypto_provider,
        })
    }

    pub fn new_with_extra_roots(
        roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let extra_roots = roots.into_iter().collect::<Vec<_>>();
        if !extra_roots
            .iter()
            .all(|root| security::parses_certificate(root))
        {
            return Err(TlsError::InvalidCertificate(CertificateError::BadEncoding));
        }
        Ok(Self {
            extra_roots,
            crypto_provider,
        })
    }

    fn verify_certificate(
        &self,
        end_entity: &pki_types::CertificateDer<'_>,
        intermediates: &[pki_types::CertificateDer<'_>],
        server_name: &str,
        ocsp_response: Option<&[u8]>,
        now: pki_types::UnixTime,
    ) -> Result<(), TlsError> {
        let chain: Vec<&[u8]> = std::iter::once(end_entity.as_ref())
            .chain(intermediates.iter().map(|cert| cert.as_ref()))
            .collect();
        let anchors: Vec<&[u8]> = self.extra_roots.iter().map(|root| root.as_ref()).collect();
        let trust = ServerTrust {
            chain: &chain,
            host: server_name,
            unix_seconds: now.as_secs(),
            ocsp_response,
            anchors: &anchors,
        };
        match trust.evaluate() {
            Ok(()) => Ok(()),
            Err(TrustError::BadCertificate) => {
                Err(TlsError::InvalidCertificate(CertificateError::BadEncoding))
            }
            Err(TrustError::Refused(call)) => Err(TlsError::General(format!("{call} failed"))),
            Err(TrustError::Status(call, status)) => {
                Err(invalid_certificate(format!("{call} failed: {status}")))
            }
            Err(TrustError::Rejected { code, description }) => Err(match code {
                ERR_SEC_HOST_NAME_MISMATCH => {
                    TlsError::InvalidCertificate(CertificateError::NotValidForName)
                }
                ERR_SEC_CREATE_CHAIN_FAILED => {
                    TlsError::InvalidCertificate(CertificateError::UnknownIssuer)
                }
                ERR_SEC_INVALID_EXTENDED_KEY_USAGE => TlsError::InvalidCertificate(
                    CertificateError::Other(OtherError(Arc::new(super::EkuError))),
                ),
                ERR_SEC_CERTIFICATE_REVOKED => {
                    TlsError::InvalidCertificate(CertificateError::Revoked)
                }
                _ => invalid_certificate(format!("{description}: {code}")),
            }),
        }
    }
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &pki_types::CertificateDer<'_>,
        intermediates: &[pki_types::CertificateDer<'_>],
        server_name: &pki_types::ServerName,
        ocsp_response: &[u8],
        now: pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, TlsError> {
        let server = server_name.to_str();
        let ocsp_data = (!ocsp_response.is_empty()).then_some(ocsp_response);
        match self.verify_certificate(end_entity, intermediates, &server, ocsp_data, now) {
            Ok(()) => Ok(rustls::client::danger::ServerCertVerified::assertion()),
            Err(e) => {
                nanocode_trace::error!("failed to verify TLS certificate: {}", e);
                Err(e)
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &pki_types::CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        verify_tls12_signature(
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
        verify_tls13_signature(
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

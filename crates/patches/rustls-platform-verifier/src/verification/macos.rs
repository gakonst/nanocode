use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types;
use rustls::{
    CertificateError, DigitallySignedStruct, Error as TlsError, OtherError, RootCertStore,
    SignatureScheme,
};

use super::{invalid_certificate, log_server_cert};

const TRUST_VARIABLE: &str = "NANOCODE_TLS_TRUST";
const KEYCHAINS: [&str; 2] = [
    "/System/Library/Keychains/SystemRootCertificates.keychain",
    "/Library/Keychains/System.keychain",
];
const SECURITY: &str = "/usr/bin/security";
const X509_CERTIFICATE: usize = 0x8000_1000;
const CERTIFICATE_HEADER: usize = 60;

#[derive(Debug)]
pub struct Verifier {
    trust: Trust,
    crypto_provider: Arc<CryptoProvider>,
}

#[derive(Debug)]
enum Trust {
    Files(Arc<WebPkiServerVerifier>),
    System(Option<Arc<WebPkiServerVerifier>>),
}

impl Verifier {
    pub fn new(crypto_provider: Arc<CryptoProvider>) -> Result<Self, TlsError> {
        Self::new_with_extra_roots([], crypto_provider)
    }

    pub fn new_with_extra_roots(
        extra_roots: impl IntoIterator<Item = pki_types::CertificateDer<'static>>,
        crypto_provider: Arc<CryptoProvider>,
    ) -> Result<Self, TlsError> {
        let extra = extra_roots.into_iter().collect::<Vec<_>>();
        let trust = match std::env::var_os(TRUST_VARIABLE) {
            None => Trust::Files(webpki(extra, keychain_roots(), &crypto_provider)?),
            Some(mode) if mode == "files" => {
                Trust::Files(webpki(extra, keychain_roots(), &crypto_provider)?)
            }
            Some(mode) if mode == "system" => Trust::System(if extra.is_empty() {
                None
            } else {
                Some(webpki(extra, Vec::new(), &crypto_provider)?)
            }),
            Some(mode) => {
                return Err(TlsError::General(format!(
                    "{TRUST_VARIABLE} must be files or system, not {}",
                    mode.to_string_lossy()
                )))
            }
        };
        Ok(Self {
            trust,
            crypto_provider,
        })
    }
}

fn webpki(
    extra: Vec<pki_types::CertificateDer<'static>>,
    system: Vec<pki_types::CertificateDer<'static>>,
    crypto_provider: &Arc<CryptoProvider>,
) -> Result<Arc<WebPkiServerVerifier>, TlsError> {
    let mut store = RootCertStore::empty();
    for certificate in extra {
        store.add(certificate)?;
    }
    let (added, ignored) = store.add_parsable_certificates(system);
    if ignored > 0 {
        log::warn!("{ignored} platform CA root certificates were ignored due to errors");
    }
    if store.is_empty() {
        return Err(TlsError::General(
            "No CA certificates were loaded from the system".to_owned(),
        ));
    }
    log::debug!("Loaded {added} CA root certificates from the system keychains");
    WebPkiServerVerifier::builder_with_provider(store.into(), crypto_provider.clone())
        .build()
        .map_err(|error| TlsError::Other(OtherError(Arc::new(error))))
}

fn keychain_roots() -> Vec<pki_types::CertificateDer<'static>> {
    let mut certificates = Vec::new();
    for path in KEYCHAINS {
        match fs::read(path) {
            Ok(bytes) => match keychain_certificates(&bytes) {
                Some(found) => certificates.extend(found),
                None => log::warn!("{path} is not a readable keychain"),
            },
            Err(error) => log::warn!("Error reading {path}: {error}"),
        }
    }
    certificates
}

fn word(bytes: &[u8], offset: usize) -> Option<usize> {
    let word: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    usize::try_from(u32::from_be_bytes(word)).ok()
}

fn keychain_certificates(bytes: &[u8]) -> Option<Vec<pki_types::CertificateDer<'static>>> {
    if bytes.get(..4)? != b"kych" {
        return None;
    }
    let schema = word(bytes, 12)?;
    let mut certificates = Vec::new();
    for table in 0..word(bytes, schema.checked_add(4)?)? {
        let table = schema.checked_add(word(bytes, schema + 8 + 4 * table)?)?;
        if word(bytes, table.checked_add(4)?)? != X509_CERTIFICATE {
            continue;
        }
        for record in 0..word(bytes, table.checked_add(24)?)? {
            let offset = word(bytes, table + 28 + 4 * record)?;
            if offset == 0 || offset % 4 != 0 {
                continue;
            }
            let record = table.checked_add(offset)?;
            let size = word(bytes, record.checked_add(16)?)?;
            let start = record.checked_add(CERTIFICATE_HEADER)?;
            let der = bytes.get(start..start.checked_add(size)?)?;
            if !der.is_empty() {
                certificates.push(pki_types::CertificateDer::from(der.to_vec()));
            }
        }
    }
    Some(certificates)
}

fn evaluate(
    server: &str,
    end_entity: &pki_types::CertificateDer<'_>,
    intermediates: &[pki_types::CertificateDer<'_>],
) -> Result<(), TlsError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "nanocode-tls-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let created = fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .or_else(|error| {
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
            fs::remove_dir_all(&directory)?;
            fs::DirBuilder::new().mode(0o700).create(&directory)
        });
    let result = created
        .map_err(|error| system_error(&directory, error))
        .and_then(|()| run_security(&directory, server, end_entity, intermediates));
    drop(fs::remove_dir_all(&directory));
    result
}

fn run_security(
    directory: &Path,
    server: &str,
    end_entity: &pki_types::CertificateDer<'_>,
    intermediates: &[pki_types::CertificateDer<'_>],
) -> Result<(), TlsError> {
    let mut command = Command::new(SECURITY);
    command
        .args(["verify-cert", "-p", "ssl", "-n", server])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (index, certificate) in std::iter::once(end_entity).chain(intermediates).enumerate() {
        let path = directory.join(format!("{index}.der"));
        fs::write(&path, certificate).map_err(|error| system_error(&path, error))?;
        command.arg("-c").arg(path);
    }
    let output = command
        .output()
        .map_err(|error| system_error(Path::new(SECURITY), error))?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr);
    if reason.contains("Host name mismatch") {
        return Err(TlsError::InvalidCertificate(
            CertificateError::NotValidForName,
        ));
    }
    Err(invalid_certificate(format!(
        "{SECURITY} verify-cert refused the certificate for {server}: {}",
        reason.trim()
    )))
}

fn system_error(path: &Path, error: io::Error) -> TlsError {
    TlsError::General(format!("{}: {error}", path.display()))
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &pki_types::CertificateDer<'_>,
        intermediates: &[pki_types::CertificateDer<'_>],
        server_name: &pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: pki_types::UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        log_server_cert(end_entity);
        let result = match &self.trust {
            Trust::Files(roots) => roots
                .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
                .map_err(map_webpki_errors),
            Trust::System(extra) => {
                if let Some(extra) = extra {
                    if let Ok(verified) = extra.verify_server_cert(
                        end_entity,
                        intermediates,
                        server_name,
                        ocsp_response,
                        now,
                    ) {
                        return Ok(verified);
                    }
                }
                evaluate(&server_name.to_str(), end_entity, intermediates)
                    .map(|()| ServerCertVerified::assertion())
            }
        };
        result.inspect_err(|error| log::error!("failed to verify TLS certificate: {error}"))
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

fn map_webpki_errors(error: TlsError) -> TlsError {
    match &error {
        TlsError::InvalidCertificate(CertificateError::InvalidPurpose)
        | TlsError::InvalidCertificate(CertificateError::InvalidPurposeContext { .. }) => {
            TlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(
                super::EkuError,
            ))))
        }
        _ => error,
    }
}

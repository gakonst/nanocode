use crate::postgres::{Host, PgError, PgOptions, TlsMode, wire};
use nanocode_executor::TcpStream;
use nanocode_io::{AsyncReadExt as _, AsyncWriteExt as _};
use nanocode_sync::SwapOption;
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject},
};
use std::{io, sync::Arc};

const ACCEPTS_TLS: u8 = b'S';
const REFUSES_TLS: u8 = b'N';

pub(super) enum Stream {
    Tcp(TcpStream),
    Tls(Box<nanocode_tls::ClientStream<TcpStream>>),
    #[cfg(unix)]
    Unix(nanocode_executor::UnixStream),
}

impl Stream {
    pub(super) async fn connect(
        options: &PgOptions,
        reused: &SwapOption<ClientConfig>,
    ) -> Result<Self, PgError> {
        let host = match &options.host {
            Host::Tcp(host) => host,
            #[cfg(unix)]
            Host::Unix(directory) => {
                let socket = directory.join(format!(".s.PGSQL.{}", options.port));
                return Ok(Self::Unix(
                    nanocode_executor::UnixStream::connect(socket).await?,
                ));
            }
            #[cfg(not(unix))]
            Host::Unix(_) => {
                return Err(PgError::Configuration(
                    "Unix sockets need a Unix host".to_owned(),
                ));
            }
        };
        let mut tcp = TcpStream::connect((host.as_str(), options.port)).await?;
        tcp.set_nodelay(true)?;
        if options.tls == TlsMode::Disable {
            return Ok(Self::Tcp(tcp));
        }
        let mut request = Vec::new();
        wire::ssl_request(&mut request);
        tcp.write_all(&request).await?;
        match tcp.read_u8().await? {
            ACCEPTS_TLS => {}
            REFUSES_TLS if options.tls == TlsMode::Prefer => return Ok(Self::Tcp(tcp)),
            REFUSES_TLS => {
                return Err(PgError::Tls(format!(
                    "the server at {host} does not accept TLS, which sslmode requires"
                )));
            }
            answer => {
                return Err(PgError::Protocol(format!(
                    "the server answered a TLS request with {answer}"
                )));
            }
        }
        let native = options.tls == TlsMode::VerifyFull && options.root_certificate.is_none();
        let config = match reused.load_full() {
            Some(config) if !native => config,
            _ => {
                let config = config(options).await?;
                if !native {
                    reused.store(Some(Arc::clone(&config)));
                }
                config
            }
        };
        let name = ServerName::try_from(host.clone())
            .map_err(|error| PgError::Tls(format!("{host} is not a TLS server name: {error}")))?;
        let tls = nanocode_tls::connect(config, name, tcp).await?;
        Ok(Self::Tls(Box::new(tls)))
    }

    pub(super) async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.write_all(bytes).await,
            Self::Tls(stream) => {
                stream.write_all(bytes).await?;
                stream.flush().await
            }
            #[cfg(unix)]
            Self::Unix(stream) => stream.write_all(bytes).await,
        }
    }

    pub(super) async fn read_buf(&mut self, buffer: &mut Vec<u8>) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read_buf(buffer).await,
            Self::Tls(stream) => stream.read_buf(buffer).await,
            #[cfg(unix)]
            Self::Unix(stream) => stream.read_buf(buffer).await,
        }
    }
}

async fn config(options: &PgOptions) -> Result<Arc<ClientConfig>, PgError> {
    nanocode_tls::cpu::check()?;
    nanocode_tls::install_default_rustls_crypto_provider();
    match (options.tls, &options.root_certificate) {
        (TlsMode::VerifyFull, None) => Ok(nanocode_tls::native_client_config().await?),
        (TlsMode::VerifyFull, Some(path)) => {
            let read = path.clone();
            let pem = nanocode_platform::spawn_blocking(move || std::fs::read(read))
                .await
                .map_err(io::Error::other)??;
            let mut roots = RootCertStore::empty();
            for certificate in CertificateDer::pem_slice_iter(&pem) {
                let certificate = certificate.map_err(|error| {
                    PgError::Configuration(format!("{}: {error}", path.display()))
                })?;
                roots.add(certificate).map_err(|error| {
                    PgError::Configuration(format!("{}: {error}", path.display()))
                })?;
            }
            Ok(Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ))
        }
        (_, _) => {
            let provider = CryptoProvider::get_default()
                .cloned()
                .ok_or_else(|| PgError::Tls("no rustls crypto provider is installed".to_owned()))?;
            Ok(Arc::new(
                ClientConfig::builder()
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(Unverified(provider)))
                    .with_no_client_auth(),
            ))
        }
    }
}

#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

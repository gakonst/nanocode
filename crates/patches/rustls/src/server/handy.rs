use alloc::vec::Vec;
use core::fmt::Debug;

use crate::server::ClientHello;
use crate::sync::Arc;
use crate::{server, sign};

/// Something which never stores sessions.
#[derive(Debug)]
pub struct NoServerSessionStorage {}

impl server::StoresServerSessions for NoServerSessionStorage {
    fn put(&self, _id: Vec<u8>, _sec: Vec<u8>) -> bool {
        false
    }
    fn get(&self, _id: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn take(&self, _id: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn can_cache(&self) -> bool {
        false
    }
}

mod cache {
    use alloc::vec::Vec;
    use core::fmt::{Debug, Formatter};

    use crate::lock::Mutex;
    use crate::sync::Arc;
    use crate::{limited_cache, server};

    /// An implementer of `StoresServerSessions` that stores everything
    /// in memory.  If enforces a limit on the number of stored sessions
    /// to bound memory usage.
    pub struct ServerSessionMemoryCache {
        cache: Mutex<limited_cache::LimitedCache<Vec<u8>, Vec<u8>>>,
    }

    impl ServerSessionMemoryCache {
        /// Make a new ServerSessionMemoryCache.  `size` is the maximum
        /// number of stored sessions, and may be rounded-up for
        /// efficiency.
        pub fn new(size: usize) -> Arc<Self> {
            Arc::new(Self {
                cache: Mutex::new(limited_cache::LimitedCache::new(size)),
            })
        }

    }

    impl server::StoresServerSessions for ServerSessionMemoryCache {
        fn put(&self, key: Vec<u8>, value: Vec<u8>) -> bool {
            self.cache
                .lock()
                .unwrap()
                .insert(key, value);
            true
        }

        fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
            self.cache
                .lock()
                .unwrap()
                .get(key)
                .cloned()
        }

        fn take(&self, key: &[u8]) -> Option<Vec<u8>> {
            self.cache.lock().unwrap().remove(key)
        }

        fn can_cache(&self) -> bool {
            true
        }
    }

    impl Debug for ServerSessionMemoryCache {
        fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("ServerSessionMemoryCache")
                .finish()
        }
    }

}

pub use cache::ServerSessionMemoryCache;

/// Something which never produces tickets.
#[derive(Debug)]
pub(super) struct NeverProducesTickets {}

impl server::ProducesTickets for NeverProducesTickets {
    fn enabled(&self) -> bool {
        false
    }
    fn lifetime(&self) -> u32 {
        0
    }
    fn encrypt(&self, _bytes: &[u8]) -> Option<Vec<u8>> {
        None
    }
    fn decrypt(&self, _bytes: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

/// An exemplar `ResolvesServerCert` implementation that always resolves to a single
/// [RFC 7250] raw public key.
///
/// [RFC 7250]: https://tools.ietf.org/html/rfc7250
#[derive(Clone, Debug)]
pub struct AlwaysResolvesServerRawPublicKeys(Arc<sign::CertifiedKey>);

impl AlwaysResolvesServerRawPublicKeys {
    /// Create a new `AlwaysResolvesServerRawPublicKeys` instance.
    pub fn new(certified_key: Arc<sign::CertifiedKey>) -> Self {
        Self(certified_key)
    }
}

impl server::ResolvesServerCert for AlwaysResolvesServerRawPublicKeys {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<sign::CertifiedKey>> {
        Some(self.0.clone())
    }

    fn only_raw_public_keys(&self) -> bool {
        true
    }
}

mod sni_resolver {
    use alloc::string::{String, ToString};
    use core::fmt::Debug;

    use pki_types::{DnsName, ServerName};

    use crate::error::Error;
    use crate::hash_map::HashMap;
    use crate::server::ClientHello;
    use crate::sync::Arc;
    use crate::webpki::{ParsedCertificate, verify_server_name};
    use crate::{server, sign};

    /// Something that resolves do different cert chains/keys based
    /// on client-supplied server name (via SNI).
    #[derive(Debug)]
    pub struct ResolvesServerCertUsingSni {
        by_name: HashMap<String, Arc<sign::CertifiedKey>>,
    }

    impl ResolvesServerCertUsingSni {
        /// Create a new and empty (i.e., knows no certificates) resolver.
        pub fn new() -> Self {
            Self {
                by_name: HashMap::new(),
            }
        }

        /// Add a new `sign::CertifiedKey` to be used for the given SNI `name`.
        ///
        /// This function fails if `name` is not a valid DNS name, or if
        /// it's not valid for the supplied certificate, or if the certificate
        /// chain is syntactically faulty.
        pub fn add(&mut self, name: &str, ck: sign::CertifiedKey) -> Result<(), Error> {
            let server_name = {
                let checked_name = DnsName::try_from(name)
                    .map_err(|_| Error::General("Bad DNS name".into()))
                    .map(|name| name.to_lowercase_owned())?;
                ServerName::DnsName(checked_name)
            };

            // Check the certificate chain for validity:
            // - it should be non-empty list
            // - the first certificate should be parsable as a x509v3,
            // - the first certificate should quote the given server name
            //   (if provided)
            //
            // These checks are not security-sensitive.  They are the
            // *server* attempting to detect accidental misconfiguration.

            ck.end_entity_cert()
                .and_then(ParsedCertificate::try_from)
                .and_then(|cert| verify_server_name(&cert, &server_name))?;

            if let ServerName::DnsName(name) = server_name {
                self.by_name
                    .insert(name.as_ref().to_string(), Arc::new(ck));
            }
            Ok(())
        }
    }

    impl server::ResolvesServerCert for ResolvesServerCertUsingSni {
        fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<sign::CertifiedKey>> {
            if let Some(name) = client_hello.server_name() {
                self.by_name.get(name).cloned()
            } else {
                // This kind of resolver requires SNI
                None
            }
        }
    }

}

pub use sni_resolver::ResolvesServerCertUsingSni;


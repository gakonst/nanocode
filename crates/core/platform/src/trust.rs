use rustls_pki_types::{
    CertificateDer,
    pem::{self, PemObject},
};
use std::{
    env,
    error::Error as StdError,
    fmt, fs, io,
    path::{Path, PathBuf},
};

const CERTIFICATE_FILE_VARIABLE: &str = "SSL_CERT_FILE";
const CERTIFICATE_DIRECTORY_VARIABLE: &str = "SSL_CERT_DIR";

#[derive(Debug, Default)]
pub struct NativeCertificates {
    pub certificates: Vec<CertificateDer<'static>>,
    pub errors: Vec<Error>,
}

#[derive(Debug)]
pub struct Error {
    pub context: &'static str,
    pub kind: ErrorKind,
}

#[derive(Debug)]
pub enum ErrorKind {
    Io {
        inner: io::Error,
        path: PathBuf,
    },
    #[cfg(target_os = "macos")]
    Os(nanocode_sys::trust_settings::TrustSettingsError),
    #[cfg(windows)]
    Os(io::Error),
    Pem(pem::Error),
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(match &self.kind {
            ErrorKind::Io { inner, .. } => inner,
            #[cfg(any(target_os = "macos", windows))]
            ErrorKind::Os(error) => error,
            ErrorKind::Pem(error) => error,
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.context)?;
        formatter.write_str(": ")?;
        match &self.kind {
            ErrorKind::Io { inner, path } => write!(formatter, "{inner} at '{}'", path.display()),
            #[cfg(any(target_os = "macos", windows))]
            ErrorKind::Os(error) => error.fmt(formatter),
            ErrorKind::Pem(error) => error.fmt(formatter),
        }
    }
}

impl NativeCertificates {
    fn pem_error(&mut self, error: pem::Error, path: &Path) {
        self.errors.push(Error {
            context: "failed to read PEM from file",
            kind: match error {
                pem::Error::Io(inner) => ErrorKind::Io {
                    inner,
                    path: path.to_owned(),
                },
                _ => ErrorKind::Pem(error),
            },
        });
    }

    fn io_error(&mut self, inner: io::Error, path: &Path, context: &'static str) {
        self.errors.push(Error {
            context,
            kind: ErrorKind::Io {
                inner,
                path: path.to_owned(),
            },
        });
    }
}

pub fn load_native_certificates() -> NativeCertificates {
    let file = env::var_os(CERTIFICATE_FILE_VARIABLE).map(PathBuf::from);
    let directories: Vec<PathBuf> = match env::var_os(CERTIFICATE_DIRECTORY_VARIABLE) {
        Some(directories) => env::split_paths(&directories)
            .filter(|directory| !directory.as_os_str().is_empty())
            .collect(),
        None => Vec::new(),
    };
    if file.is_some() || !directories.is_empty() {
        return load_paths(file.as_deref(), &directories);
    }
    platform::load()
}

fn load_paths(file: Option<&Path>, directories: &[PathBuf]) -> NativeCertificates {
    let mut loaded = NativeCertificates::default();
    if file.is_none() && directories.is_empty() {
        return loaded;
    }
    if let Some(file) = file {
        load_pem(file, &mut loaded);
    }
    for directory in directories {
        load_directory(directory, &mut loaded);
    }
    loaded
        .certificates
        .sort_unstable_by(|left, right| left.as_ref().cmp(right.as_ref()));
    loaded.certificates.dedup();
    loaded
}

fn load_directory(directory: &Path, loaded: &mut NativeCertificates) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            loaded.io_error(error, directory, "opening directory");
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                loaded.io_error(error, directory, "reading directory entries");
                continue;
            }
        };
        let path = entry.path();
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                loaded.io_error(error, &path, "failed to open file");
                continue;
            }
        };
        if metadata.is_file() {
            load_pem(&path, loaded);
        }
    }
}

fn load_pem(path: &Path, loaded: &mut NativeCertificates) {
    let certificates = match CertificateDer::pem_file_iter(path) {
        Ok(certificates) => certificates,
        Err(error) => {
            loaded.pem_error(error, path);
            return;
        }
    };
    for certificate in certificates {
        match certificate {
            Ok(certificate) => loaded.certificates.push(certificate),
            Err(error) => loaded.pem_error(error, path),
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use super::{
        CERTIFICATE_DIRECTORY_VARIABLE, CERTIFICATE_FILE_VARIABLE, NativeCertificates, load_paths,
    };
    use std::{
        env,
        path::{Path, PathBuf},
    };

    #[cfg(target_os = "linux")]
    const CERTIFICATE_DIRECTORIES: &[&str] = &[
        "/etc/ssl/certs",
        "/etc/pki/tls/certs",
        "/etc/security/certificates",
    ];

    #[cfg(target_os = "freebsd")]
    const CERTIFICATE_DIRECTORIES: &[&str] = &["/etc/ssl/certs", "/usr/local/share/certs"];

    #[cfg(any(target_os = "illumos", target_os = "solaris"))]
    const CERTIFICATE_DIRECTORIES: &[&str] = &["/etc/certs/CA"];

    #[cfg(target_os = "netbsd")]
    const CERTIFICATE_DIRECTORIES: &[&str] = &["/etc/openssl/certs"];

    #[cfg(target_os = "aix")]
    const CERTIFICATE_DIRECTORIES: &[&str] = &["/var/ssl/certs"];

    #[cfg(not(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "illumos",
        target_os = "solaris",
        target_os = "netbsd",
        target_os = "aix"
    )))]
    const CERTIFICATE_DIRECTORIES: &[&str] = &["/etc/ssl/certs"];

    #[cfg(target_os = "linux")]
    const CERTIFICATE_FILES: &[&str] = &[
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
        "/etc/pki/tls/cacert.pem",
        "/etc/ssl/cert.pem",
        "/opt/etc/ssl/certs/ca-certificates.crt",
        "/etc/ssl/certs/cacert.pem",
    ];

    #[cfg(target_os = "freebsd")]
    const CERTIFICATE_FILES: &[&str] = &["/usr/local/etc/ssl/cert.pem"];

    #[cfg(target_os = "dragonfly")]
    const CERTIFICATE_FILES: &[&str] = &["/usr/local/share/certs/ca-root-nss.crt"];

    #[cfg(target_os = "netbsd")]
    const CERTIFICATE_FILES: &[&str] = &["/etc/openssl/certs/ca-certificates.crt"];

    #[cfg(target_os = "openbsd")]
    const CERTIFICATE_FILES: &[&str] = &["/etc/ssl/cert.pem"];

    #[cfg(target_os = "solaris")]
    const CERTIFICATE_FILES: &[&str] = &["/etc/certs/ca-certificates.crt"];

    #[cfg(target_os = "illumos")]
    const CERTIFICATE_FILES: &[&str] = &["/etc/ssl/cacert.pem", "/etc/certs/ca-certificates.crt"];

    #[cfg(target_os = "android")]
    const CERTIFICATE_FILES: &[&str] = &["/data/data/com.termux/files/usr/etc/tls/cert.pem"];

    #[cfg(target_os = "haiku")]
    const CERTIFICATE_FILES: &[&str] = &["/boot/system/data/ssl/CARootCertificates.pem"];

    #[cfg(not(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
        target_os = "illumos",
        target_os = "android",
        target_os = "haiku",
    )))]
    const CERTIFICATE_FILES: &[&str] = &["/etc/ssl/certs/ca-certificates.crt"];

    fn existing(variable: &str) -> Option<PathBuf> {
        env::var_os(variable)
            .map(PathBuf::from)
            .filter(|path| path.exists())
    }

    pub(super) fn load() -> NativeCertificates {
        let file = existing(CERTIFICATE_FILE_VARIABLE).or_else(|| {
            CERTIFICATE_FILES
                .iter()
                .map(Path::new)
                .find(|path| path.exists())
                .map(PathBuf::from)
        });
        let directories: Vec<PathBuf> = existing(CERTIFICATE_DIRECTORY_VARIABLE)
            .into_iter()
            .chain(
                CERTIFICATE_DIRECTORIES
                    .iter()
                    .map(PathBuf::from)
                    .filter(|path| path.exists()),
            )
            .collect();
        load_paths(file.as_deref(), &directories)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{CertificateDer, Error, ErrorKind, NativeCertificates};
    use nanocode_sys::trust_settings::{self, Domain, TrustSetting, TrustSettingsError};
    use std::collections::BTreeMap;

    const DOMAINS: [(Domain, &str); 3] = [
        (Domain::User, "failed to load user trust settings"),
        (Domain::Admin, "failed to load admin trust settings"),
        (Domain::System, "failed to load system trust settings"),
    ];

    pub(super) fn load() -> NativeCertificates {
        let mut loaded = NativeCertificates::default();
        let mut found: BTreeMap<Vec<u8>, TrustSetting> = BTreeMap::new();
        for (domain, context) in DOMAINS {
            let certificates = match trust_settings::certificates(domain) {
                Ok(certificates) => certificates,
                Err(error) => {
                    loaded.errors.push(Error {
                        context,
                        kind: ErrorKind::Os(error),
                    });
                    continue;
                }
            };
            for certificate in certificates {
                let trust = match certificate.trust {
                    Ok(trust) => trust.unwrap_or(TrustSetting::TrustRoot),
                    Err(status) => {
                        loaded.errors.push(Error {
                            context: "certificate not trusted",
                            kind: ErrorKind::Os(TrustSettingsError::Status(status)),
                        });
                        continue;
                    }
                };
                found.entry(certificate.der).or_insert(trust);
            }
        }
        loaded.certificates = found
            .into_iter()
            .filter(|(_, trust)| {
                matches!(trust, TrustSetting::TrustRoot | TrustSetting::TrustAsRoot)
            })
            .map(|(der, _)| CertificateDer::from(der))
            .collect();
        loaded
    }
}

#[cfg(windows)]
mod platform {
    use super::{CertificateDer, Error, ErrorKind, NativeCertificates};
    use nanocode_sys::certificate_store::{self, Usages};

    const SERVER_AUTHENTICATION: &str = "1.3.6.1.5.5.7.3.1";

    pub(super) fn load() -> NativeCertificates {
        let mut loaded = NativeCertificates::default();
        let certificates = match certificate_store::current_user_roots() {
            Ok(certificates) => certificates,
            Err(error) => {
                loaded.errors.push(Error {
                    context: "failed to open current user certificate store",
                    kind: ErrorKind::Os(error),
                });
                return loaded;
            }
        };
        for certificate in certificates {
            let usable = match certificate.usages {
                Ok(Usages::All) => true,
                Ok(Usages::Identifiers(identifiers)) => identifiers
                    .iter()
                    .any(|identifier| identifier == SERVER_AUTHENTICATION),
                Err(error) => {
                    loaded.errors.push(Error {
                        context: "failed to read certificate usages",
                        kind: ErrorKind::Os(error),
                    });
                    continue;
                }
            };
            if usable && certificate.time_valid {
                loaded
                    .certificates
                    .push(CertificateDer::from(certificate.der));
            }
        }
        loaded
    }
}

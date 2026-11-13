#[cfg(any(windows, target_vendor = "apple"))]
use std::sync::Arc;

#[cfg(all(any(unix, target_arch = "wasm32"), not(target_vendor = "apple")))]
mod others;

#[cfg(all(any(unix, target_arch = "wasm32"), not(target_vendor = "apple")))]
pub use others::Verifier;

#[cfg(all(unix, not(target_vendor = "apple"), not(target_arch = "wasm32")))]
pub use others::system::roots as system_roots;

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
mod apple;

#[cfg(all(target_vendor = "apple", not(target_os = "macos")))]
pub use apple::Verifier;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::Verifier;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::Verifier;

/// An EKU was invalid for the use case of verifying a server certificate.
///
/// This error is used primarily for tests.
#[cfg_attr(windows, allow(dead_code))] // not used by windows verifier
#[derive(Debug, PartialEq)]
pub(crate) struct EkuError;

impl std::fmt::Display for EkuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("certificate had invalid extensions")
    }
}

impl std::error::Error for EkuError {}

// Unknown certificate error shorthand. Used when we need to construct an "Other" certificate
// error with a platform specific error message.
#[cfg(any(windows, target_vendor = "apple"))]
fn invalid_certificate(reason: impl Into<String>) -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
        Arc::from(Box::from(reason.into())),
    )))
}

/// List of EKUs that one or more of that *must* be in the end-entity certificate.
///
/// Legacy server-gated crypto OIDs are assumed to no longer be in use.
///
/// Currently supported:
/// - id-kp-serverAuth
// TODO: Chromium also allows for `OID_ANY_EKU` on Android.
#[cfg(target_os = "windows")]
// XXX: Windows requires that we NUL terminate EKU strings.
// See https://github.com/rustls/rustls-platform-verifier/issues/126#issuecomment-2306232794.
const ALLOWED_EKUS: &[windows_sys::core::PCSTR] =
    &[windows_sys::Win32::Security::Cryptography::szOID_PKIX_KP_SERVER_AUTH];

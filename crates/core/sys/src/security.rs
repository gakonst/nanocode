use std::ffi::c_void;

type Reference = *const c_void;
pub type Status = i32;

const UTF8: u32 = 0x0800_0100;
const UTF8_BYTES_PER_UNIT_MAX: isize = 3;
const STATUS_SUCCESS: Status = 0;

#[repr(C)]
struct Range {
    location: isize,
    length: isize,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    static kCFAbsoluteTimeIntervalSince1970: f64;
    fn CFDataCreate(allocator: Reference, bytes: *const u8, length: isize) -> Reference;
    fn CFArrayCreate(
        allocator: Reference,
        values: *const Reference,
        count: isize,
        callbacks: *const c_void,
    ) -> Reference;
    fn CFDateCreate(allocator: Reference, at: f64) -> Reference;
    fn CFStringCreateWithBytes(
        allocator: Reference,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> Reference;
    fn CFStringGetLength(string: Reference) -> isize;
    fn CFStringGetBytes(
        string: Reference,
        range: Range,
        encoding: u32,
        loss: u8,
        external: u8,
        buffer: *mut u8,
        capacity: isize,
        used: *mut isize,
    ) -> isize;
    fn CFErrorGetCode(error: Reference) -> isize;
    fn CFErrorCopyDescription(error: Reference) -> Reference;
    fn CFRelease(object: Reference);
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCertificateCreateWithData(allocator: Reference, data: Reference) -> Reference;
    fn SecPolicyCreateSSL(server: u8, hostname: Reference) -> Reference;
    fn SecTrustCreateWithCertificates(
        certificates: Reference,
        policies: Reference,
        trust: *mut Reference,
    ) -> Status;
    fn SecTrustSetVerifyDate(trust: Reference, date: Reference) -> Status;
    fn SecTrustSetOCSPResponse(trust: Reference, response: Reference) -> Status;
    fn SecTrustSetAnchorCertificates(trust: Reference, anchors: Reference) -> Status;
    fn SecTrustSetAnchorCertificatesOnly(trust: Reference, only: u8) -> Status;
    fn SecTrustEvaluateWithError(trust: Reference, error: *mut Reference) -> bool;
}

struct Owned(Reference);

impl Owned {
    fn new(reference: Reference) -> Option<Self> {
        (!reference.is_null()).then_some(Self(reference))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

#[derive(Debug)]
pub enum TrustError {
    BadCertificate,
    Refused(&'static str),
    Status(&'static str, Status),
    Rejected { code: isize, description: String },
}

fn length(bytes: &[u8]) -> isize {
    isize::try_from(bytes.len()).expect("a slice is at most isize::MAX bytes")
}

fn data(bytes: &[u8]) -> Option<Owned> {
    Owned::new(unsafe { CFDataCreate(std::ptr::null(), bytes.as_ptr(), length(bytes)) })
}

fn array(items: &[Owned]) -> Option<Owned> {
    let references: Vec<Reference> = items.iter().map(|item| item.0).collect();
    Owned::new(unsafe {
        CFArrayCreate(
            std::ptr::null(),
            references.as_ptr(),
            isize::try_from(references.len()).expect("an array is at most isize::MAX items"),
            &raw const kCFTypeArrayCallBacks,
        )
    })
}

fn certificate(der: &[u8]) -> Result<Owned, TrustError> {
    let data = data(der).ok_or(TrustError::Refused("CFDataCreate"))?;
    Owned::new(unsafe { SecCertificateCreateWithData(std::ptr::null(), data.0) })
        .ok_or(TrustError::BadCertificate)
}

pub fn parses_certificate(der: &[u8]) -> bool {
    certificate(der).is_ok()
}

const fn checked(call: &'static str, status: Status) -> Result<(), TrustError> {
    if status != STATUS_SUCCESS {
        return Err(TrustError::Status(call, status));
    }
    Ok(())
}

fn string(text: &str) -> Option<Owned> {
    Owned::new(unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            text.as_ptr(),
            length(text.as_bytes()),
            UTF8,
            0,
        )
    })
}

fn text(string: &Owned) -> String {
    let length = unsafe { CFStringGetLength(string.0) };
    let capacity = length * UTF8_BYTES_PER_UNIT_MAX;
    let mut buffer = vec![0_u8; usize::try_from(capacity).unwrap_or(0)];
    let mut used = 0;
    unsafe {
        CFStringGetBytes(
            string.0,
            Range {
                location: 0,
                length,
            },
            UTF8,
            0,
            0,
            buffer.as_mut_ptr(),
            capacity,
            &mut used,
        )
    };
    buffer.truncate(usize::try_from(used).unwrap_or(0));
    String::from_utf8_lossy(&buffer).into_owned()
}

pub struct ServerTrust<'a> {
    pub chain: &'a [&'a [u8]],
    pub host: &'a str,
    pub unix_seconds: u64,
    pub ocsp_response: Option<&'a [u8]>,
    pub anchors: &'a [&'a [u8]],
}

impl ServerTrust<'_> {
    pub fn evaluate(&self) -> Result<(), TrustError> {
        assert!(
            !self.chain.is_empty(),
            "a server presents at least one certificate"
        );
        let certificates = self
            .chain
            .iter()
            .map(|der| certificate(der))
            .collect::<Result<Vec<_>, _>>()?;
        let certificates = array(&certificates).ok_or(TrustError::Refused("CFArrayCreate"))?;
        let host = string(self.host).ok_or(TrustError::Refused("CFStringCreateWithBytes"))?;
        let policy = Owned::new(unsafe { SecPolicyCreateSSL(1, host.0) })
            .ok_or(TrustError::Refused("SecPolicyCreateSSL"))?;
        let mut trust = std::ptr::null();
        checked("SecTrustCreateWithCertificates", unsafe {
            SecTrustCreateWithCertificates(certificates.0, policy.0, &mut trust)
        })?;
        let trust =
            Owned::new(trust).ok_or(TrustError::Refused("SecTrustCreateWithCertificates"))?;
        let epoch = unsafe { kCFAbsoluteTimeIntervalSince1970 };
        let date =
            Owned::new(unsafe { CFDateCreate(std::ptr::null(), self.unix_seconds as f64 - epoch) })
                .ok_or(TrustError::Refused("CFDateCreate"))?;
        checked("SecTrustSetVerifyDate", unsafe {
            SecTrustSetVerifyDate(trust.0, date.0)
        })?;
        if let Some(response) = self.ocsp_response {
            let responses = [data(response).ok_or(TrustError::Refused("CFDataCreate"))?];
            let responses = array(&responses).ok_or(TrustError::Refused("CFArrayCreate"))?;
            checked("SecTrustSetOCSPResponse", unsafe {
                SecTrustSetOCSPResponse(trust.0, responses.0)
            })?;
        }
        if !self.anchors.is_empty() {
            let anchors = self
                .anchors
                .iter()
                .map(|der| certificate(der))
                .collect::<Result<Vec<_>, _>>()?;
            let anchors = array(&anchors).ok_or(TrustError::Refused("CFArrayCreate"))?;
            checked("SecTrustSetAnchorCertificates", unsafe {
                SecTrustSetAnchorCertificates(trust.0, anchors.0)
            })?;
            checked("SecTrustSetAnchorCertificatesOnly", unsafe {
                SecTrustSetAnchorCertificatesOnly(trust.0, 0)
            })?;
        }
        let mut error = std::ptr::null();
        if unsafe { SecTrustEvaluateWithError(trust.0, &mut error) } {
            return Ok(());
        }
        let error = Owned::new(error).ok_or(TrustError::Refused("SecTrustEvaluateWithError"))?;
        let code = unsafe { CFErrorGetCode(error.0) };
        let description = Owned::new(unsafe { CFErrorCopyDescription(error.0) })
            .map(|description| text(&description))
            .unwrap_or_default();
        Err(TrustError::Rejected { code, description })
    }
}

use crate::libc;
use std::{
    ffi::{CStr, c_void},
    fmt,
    marker::PhantomData,
    mem,
    num::NonZeroI32,
    ptr::{self, NonNull},
    slice,
    sync::OnceLock,
};

type OsStatus = i32;
type Index = isize;
type TypeId = usize;
type Boolean = u8;
type StringEncoding = u32;
type NumberType = isize;
type Reference = *const c_void;

#[repr(C)]
struct Range {
    location: Index,
    length: Index,
}

type CopyCertificates = unsafe extern "C" fn(u32, *mut Reference) -> OsStatus;
type CopyTrustSettings = unsafe extern "C" fn(Reference, u32, *mut Reference) -> OsStatus;
type CopyData = unsafe extern "C" fn(Reference) -> Reference;
type CopyErrorMessage = unsafe extern "C" fn(OsStatus, *mut c_void) -> Reference;
type GetLength = unsafe extern "C" fn(Reference) -> Index;
type GetValueAtIndex = unsafe extern "C" fn(Reference, Index) -> Reference;
type Release = unsafe extern "C" fn(Reference);
type GetBytePointer = unsafe extern "C" fn(Reference) -> *const u8;
type GetValueIfPresent = unsafe extern "C" fn(Reference, Reference, *mut Reference) -> Boolean;
type CreateString =
    unsafe extern "C" fn(Reference, *const u8, Index, StringEncoding, Boolean) -> Reference;
type Equal = unsafe extern "C" fn(Reference, Reference) -> Boolean;
type GetNumberValue = unsafe extern "C" fn(Reference, NumberType, *mut c_void) -> Boolean;
type GetTypeId = unsafe extern "C" fn(Reference) -> TypeId;
type ClassTypeId = unsafe extern "C" fn() -> TypeId;
type GetBytes = unsafe extern "C" fn(
    Reference,
    Range,
    StringEncoding,
    u8,
    Boolean,
    *mut u8,
    Index,
    *mut Index,
) -> Index;

const SECURITY_PATH: &CStr = c"/System/Library/Frameworks/Security.framework/Security";
const CORE_FOUNDATION_PATH: &CStr =
    c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation";
const STATUS_SUCCESS: OsStatus = 0;
const STATUS_NO_TRUST_SETTINGS: OsStatus = -25263;
const DOMAIN_USER: u32 = 0;
const DOMAIN_ADMIN: u32 = 1;
const DOMAIN_SYSTEM: u32 = 2;
const RESULT_TRUST_ROOT: i64 = 1;
const RESULT_TRUST_AS_ROOT: i64 = 2;
const RESULT_DENY: i64 = 3;
const NUMBER_SIGNED_64: NumberType = 4;
const ENCODING_UTF8: StringEncoding = 0x0800_0100;
const FALSE: Boolean = 0;
const POLICY_NAME_KEY: &str = "kSecTrustSettingsPolicyName";
const SSL_SERVER_POLICY_NAME: &str = "sslServer";
const RESULT_KEY: &str = "kSecTrustSettingsResult";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    User,
    Admin,
    System,
}

impl Domain {
    const fn value(self) -> u32 {
        match self {
            Self::User => DOMAIN_USER,
            Self::Admin => DOMAIN_ADMIN,
            Self::System => DOMAIN_SYSTEM,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustSetting {
    TrustRoot,
    TrustAsRoot,
    Deny,
}

#[derive(Debug)]
pub struct Certificate {
    pub der: Vec<u8>,
    pub trust: Result<Option<TrustSetting>, Status>,
}

#[derive(Clone)]
pub struct Status {
    code: NonZeroI32,
    message: Option<String>,
}

impl Status {
    pub const fn code(&self) -> i32 {
        self.code.get()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut builder = formatter.debug_struct("Error");
        builder.field("code", &self.code);
        if let Some(message) = &self.message {
            builder.field("message", message);
        }
        builder.finish()
    }
}

impl fmt::Display for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.message {
            Some(message) => formatter.write_str(message),
            None => write!(formatter, "error code {}", self.code),
        }
    }
}

impl std::error::Error for Status {}

#[derive(Clone, Debug)]
pub struct LibraryError {
    name: &'static CStr,
    message: Option<String>,
}

impl fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.name.to_string_lossy();
        match &self.message {
            Some(message) => write!(formatter, "loading {name}: {message}"),
            None => write!(formatter, "loading {name} failed"),
        }
    }
}

impl std::error::Error for LibraryError {}

#[derive(Clone)]
pub enum TrustSettingsError {
    Status(Status),
    Library(LibraryError),
}

impl fmt::Debug for TrustSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => fmt::Debug::fmt(status, formatter),
            Self::Library(error) => fmt::Debug::fmt(error, formatter),
        }
    }
}

impl fmt::Display for TrustSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status) => fmt::Display::fmt(status, formatter),
            Self::Library(error) => fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for TrustSettingsError {}

pub fn certificates(domain: Domain) -> Result<Vec<Certificate>, TrustSettingsError> {
    let library = library().map_err(TrustSettingsError::Library)?;
    let Some(array) = library
        .copy_certificates(domain)
        .map_err(TrustSettingsError::Status)?
    else {
        return Ok(Vec::new());
    };
    let keys = Keys {
        policy_name: library.create_string(POLICY_NAME_KEY),
        ssl_server: library.create_string(SSL_SERVER_POLICY_NAME),
        result: library.create_string(RESULT_KEY),
    };
    let certificates = library
        .array_values(array.borrow())
        .into_iter()
        .map(|certificate| Certificate {
            der: library.certificate_der(certificate),
            trust: library.trust_setting(certificate, domain, &keys),
        })
        .collect();
    Ok(certificates)
}

struct Keys {
    policy_name: Owned,
    ssl_server: Owned,
    result: Owned,
}

struct Library {
    copy_certificates: CopyCertificates,
    copy_trust_settings: CopyTrustSettings,
    copy_data: CopyData,
    copy_error_message: CopyErrorMessage,
    array_count: GetLength,
    array_value: GetValueAtIndex,
    release: Release,
    data_length: GetLength,
    data_bytes: GetBytePointer,
    dictionary_value: GetValueIfPresent,
    dictionary_type: ClassTypeId,
    create_string: CreateString,
    string_length: GetLength,
    string_bytes: GetBytes,
    equal: Equal,
    number_value: GetNumberValue,
    number_type: ClassTypeId,
    type_of: GetTypeId,
}

static LIBRARY: OnceLock<Result<Library, LibraryError>> = OnceLock::new();

fn library() -> Result<&'static Library, LibraryError> {
    LIBRARY
        .get_or_init(Library::load)
        .as_ref()
        .map_err(Clone::clone)
}

fn dynamic_error() -> Option<String> {
    let message = unsafe { libc::dlerror() };
    if message.is_null() {
        return None;
    }
    let message = unsafe { CStr::from_ptr(message) };
    Some(message.to_string_lossy().into_owned())
}

fn open(path: &'static CStr) -> Result<NonNull<c_void>, LibraryError> {
    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    NonNull::new(handle).ok_or_else(|| LibraryError {
        name: path,
        message: dynamic_error(),
    })
}

fn resolve(handle: NonNull<c_void>, name: &'static CStr) -> Result<*mut c_void, LibraryError> {
    let address = unsafe { libc::dlsym(handle.as_ptr(), name.as_ptr()) };
    if address.is_null() {
        return Err(LibraryError {
            name,
            message: dynamic_error(),
        });
    }
    Ok(address)
}

impl Library {
    fn load() -> Result<Self, LibraryError> {
        let security = open(SECURITY_PATH)?;
        let core_foundation = open(CORE_FOUNDATION_PATH)?;
        let copy_certificates = resolve(security, c"SecTrustSettingsCopyCertificates")?;
        let copy_trust_settings = resolve(security, c"SecTrustSettingsCopyTrustSettings")?;
        let copy_data = resolve(security, c"SecCertificateCopyData")?;
        let copy_error_message = resolve(security, c"SecCopyErrorMessageString")?;
        let array_count = resolve(core_foundation, c"CFArrayGetCount")?;
        let array_value = resolve(core_foundation, c"CFArrayGetValueAtIndex")?;
        let release = resolve(core_foundation, c"CFRelease")?;
        let data_length = resolve(core_foundation, c"CFDataGetLength")?;
        let data_bytes = resolve(core_foundation, c"CFDataGetBytePtr")?;
        let dictionary_value = resolve(core_foundation, c"CFDictionaryGetValueIfPresent")?;
        let dictionary_type = resolve(core_foundation, c"CFDictionaryGetTypeID")?;
        let create_string = resolve(core_foundation, c"CFStringCreateWithBytes")?;
        let string_length = resolve(core_foundation, c"CFStringGetLength")?;
        let string_bytes = resolve(core_foundation, c"CFStringGetBytes")?;
        let equal = resolve(core_foundation, c"CFEqual")?;
        let number_value = resolve(core_foundation, c"CFNumberGetValue")?;
        let number_type = resolve(core_foundation, c"CFNumberGetTypeID")?;
        let type_of = resolve(core_foundation, c"CFGetTypeID")?;
        Ok(unsafe {
            Self {
                copy_certificates: mem::transmute::<*mut c_void, CopyCertificates>(
                    copy_certificates,
                ),
                copy_trust_settings: mem::transmute::<*mut c_void, CopyTrustSettings>(
                    copy_trust_settings,
                ),
                copy_data: mem::transmute::<*mut c_void, CopyData>(copy_data),
                copy_error_message: mem::transmute::<*mut c_void, CopyErrorMessage>(
                    copy_error_message,
                ),
                array_count: mem::transmute::<*mut c_void, GetLength>(array_count),
                array_value: mem::transmute::<*mut c_void, GetValueAtIndex>(array_value),
                release: mem::transmute::<*mut c_void, Release>(release),
                data_length: mem::transmute::<*mut c_void, GetLength>(data_length),
                data_bytes: mem::transmute::<*mut c_void, GetBytePointer>(data_bytes),
                dictionary_value: mem::transmute::<*mut c_void, GetValueIfPresent>(
                    dictionary_value,
                ),
                dictionary_type: mem::transmute::<*mut c_void, ClassTypeId>(dictionary_type),
                create_string: mem::transmute::<*mut c_void, CreateString>(create_string),
                string_length: mem::transmute::<*mut c_void, GetLength>(string_length),
                string_bytes: mem::transmute::<*mut c_void, GetBytes>(string_bytes),
                equal: mem::transmute::<*mut c_void, Equal>(equal),
                number_value: mem::transmute::<*mut c_void, GetNumberValue>(number_value),
                number_type: mem::transmute::<*mut c_void, ClassTypeId>(number_type),
                type_of: mem::transmute::<*mut c_void, GetTypeId>(type_of),
            }
        })
    }

    fn owned(&'static self, reference: Reference) -> Option<Owned> {
        NonNull::new(reference.cast_mut()).map(|reference| Owned {
            reference,
            library: self,
        })
    }

    fn status(&'static self, code: OsStatus) -> Status {
        let code = NonZeroI32::new(code).expect("a failing OSStatus is not errSecSuccess");
        Status {
            code,
            message: self.error_message(code.get()),
        }
    }

    fn error_message(&'static self, code: OsStatus) -> Option<String> {
        let message = unsafe { (self.copy_error_message)(code, ptr::null_mut()) };
        let message = self.owned(message)?;
        Some(self.string(message.borrow()))
    }

    fn copy_certificates(&'static self, domain: Domain) -> Result<Option<Owned>, Status> {
        let mut array: Reference = ptr::null();
        let status = unsafe { (self.copy_certificates)(domain.value(), &mut array) };
        match status {
            STATUS_NO_TRUST_SETTINGS => Ok(None),
            STATUS_SUCCESS => {
                let array = self.owned(array);
                assert!(
                    array.is_some(),
                    "SecTrustSettingsCopyCertificates succeeded without an array"
                );
                Ok(array)
            }
            failed => Err(self.status(failed)),
        }
    }

    fn copy_trust_settings(
        &'static self,
        certificate: Borrowed<'_>,
        domain: Domain,
    ) -> Result<Owned, Status> {
        let mut array: Reference = ptr::null();
        let status = unsafe {
            (self.copy_trust_settings)(certificate.reference(), domain.value(), &mut array)
        };
        if status != STATUS_SUCCESS {
            return Err(self.status(status));
        }
        Ok(self
            .owned(array)
            .expect("SecTrustSettingsCopyTrustSettings succeeded without an array"))
    }

    fn array_values<'array>(&'static self, array: Borrowed<'array>) -> Vec<Borrowed<'array>> {
        let count = unsafe { (self.array_count)(array.reference()) };
        assert!(count >= 0, "CFArrayGetCount is never negative");
        (0..count)
            .map(|index| {
                let value = unsafe { (self.array_value)(array.reference(), index) };
                Borrowed::new(value).expect("a Security array holds no null values")
            })
            .collect()
    }

    fn certificate_der(&'static self, certificate: Borrowed<'_>) -> Vec<u8> {
        let data = unsafe { (self.copy_data)(certificate.reference()) };
        let data = self
            .owned(data)
            .expect("SecCertificateCopyData returns the certificate's data");
        let length = unsafe { (self.data_length)(data.borrow().reference()) };
        let length = usize::try_from(length).expect("CFDataGetLength is never negative");
        let bytes = unsafe { (self.data_bytes)(data.borrow().reference()) };
        if bytes.is_null() {
            return Vec::new();
        }
        let bytes = unsafe { slice::from_raw_parts(bytes, length) };
        bytes.to_vec()
    }

    fn create_string(&'static self, text: &'static str) -> Owned {
        let length = Index::try_from(text.len()).expect("a key's length fits a CFIndex");
        let string = unsafe {
            (self.create_string)(ptr::null(), text.as_ptr(), length, ENCODING_UTF8, FALSE)
        };
        self.owned(string)
            .expect("CFStringCreateWithBytes creates a string from UTF-8")
    }

    fn string(&'static self, string: Borrowed<'_>) -> String {
        let length = unsafe { (self.string_length)(string.reference()) };
        let range = || Range {
            location: 0,
            length,
        };
        let mut required: Index = 0;
        unsafe {
            (self.string_bytes)(
                string.reference(),
                range(),
                ENCODING_UTF8,
                0,
                FALSE,
                ptr::null_mut(),
                0,
                &mut required,
            )
        };
        let capacity =
            usize::try_from(required).expect("CFStringGetBytes never needs a negative length");
        let mut buffer = vec![0_u8; capacity];
        let mut used: Index = 0;
        let written = unsafe {
            (self.string_bytes)(
                string.reference(),
                range(),
                ENCODING_UTF8,
                0,
                FALSE,
                buffer.as_mut_ptr(),
                required,
                &mut used,
            )
        };
        assert_eq!(written, length, "CFStringGetBytes converts every character");
        assert_eq!(
            used, required,
            "CFStringGetBytes fills the length it asked for"
        );
        String::from_utf8(buffer).expect("CFStringGetBytes with kCFStringEncodingUTF8 writes UTF-8")
    }

    fn dictionary_value<'value>(
        &'static self,
        dictionary: Borrowed<'value>,
        key: Borrowed<'_>,
    ) -> Option<Borrowed<'value>> {
        let mut value: Reference = ptr::null();
        let present =
            unsafe { (self.dictionary_value)(dictionary.reference(), key.reference(), &mut value) };
        if present == FALSE {
            return None;
        }
        Some(Borrowed::new(value).expect("a CFDictionary holds no null values"))
    }

    fn equal(&'static self, left: Borrowed<'_>, right: Borrowed<'_>) -> bool {
        unsafe { (self.equal)(left.reference(), right.reference()) != FALSE }
    }

    fn is_type(&'static self, value: Borrowed<'_>, class: ClassTypeId) -> bool {
        let found = unsafe { (self.type_of)(value.reference()) };
        let expected = unsafe { class() };
        found == expected
    }

    fn number(&'static self, number: Borrowed<'_>) -> Option<i64> {
        assert!(
            self.is_type(number, self.number_type),
            "kSecTrustSettingsResult is a CFNumber"
        );
        let mut value: i64 = 0;
        let converted = unsafe {
            (self.number_value)(
                number.reference(),
                NUMBER_SIGNED_64,
                ptr::from_mut(&mut value).cast(),
            )
        };
        (converted != FALSE).then_some(value)
    }

    fn trust_setting(
        &'static self,
        certificate: Borrowed<'_>,
        domain: Domain,
        keys: &Keys,
    ) -> Result<Option<TrustSetting>, Status> {
        let settings = self.copy_trust_settings(certificate, domain)?;
        for setting in self.array_values(settings.borrow()) {
            assert!(
                self.is_type(setting, self.dictionary_type),
                "a trust setting is a CFDictionary"
            );
            if let Some(name) = self.dictionary_value(setting, keys.policy_name.borrow())
                && !self.equal(name, keys.ssl_server.borrow())
            {
                continue;
            }
            let result = self
                .dictionary_value(setting, keys.result.borrow())
                .and_then(|number| self.number(number))
                .unwrap_or(RESULT_TRUST_ROOT);
            match result {
                RESULT_TRUST_ROOT => return Ok(Some(TrustSetting::TrustRoot)),
                RESULT_TRUST_AS_ROOT => return Ok(Some(TrustSetting::TrustAsRoot)),
                RESULT_DENY => return Ok(Some(TrustSetting::Deny)),
                _ => continue,
            }
        }
        Ok(None)
    }
}

struct Owned {
    reference: NonNull<c_void>,
    library: &'static Library,
}

impl Owned {
    const fn borrow(&self) -> Borrowed<'_> {
        Borrowed {
            reference: self.reference,
            owner: PhantomData,
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { (self.library.release)(self.reference.as_ptr()) }
    }
}

#[derive(Clone, Copy)]
struct Borrowed<'owner> {
    reference: NonNull<c_void>,
    owner: PhantomData<&'owner Owned>,
}

impl Borrowed<'_> {
    fn new(reference: Reference) -> Option<Self> {
        NonNull::new(reference.cast_mut()).map(|reference| Self {
            reference,
            owner: PhantomData,
        })
    }

    const fn reference(self) -> Reference {
        self.reference.as_ptr().cast_const()
    }
}

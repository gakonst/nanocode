use crate::windows_sys::Win32::{
    Foundation::{CRYPT_E_NOT_FOUND, S_OK},
    Security::Cryptography::{
        CERT_CONTEXT, CERT_STORE_PROV_SYSTEM_W, CERT_SYSTEM_STORE_CURRENT_USER_ID,
        CERT_SYSTEM_STORE_LOCAL_MACHINE_ID, CERT_SYSTEM_STORE_LOCATION_SHIFT, CTL_USAGE,
        CertCloseStore, CertEnumCertificatesInStore, CertGetEnhancedKeyUsage, CertOpenStore,
        CertVerifyTimeValidity, HCERTSTORE,
    },
};
use std::{ffi::CStr, io, mem, ptr, slice};

const ROOT_STORE: &str = "ROOT";
const TIME_VALID: i32 = 0;
const FAILED: i32 = 0;

#[derive(Debug, PartialEq, Eq)]
pub enum Usages {
    All,
    Identifiers(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    CurrentUser,
    LocalMachine,
}

#[derive(Debug)]
pub struct Certificate {
    pub der: Vec<u8>,
    pub encoding: u32,
    pub usages: io::Result<Usages>,
    pub time_valid: bool,
}

struct Store(HCERTSTORE);

impl Drop for Store {
    fn drop(&mut self) {
        unsafe { CertCloseStore(self.0, 0) };
    }
}

pub fn current_user_roots() -> io::Result<Vec<Certificate>> {
    system_certificates(Location::CurrentUser, ROOT_STORE)
}

pub fn system_certificates(location: Location, store_name: &str) -> io::Result<Vec<Certificate>> {
    let location_id = match location {
        Location::CurrentUser => CERT_SYSTEM_STORE_CURRENT_USER_ID,
        Location::LocalMachine => CERT_SYSTEM_STORE_LOCAL_MACHINE_ID,
    };
    let name: Vec<u16> = store_name.encode_utf16().chain([0]).collect();
    let store = unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            0,
            0,
            location_id << CERT_SYSTEM_STORE_LOCATION_SHIFT,
            name.as_ptr().cast(),
        )
    };
    if store.is_null() {
        return Err(io::Error::last_os_error());
    }
    let store = Store(store);
    let mut certificates = Vec::new();
    let mut context: *mut CERT_CONTEXT = ptr::null_mut();
    loop {
        context = unsafe { CertEnumCertificatesInStore(store.0, context) };
        let Some(current) = (unsafe { context.as_ref() }) else {
            return Ok(certificates);
        };
        certificates.push(certificate(current));
    }
}

fn certificate(context: &CERT_CONTEXT) -> Certificate {
    let length = usize::try_from(context.cbCertEncoded).expect("a u32 length fits usize");
    let der = unsafe { slice::from_raw_parts(context.pbCertEncoded, length) };
    let time_valid =
        unsafe { CertVerifyTimeValidity(ptr::null(), context.pCertInfo) } == TIME_VALID;
    Certificate {
        der: der.to_vec(),
        encoding: context.dwCertEncodingType,
        usages: usages(context),
        time_valid,
    }
}

fn usages(context: &CERT_CONTEXT) -> io::Result<Usages> {
    let mut length: u32 = 0;
    if unsafe { CertGetEnhancedKeyUsage(context, 0, ptr::null_mut(), &mut length) } == FAILED {
        return Err(io::Error::last_os_error());
    }
    let bytes = usize::try_from(length).expect("a u32 length fits usize");
    let mut buffer = vec![0_u64; bytes.div_ceil(mem::size_of::<u64>())];
    let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
    if unsafe { CertGetEnhancedKeyUsage(context, 0, usage, &mut length) } == FAILED {
        return Err(io::Error::last_os_error());
    }
    let usage = unsafe { &*usage };
    if usage.cUsageIdentifier == 0 {
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(CRYPT_E_NOT_FOUND) => return Ok(Usages::All),
            Some(S_OK) => {}
            _ => return Err(error),
        }
    }
    let count = usize::try_from(usage.cUsageIdentifier).expect("a u32 count fits usize");
    if count == 0 {
        return Ok(Usages::Identifiers(Vec::new()));
    }
    let identifiers = unsafe { slice::from_raw_parts(usage.rgpszUsageIdentifier, count) };
    Ok(Usages::Identifiers(
        identifiers
            .iter()
            .map(|identifier| {
                let identifier = unsafe { CStr::from_ptr(identifier.cast_const().cast()) };
                identifier.to_string_lossy().into_owned()
            })
            .collect(),
    ))
}

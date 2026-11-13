use std::io;

use nanocode_sys::certificate_store::{self, Location, Usages};
use windows_sys::Win32::{
    Foundation::{CRYPT_E_NOT_FOUND, GetLastError},
    Security::Cryptography::{
        CRL_CONTEXT, CertCloseStore, CertEnumCRLsInStore, CertOpenSystemStoreW,
        PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodingType {
    X509Asn,
    Pkcs7Asn,
    Other(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CertificateUses {
    All,
    Oids(Vec<String>),
}

#[derive(Debug)]
pub struct CertificateEntry {
    pub der: Vec<u8>,
    pub encoding: EncodingType,
    pub valid_uses: io::Result<CertificateUses>,
}

#[derive(Debug)]
pub struct CertificateEntries {
    pub had_open_store: bool,
    pub entries: Vec<CertificateEntry>,
}

#[derive(Debug)]
pub struct CrlEntry {
    pub der: Vec<u8>,
    pub encoding: EncodingType,
}

fn encoding_type(raw: u32) -> EncodingType {
    if raw & X509_ASN_ENCODING != 0 {
        EncodingType::X509Asn
    } else if raw & PKCS_7_ASN_ENCODING != 0 {
        EncodingType::Pkcs7Asn
    } else {
        EncodingType::Other(raw)
    }
}

pub fn enum_certificates(store_name: &str) -> CertificateEntries {
    let locations = [Location::CurrentUser, Location::LocalMachine];
    let mut had_open_store = false;
    let mut entries = Vec::new();

    for location in locations {
        let Ok(certificates) = certificate_store::system_certificates(location, store_name) else {
            continue;
        };
        had_open_store = true;

        for certificate in certificates {
            let valid_uses = certificate.usages.map_or_else(
                |err| Err(io::Error::other(err)),
                |usages| {
                    Ok(match usages {
                        Usages::All => CertificateUses::All,
                        Usages::Identifiers(oids) => CertificateUses::Oids(oids),
                    })
                },
            );
            entries.push(CertificateEntry {
                der: certificate.der,
                encoding: encoding_type(certificate.encoding),
                valid_uses,
            });
        }
    }

    CertificateEntries {
        had_open_store,
        entries,
    }
}

pub fn enum_crls(store_name: &str) -> io::Result<Vec<CrlEntry>> {
    let store_name_wide: Vec<u16> = store_name
        .encode_utf16()
        .chain(core::iter::once(0))
        .collect();

    let store = unsafe { CertOpenSystemStoreW(0, store_name_wide.as_ptr()) };
    if store.is_null() {
        return Err(io::Error::last_os_error());
    }

    let mut result = Vec::new();
    let mut crl_context: *const CRL_CONTEXT = core::ptr::null();
    loop {
        crl_context = unsafe { CertEnumCRLsInStore(store, crl_context) };
        if crl_context.is_null() {
            let err = unsafe { GetLastError() };
            if err != CRYPT_E_NOT_FOUND as u32 {
                unsafe {
                    CertCloseStore(store, 0);
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            break;
        }

        let crl = unsafe { &*crl_context };
        let der =
            unsafe { core::slice::from_raw_parts(crl.pbCrlEncoded, crl.cbCrlEncoded as usize) }
                .to_vec();
        result.push(CrlEntry {
            der,
            encoding: encoding_type(crl.dwCertEncodingType),
        });
    }

    unsafe {
        CertCloseStore(store, 0);
    }

    Ok(result)
}

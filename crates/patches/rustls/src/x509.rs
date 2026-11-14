// Additional x509/asn1 functions to those provided in webpki/ring.

use alloc::vec::Vec;

/// Prepend stuff to `bytes` to put it in a DER SEQUENCE.
pub(crate) fn wrap_in_sequence(bytes: &[u8]) -> Vec<u8> {
    asn1_wrap(DER_SEQUENCE_TAG, bytes, &[])
}

/// Prepend stuff to `bytes_a` + `bytes_b` to put it in a DER SEQUENCE.
#[allow(dead_code)]
pub(crate) fn wrap_concat_in_sequence(bytes_a: &[u8], bytes_b: &[u8]) -> Vec<u8> {
    asn1_wrap(DER_SEQUENCE_TAG, bytes_a, bytes_b)
}

/// Prepend stuff to `bytes` to put it in a DER BIT STRING.
pub(crate) fn wrap_in_bit_string(bytes: &[u8]) -> Vec<u8> {
    asn1_wrap(DER_BIT_STRING_TAG, &[0u8], bytes)
}

/// Prepend stuff to `bytes` to put it in a DER OCTET STRING.
#[allow(dead_code)]
pub(crate) fn wrap_in_octet_string(bytes: &[u8]) -> Vec<u8> {
    asn1_wrap(DER_OCTET_STRING_TAG, bytes, &[])
}

fn asn1_wrap(tag: u8, bytes_a: &[u8], bytes_b: &[u8]) -> Vec<u8> {
    let len = bytes_a.len() + bytes_b.len();

    if len <= 0x7f {
        // Short form
        let mut ret = Vec::with_capacity(2 + len);
        ret.push(tag);
        ret.push(len as u8);
        ret.extend_from_slice(bytes_a);
        ret.extend_from_slice(bytes_b);
        ret
    } else {
        // Long form
        let size = len.to_be_bytes();
        let leading_zero_bytes = size
            .iter()
            .position(|&x| x != 0)
            .unwrap_or(size.len());
        assert!(leading_zero_bytes < size.len());
        let encoded_bytes = size.len() - leading_zero_bytes;

        let mut ret = Vec::with_capacity(2 + encoded_bytes + len);
        ret.push(tag);

        ret.push(0x80 + encoded_bytes as u8);
        ret.extend_from_slice(&size[leading_zero_bytes..]);

        ret.extend_from_slice(bytes_a);
        ret.extend_from_slice(bytes_b);
        ret
    }
}

const DER_SEQUENCE_TAG: u8 = 0x30;
const DER_BIT_STRING_TAG: u8 = 0x03;
const DER_OCTET_STRING_TAG: u8 = 0x04;


// Copyright 2015-2020 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

use alloc::format;

use pki_types::IpAddr;
use pki_types::ServerName;

use super::{GeneralName, NameIterator};
use crate::cert::Cert;
use crate::error::{Error, InvalidNameContext};

pub(crate) fn verify_ip_address_names(reference: &IpAddr, cert: &Cert<'_>) -> Result<(), Error> {
    let ip_address = match reference {
        IpAddr::V4(ip) => crate::untrusted::Input::from(ip.as_ref()),
        IpAddr::V6(ip) => crate::untrusted::Input::from(ip.as_ref()),
    };

    let result = NameIterator::new(cert.subject_alt_name).find_map(|result| {
        let name = match result {
            Ok(name) => name,
            Err(err) => return Some(Err(err)),
        };

        let presented_id = match name {
            GeneralName::IpAddress(presented) => presented,
            _ => return None,
        };

        match presented_id_matches_reference_id(presented_id, ip_address) {
            true => Some(Ok(())),
            false => None,
        }
    });

    match result {
        Some(result) => return result,
        None => {}
    }

    {
        Err(Error::CertNotValidForName(InvalidNameContext {
            expected: ServerName::from(*reference),
            presented: NameIterator::new(cert.subject_alt_name)
                .filter_map(|result| Some(format!("{:?}", result.ok()?)))
                .collect(),
        }))
    }
}

// https://tools.ietf.org/html/rfc5280#section-4.2.1.6 says:
//   When the subjectAltName extension contains an iPAddress, the address
//   MUST be stored in the octet string in "network byte order", as
//   specified in [RFC791].  The least significant bit (LSB) of each octet
//   is the LSB of the corresponding byte in the network address.  For IP
//   version 4, as specified in [RFC791], the octet string MUST contain
//   exactly four octets.  For IP version 6, as specified in
//   [RFC2460], the octet string MUST contain exactly sixteen octets.
fn presented_id_matches_reference_id(
    presented_id: crate::untrusted::Input<'_>,
    reference_id: crate::untrusted::Input<'_>,
) -> bool {
    match (presented_id.len(), reference_id.len()) {
        (4, 4) => (),
        (16, 16) => (),
        _ => {
            return false;
        }
    };

    let mut presented_ip_address = crate::untrusted::Reader::new(presented_id);
    let mut reference_ip_address = crate::untrusted::Reader::new(reference_id);
    while !presented_ip_address.at_end() {
        let presented_ip_address_byte = presented_ip_address.read_byte().unwrap();
        let reference_ip_address_byte = reference_ip_address.read_byte().unwrap();
        if presented_ip_address_byte != reference_ip_address_byte {
            return false;
        }
    }

    true
}

// https://tools.ietf.org/html/rfc5280#section-4.2.1.10 says:
//
//     For IPv4 addresses, the iPAddress field of GeneralName MUST contain
//     eight (8) octets, encoded in the style of RFC 4632 (CIDR) to represent
//     an address range [RFC4632].  For IPv6 addresses, the iPAddress field
//     MUST contain 32 octets similarly encoded.  For example, a name
//     constraint for "class C" subnet 192.0.2.0 is represented as the
//     octets C0 00 02 00 FF FF FF 00, representing the CIDR notation
//     192.0.2.0/24 (mask 255.255.255.0).
pub(super) fn presented_id_matches_constraint(
    name: crate::untrusted::Input<'_>,
    constraint: crate::untrusted::Input<'_>,
) -> Result<bool, Error> {
    match (name.len(), constraint.len()) {
        (4, 8) => (),
        (16, 32) => (),

        // an IPv4 address never matches an IPv6 constraint, and vice versa.
        (4, 32) | (16, 8) => {
            return Ok(false);
        }

        // invalid constraint length
        (4, _) | (16, _) => {
            return Err(Error::InvalidNetworkMaskConstraint);
        }

        // invalid name length, or anything else
        _ => {
            return Err(Error::BadDer);
        }
    };

    let (constraint_address, constraint_mask) = constraint.read_all(Error::BadDer, |value| {
        let address = value.read_bytes(constraint.len() / 2).unwrap();
        let mask = value.read_bytes(constraint.len() / 2).unwrap();
        Ok((address, mask))
    })?;

    let mut name = crate::untrusted::Reader::new(name);
    let mut constraint_address = crate::untrusted::Reader::new(constraint_address);
    let mut constraint_mask = crate::untrusted::Reader::new(constraint_mask);
    let mut seen_zero_bit = false;

    loop {
        // Iterate through the name, constraint address, and constraint mask
        // a byte at a time.
        let name_byte = name.read_byte().unwrap();
        let constraint_address_byte = constraint_address.read_byte().unwrap();
        let constraint_mask_byte = constraint_mask.read_byte().unwrap();

        // A valid mask consists of a sequence of 1 bits, followed by a
        // sequence of 0 bits.  Either sequence could be empty.

        let leading = constraint_mask_byte.leading_ones();
        let trailing = constraint_mask_byte.trailing_zeros();

        // At the resolution of a single octet, a valid mask is one where
        // leading_ones() and trailing_zeros() sums to 8.
        // This includes all-ones and all-zeroes.
        if leading + trailing != 8 {
            return Err(Error::InvalidNetworkMaskConstraint);
        }

        // There should be no bits set after the first octet with a zero bit is seen.
        if seen_zero_bit && constraint_mask_byte != 0x00 {
            return Err(Error::InvalidNetworkMaskConstraint);
        }

        // Note when a zero bit is seen for later octets.
        if constraint_mask_byte != 0xff {
            seen_zero_bit = true;
        }

        if ((name_byte ^ constraint_address_byte) & constraint_mask_byte) != 0 {
            return Ok(false);
        }
        if name.at_end() {
            break;
        }
    }

    Ok(true)
}



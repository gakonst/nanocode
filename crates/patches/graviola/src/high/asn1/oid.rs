// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

#![allow(non_upper_case_globals, dead_code)]

asn1_oid! {
    id_ecPublicKey OBJECT IDENTIFIER ::= {
        iso(1) member_body(2) us(840) 10045 keyType(2) 1
    }
}

asn1_oid! {
    id_prime256v1 OBJECT IDENTIFIER ::= {
        iso(1) member_body(2) us(840) 10045 curves(3) prime(1) 7
    }
}

asn1_oid! {
    secp384r1 OBJECT IDENTIFIER ::= {
        iso(1) identified_organization(3) certicom(132) curve(0) ansip384r1(34)
    }
}

asn1_oid! {
    rsaEncryption OBJECT IDENTIFIER ::= {
        iso(1) member_body(2)
        us(840) rsadsi(113549) pkcs(1) 1 rsaEncryption(1)
    }
}

asn1_oid! {
    id_sha256 OBJECT IDENTIFIER ::= {
        joint_iso_itu_t(2) country(16) us(840) organization(1) gov(101)
        csor(3) nistalgorithm(4) hashalgs(2) 1
    }
}

asn1_oid! {
    id_sha384 OBJECT IDENTIFIER ::= {
        joint_iso_itu_t(2) country(16) us(840) organization(1) gov(101)
        csor(3) nistalgorithm(4) hashalgs(2) 2
    }
}

asn1_oid! {
    id_sha512 OBJECT IDENTIFIER ::= {
        joint_iso_itu_t(2) country(16) us(840) organization(1) gov(101)
        csor(3) nistalgorithm(4) hashalgs(2) 3
    }
}

asn1_oid! {
    id_ed25519 OBJECT IDENTIFIER ::= { 1 3 101 112 }
}

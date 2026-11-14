// Written for Graviola by Joe Birr-Pixton, 2024.
// SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0

asn1_struct! {
    RSAPublicKey ::= SEQUENCE {
        modulus           INTEGER,
        publicExponent    INTEGER
    }
}

asn1_struct! {
    RSAPrivateKey ::= SEQUENCE {
        version           Version,
        modulus           INTEGER,
        publicExponent    INTEGER,
        privateExponent   INTEGER,
        prime1            INTEGER,
        prime2            INTEGER,
        exponent1         INTEGER,
        exponent2         INTEGER,
        coefficient       INTEGER
    }
}

asn1_enum! {
    Version ::= INTEGER { two_prime(0), multi(1) }
}

// PKCS#8 v2, RFC5958
asn1_struct! {
    OneAsymmetricKey ::= SEQUENCE {
        version                   Pkcs8Version,
        privateKeyAlgorithm       AlgorithmIdentifier REF,
        privateKey                OCTET STRING,
        // no attributes accepted
        publicKey [1]             BIT STRING
    }
}

asn1_enum! {
    Pkcs8Version ::= INTEGER { Pkcs8v1(0), Pkcs8v2(1) }
}

asn1_struct! {
    AlgorithmIdentifier ::= SEQUENCE {
        algorithm                 OBJECT IDENTIFIER,
        parameters                ANY OPTIONAL
    }
}

asn1_struct! {
    EcPrivateKey ::= SEQUENCE {
        version                   EcPrivateKeyVer,
        privateKey                OCTET STRING,
        parameters [0] EXPLICIT   OBJECT IDENTIFIER,
        publicKey  [1] EXPLICIT   BIT STRING
    }
}

asn1_enum! {
    EcPrivateKeyVer ::= INTEGER { ecPrivkeyVer1(1) }
}

asn1_struct! {
    SubjectPublicKeyInfo ::= SEQUENCE {
        algorithm         AlgorithmIdentifier REF,
        subjectPublicKey  BIT STRING
    }
}

asn1_struct! {
    DigestInfo ::= SEQUENCE {
        digestAlgorithm AlgorithmIdentifier REF,
        digest          OCTET STRING
    }
}

asn1_struct! {
    EcdsaSigValue  ::=  SEQUENCE  {
           r     INTEGER,
           s     INTEGER
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::high::asn1::{Any, Encoder, Error, Null, OctetString, Parser, Type, oid};

    #[test]
    fn parse_public_key() {
        let data = include_bytes!("testdata/rsapublickey-1k.bin");
        let key = RSAPublicKey::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);

        dbg!(data.len());
        assert_eq!(key.encoded_len(), data.len());

        let mut buf = [0u8; 256];
        let encode_len = key.encode(&mut Encoder::new(&mut buf)).unwrap();
        dbg!(encode_len);
        println!("{:?}", &buf[..encode_len]);
        assert_eq!(data, &buf[..encode_len]);

        truncation_check::<RSAPublicKey<'_>>(data);
        roundtrip_check::<RSAPublicKey<'_>>(data);
    }

    #[test]
    fn parse_private_key() {
        let data = include_bytes!("testdata/rsaprivatekey-1k.bin");
        let key = RSAPrivateKey::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);

        dbg!(data.len());
        assert_eq!(key.encoded_len(), data.len());
        truncation_check::<RSAPrivateKey<'_>>(data);
        roundtrip_check::<RSAPrivateKey<'_>>(data);
    }

    #[test]
    fn parse_pkcs8_key() {
        let data = include_bytes!("testdata/nistp256-p8.bin");
        truncation_check::<OneAsymmetricKey<'_>>(data);
        roundtrip_check::<OneAsymmetricKey<'_>>(data);

        let key = OneAsymmetricKey::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);
        assert_eq!(key.version, Pkcs8Version::Pkcs8v1);
        assert_eq!(key.privateKeyAlgorithm.algorithm, oid::id_ecPublicKey);
        assert_eq!(
            key.privateKeyAlgorithm.parameters,
            Some(Any::ObjectId(oid::id_prime256v1.clone()))
        );

        let inner = EcPrivateKey::parse(&mut Parser::new(key.privateKey.octets)).unwrap();
        dbg!(&inner);

        truncation_check::<EcPrivateKey<'_>>(key.privateKey.octets);
        roundtrip_check::<EcPrivateKey<'_>>(key.privateKey.octets);
    }

    #[test]
    fn parse_pkcs8_v2_key() {
        let data = include_bytes!("testdata/ed25519-p8v2.bin");
        truncation_check::<OneAsymmetricKey<'_>>(data);
        roundtrip_check::<OneAsymmetricKey<'_>>(data);

        let key = OneAsymmetricKey::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);
        assert_eq!(key.version, Pkcs8Version::Pkcs8v2);
        assert_eq!(key.privateKeyAlgorithm.algorithm, oid::id_ed25519);
        assert_eq!(key.privateKeyAlgorithm.parameters, None,);

        dbg!(key.privateKey.octets);
        let inner = OctetString::parse(&mut Parser::new(key.privateKey.octets)).unwrap();
        dbg!(&inner);

        truncation_check::<OctetString<'_>>(key.privateKey.octets);
        roundtrip_check::<OctetString<'_>>(key.privateKey.octets);
    }

    #[test]
    fn parse_sec1_key() {
        let data = include_bytes!("testdata/nistp256-sec1.bin");
        let key = EcPrivateKey::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);
        truncation_check::<EcPrivateKey<'_>>(data);
        roundtrip_check::<EcPrivateKey<'_>>(data);
    }

    #[test]
    fn parse_rsa_spki() {
        let data = include_bytes!("testdata/spki-rsa-2k.bin");
        truncation_check::<SubjectPublicKeyInfo<'_>>(data);
        roundtrip_check::<SubjectPublicKeyInfo<'_>>(data);

        let key = SubjectPublicKeyInfo::parse(&mut Parser::new(data)).unwrap();
        dbg!(&key);
        assert_eq!(key.algorithm.algorithm, oid::rsaEncryption);
        assert_eq!(key.algorithm.parameters, Some(Any::Null(Null)));

        let rsa_key = RSAPublicKey::parse(&mut Parser::new(key.subjectPublicKey.octets)).unwrap();
        dbg!(&rsa_key);

        truncation_check::<RSAPublicKey<'_>>(key.subjectPublicKey.octets);
        roundtrip_check::<RSAPublicKey<'_>>(key.subjectPublicKey.octets);
    }

    #[test]
    fn parse_ec_spki() {
        let data = include_bytes!("testdata/spki-ec-nistp256.bin");
        let key = SubjectPublicKeyInfo::parse(&mut Parser::new(data)).unwrap();
        assert_eq!(key.algorithm.algorithm, oid::id_ecPublicKey);
        assert_eq!(
            key.algorithm.parameters,
            Some(Any::ObjectId(oid::id_prime256v1.clone()))
        );

        assert_eq!(key.subjectPublicKey.octets[0], 0x04);

        truncation_check::<SubjectPublicKeyInfo<'_>>(data);
        roundtrip_check::<SubjectPublicKeyInfo<'_>>(data);
    }

    fn truncation_check<'a, T: Type<'a>>(bytes: &'a [u8]) {
        // base case
        T::from_bytes(bytes).unwrap();

        for prefix in 0..bytes.len() {
            assert_eq!(
                T::from_bytes(&bytes[..prefix]).unwrap_err(),
                Error::UnexpectedEof
            );
        }
    }

    fn roundtrip_check<'a, T: Type<'a>>(bytes: &'a [u8]) {
        let t = T::from_bytes(bytes).unwrap();
        dbg!(&t);
        let mut buf = vec![0; bytes.len()];
        let len = t.encode(&mut Encoder::new(&mut buf)).unwrap();
        assert_eq!(&buf, bytes);
        assert_eq!(len, bytes.len());
    }
}

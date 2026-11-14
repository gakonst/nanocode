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


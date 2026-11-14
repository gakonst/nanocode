use graviola::{
    hashing::{Sha256, Sha384, Sha512},
    key_agreement::{p256, p384},
    signing::{
        ecdsa::{P256, P384, VerifyingKey},
        eddsa::Ed25519VerifyingKey,
        rsa,
    },
};
use rustls::{
    SignatureScheme,
    crypto::WebPkiSupportedAlgorithms,
    pki_types::{AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id},
};
use std::fmt;

pub(super) static ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        ECDSA_P256_SHA512,
        ECDSA_P256_SHA384,
        ECDSA_P256_SHA256,
        ECDSA_P384_SHA512,
        ECDSA_P384_SHA384,
        ECDSA_P384_SHA256,
        ED25519,
        RSA_PSS_SHA512,
        RSA_PSS_SHA384,
        RSA_PSS_SHA256,
        RSA_PKCS1_SHA512,
        RSA_PKCS1_SHA384,
        RSA_PKCS1_SHA256,
    ],
    mapping: &[
        (
            SignatureScheme::ECDSA_NISTP256_SHA256,
            &[ECDSA_P256_SHA256, ECDSA_P384_SHA256],
        ),
        (
            SignatureScheme::ECDSA_NISTP384_SHA384,
            &[ECDSA_P384_SHA384, ECDSA_P256_SHA384],
        ),
        (SignatureScheme::ED25519, &[ED25519]),
        (SignatureScheme::RSA_PSS_SHA512, &[RSA_PSS_SHA512]),
        (SignatureScheme::RSA_PSS_SHA384, &[RSA_PSS_SHA384]),
        (SignatureScheme::RSA_PSS_SHA256, &[RSA_PSS_SHA256]),
        (SignatureScheme::RSA_PKCS1_SHA512, &[RSA_PKCS1_SHA512]),
        (SignatureScheme::RSA_PKCS1_SHA384, &[RSA_PKCS1_SHA384]),
        (SignatureScheme::RSA_PKCS1_SHA256, &[RSA_PKCS1_SHA256]),
    ],
};

type Check = fn(&[u8], &[u8], &[u8]) -> Result<(), graviola::Error>;

struct Algorithm {
    name: &'static str,
    key: AlgorithmIdentifier,
    signature: AlgorithmIdentifier,
    check: Check,
}

impl fmt::Debug for Algorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

impl SignatureVerificationAlgorithm for Algorithm {
    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.key
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature
    }

    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        (self.check)(public_key, message, signature).map_err(|_| InvalidSignature)
    }
}

fn p256_key(public_key: &[u8]) -> Result<VerifyingKey<P256>, graviola::Error> {
    p256::PublicKey::from_x962_uncompressed(public_key)
        .map(|public_key| VerifyingKey { public_key })
}

fn p384_key(public_key: &[u8]) -> Result<VerifyingKey<P384>, graviola::Error> {
    p384::PublicKey::from_x962_uncompressed(public_key)
        .map(|public_key| VerifyingKey { public_key })
}

static ECDSA_P256_SHA256: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P256_SHA256",
    key: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA256,
    check: |key, message, signature| p256_key(key)?.verify_asn1::<Sha256>(&[message], signature),
};

static ECDSA_P256_SHA384: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P256_SHA384",
    key: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA384,
    check: |key, message, signature| p256_key(key)?.verify_asn1::<Sha384>(&[message], signature),
};

static ECDSA_P256_SHA512: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P256_SHA512",
    key: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA512,
    check: |key, message, signature| p256_key(key)?.verify_asn1::<Sha512>(&[message], signature),
};

static ECDSA_P384_SHA256: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P384_SHA256",
    key: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA256,
    check: |key, message, signature| p384_key(key)?.verify_asn1::<Sha256>(&[message], signature),
};

static ECDSA_P384_SHA384: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P384_SHA384",
    key: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA384,
    check: |key, message, signature| p384_key(key)?.verify_asn1::<Sha384>(&[message], signature),
};

static ECDSA_P384_SHA512: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ECDSA_P384_SHA512",
    key: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA512,
    check: |key, message, signature| p384_key(key)?.verify_asn1::<Sha512>(&[message], signature),
};

static ED25519: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "ED25519",
    key: alg_id::ED25519,
    signature: alg_id::ED25519,
    check: |key, message, signature| {
        Ed25519VerifyingKey::from_bytes(key)?.verify(signature, message)
    },
};

static RSA_PSS_SHA256: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PSS_SHA256",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PSS_SHA256,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pss_sha256(signature, message)
    },
};

static RSA_PSS_SHA384: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PSS_SHA384",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PSS_SHA384,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pss_sha384(signature, message)
    },
};

static RSA_PSS_SHA512: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PSS_SHA512",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PSS_SHA512,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pss_sha512(signature, message)
    },
};

static RSA_PKCS1_SHA256: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PKCS1_SHA256",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PKCS1_SHA256,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pkcs1_sha256(signature, message)
    },
};

static RSA_PKCS1_SHA384: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PKCS1_SHA384",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PKCS1_SHA384,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pkcs1_sha384(signature, message)
    },
};

static RSA_PKCS1_SHA512: &dyn SignatureVerificationAlgorithm = &Algorithm {
    name: "RSA_PKCS1_SHA512",
    key: alg_id::RSA_ENCRYPTION,
    signature: alg_id::RSA_PKCS1_SHA512,
    check: |key, message, signature| {
        rsa::VerifyingKey::from_pkcs1_der(key)?.verify_pkcs1_sha512(signature, message)
    },
};

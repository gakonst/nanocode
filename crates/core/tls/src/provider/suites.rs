use super::{
    aead::{AES_128_KEY_BYTES, AES_256_KEY_BYTES, AesGcm, Chacha20Poly1305},
    hash::{HMAC_SHA256, HMAC_SHA384, SHA256, SHA384},
};
use rustls::{
    CipherSuite, SignatureScheme, SupportedCipherSuite, Tls12CipherSuite, Tls13CipherSuite,
    crypto::{CipherSuiteCommon, KeyExchangeAlgorithm, tls12::PrfUsingHmac, tls13::HkdfUsingHmac},
};

const AES_GCM_CONFIDENTIALITY_LIMIT: u64 = 1 << 24;
const CHACHA20_POLY1305_CONFIDENTIALITY_LIMIT: u64 = u64::MAX;

pub(super) static ALL: &[SupportedCipherSuite] = &[
    TLS13_AES_128_GCM_SHA256,
    TLS13_AES_256_GCM_SHA384,
    TLS13_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
    TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
];

const TLS13_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_128_GCM_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA256),
        aead_alg: &AesGcm(AES_128_KEY_BYTES),
        quic: None,
    });

const TLS13_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_256_GCM_SHA384,
            hash_provider: &SHA384,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA384),
        aead_alg: &AesGcm(AES_256_KEY_BYTES),
        quic: None,
    });

const TLS13_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: CHACHA20_POLY1305_CONFIDENTIALITY_LIMIT,
        },
        hkdf_provider: &HkdfUsingHmac(&HMAC_SHA256),
        aead_alg: &Chacha20Poly1305,
        quic: None,
    });

const TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &AesGcm(AES_128_KEY_BYTES),
        prf_provider: &PrfUsingHmac(&HMAC_SHA256),
    });

const TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
            hash_provider: &SHA384,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &AesGcm(AES_256_KEY_BYTES),
        prf_provider: &PrfUsingHmac(&HMAC_SHA384),
    });

const TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: CHACHA20_POLY1305_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_ECDSA_SCHEMES,
        aead_alg: &Chacha20Poly1305,
        prf_provider: &PrfUsingHmac(&HMAC_SHA256),
    });

const TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &AesGcm(AES_128_KEY_BYTES),
        prf_provider: &PrfUsingHmac(&HMAC_SHA256),
    });

const TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
            hash_provider: &SHA384,
            confidentiality_limit: AES_GCM_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &AesGcm(AES_256_KEY_BYTES),
        prf_provider: &PrfUsingHmac(&HMAC_SHA384),
    });

const TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls12(&Tls12CipherSuite {
        common: CipherSuiteCommon {
            suite: CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
            hash_provider: &SHA256,
            confidentiality_limit: CHACHA20_POLY1305_CONFIDENTIALITY_LIMIT,
        },
        kx: KeyExchangeAlgorithm::ECDHE,
        sign: TLS12_RSA_SCHEMES,
        aead_alg: &Chacha20Poly1305,
        prf_provider: &PrfUsingHmac(&HMAC_SHA256),
    });

static TLS12_ECDSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::ECDSA_NISTP521_SHA512,
    SignatureScheme::ECDSA_NISTP384_SHA384,
    SignatureScheme::ECDSA_NISTP256_SHA256,
];

static TLS12_RSA_SCHEMES: &[SignatureScheme] = &[
    SignatureScheme::RSA_PSS_SHA512,
    SignatureScheme::RSA_PSS_SHA384,
    SignatureScheme::RSA_PSS_SHA256,
    SignatureScheme::RSA_PKCS1_SHA512,
    SignatureScheme::RSA_PKCS1_SHA384,
    SignatureScheme::RSA_PKCS1_SHA256,
];

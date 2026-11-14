use graviola::{
    hashing::Sha256,
    signing::ecdsa::{P256, SigningKey},
};
use rustls::{
    Error, SignatureAlgorithm, SignatureScheme,
    crypto::KeyProvider,
    pki_types::{PrivateKeyDer, SubjectPublicKeyInfoDer},
    sign,
};
use std::{fmt, sync::Arc};

const SPKI_BYTES_MAX: usize = 128;
const SIGNATURE_BYTES_MAX: usize = 128;

#[derive(Debug)]
pub(super) struct P256Keys;

impl KeyProvider for P256Keys {
    fn load_private_key(
        &self,
        key: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn sign::SigningKey>, Error> {
        let key = match key {
            PrivateKeyDer::Pkcs8(key) => SigningKey::<P256>::from_pkcs8_der(key.secret_pkcs8_der()),
            PrivateKeyDer::Sec1(key) => SigningKey::<P256>::from_sec1_der(key.secret_sec1_der()),
            _ => {
                return Err(Error::General(
                    "only ECDSA P-256 server keys are supported".to_owned(),
                ));
            }
        }
        .map_err(|error| Error::General(format!("not an ECDSA P-256 key: {error:?}")))?;
        Ok(Arc::new(EcdsaP256(Arc::new(key))))
    }
}

#[derive(Clone)]
struct EcdsaP256(Arc<SigningKey<P256>>);

impl fmt::Debug for EcdsaP256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("EcdsaP256").finish_non_exhaustive()
    }
}

impl sign::SigningKey for EcdsaP256 {
    fn choose_scheme(&self, schemes: &[SignatureScheme]) -> Option<Box<dyn sign::Signer>> {
        schemes
            .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
            .then(|| Box::new(self.clone()) as Box<dyn sign::Signer>)
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'static>> {
        let mut buffer = [0; SPKI_BYTES_MAX];
        let spki = self.0.to_spki_der(&mut buffer).ok()?;
        Some(SubjectPublicKeyInfoDer::from(spki.to_vec()))
    }
}

impl sign::Signer for EcdsaP256 {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let mut buffer = [0; SIGNATURE_BYTES_MAX];
        let signature = self
            .0
            .sign_asn1::<Sha256>(&[message], &mut buffer)
            .map_err(|error| Error::General(format!("signing failed: {error:?}")))?;
        Ok(signature.to_vec())
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}

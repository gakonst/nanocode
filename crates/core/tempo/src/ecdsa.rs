use crate::{
    Address, Error,
    curve::{Curve, P256, Point, SECP256K1},
    field::{self, BYTES, Limbs},
    sha256::HmacSha256,
};

const SIGNATURE_BYTES: usize = 65;
const PARITY_OFFSET: u8 = 27;
const RECOVERY_REDUCED: u8 = 2;
const DRBG_SEPARATORS: [u8; 2] = [0x00, 0x01];
const DRBG_RESEED: u8 = 0x00;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    pub r: [u8; BYTES],
    pub s: [u8; BYTES],
    pub y_parity: bool,
}

impl Signature {
    pub fn to_bytes(&self) -> [u8; SIGNATURE_BYTES] {
        let mut bytes = [0u8; SIGNATURE_BYTES];
        bytes[..BYTES].copy_from_slice(&self.r);
        bytes[BYTES..2 * BYTES].copy_from_slice(&self.s);
        bytes[2 * BYTES] = PARITY_OFFSET + self.y_parity as u8;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: &[u8; SIGNATURE_BYTES] = bytes.try_into().map_err(|_| Error::InvalidLength {
            expected: SIGNATURE_BYTES,
            actual: bytes.len(),
        })?;
        let v = bytes[2 * BYTES];
        let y_parity = match v {
            0 | 1 => v == 1,
            27 | 28 => v == 28,
            v if v >= 35 => v % 2 == 0,
            v => return Err(Error::InvalidRecoveryId(v)),
        };
        let mut r = [0u8; BYTES];
        let mut s = [0u8; BYTES];
        r.copy_from_slice(&bytes[..BYTES]);
        s.copy_from_slice(&bytes[BYTES..2 * BYTES]);
        Ok(Self { r, s, y_parity })
    }
}

#[derive(Clone)]
pub struct SigningKey {
    curve: &'static Curve,
    secret: [u8; BYTES],
    public_x: [u8; BYTES],
    public_y: [u8; BYTES],
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SigningKey(<redacted>)")
    }
}

impl SigningKey {
    pub fn secp256k1(secret: &[u8]) -> Result<Self, Error> {
        Self::new(&SECP256K1, secret)
    }

    pub fn p256(secret: &[u8]) -> Result<Self, Error> {
        Self::new(&P256, secret)
    }

    fn new(curve: &'static Curve, secret: &[u8]) -> Result<Self, Error> {
        let secret: [u8; BYTES] = secret.try_into().map_err(|_| Error::InvalidLength {
            expected: BYTES,
            actual: secret.len(),
        })?;
        let scalar = field::from_be_bytes(&secret);
        let valid = curve.order.is_canonical(&scalar) & (field::is_zero(&scalar) ^ 1);
        if valid == 0 {
            return Err(Error::InvalidPrivateKey);
        }
        let public = curve
            .to_affine(&curve.multiply_generator(&secret))
            .ok_or(Error::InvalidPrivateKey)?;
        Ok(Self {
            curve,
            secret,
            public_x: public.x,
            public_y: public.y,
        })
    }

    pub const fn public_key(&self) -> (&[u8; BYTES], &[u8; BYTES]) {
        (&self.public_x, &self.public_y)
    }

    pub fn address(&self) -> Address {
        Address::from_public_key(&self.public_x, &self.public_y)
    }

    pub fn sign_prehash(&self, prehash: &[u8; BYTES]) -> Result<Signature, Error> {
        let curve = self.curve;
        let order = &curve.order;
        let nonce = rfc6979_nonce(curve, &self.secret, prehash);
        let point = curve
            .to_affine(&curve.multiply_generator(&nonce))
            .ok_or(Error::SignatureFailed)?;
        let x = field::from_be_bytes(&point.x);
        let r = order.reduce(&x);
        let x_reduced = field::equal(&r, &x) ^ 1;
        if field::is_zero(&r) == 1 {
            return Err(Error::SignatureFailed);
        }
        let z = order.encode(&order.reduce(&field::from_be_bytes(prehash)));
        let d = order.encode(&field::from_be_bytes(&self.secret));
        let k = order.encode(&field::from_be_bytes(&nonce));
        let r_montgomery = order.encode(&r);
        let sum = order.add(&z, &order.mul(&r_montgomery, &d));
        let s = order.decode(&order.mul(&order.invert(&k), &sum));
        if field::is_zero(&s) == 1 {
            return Err(Error::SignatureFailed);
        }
        let negated = order.negate(&s);
        let high = is_high(order.value, &s);
        let s = field::select(&negated, &s, high);
        let recovery = (point.y_odd as u8 ^ high as u8) | ((x_reduced as u8) * RECOVERY_REDUCED);
        if recovery & RECOVERY_REDUCED != 0 {
            return Err(Error::InvalidRecoveryId(recovery));
        }
        Ok(Signature {
            r: field::to_be_bytes(&r),
            s: field::to_be_bytes(&s),
            y_parity: recovery & 1 == 1,
        })
    }
}

const fn is_high(order: Limbs, s: &Limbs) -> u64 {
    let half = [
        (order[0] >> 1) | (order[1] << (u64::BITS - 1)),
        (order[1] >> 1) | (order[2] << (u64::BITS - 1)),
        (order[2] >> 1) | (order[3] << (u64::BITS - 1)),
        order[3] >> 1,
    ];
    field::subtract(&half, s).1
}

fn rfc6979_nonce(curve: &Curve, secret: &[u8; BYTES], prehash: &[u8; BYTES]) -> [u8; BYTES] {
    let mut key = [0u8; BYTES];
    let mut value = [DRBG_SEPARATORS[1]; BYTES];
    for separator in DRBG_SEPARATORS {
        let mut mac = HmacSha256::new(&key);
        mac.update(&value);
        mac.update(&[separator]);
        mac.update(secret);
        mac.update(prehash);
        key = mac.finalize();
        let mut mac = HmacSha256::new(&key);
        mac.update(&value);
        value = mac.finalize();
    }
    loop {
        let mut mac = HmacSha256::new(&key);
        mac.update(&value);
        value = mac.finalize();
        let candidate = field::from_be_bytes(&value);
        let valid = curve.order.is_canonical(&candidate) & (field::is_zero(&candidate) ^ 1);
        if valid == 1 {
            return value;
        }
        let mut mac = HmacSha256::new(&key);
        mac.update(&value);
        mac.update(&[DRBG_RESEED]);
        key = mac.finalize();
        let mut mac = HmacSha256::new(&key);
        mac.update(&value);
        value = mac.finalize();
    }
}

const fn scalar(curve: &Curve, bytes: &[u8; BYTES]) -> Result<Limbs, Error> {
    let value = field::from_be_bytes(bytes);
    if curve.order.is_canonical(&value) == 0 || field::is_zero(&value) == 1 {
        return Err(Error::InvalidSignature);
    }
    Ok(value)
}

fn combine(
    curve: &Curve,
    first: &Limbs,
    second: &Limbs,
    point: &Point,
) -> Option<crate::curve::Affine> {
    let order = &curve.order;
    let first = field::to_be_bytes(&order.decode(first));
    let second = field::to_be_bytes(&order.decode(second));
    let sum = curve.add(
        &curve.multiply_generator(&first),
        &curve.multiply(point, &second),
    );
    curve.to_affine(&sum)
}

pub fn recover_secp256k1(prehash: &[u8; BYTES], signature: &Signature) -> Result<Address, Error> {
    let curve = &SECP256K1;
    let order = &curve.order;
    let r = scalar(curve, &signature.r)?;
    let s = scalar(curve, &signature.s)?;
    if is_high(order.value, &s) == 1 {
        return Err(Error::InvalidSignature);
    }
    let point = curve
        .decompress(&signature.r, signature.y_parity)
        .ok_or(Error::InvalidSignature)?;
    let r_inverse = order.invert(&order.encode(&r));
    let z = order.encode(&order.reduce(&field::from_be_bytes(prehash)));
    let first = order.negate(&order.mul(&z, &r_inverse));
    let second = order.mul(&order.encode(&s), &r_inverse);
    let public = combine(curve, &first, &second, &point).ok_or(Error::InvalidSignature)?;
    Ok(Address::from_public_key(&public.x, &public.y))
}

pub fn verify_p256(
    prehash: &[u8; BYTES],
    r: &[u8; BYTES],
    s: &[u8; BYTES],
    public_x: &[u8; BYTES],
    public_y: &[u8; BYTES],
) -> Result<(), Error> {
    let curve = &P256;
    let order = &curve.order;
    let r_value = scalar(curve, r)?;
    let s_value = scalar(curve, s)?;
    let public = curve
        .lift(public_x, public_y)
        .ok_or(Error::InvalidPublicKey)?;
    let s_inverse = order.invert(&order.encode(&s_value));
    let z = order.encode(&order.reduce(&field::from_be_bytes(prehash)));
    let first = order.mul(&z, &s_inverse);
    let second = order.mul(&order.encode(&r_value), &s_inverse);
    let point = combine(curve, &first, &second, &public).ok_or(Error::InvalidSignature)?;
    let x = order.reduce(&field::from_be_bytes(&point.x));
    if field::equal(&x, &r_value) == 0 {
        return Err(Error::InvalidSignature);
    }
    Ok(())
}

pub const fn p256_s_is_high(s: &[u8; BYTES]) -> bool {
    is_high(P256.order.value, &field::from_be_bytes(s)) == 1
}

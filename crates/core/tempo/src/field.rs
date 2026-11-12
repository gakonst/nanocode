pub(crate) type Limbs = [u64; LIMBS];

pub(crate) const LIMBS: usize = 4;
pub(crate) const BYTES: usize = 32;
const LIMB_BITS: u32 = u64::BITS;
const LIMB_BYTES: usize = BYTES / LIMBS;

#[derive(Clone, Copy)]
pub(crate) struct Modulus {
    pub(crate) value: Limbs,
    inverse: u64,
    pub(crate) one: Limbs,
    square: Limbs,
}

impl Modulus {
    pub(crate) const fn new(value: Limbs) -> Self {
        assert!(value[0] & 1 == 1);
        assert!(value[LIMBS - 1] >> (LIMB_BITS - 1) == 1);
        let one = negate(&value);
        let mut square = one;
        let mut doubling = 0;
        while doubling < LIMB_BITS as usize * LIMBS {
            square = add_raw(&square, &square, &value);
            doubling += 1;
        }
        Self {
            value,
            inverse: negated_inverse(value[0]),
            one,
            square,
        }
    }

    pub(crate) const fn add(&self, a: &Limbs, b: &Limbs) -> Limbs {
        add_raw(a, b, &self.value)
    }

    pub(crate) const fn sub(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let (difference, borrow) = subtract(a, b);
        let mask = 0u64.wrapping_sub(borrow);
        let masked = [
            self.value[0] & mask,
            self.value[1] & mask,
            self.value[2] & mask,
            self.value[3] & mask,
        ];
        add_carry(&difference, &masked).0
    }

    pub(crate) const fn negate(&self, a: &Limbs) -> Limbs {
        self.sub(&[0; LIMBS], a)
    }

    pub(crate) const fn mul(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let mut t = [0u64; LIMBS + 2];
        let mut i = 0;
        while i < LIMBS {
            let mut carry = 0u128;
            let mut j = 0;
            while j < LIMBS {
                let v = t[j] as u128 + (a[j] as u128) * (b[i] as u128) + carry;
                t[j] = v as u64;
                carry = v >> LIMB_BITS;
                j += 1;
            }
            let v = t[LIMBS] as u128 + carry;
            t[LIMBS] = v as u64;
            t[LIMBS + 1] = (v >> LIMB_BITS) as u64;
            let m = t[0].wrapping_mul(self.inverse);
            let mut carry = (t[0] as u128 + (m as u128) * (self.value[0] as u128)) >> LIMB_BITS;
            let mut j = 1;
            while j < LIMBS {
                let v = t[j] as u128 + (m as u128) * (self.value[j] as u128) + carry;
                t[j - 1] = v as u64;
                carry = v >> LIMB_BITS;
                j += 1;
            }
            let v = t[LIMBS] as u128 + carry;
            t[LIMBS - 1] = v as u64;
            let v = t[LIMBS + 1] as u128 + (v >> LIMB_BITS);
            t[LIMBS] = v as u64;
            t[LIMBS + 1] = 0;
            i += 1;
        }
        reduce_once(&[t[0], t[1], t[2], t[3]], t[LIMBS], &self.value)
    }

    pub(crate) const fn square(&self, a: &Limbs) -> Limbs {
        self.mul(a, a)
    }

    pub(crate) const fn encode(&self, a: &Limbs) -> Limbs {
        self.mul(a, &self.square)
    }

    pub(crate) const fn decode(&self, a: &Limbs) -> Limbs {
        self.mul(a, &[1, 0, 0, 0])
    }

    pub(crate) const fn reduce(&self, a: &Limbs) -> Limbs {
        reduce_once(a, 0, &self.value)
    }

    pub(crate) const fn is_canonical(&self, a: &Limbs) -> u64 {
        subtract(a, &self.value).1
    }

    pub(crate) const fn pow(&self, base: &Limbs, exponent: &Limbs) -> Limbs {
        let mut result = self.one;
        let mut limb = LIMBS;
        while limb > 0 {
            limb -= 1;
            let mut bit = LIMB_BITS;
            while bit > 0 {
                bit -= 1;
                result = self.square(&result);
                if (exponent[limb] >> bit) & 1 == 1 {
                    result = self.mul(&result, base);
                }
            }
        }
        result
    }

    pub(crate) const fn invert(&self, a: &Limbs) -> Limbs {
        let exponent = subtract(&self.value, &[2, 0, 0, 0]).0;
        self.pow(a, &exponent)
    }

    pub(crate) const fn small(&self, value: u64) -> Limbs {
        self.encode(&[value, 0, 0, 0])
    }
}

const fn negated_inverse(low: u64) -> u64 {
    let mut inverse: u64 = 1;
    let mut precision = 1;
    while precision < LIMB_BITS {
        inverse = inverse.wrapping_mul(2u64.wrapping_sub(low.wrapping_mul(inverse)));
        precision *= 2;
    }
    inverse.wrapping_neg()
}

const fn negate(a: &Limbs) -> Limbs {
    let inverted = [!a[0], !a[1], !a[2], !a[3]];
    add_carry(&inverted, &[1, 0, 0, 0]).0
}

pub(crate) const fn add_carry(a: &Limbs, b: &Limbs) -> (Limbs, u64) {
    let mut sum = [0u64; LIMBS];
    let mut carry = 0u64;
    let mut i = 0;
    while i < LIMBS {
        let v = a[i] as u128 + b[i] as u128 + carry as u128;
        sum[i] = v as u64;
        carry = (v >> LIMB_BITS) as u64;
        i += 1;
    }
    (sum, carry)
}

pub(crate) const fn subtract(a: &Limbs, b: &Limbs) -> (Limbs, u64) {
    let mut difference = [0u64; LIMBS];
    let mut borrow = 0u64;
    let mut i = 0;
    while i < LIMBS {
        let (d, b1) = a[i].overflowing_sub(b[i]);
        let (d, b2) = d.overflowing_sub(borrow);
        difference[i] = d;
        borrow = (b1 | b2) as u64;
        i += 1;
    }
    (difference, borrow)
}

const fn add_raw(a: &Limbs, b: &Limbs, modulus: &Limbs) -> Limbs {
    let (sum, carry) = add_carry(a, b);
    reduce_once(&sum, carry, modulus)
}

const fn reduce_once(a: &Limbs, high: u64, modulus: &Limbs) -> Limbs {
    let (difference, borrow) = subtract(a, modulus);
    let take = (high | (borrow ^ 1)) & 1;
    select(&difference, a, take)
}

pub(crate) const fn select(when_one: &Limbs, when_zero: &Limbs, choice: u64) -> Limbs {
    let mask = 0u64.wrapping_sub(choice & 1);
    [
        (when_one[0] & mask) | (when_zero[0] & !mask),
        (when_one[1] & mask) | (when_zero[1] & !mask),
        (when_one[2] & mask) | (when_zero[2] & !mask),
        (when_one[3] & mask) | (when_zero[3] & !mask),
    ]
}

pub(crate) const fn is_zero(a: &Limbs) -> u64 {
    let any = a[0] | a[1] | a[2] | a[3];
    ((any | any.wrapping_neg()) >> (LIMB_BITS - 1)) ^ 1
}

pub(crate) const fn equal(a: &Limbs, b: &Limbs) -> u64 {
    is_zero(&[a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2], a[3] ^ b[3]])
}

pub(crate) const fn from_be_bytes(bytes: &[u8; BYTES]) -> Limbs {
    let mut limbs = [0u64; LIMBS];
    let mut i = 0;
    while i < BYTES {
        let limb = LIMBS - 1 - i / LIMB_BYTES;
        limbs[limb] = (limbs[limb] << u8::BITS) | bytes[i] as u64;
        i += 1;
    }
    limbs
}

pub(crate) const fn to_be_bytes(limbs: &Limbs) -> [u8; BYTES] {
    let mut bytes = [0u8; BYTES];
    let mut i = 0;
    while i < BYTES {
        let limb = LIMBS - 1 - i / LIMB_BYTES;
        let shift = (LIMB_BYTES - 1 - i % LIMB_BYTES) as u32 * u8::BITS;
        bytes[i] = (limbs[limb] >> shift) as u8;
        i += 1;
    }
    bytes
}

pub(crate) const fn from_hex(text: &str) -> Limbs {
    let digits = text.as_bytes();
    assert!(digits.len() == BYTES * 2);
    let mut bytes = [0u8; BYTES];
    let mut i = 0;
    while i < BYTES {
        bytes[i] = (nibble(digits[2 * i]) << 4) | nibble(digits[2 * i + 1]);
        i += 1;
    }
    from_be_bytes(&bytes)
}

const fn nibble(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        b'A'..=b'F' => digit - b'A' + 10,
        _ => panic!("curve constants are hexadecimal"),
    }
}

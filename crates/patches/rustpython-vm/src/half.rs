#[derive(Clone, Copy)]
#[repr(transparent)]
pub(crate) struct Half(u16);

impl Half {
    const SIGN_MASK: u16 = 0x8000;
    const EXPONENT_MASK: u16 = 0x7c00;
    const MANTISSA_MASK: u16 = 0x03ff;
    const MAGNITUDE_MASK: u16 = 0x7fff;
    const EXPONENT_BIAS: i64 = 15;
    const DOUBLE_EXPONENT_BIAS: i64 = 1023;
    const DOUBLE_HIGH_SIGN_MASK: u32 = 0x8000_0000;
    const DOUBLE_HIGH_EXPONENT_MASK: u32 = 0x7ff0_0000;
    const DOUBLE_HIGH_MANTISSA_MASK: u32 = 0x000f_ffff;
    const DOUBLE_HIGH_HIDDEN_BIT: u32 = 0x0010_0000;
    const DOUBLE_EXPONENT_SHIFT: u32 = 20;
    const DOUBLE_INFINITY_BITS: u64 = 0x7ff0_0000_0000_0000;
    const DOUBLE_QUIET_NAN_BITS: u64 = 0x7ff8_0000_0000_0000;
    const DOUBLE_MANTISSA_MASK: u64 = 0x000f_ffff_ffff_ffff;
    const QUIET_BIT: u32 = 0x0200;
    const MANTISSA_BITS: u32 = 10;
    const EXPONENT_MAX: i64 = 0x1f;
    const SUBNORMAL_SHIFT_LIMIT: i64 = 21;
    const HIGH_WORD_SHIFT: u32 = 32;
    const SIGN_SHIFT_TO_DOUBLE: u32 = 48;
    const MANTISSA_SHIFT_TO_DOUBLE: u32 = 42;
    const SUBNORMAL_LEADING_BITS: u32 = 6;
    const SUBNORMAL_SHIFT_BASE: u32 = 43;
    const DOUBLE_EXPONENT_SHIFT_FULL: u32 = 52;

    pub(crate) const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    pub(crate) const fn to_bits(self) -> u16 {
        self.0
    }

    pub(crate) const fn is_infinite(self) -> bool {
        self.0 & Self::MAGNITUDE_MASK == Self::EXPONENT_MASK
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(crate) const fn from_f64(value: f64) -> Self {
        let bits = value.to_bits();
        let high = (bits >> Self::HIGH_WORD_SHIFT) as u32;
        let sign = high & Self::DOUBLE_HIGH_SIGN_MASK;
        let exponent = high & Self::DOUBLE_HIGH_EXPONENT_MASK;
        let mantissa = high & Self::DOUBLE_HIGH_MANTISSA_MASK;
        let half_sign = sign >> 16;
        if exponent == Self::DOUBLE_HIGH_EXPONENT_MASK {
            let quiet = if mantissa == 0 && bits as u32 == 0 {
                0
            } else {
                Self::QUIET_BIT
            };
            return Self(
                (half_sign | Self::EXPONENT_MASK as u32 | quiet | (mantissa >> Self::MANTISSA_BITS))
                    as u16,
            );
        }
        let half_exponent = ((exponent >> Self::DOUBLE_EXPONENT_SHIFT) as i64)
            - Self::DOUBLE_EXPONENT_BIAS
            + Self::EXPONENT_BIAS;
        if half_exponent >= Self::EXPONENT_MAX {
            return Self((half_sign | Self::EXPONENT_MASK as u32) as u16);
        }
        if half_exponent <= 0 {
            if Self::MANTISSA_BITS as i64 - half_exponent > Self::SUBNORMAL_SHIFT_LIMIT {
                return Self(half_sign as u16);
            }
            let mantissa = mantissa | Self::DOUBLE_HIGH_HIDDEN_BIT;
            let mut half_mantissa = mantissa >> (Self::MANTISSA_BITS as i64 + 1 - half_exponent);
            let round_bit = 1 << (Self::MANTISSA_BITS as i64 - half_exponent);
            if (mantissa & round_bit) != 0 && (mantissa & (3 * round_bit - 1)) != 0 {
                half_mantissa += 1;
            }
            return Self((half_sign | half_mantissa) as u16);
        }
        let half_exponent = (half_exponent as u32) << Self::MANTISSA_BITS;
        let half_mantissa = mantissa >> Self::MANTISSA_BITS;
        let round_bit = Self::QUIET_BIT;
        let assembled = half_sign | half_exponent | half_mantissa;
        if (mantissa & round_bit) != 0 && (mantissa & (3 * round_bit - 1)) != 0 {
            Self((assembled + 1) as u16)
        } else {
            Self(assembled as u16)
        }
    }

    #[allow(clippy::cast_sign_loss)]
    pub(crate) const fn to_f64(self) -> f64 {
        let bits = self.0;
        if bits & Self::MAGNITUDE_MASK == 0 {
            return f64::from_bits((bits as u64) << Self::SIGN_SHIFT_TO_DOUBLE);
        }
        let sign = ((bits & Self::SIGN_MASK) as u64) << Self::SIGN_SHIFT_TO_DOUBLE;
        let exponent = (bits & Self::EXPONENT_MASK) as u64;
        let mantissa = (bits & Self::MANTISSA_MASK) as u64;
        if exponent == Self::EXPONENT_MASK as u64 {
            if mantissa == 0 {
                return f64::from_bits(sign | Self::DOUBLE_INFINITY_BITS);
            }
            return f64::from_bits(
                sign | Self::DOUBLE_QUIET_NAN_BITS | (mantissa << Self::MANTISSA_SHIFT_TO_DOUBLE),
            );
        }
        if exponent == 0 {
            let adjust = (mantissa as u16).leading_zeros() - Self::SUBNORMAL_LEADING_BITS;
            let double_exponent = ((Self::DOUBLE_EXPONENT_BIAS - Self::EXPONENT_BIAS - adjust as i64)
                as u64)
                << Self::DOUBLE_EXPONENT_SHIFT_FULL;
            let double_mantissa =
                (mantissa << (Self::SUBNORMAL_SHIFT_BASE + adjust)) & Self::DOUBLE_MANTISSA_MASK;
            return f64::from_bits(sign | double_exponent | double_mantissa);
        }
        let unbiased = ((exponent as i64) >> Self::MANTISSA_BITS) - Self::EXPONENT_BIAS;
        let double_exponent =
            ((unbiased + Self::DOUBLE_EXPONENT_BIAS) as u64) << Self::DOUBLE_EXPONENT_SHIFT_FULL;
        f64::from_bits(sign | double_exponent | (mantissa << Self::MANTISSA_SHIFT_TO_DOUBLE))
    }
}

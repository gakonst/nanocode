use core::ops::RangeInclusive;
use nanocode_sys::DecimalText;

const TEXT_BYTES: usize = 24;
const NAN: &str = "NaN";
const INFINITY: &str = "inf";
const NEG_INFINITY: &str = "-inf";
const EXTRA_SHIFT: u32 = 6;
const EXTRA_SHIFT_32: u32 = 34;
const POW10_COUNT: usize = 618;
const DEC_EXP_MIN: i32 = -293;
const QUAD_SCALE: u64 = 10_000;
const OCTET_SCALE: u64 = 100_000_000;
const PAIR_SCALE: u32 = 100;
const RADIX: u64 = 10;

#[derive(Clone, Copy)]
struct Wide {
    hi: u64,
    lo: u64,
}

const fn umul128_hi64(x: u64, y: u64) -> u64 {
    ((x as u128 * y as u128) >> 64) as u64
}

const fn umul128_add_hi64(x: u64, y: u64, c: u64) -> u64 {
    ((x as u128 * y as u128 + c as u128) >> 64) as u64
}

const fn umul192_hi128(x_hi: u64, x_lo: u64, y: u64) -> Wide {
    let p = x_hi as u128 * y as u128;
    let lo = (p as u64).wrapping_add(umul128_hi64(x_lo, y));
    Wide {
        hi: (p >> 64) as u64 + (lo < p as u64) as u64,
        lo,
    }
}

const fn div10(x: u64) -> u64 {
    const DIV10_SIG64: u64 = (1 << 63) / 5 + 1;
    umul128_hi64(x, DIV10_SIG64)
}

const fn compute_dec_exp(bin_exp: i32, regular: bool) -> i32 {
    const LOG10_3_OVER_4_SIG: i32 = 131_072;
    const LOG10_2_SIG: i32 = 315_653;
    const LOG10_2_EXP: i32 = 20;
    (bin_exp * LOG10_2_SIG - !regular as i32 * LOG10_3_OVER_4_SIG) >> LOG10_2_EXP
}

const fn compute_exp_shift(bin_exp: i32, dec_exp: i32) -> u8 {
    const LOG2_POW10_SIG: i32 = 217_707;
    const LOG2_POW10_EXP: i32 = 16;
    let pow10_bin_exp = (-dec_exp * LOG2_POW10_SIG) >> LOG2_POW10_EXP;
    (bin_exp + pow10_bin_exp + 1) as u8
}

const POW10_MINOR: [u64; 28] = [
    0x8000000000000000, 0xa000000000000000, 0xc800000000000000,
    0xfa00000000000000, 0x9c40000000000000, 0xc350000000000000,
    0xf424000000000000, 0x9896800000000000, 0xbebc200000000000,
    0xee6b280000000000, 0x9502f90000000000, 0xba43b74000000000,
    0xe8d4a51000000000, 0x9184e72a00000000, 0xb5e620f480000000,
    0xe35fa931a0000000, 0x8e1bc9bf04000000, 0xb1a2bc2ec5000000,
    0xde0b6b3a76400000, 0x8ac7230489e80000, 0xad78ebc5ac620000,
    0xd8d726b7177a8000, 0x878678326eac9000, 0xa968163f0a57b400,
    0xd3c21bcecceda100, 0x84595161401484a0, 0xa56fa5b99019a5c8,
    0xcecb8f27f4200f3a,
];

const POW10_MAJOR: [Wide; 23] = [
    Wide { hi: 0xaf8e5410288e1b6f, lo: 0x07ecf0ae5ee44dda },
    Wide { hi: 0xb1442798f49ffb4a, lo: 0x99cd11cfdf41779d },
    Wide { hi: 0xb2fe3f0b8599ef07, lo: 0x861fa7e6dcb4aa15 },
    Wide { hi: 0xb4bca50b065abe63, lo: 0x0fed077a756b53aa },
    Wide { hi: 0xb67f6455292cbf08, lo: 0x1a3bc84c17b1d543 },
    Wide { hi: 0xb84687c269ef3bfb, lo: 0x3d5d514f40eea742 },
    Wide { hi: 0xba121a4650e4ddeb, lo: 0x92f34d62616ce413 },
    Wide { hi: 0xbbe226efb628afea, lo: 0x890489f70a55368c },
    Wide { hi: 0xbdb6b8e905cb600f, lo: 0x5400e987bbc1c921 },
    Wide { hi: 0xbf8fdb78849a5f96, lo: 0xde98520472bdd034 },
    Wide { hi: 0xc16d9a0095928a27, lo: 0x75b7053c0f178294 },
    Wide { hi: 0xc350000000000000, lo: 0x0000000000000000 },
    Wide { hi: 0xc5371912364ce305, lo: 0x6c28000000000000 },
    Wide { hi: 0xc722f0ef9d80aad6, lo: 0x424d3ad2b7b97ef6 },
    Wide { hi: 0xc913936dd571c84c, lo: 0x03bc3a19cd1e38ea },
    Wide { hi: 0xcb090c8001ab551c, lo: 0x5cadf5bfd3072cc6 },
    Wide { hi: 0xcd036837130890a1, lo: 0x36dba887c37a8c10 },
    Wide { hi: 0xcf02b2c21207ef2e, lo: 0x94f967e45e03f4bc },
    Wide { hi: 0xd106f86e69d785c7, lo: 0xe13336d701beba52 },
    Wide { hi: 0xd31045a8341ca07c, lo: 0x1ede48111209a051 },
    Wide { hi: 0xd51ea6fa85785631, lo: 0x552a74227f3ea566 },
    Wide { hi: 0xd732290fbacaf133, lo: 0xa97c177947ad4096 },
    Wide { hi: 0xd94ad8b1c7380874, lo: 0x18375281ae7822bc },
];

const POW10_FIXUPS: [u32; 20] = [
    0x0a4e363f, 0x00001840, 0x00006400, 0x24200040, 0x00000000,
    0x0c000000, 0x82c81380, 0x5e4ce01f, 0xd730f60f, 0x0000001b,
    0x00000000, 0xcdf7fffc, 0x6e8201d8, 0x40cd3fd1, 0xdb642501,
    0x00000d0d, 0x14042400, 0x53713840, 0x11781db4, 0x00000000,
];

const fn pow10_compute(i: u32) -> Wide {
    const STRIDE: u32 = POW10_MINOR.len() as u32;
    const MAJOR_OFFSET: u32 = 10;
    let m = POW10_MINOR[((i + MAJOR_OFFSET) % STRIDE) as usize];
    let h = POW10_MAJOR[((i + MAJOR_OFFSET) / STRIDE) as usize];
    let h1 = umul128_hi64(h.lo, m);
    let c0 = h.lo.wrapping_mul(m);
    let c1 = h1.wrapping_add(h.hi.wrapping_mul(m));
    let c2 = (c1 < h1) as u64 + umul128_hi64(h.hi, m);
    let mut result = if (c2 >> 63) != 0 {
        Wide { hi: c2, lo: c1 }
    } else {
        Wide {
            hi: (c2 << 1) | (c1 >> 63),
            lo: (c1 << 1) | (c0 >> 63),
        }
    };
    result.lo -= ((POW10_FIXUPS[(i >> 5) as usize] >> (i & 31)) & 1) as u64;
    result
}

const fn pow10_table() -> [Wide; POW10_COUNT] {
    let mut table = [Wide { hi: 0, lo: 0 }; POW10_COUNT];
    let mut i = 0;
    while i < POW10_COUNT {
        table[i] = pow10_compute(i as u32);
        i += 1;
    }
    table
}

static POW10: [Wide; POW10_COUNT] = pow10_table();

fn pow10(dec_exp: i32) -> Wide {
    POW10[(dec_exp - DEC_EXP_MIN) as usize]
}

struct Decimal {
    sig: i64,
    exp: i32,
    last_digit: u8,
    has_last_digit: bool,
}

mod private {
    pub trait Sealed: Copy {
        const NUM_BITS: u32;
        const NUM_SIG_BITS: u32;
        const MAX_DIGITS10: i32;
        const FIXED_DEC_EXP: core::ops::RangeInclusive<i32>;
        const THRESHOLD: u64;
        const DIGITS: usize;
        fn to_bits_u64(self) -> u64;
        fn shift_sig(sig: u64, shift: u32) -> u64;
    }
}

pub trait Float: private::Sealed {}
impl Float for f32 {}
impl Float for f64 {}

impl private::Sealed for f32 {
    const NUM_BITS: u32 = 32;
    const NUM_SIG_BITS: u32 = f32::MANTISSA_DIGITS - 1;
    const MAX_DIGITS10: i32 = 9;
    const FIXED_DEC_EXP: RangeInclusive<i32> = -6..=12;
    const THRESHOLD: u64 = 10_000_000;
    const DIGITS: usize = 8;
    fn to_bits_u64(self) -> u64 {
        u64::from(self.to_bits())
    }
    fn shift_sig(sig: u64, shift: u32) -> u64 {
        u64::from((sig as u32) << shift)
    }
}

impl private::Sealed for f64 {
    const NUM_BITS: u32 = 64;
    const NUM_SIG_BITS: u32 = f64::MANTISSA_DIGITS - 1;
    const MAX_DIGITS10: i32 = 17;
    const FIXED_DEC_EXP: RangeInclusive<i32> = -5..=15;
    const THRESHOLD: u64 = 1_000_000_000_000_000;
    const DIGITS: usize = 16;
    fn to_bits_u64(self) -> u64 {
        self.to_bits()
    }
    fn shift_sig(sig: u64, shift: u32) -> u64 {
        sig << shift
    }
}

const fn exp_bits(num_bits: u32, num_sig_bits: u32) -> u32 {
    num_bits - num_sig_bits - 1
}

const fn exp_offset(num_bits: u32, num_sig_bits: u32) -> i64 {
    ((1i64 << (exp_bits(num_bits, num_sig_bits) - 1)) - 1) + num_sig_bits as i64
}

fn to_decimal_irregular<F: Float>(bin_sig: u64, bin_exp: i32) -> Decimal {
    let dec_exp = compute_dec_exp(bin_exp, false);
    let shift = u32::from(compute_exp_shift(bin_exp, dec_exp + 1).wrapping_add(EXTRA_SHIFT as u8));
    let pow10 = pow10(-dec_exp - 1);
    let p = umul192_hi128(pow10.hi, pow10.lo, F::shift_sig(bin_sig, shift));
    let mut integral = p.hi >> EXTRA_SHIFT;
    let fractional = (p.hi << (64 - EXTRA_SHIFT)) | (p.lo >> EXTRA_SHIFT);
    let half_ulp = pow10.hi >> (EXTRA_SHIFT + 1 - shift);
    let round_up = half_ulp > u64::MAX - fractional;
    let round_down = (half_ulp >> 1) > fractional;
    integral += u64::from(round_up);
    let mut digit = umul128_add_hi64(fractional, 10, (1 << 63) - 1) as i32;
    let lo = umul128_add_hi64(fractional.wrapping_sub(half_ulp >> 1), 10, !0) as i32;
    if digit < lo {
        digit = lo;
    }
    Decimal {
        sig: integral as i64,
        exp: dec_exp,
        last_digit: digit as u8,
        has_last_digit: !(round_up || round_down),
    }
}

fn to_decimal_32(bin_sig: u64, bin_exp: i32) -> Decimal {
    let dec_exp = compute_dec_exp(bin_exp, true);
    let shift = u32::from(compute_exp_shift(bin_exp, dec_exp + 1).wrapping_add(EXTRA_SHIFT as u8))
        + (EXTRA_SHIFT_32 - EXTRA_SHIFT);
    let even = 1 - (bin_sig & 1);
    let pow10_hi = pow10(-dec_exp - 1).hi;
    let p = umul128_hi64(pow10_hi + 1, bin_sig << shift);
    let mut integral = p >> EXTRA_SHIFT_32;
    let fractional = p & ((1u64 << EXTRA_SHIFT_32) - 1);
    let half_ulp = (pow10_hi >> (65 - shift)) + even;
    let round_up = ((fractional + half_ulp) >> EXTRA_SHIFT_32) != 0;
    let round_down = half_ulp > fractional;
    integral += u64::from(round_up);
    let mut digit = ((fractional * 10 + (1u64 << (EXTRA_SHIFT_32 - 1))) >> EXTRA_SHIFT_32) as i32;
    if fractional == (1u64 << (EXTRA_SHIFT_32 - 2)) {
        digit = 2;
    }
    Decimal {
        sig: integral as i64,
        exp: dec_exp,
        last_digit: digit as u8,
        has_last_digit: !(round_up || round_down),
    }
}

fn to_decimal_64(bin_sig: u64, bin_exp: i32) -> Decimal {
    const BIASED_HALF: u64 = (1 << 63) + 6;
    let dec_exp = compute_dec_exp(bin_exp, true);
    let shift = u32::from(compute_exp_shift(bin_exp, dec_exp + 1).wrapping_add(EXTRA_SHIFT as u8));
    let even = 1 - (bin_sig & 1);
    let pow10 = pow10(-dec_exp - 1);
    let p = umul192_hi128(pow10.hi, pow10.lo, bin_sig << shift);
    let mut integral = p.hi >> EXTRA_SHIFT;
    let fractional = (p.hi << (64 - EXTRA_SHIFT)) | (p.lo >> EXTRA_SHIFT);
    let half_ulp = (pow10.hi >> (EXTRA_SHIFT + 1 - shift)) + even;
    let round_up = fractional.wrapping_add(half_ulp) < fractional;
    let round_down = half_ulp > fractional;
    integral += u64::from(round_up);
    let mut digit = umul128_add_hi64(fractional, 10, BIASED_HALF) as i32;
    if fractional == (1u64 << 62) {
        digit = 2;
    }
    Decimal {
        sig: integral as i64,
        exp: dec_exp,
        last_digit: digit as u8,
        has_last_digit: !(round_up || round_down),
    }
}

fn to_decimal<F: Float>(bin_sig: u64, raw_exp: i64, regular: bool) -> Decimal {
    let bin_exp = (raw_exp - exp_offset(F::NUM_BITS, F::NUM_SIG_BITS)) as i32;
    if !regular {
        return to_decimal_irregular::<F>(bin_sig, bin_exp);
    }
    if F::NUM_BITS == 32 {
        to_decimal_32(bin_sig, bin_exp)
    } else {
        to_decimal_64(bin_sig, bin_exp)
    }
}

fn put_octet<const N: usize>(text: &mut DecimalText<N>, at: usize, octet: u64) {
    text.put_quad(at, (octet / QUAD_SCALE) as u32);
    text.put_quad(at + 4, (octet % QUAD_SCALE) as u32);
}

fn trailing_zero_digits(mut value: u64) -> usize {
    let mut zeros = 0;
    while value % RADIX == 0 {
        value /= RADIX;
        zeros += 1;
    }
    zeros
}

fn put_exponent<const N: usize>(text: &mut DecimalText<N>, at: usize, dec_exp: i32) -> usize {
    text.put_exponent(at, dec_exp < 0);
    let magnitude = dec_exp.unsigned_abs();
    let digits = at + 2;
    if magnitude >= PAIR_SCALE {
        text.put_digit(digits, magnitude / PAIR_SCALE);
        text.put_pair(digits + 1, magnitude % PAIR_SCALE);
        digits + 3
    } else if magnitude >= RADIX as u32 {
        text.put_pair(digits, magnitude);
        digits + 2
    } else {
        text.put_digit(digits, magnitude);
        digits + 1
    }
}

fn write<F: Float>(value: F, text: &mut DecimalText<TEXT_BYTES>) -> usize {
    let bits = value.to_bits_u64();
    let exp_mask = (1u64 << exp_bits(F::NUM_BITS, F::NUM_SIG_BITS)) - 1;
    let implicit_bit = 1u64 << F::NUM_SIG_BITS;
    let bin_exp = ((bits >> F::NUM_SIG_BITS) & exp_mask) as i64;
    let bin_sig = bits & (implicit_bit - 1);
    let negative = (bits >> (F::NUM_BITS - 1)) != 0;
    if negative {
        text.put_minus(0);
    }
    let base = usize::from(negative);
    let threshold = F::THRESHOLD as i64;

    let mut dec;
    if bin_exp == 0 {
        if bin_sig == 0 {
            text.put_digit(base, 0);
            text.put_point(base + 1);
            text.put_digit(base + 2, 0);
            return base + 3;
        }
        dec = to_decimal::<F>(bin_sig, 1, true);
        let mut dec_sig = dec.sig * 10 + (-i64::from(dec.has_last_digit) & i64::from(dec.last_digit));
        let mut dec_exp = dec.exp;
        while dec_sig < threshold {
            dec_sig *= 10;
            dec_exp -= 1;
        }
        let quotient = div10(dec_sig as u64);
        let last_digit = dec_sig - quotient as i64 * 10;
        dec = Decimal {
            sig: quotient as i64,
            exp: dec_exp,
            last_digit: last_digit as u8,
            has_last_digit: last_digit != 0,
        };
    } else {
        dec = to_decimal::<F>(bin_sig | implicit_bit, bin_exp, bin_sig != 0);
    }
    let mut has_last_digit = dec.has_last_digit;
    let has_extra_digit = dec.sig >= threshold;
    let mut dec_exp = dec.exp + F::MAX_DIGITS10 - 2 + i32::from(has_extra_digit);
    if F::NUM_BITS == 32 && dec.sig < 1_000_000 {
        dec.sig = 10 * dec.sig + (-i64::from(has_last_digit) & i64::from(dec.last_digit));
        has_last_digit = false;
        dec_exp -= 1;
    }

    let sig = dec.sig as u64;
    let start = base + usize::from(has_extra_digit);
    if F::DIGITS == 16 {
        put_octet(text, start, sig / OCTET_SCALE);
        put_octet(text, start + 8, sig % OCTET_SCALE);
    } else {
        put_octet(text, start, sig);
    }
    if has_last_digit {
        text.put_digit(start + F::DIGITS, u32::from(dec.last_digit));
    }
    let num_digits = F::DIGITS - trailing_zero_digits(sig);
    let length = usize::from(has_extra_digit)
        + if has_last_digit {
            F::DIGITS + 1
        } else {
            num_digits
        }
        - 1;

    if F::FIXED_DEC_EXP.contains(&dec_exp) {
        if length as i32 - 1 <= dec_exp {
            let point = base + dec_exp as usize + 1;
            text.copy_within(base + 1..base + 1 + length, base);
            text.put_zeros(base + length..point + 2);
            text.put_point(point);
            return point + 2;
        } else if 0 <= dec_exp {
            let point = base + dec_exp as usize + 1;
            text.copy_within(base + 1..point + 1, base);
            text.put_point(point);
            return base + length + 1;
        } else {
            let lead = (1 - dec_exp) as usize;
            text.copy_within(base + 1..base + 1 + length, base + lead);
            text.put_zeros(base..base + lead);
            text.put_point(base + 1);
            return base + lead + length;
        }
    }

    text.copy_within(base + 1..base + 2, base);
    text.put_point(base + 1);
    put_exponent(text, base + length + usize::from(length > 1), dec_exp)
}

pub struct Buffer {
    text: DecimalText<TEXT_BYTES>,
}

impl Buffer {
    #[inline]
    pub const fn new() -> Self {
        Buffer {
            text: DecimalText::new(),
        }
    }

    pub fn format<F: Float>(&mut self, f: F) -> &str {
        let bits = f.to_bits_u64();
        let exp_mask = ((1u64 << exp_bits(F::NUM_BITS, F::NUM_SIG_BITS)) - 1) << F::NUM_SIG_BITS;
        if bits & exp_mask != exp_mask {
            return self.format_finite(f);
        }
        if bits & ((1u64 << F::NUM_SIG_BITS) - 1) != 0 {
            NAN
        } else if (bits >> (F::NUM_BITS - 1)) != 0 {
            NEG_INFINITY
        } else {
            INFINITY
        }
    }

    pub fn format_finite<F: Float>(&mut self, f: F) -> &str {
        let end = write(f, &mut self.text);
        self.text.span(0, end)
    }
}

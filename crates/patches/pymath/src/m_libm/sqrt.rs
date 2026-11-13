use super::{
    EXPONENT_BIAS, EXPONENT_BITS, EXPONENT_MASK, EXPONENT_SATURATED, IMPLICIT_BIT, SIGN_MASK,
    SIGNIFICAND_BITS, SIGNIFICAND_MASK, from_exponent,
};

const TABLE_INDEX_BITS: u32 = 6;
const TABLE_INDEX_MASK: usize = 0b111_1111;
const SECOND_SET_ROUNDS: u32 = 2;
const FINAL_ROUNDS: u32 = 2;

pub fn sqrt(x: f64) -> f64 {
    let mut ix = x.to_bits();
    let mut top = (ix >> SIGNIFICAND_BITS) as u32;
    let special_case = top.wrapping_sub(1) >= EXPONENT_SATURATED - 1;
    if special_case {
        if ix << 1 == 0 {
            return x;
        }
        if ix == EXPONENT_MASK {
            return x;
        }
        if ix > EXPONENT_MASK {
            return f64::NAN;
        }
        let scaled = x * from_exponent(SIGNIFICAND_BITS + EXPONENT_BIAS);
        ix = scaled.to_bits();
        top = ((scaled.to_bits() >> SIGNIFICAND_BITS) as u32 & EXPONENT_SATURATED)
            .wrapping_sub(SIGNIFICAND_BITS);
    }
    let mut e = top;
    let mut m_u2 = (ix | IMPLICIT_BIT) << EXPONENT_BITS;
    let even = (e & 1) != 0;
    if even {
        m_u2 >>= 1;
    }
    e = (e.wrapping_add(EXPONENT_SATURATED >> 1)) >> 1;
    let i = (ix >> (SIGNIFICAND_BITS - TABLE_INDEX_BITS)) as usize & TABLE_INDEX_MASK;
    let r1_u0: u32 = u32::from(RSQRT_TAB[i]) << (u32::BITS - u16::BITS);
    let s2_u2: u32 = (m_u2 >> (u64::BITS - u32::BITS)) as u32;
    let (r2_u0, _) = goldschmidt_u32(r1_u0, s2_u2, SECOND_SET_ROUNDS);
    let r_u0: u64 = u64::from(r2_u0) << (u64::BITS - u32::BITS);
    let (_, s_u2) = goldschmidt_u64(r_u0, m_u2, FINAL_ROUNDS);
    let mut m = s_u2 >> (EXPONENT_BITS - 2);
    let shift = 2 * SIGNIFICAND_BITS - (u64::BITS - 2);
    let d0 = (m_u2 << shift).wrapping_sub(m.wrapping_mul(m));
    let d1 = m.wrapping_sub(d0);
    m += d1 >> (u64::BITS - 1);
    m &= SIGNIFICAND_MASK;
    m |= u64::from(e) << SIGNIFICAND_BITS;
    let mut y = f64::from_bits(m);
    let d2 = d1.wrapping_add(m).wrapping_add(1);
    let mut tiny = if d2 == 0 { 0 } else { IMPLICIT_BIT };
    tiny |= (d1 ^ d2) & SIGN_MASK;
    let t = f64::from_bits(tiny);
    y = y + t;
    y
}

const fn high_u32(a: u32, b: u32) -> u32 {
    ((a as u64 * b as u64) >> u32::BITS) as u32
}

const fn high_u64(a: u64, b: u64) -> u64 {
    ((a as u128 * b as u128) >> u64::BITS) as u64
}

fn goldschmidt_u32(mut r_u0: u32, mut s_u2: u32, count: u32) -> (u32, u32) {
    let three_u2 = 0b11_u32 << (u32::BITS - 2);
    let mut u_u0 = r_u0;
    for i in 0..count {
        s_u2 = high_u32(s_u2, u_u0);
        if i > 0 {
            s_u2 <<= 1;
        }
        let d_u2 = high_u32(s_u2, r_u0);
        u_u0 = three_u2.wrapping_sub(d_u2);
        r_u0 = high_u32(r_u0, u_u0) << 1;
    }
    (r_u0, s_u2)
}

fn goldschmidt_u64(mut r_u0: u64, mut s_u2: u64, count: u32) -> (u64, u64) {
    let three_u2 = 0b11_u64 << (u64::BITS - 2);
    let mut u_u0 = r_u0;
    for i in 0..count {
        s_u2 = high_u64(s_u2, u_u0);
        if i > 0 && i + 1 < count {
            s_u2 <<= 1;
        }
        let d_u2 = high_u64(s_u2, r_u0);
        u_u0 = three_u2.wrapping_sub(d_u2);
        r_u0 = high_u64(r_u0, u_u0) << 1;
    }
    (r_u0, s_u2)
}

#[rustfmt::skip]
static RSQRT_TAB: [u16; 128] = [
    0xb451, 0xb2f0, 0xb196, 0xb044, 0xaef9, 0xadb6, 0xac79, 0xab43,
    0xaa14, 0xa8eb, 0xa7c8, 0xa6aa, 0xa592, 0xa480, 0xa373, 0xa26b,
    0xa168, 0xa06a, 0x9f70, 0x9e7b, 0x9d8a, 0x9c9d, 0x9bb5, 0x9ad1,
    0x99f0, 0x9913, 0x983a, 0x9765, 0x9693, 0x95c4, 0x94f8, 0x9430,
    0x936b, 0x92a9, 0x91ea, 0x912e, 0x9075, 0x8fbe, 0x8f0a, 0x8e59,
    0x8daa, 0x8cfe, 0x8c54, 0x8bac, 0x8b07, 0x8a64, 0x89c4, 0x8925,
    0x8889, 0x87ee, 0x8756, 0x86c0, 0x862b, 0x8599, 0x8508, 0x8479,
    0x83ec, 0x8361, 0x82d8, 0x8250, 0x81c9, 0x8145, 0x80c2, 0x8040,
    0xff02, 0xfd0e, 0xfb25, 0xf947, 0xf773, 0xf5aa, 0xf3ea, 0xf234,
    0xf087, 0xeee3, 0xed47, 0xebb3, 0xea27, 0xe8a3, 0xe727, 0xe5b2,
    0xe443, 0xe2dc, 0xe17a, 0xe020, 0xdecb, 0xdd7d, 0xdc34, 0xdaf1,
    0xd9b3, 0xd87b, 0xd748, 0xd61a, 0xd4f1, 0xd3cd, 0xd2ad, 0xd192,
    0xd07b, 0xcf69, 0xce5b, 0xcd51, 0xcc4a, 0xcb48, 0xca4a, 0xc94f,
    0xc858, 0xc764, 0xc674, 0xc587, 0xc49d, 0xc3b7, 0xc2d4, 0xc1f4,
    0xc116, 0xc03c, 0xbf65, 0xbe90, 0xbdbe, 0xbcef, 0xbc23, 0xbb59,
    0xba91, 0xb9cc, 0xb90a, 0xb84a, 0xb78c, 0xb6d0, 0xb617, 0xb560,
];

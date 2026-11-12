use alloc::vec::Vec;
use core::cmp::Ordering;

use crate::BigUint;
use crate::limbs::{self, DoubleLimb, LIMB_BITS, LIMB_BITS_U64, Limb};

const LEHMER_LEADING_BITS: u64 = LIMB_BITS_U64 - 2;
const SUBTRACTION_QUOTIENT_MAX: i64 = 4;

pub(crate) fn gcd(left: &BigUint, right: &BigUint) -> BigUint {
    if left.is_zero() {
        return right.clone();
    }
    if right.is_zero() {
        return left.clone();
    }
    if let (Some(a), Some(b)) = (left.small(), right.small()) {
        return BigUint::from_limb(gcd_limb(a, b));
    }
    let (mut larger, mut smaller) = match limbs::compare(left.limbs(), right.limbs()) {
        Ordering::Less => (right.limbs().to_vec(), left.limbs().to_vec()),
        _ => (left.limbs().to_vec(), right.limbs().to_vec()),
    };
    loop {
        if smaller.is_empty() {
            return BigUint::from_limbs(larger);
        }
        if larger.len() <= 2 {
            let a = to_double_limb(&larger);
            let b = to_double_limb(&smaller);
            return BigUint::from_double_limb(gcd_double_limb(a, b));
        }
        let cofactors = if larger.len() == smaller.len() {
            lehmer_cofactors(&larger, &smaller)
        } else {
            None
        };
        match cofactors {
            None => {
                let remainder = limbs::div_rem(&larger, &smaller).1;
                larger = core::mem::replace(&mut smaller, remainder);
            }
            Some(cofactors) => {
                apply_cofactors(&mut larger, &mut smaller, &cofactors);
                if limbs::compare(&larger, &smaller) == Ordering::Less {
                    core::mem::swap(&mut larger, &mut smaller);
                }
            }
        }
    }
}

struct Cofactors {
    a: i64,
    b: i64,
    c: i64,
    d: i64,
}

fn leading_bits(value: &[Limb], shift: u64) -> i64 {
    let index = (shift / LIMB_BITS_U64) as usize;
    let offset = (shift % LIMB_BITS_U64) as u32;
    let low = value.get(index).copied().unwrap_or(0);
    let high = value.get(index + 1).copied().unwrap_or(0);
    let combined = (DoubleLimb::from(high) << LIMB_BITS) | DoubleLimb::from(low);
    let leading = (combined >> offset) as Limb;
    assert!(u64::from(leading.leading_zeros()) >= LIMB_BITS_U64 - LEHMER_LEADING_BITS);
    i64::try_from(leading).expect("the leading bits fit")
}

fn lehmer_cofactors(larger: &[Limb], smaller: &[Limb]) -> Option<Cofactors> {
    let shift = limbs::bits(larger) - LEHMER_LEADING_BITS;
    let mut x = leading_bits(larger, shift);
    let mut y = leading_bits(smaller, shift);
    let (mut a, mut b, mut c, mut d): (i64, i64, i64, i64) = (1, 0, 0, 1);
    loop {
        if y + c == 0 || y + d == 0 {
            break;
        }
        let quotient = quotient_of(x + a, y + c);
        if quotient != quotient_of(x + b, y + d) {
            break;
        }
        (a, c) = (c, a - quotient * c);
        (b, d) = (d, b - quotient * d);
        (x, y) = (y, x - quotient * y);
    }
    if b == 0 {
        return None;
    }
    Some(Cofactors { a, b, c, d })
}

const fn quotient_of(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 && denominator > 0 && numerator / SUBTRACTION_QUOTIENT_MAX < denominator {
        let mut quotient = 0;
        let mut remainder = numerator;
        while remainder >= denominator {
            remainder -= denominator;
            quotient += 1;
        }
        return quotient;
    }
    numerator / denominator
}

fn apply_cofactors(larger: &mut Vec<Limb>, smaller: &mut Vec<Limb>, cofactors: &Cofactors) {
    assert_eq!(larger.len(), smaller.len());
    let mut carry_larger: i128 = 0;
    let mut carry_smaller: i128 = 0;
    for (large, small) in larger.iter_mut().zip(smaller.iter_mut()) {
        let large_value = i128::from(*large);
        let small_value = i128::from(*small);
        let next_large = i128::from(cofactors.a) * large_value
            + i128::from(cofactors.b) * small_value
            + carry_larger;
        let next_small = i128::from(cofactors.c) * large_value
            + i128::from(cofactors.d) * small_value
            + carry_smaller;
        *large = next_large as Limb;
        *small = next_small as Limb;
        carry_larger = next_large >> LIMB_BITS;
        carry_smaller = next_small >> LIMB_BITS;
    }
    assert_eq!(carry_larger, 0);
    assert_eq!(carry_smaller, 0);
    limbs::normalize(larger);
    limbs::normalize(smaller);
}

fn to_double_limb(value: &[Limb]) -> DoubleLimb {
    assert!(value.len() <= 2);
    value
        .iter()
        .rev()
        .fold(0, |wide, &limb| (wide << LIMB_BITS) | DoubleLimb::from(limb))
}

pub(crate) const fn gcd_limb(mut a: Limb, mut b: Limb) -> Limb {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

fn gcd_double_limb(mut a: DoubleLimb, mut b: DoubleLimb) -> DoubleLimb {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
        if let (Ok(small_a), Ok(small_b)) = (Limb::try_from(a), Limb::try_from(b)) {
            return DoubleLimb::from(gcd_limb(small_a, small_b)) << shift;
        }
    }
}


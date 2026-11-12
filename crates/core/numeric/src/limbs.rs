use alloc::vec;
use alloc::vec::Vec;
use core::cmp::Ordering;

pub(crate) type Limb = u64;
pub(crate) type DoubleLimb = u128;
pub(crate) const LIMB_BITS: u32 = Limb::BITS;
pub(crate) const LIMB_BITS_U64: u64 = LIMB_BITS as u64;

const KARATSUBA_LIMBS_MIN: usize = 32;
const KARATSUBA_SQUARE_LIMBS_MIN: usize = 48;
const TOOM3_LIMBS_MIN: usize = 1200;

pub(crate) fn trim(limbs: &[Limb]) -> &[Limb] {
    let mut length = limbs.len();
    while length > 0 && limbs[length - 1] == 0 {
        length -= 1;
    }
    &limbs[..length]
}

pub(crate) fn normalize(limbs: &mut Vec<Limb>) {
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
}

pub(crate) fn compare(left: &[Limb], right: &[Limb]) -> Ordering {
    debug_assert!(left.last() != Some(&0));
    debug_assert!(right.last() != Some(&0));
    left.len()
        .cmp(&right.len())
        .then_with(|| left.iter().rev().cmp(right.iter().rev()))
}

pub(crate) fn add_assign(target: &mut [Limb], addend: &[Limb]) -> bool {
    assert!(target.len() >= addend.len());
    let (low, high) = target.split_at_mut(addend.len());
    let mut carry = false;
    for (slot, &value) in low.iter_mut().zip(addend) {
        let (sum, carry_value) = slot.overflowing_add(value);
        let (sum, carry_in) = sum.overflowing_add(Limb::from(carry));
        *slot = sum;
        carry = carry_value | carry_in;
    }
    if carry {
        for slot in high {
            let (sum, overflow) = slot.overflowing_add(1);
            *slot = sum;
            if !overflow {
                return false;
            }
        }
    }
    carry
}

pub(crate) fn sub_assign(target: &mut [Limb], subtrahend: &[Limb]) -> bool {
    assert!(target.len() >= subtrahend.len());
    let (low, high) = target.split_at_mut(subtrahend.len());
    let mut borrow = false;
    for (slot, &value) in low.iter_mut().zip(subtrahend) {
        let (difference, borrow_value) = slot.overflowing_sub(value);
        let (difference, borrow_in) = difference.overflowing_sub(Limb::from(borrow));
        *slot = difference;
        borrow = borrow_value | borrow_in;
    }
    if borrow {
        for slot in high {
            let (difference, underflow) = slot.overflowing_sub(1);
            *slot = difference;
            if !underflow {
                return false;
            }
        }
    }
    borrow
}

pub(crate) fn add(left: &[Limb], right: &[Limb]) -> Vec<Limb> {
    let (long, short) = if left.len() >= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut sum = Vec::with_capacity(long.len() + 1);
    let mut carry: Limb = 0;
    sum.extend(long.iter().zip(short).map(|(&a, &b)| {
        let wide = DoubleLimb::from(a) + DoubleLimb::from(b) + DoubleLimb::from(carry);
        carry = (wide >> LIMB_BITS) as Limb;
        wide as Limb
    }));
    sum.extend(long[short.len()..].iter().map(|&a| {
        let wide = DoubleLimb::from(a) + DoubleLimb::from(carry);
        carry = (wide >> LIMB_BITS) as Limb;
        wide as Limb
    }));
    if carry != 0 {
        sum.push(carry);
    }
    sum
}

pub(crate) fn sub(minuend: &[Limb], subtrahend: &[Limb]) -> Vec<Limb> {
    assert!(minuend.len() >= subtrahend.len());
    let mut difference = Vec::with_capacity(minuend.len());
    let mut borrow: Limb = 0;
    difference.extend(minuend.iter().zip(subtrahend).map(|(&a, &b)| {
        let wide = DoubleLimb::from(a)
            .wrapping_sub(DoubleLimb::from(b))
            .wrapping_sub(DoubleLimb::from(borrow));
        borrow = (wide >> (DoubleLimb::BITS - 1)) as Limb;
        wide as Limb
    }));
    difference.extend(minuend[subtrahend.len()..].iter().map(|&a| {
        let wide = DoubleLimb::from(a).wrapping_sub(DoubleLimb::from(borrow));
        borrow = (wide >> (DoubleLimb::BITS - 1)) as Limb;
        wide as Limb
    }));
    assert_eq!(borrow, 0);
    normalize(&mut difference);
    difference
}

pub(crate) fn mul_limb(value: &[Limb], factor: Limb) -> Vec<Limb> {
    let mut product = Vec::with_capacity(value.len() + 1);
    let mut carry: Limb = 0;
    product.extend(value.iter().map(|&limb| {
        let wide = DoubleLimb::from(limb) * DoubleLimb::from(factor) + DoubleLimb::from(carry);
        carry = (wide >> LIMB_BITS) as Limb;
        wide as Limb
    }));
    if carry != 0 {
        product.push(carry);
    }
    product
}

pub(crate) fn mul_limb_add_assign(target: &mut Vec<Limb>, factor: Limb, addend: Limb) {
    let mut carry = addend;
    for slot in target.iter_mut() {
        let product = DoubleLimb::from(*slot) * DoubleLimb::from(factor) + DoubleLimb::from(carry);
        *slot = product as Limb;
        carry = (product >> LIMB_BITS) as Limb;
    }
    if carry != 0 {
        target.push(carry);
    }
}

fn add_mul_limb(target: &mut [Limb], source: &[Limb], factor: Limb) -> Limb {
    assert_eq!(target.len(), source.len());
    let mut carry: Limb = 0;
    for (slot, &value) in target.iter_mut().zip(source) {
        let product = DoubleLimb::from(value) * DoubleLimb::from(factor)
            + DoubleLimb::from(*slot)
            + DoubleLimb::from(carry);
        *slot = product as Limb;
        carry = (product >> LIMB_BITS) as Limb;
    }
    carry
}

fn sub_mul_limb(target: &mut [Limb], source: &[Limb], factor: Limb) -> Limb {
    assert_eq!(target.len(), source.len());
    let mut borrow: Limb = 0;
    for (slot, &value) in target.iter_mut().zip(source) {
        let product = DoubleLimb::from(value) * DoubleLimb::from(factor) + DoubleLimb::from(borrow);
        let low = product as Limb;
        let (difference, underflow) = slot.overflowing_sub(low);
        *slot = difference;
        borrow = (product >> LIMB_BITS) as Limb + Limb::from(underflow);
    }
    borrow
}

pub(crate) fn mul(left: &[Limb], right: &[Limb]) -> Vec<Limb> {
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    if right.len() == 1 && !core::ptr::eq(left, right) {
        let mut product = mul_limb(left, right[0]);
        normalize(&mut product);
        return product;
    }
    if left.len() == 1 && !core::ptr::eq(left, right) {
        let mut product = mul_limb(right, left[0]);
        normalize(&mut product);
        return product;
    }
    let mut product = vec![0; left.len() + right.len()];
    if core::ptr::eq(left, right) {
        square_into(&mut product, left);
    } else if left.len() >= right.len() {
        mul_into(&mut product, left, right);
    } else {
        mul_into(&mut product, right, left);
    }
    normalize(&mut product);
    product
}

pub(crate) fn square(value: &[Limb]) -> Vec<Limb> {
    mul(value, value)
}

fn mul_schoolbook(product: &mut [Limb], long: &[Limb], short: &[Limb]) {
    for (index, &factor) in short.iter().enumerate() {
        let carry = add_mul_limb(&mut product[index..index + long.len()], long, factor);
        product[index + long.len()] = carry;
    }
}

fn mul_into(product: &mut [Limb], long: &[Limb], short: &[Limb]) {
    assert!(long.len() >= short.len());
    assert_eq!(product.len(), long.len() + short.len());
    let short = trim(short);
    if short.is_empty() {
        return;
    }
    let product = &mut product[..long.len() + short.len()];
    if short.len() < KARATSUBA_LIMBS_MIN {
        mul_schoolbook(product, long, short);
        return;
    }
    if long.len() >= 2 * short.len() {
        mul_unbalanced(product, long, short);
        return;
    }
    if short.len() >= TOOM3_LIMBS_MIN && 3 * short.len() > 2 * long.len() + 4 {
        mul_toom3(product, long, short);
        return;
    }
    mul_karatsuba(product, long, short);
}

fn mul_unbalanced(product: &mut [Limb], long: &[Limb], short: &[Limb]) {
    let mut partial = vec![0; 2 * short.len()];
    for (chunk_index, chunk) in long.chunks(short.len()).enumerate() {
        let offset = chunk_index * short.len();
        let partial = &mut partial[..chunk.len() + short.len()];
        partial.fill(0);
        if chunk.len() >= short.len() {
            mul_into(partial, chunk, short);
        } else {
            mul_into(partial, short, chunk);
        }
        let carry = add_assign(&mut product[offset..], trim(partial));
        assert!(!carry);
    }
}

fn mul_karatsuba(product: &mut [Limb], long: &[Limb], short: &[Limb]) {
    let split = long.len() / 2;
    assert!(short.len() > split);
    let (long_low, long_high) = long.split_at(split);
    let (short_low, short_high) = short.split_at(split);
    let long_low = trim(long_low);
    let short_low = trim(short_low);
    {
        let (low_product, high_product) = product.split_at_mut(2 * split);
        mul_any_into(&mut low_product[..long_low.len() + short_low.len()], long_low, short_low);
        mul_any_into(high_product, long_high, short_high);
    }
    let long_sum = add(long_low, long_high);
    let short_sum = add(short_low, short_high);
    let mut middle = mul(trim(&long_sum), trim(&short_sum));
    let borrow = sub_assign(&mut middle, trim(&product[..2 * split]));
    assert!(!borrow);
    let borrow = sub_assign(&mut middle, trim(&product[2 * split..]));
    assert!(!borrow);
    let carry = add_assign(&mut product[split..], trim(&middle));
    assert!(!carry);
}

fn mul_any_into(product: &mut [Limb], left: &[Limb], right: &[Limb]) {
    if left.is_empty() || right.is_empty() {
        return;
    }
    if left.len() >= right.len() {
        mul_into(product, left, right);
    } else {
        mul_into(product, right, left);
    }
}

#[derive(Clone)]
struct Signed {
    negative: bool,
    magnitude: Vec<Limb>,
}

impl Signed {
    const fn from_magnitude(magnitude: Vec<Limb>) -> Self {
        Self {
            negative: false,
            magnitude,
        }
    }

    fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            return Self {
                negative: self.negative,
                magnitude: trimmed(add(&self.magnitude, &other.magnitude)),
            };
        }
        match compare(&self.magnitude, &other.magnitude) {
            Ordering::Equal => Self::from_magnitude(Vec::new()),
            Ordering::Greater => Self {
                negative: self.negative,
                magnitude: sub(&self.magnitude, &other.magnitude),
            },
            Ordering::Less => Self {
                negative: other.negative,
                magnitude: sub(&other.magnitude, &self.magnitude),
            },
        }
    }

    const fn negated(mut self) -> Self {
        if !self.magnitude.is_empty() {
            self.negative = !self.negative;
        }
        self
    }

    fn sub(&self, other: &Self) -> Self {
        self.add(&other.clone().negated())
    }

    fn mul(&self, other: &Self) -> Self {
        let magnitude = mul(&self.magnitude, &other.magnitude);
        Self {
            negative: !magnitude.is_empty() && self.negative != other.negative,
            magnitude,
        }
    }

    fn shl(&self, bits: u64) -> Self {
        Self {
            negative: self.negative,
            magnitude: trimmed(shl(&self.magnitude, bits)),
        }
    }

    fn div_limb_exact(&self, divisor: Limb) -> Self {
        let (quotient, remainder) = div_rem_limb(&self.magnitude, divisor);
        assert_eq!(remainder, 0);
        Self {
            negative: self.negative && !quotient.is_empty(),
            magnitude: quotient,
        }
    }

    fn shr_exact(&self, bits: u64) -> Self {
        let magnitude = shr(&self.magnitude, bits);
        Self {
            negative: self.negative && !magnitude.is_empty(),
            magnitude,
        }
    }
}

fn trimmed(mut limbs: Vec<Limb>) -> Vec<Limb> {
    normalize(&mut limbs);
    limbs
}

fn mul_toom3(product: &mut [Limb], long: &[Limb], short: &[Limb]) {
    let part = long.len().div_ceil(3);
    assert!(short.len() > 2 * part);
    let pieces = |value: &[Limb]| -> [Signed; 3] {
        let low = trim(&value[..part]).to_vec();
        let middle = trim(&value[part..2 * part]).to_vec();
        let high = trim(&value[2 * part..]).to_vec();
        [
            Signed::from_magnitude(low),
            Signed::from_magnitude(middle),
            Signed::from_magnitude(high),
        ]
    };
    let [a0, a1, a2] = pieces(long);
    let [b0, b1, b2] = pieces(short);
    let a02 = a0.add(&a2);
    let b02 = b0.add(&b2);
    let a_one = a02.add(&a1);
    let b_one = b02.add(&b1);
    let a_minus_one = a02.sub(&a1);
    let b_minus_one = b02.sub(&b1);
    let a_minus_two = a_minus_one.add(&a2).shl(1).sub(&a0);
    let b_minus_two = b_minus_one.add(&b2).shl(1).sub(&b0);
    let value_zero = a0.mul(&b0);
    let value_one = a_one.mul(&b_one);
    let value_minus_one = a_minus_one.mul(&b_minus_one);
    let value_minus_two = a_minus_two.mul(&b_minus_two);
    let value_infinity = a2.mul(&b2);
    let mut r3 = value_minus_two.sub(&value_one).div_limb_exact(3);
    let mut r1 = value_one.sub(&value_minus_one).shr_exact(1);
    let mut r2 = value_minus_one.sub(&value_zero);
    r3 = r2.sub(&r3).shr_exact(1).add(&value_infinity.shl(1));
    r2 = r2.add(&r1).sub(&value_infinity);
    r1 = r1.sub(&r3);
    let coefficients = [value_zero, r1, r2, r3, value_infinity];
    for (index, coefficient) in coefficients.iter().enumerate() {
        assert!(!coefficient.negative);
        let offset = index * part;
        if coefficient.magnitude.is_empty() {
            continue;
        }
        let carry = add_assign(&mut product[offset..], &coefficient.magnitude);
        assert!(!carry);
    }
}

fn square_schoolbook(product: &mut [Limb], value: &[Limb]) {
    let length = value.len();
    for index in 0..length {
        let factor = value[index];
        let rest = &value[index + 1..];
        let carry = add_mul_limb(&mut product[2 * index + 1..2 * index + 1 + rest.len()], rest, factor);
        product[index + length] = carry;
    }
    let mut carry: Limb = 0;
    for slot in product.iter_mut() {
        let next = *slot >> (LIMB_BITS - 1);
        *slot = (*slot << 1) | carry;
        carry = next;
    }
    assert_eq!(carry, 0);
    let mut carry = false;
    for index in 0..length {
        let diagonal = DoubleLimb::from(value[index]) * DoubleLimb::from(value[index]);
        let low = diagonal as Limb;
        let high = (diagonal >> LIMB_BITS) as Limb;
        let (sum, overflow_low) = product[2 * index].overflowing_add(low);
        let (sum, overflow_carry) = sum.overflowing_add(Limb::from(carry));
        product[2 * index] = sum;
        let carry_low = overflow_low | overflow_carry;
        let (sum, overflow_high) = product[2 * index + 1].overflowing_add(high);
        let (sum, overflow_carry) = sum.overflowing_add(Limb::from(carry_low));
        product[2 * index + 1] = sum;
        carry = overflow_high | overflow_carry;
    }
    assert!(!carry);
}

fn square_into(product: &mut [Limb], value: &[Limb]) {
    assert_eq!(product.len(), 2 * value.len());
    let value = trim(value);
    if value.is_empty() {
        return;
    }
    if value.len() < KARATSUBA_SQUARE_LIMBS_MIN {
        square_schoolbook(&mut product[..2 * value.len()], value);
        return;
    }
    if value.len() >= TOOM3_LIMBS_MIN {
        mul_toom3(&mut product[..2 * value.len()], value, value);
        return;
    }
    let split = value.len() / 2;
    let (low, high) = value.split_at(split);
    let low = trim(low);
    {
        let (low_product, high_product) = product.split_at_mut(2 * split);
        square_into(&mut low_product[..2 * low.len()], low);
        square_into(high_product, high);
    }
    let sum = add(low, high);
    let mut middle = square(trim(&sum));
    let borrow = sub_assign(&mut middle, trim(&product[..2 * split]));
    assert!(!borrow);
    let borrow = sub_assign(&mut middle, trim(&product[2 * split..]));
    assert!(!borrow);
    let carry = add_assign(&mut product[split..], trim(&middle));
    assert!(!carry);
}

pub(crate) fn shl(value: &[Limb], bits: u64) -> Vec<Limb> {
    if value.is_empty() {
        return Vec::new();
    }
    let limb_shift = usize::try_from(bits / LIMB_BITS_U64).expect("shift fits in memory");
    let bit_shift = (bits % LIMB_BITS_U64) as u32;
    let mut shifted = Vec::with_capacity(limb_shift + value.len() + 1);
    shifted.resize(limb_shift, 0);
    if bit_shift == 0 {
        shifted.extend_from_slice(value);
        return shifted;
    }
    let mut carry: Limb = 0;
    for &limb in value {
        shifted.push((limb << bit_shift) | carry);
        carry = limb >> (LIMB_BITS - bit_shift);
    }
    if carry != 0 {
        shifted.push(carry);
    }
    shifted
}

pub(crate) fn shr(value: &[Limb], bits: u64) -> Vec<Limb> {
    let limb_shift = bits / LIMB_BITS_U64;
    if limb_shift >= value.len() as u64 {
        return Vec::new();
    }
    let limb_shift = limb_shift as usize;
    let bit_shift = (bits % LIMB_BITS_U64) as u32;
    let source = &value[limb_shift..];
    if bit_shift == 0 {
        return source.to_vec();
    }
    let mut shifted = Vec::with_capacity(source.len());
    for index in 0..source.len() {
        let high = source.get(index + 1).map_or(0, |&next| next << (LIMB_BITS - bit_shift));
        shifted.push((source[index] >> bit_shift) | high);
    }
    normalize(&mut shifted);
    shifted
}

pub(crate) fn bits(value: &[Limb]) -> u64 {
    match value.last() {
        None => 0,
        Some(&top) => {
            (value.len() as u64) * LIMB_BITS_U64 - u64::from(top.leading_zeros())
        }
    }
}

pub(crate) fn trailing_zeros(value: &[Limb]) -> Option<u64> {
    let index = value.iter().position(|&limb| limb != 0)?;
    Some(index as u64 * LIMB_BITS_U64 + u64::from(value[index].trailing_zeros()))
}

#[derive(Clone, Copy)]
pub(crate) struct LimbDivisor {
    normalized: Limb,
    shift: u32,
    reciprocal: Limb,
}

impl LimbDivisor {
    pub(crate) fn new(divisor: Limb) -> Self {
        assert!(divisor != 0, "division by zero");
        let shift = divisor.leading_zeros();
        let normalized = divisor << shift;
        let numerator = (DoubleLimb::from(!normalized) << LIMB_BITS) | DoubleLimb::from(Limb::MAX);
        let reciprocal = (numerator / DoubleLimb::from(normalized)) as Limb;
        Self {
            normalized,
            shift,
            reciprocal,
        }
    }

    fn divide_normalized(&self, high: Limb, low: Limb) -> (Limb, Limb) {
        debug_assert!(high < self.normalized);
        let product = DoubleLimb::from(self.reciprocal) * DoubleLimb::from(high)
            + ((DoubleLimb::from(high) << LIMB_BITS) | DoubleLimb::from(low));
        let mut quotient = ((product >> LIMB_BITS) as Limb).wrapping_add(1);
        let quotient_low = product as Limb;
        let mut remainder = low.wrapping_sub(quotient.wrapping_mul(self.normalized));
        if remainder > quotient_low {
            quotient = quotient.wrapping_sub(1);
            remainder = remainder.wrapping_add(self.normalized);
        }
        if remainder >= self.normalized {
            quotient += 1;
            remainder -= self.normalized;
        }
        (quotient, remainder)
    }

    fn step(&self, remainder: Limb, limb: Limb) -> (Limb, Limb) {
        if self.shift == 0 {
            return self.divide_normalized(remainder, limb);
        }
        let high = (remainder << self.shift) | (limb >> (LIMB_BITS - self.shift));
        let (quotient, scaled) = self.divide_normalized(high, limb << self.shift);
        (quotient, scaled >> self.shift)
    }
}

pub(crate) fn div_rem_limb(dividend: &[Limb], divisor: Limb) -> (Vec<Limb>, Limb) {
    let mut quotient = dividend.to_vec();
    let remainder = div_rem_limb_in_place_with(&mut quotient, &LimbDivisor::new(divisor));
    (quotient, remainder)
}

pub(crate) fn div_rem_limb_in_place_with(value: &mut Vec<Limb>, divisor: &LimbDivisor) -> Limb {
    let mut remainder: Limb = 0;
    for slot in value.iter_mut().rev() {
        let (quotient, next) = divisor.step(remainder, *slot);
        *slot = quotient;
        remainder = next;
    }
    normalize(value);
    remainder
}

pub(crate) fn rem_limb(dividend: &[Limb], divisor: Limb) -> Limb {
    let divisor = LimbDivisor::new(divisor);
    let mut remainder: Limb = 0;
    for &limb in dividend.iter().rev() {
        remainder = divisor.step(remainder, limb).1;
    }
    remainder
}

pub(crate) fn div_rem(dividend: &[Limb], divisor: &[Limb]) -> (Vec<Limb>, Vec<Limb>) {
    assert!(!divisor.is_empty());
    if compare(dividend, divisor) == Ordering::Less {
        return (Vec::new(), dividend.to_vec());
    }
    if divisor.len() == 1 {
        let (quotient, remainder) = div_rem_limb(dividend, divisor[0]);
        let remainder = if remainder == 0 {
            Vec::new()
        } else {
            vec![remainder]
        };
        return (quotient, remainder);
    }
    if dividend.len() == divisor.len()
        || (dividend.len() == divisor.len() + 1 && compare(&dividend[1..], divisor) == Ordering::Less)
    {
        return div_rem_one_limb_quotient(dividend, divisor);
    }
    div_rem_knuth(dividend, divisor)
}

fn shifted_limb(value: &[Limb], index: usize, shift: u32) -> Limb {
    let limb = value.get(index).copied().unwrap_or(0);
    if shift == 0 {
        return limb;
    }
    let below = if index == 0 { 0 } else { value[index - 1] };
    (limb << shift) | (below >> (LIMB_BITS - shift))
}

fn div_rem_one_limb_quotient(dividend: &[Limb], divisor: &[Limb]) -> (Vec<Limb>, Vec<Limb>) {
    let length = divisor.len();
    assert!(length >= 2);
    assert!(dividend.len() == length || dividend.len() == length + 1);
    let shift = divisor[length - 1].leading_zeros();
    let divisor_top = shifted_limb(divisor, length - 1, shift);
    let divisor_next = DoubleLimb::from(shifted_limb(divisor, length - 2, shift));
    let window_top = shifted_limb(dividend, length, shift);
    let window_middle = shifted_limb(dividend, length - 1, shift);
    let window_low = shifted_limb(dividend, length - 2, shift);
    let top_divisor = LimbDivisor::new(divisor_top);
    let base = DoubleLimb::from(Limb::MAX) + 1;
    let divisor_top_wide = DoubleLimb::from(divisor_top);
    let (mut estimate, mut estimate_remainder) = if window_top >= divisor_top {
        let numerator = (DoubleLimb::from(window_top) << LIMB_BITS) | DoubleLimb::from(window_middle);
        let estimate = DoubleLimb::from(Limb::MAX);
        (estimate, numerator - estimate * divisor_top_wide)
    } else {
        let (estimate, remainder) = top_divisor.divide_normalized(window_top, window_middle);
        (DoubleLimb::from(estimate), DoubleLimb::from(remainder))
    };
    while estimate_remainder < base
        && estimate * divisor_next > ((estimate_remainder << LIMB_BITS) | DoubleLimb::from(window_low))
    {
        estimate -= 1;
        estimate_remainder += divisor_top_wide;
    }
    let mut estimate = estimate as Limb;
    let mut remainder = dividend.to_vec();
    let (low, high) = remainder.split_at_mut(length);
    let borrow = sub_mul_limb(low, divisor, estimate);
    let mut top = i128::from(high.first().copied().unwrap_or(0)) - i128::from(borrow);
    while top < 0 {
        estimate -= 1;
        top += i128::from(add_assign(low, divisor));
    }
    if let Some(slot) = high.first_mut() {
        *slot = Limb::try_from(top).expect("the remainder's top limb fits");
    } else {
        assert_eq!(top, 0);
    }
    normalize(&mut remainder);
    let quotient = if estimate == 0 { Vec::new() } else { vec![estimate] };
    (quotient, remainder)
}

fn div_rem_knuth(dividend: &[Limb], divisor: &[Limb]) -> (Vec<Limb>, Vec<Limb>) {
    let shift = divisor[divisor.len() - 1].leading_zeros();
    let shifted_divisor;
    let normalized_divisor: &[Limb] = if shift == 0 {
        divisor
    } else {
        shifted_divisor = shl(divisor, u64::from(shift));
        &shifted_divisor
    };
    assert_eq!(normalized_divisor.len(), divisor.len());
    let mut remainder = shl(dividend, u64::from(shift));
    if remainder.len() == dividend.len() {
        remainder.push(0);
    }
    let divisor_length = normalized_divisor.len();
    let quotient_length = dividend.len() - divisor_length + 1;
    let mut quotient = vec![0; quotient_length];
    let top_divisor = LimbDivisor::new(normalized_divisor[divisor_length - 1]);
    let divisor_top = DoubleLimb::from(normalized_divisor[divisor_length - 1]);
    let divisor_next = DoubleLimb::from(normalized_divisor[divisor_length - 2]);
    let base = DoubleLimb::from(Limb::MAX) + 1;
    for position in (0..quotient_length).rev() {
        let top = remainder[position + divisor_length];
        let next = remainder[position + divisor_length - 1];
        let (mut estimate, mut estimate_remainder) = if DoubleLimb::from(top) >= divisor_top {
            let numerator = (DoubleLimb::from(top) << LIMB_BITS) | DoubleLimb::from(next);
            let estimate = DoubleLimb::from(Limb::MAX);
            (estimate, numerator - estimate * divisor_top)
        } else {
            let (estimate, estimate_remainder) = top_divisor.divide_normalized(top, next);
            (DoubleLimb::from(estimate), DoubleLimb::from(estimate_remainder))
        };
        while estimate_remainder < base
            && estimate * divisor_next
                > ((estimate_remainder << LIMB_BITS)
                    | DoubleLimb::from(remainder[position + divisor_length - 2]))
        {
            estimate -= 1;
            estimate_remainder += divisor_top;
        }
        let window = &mut remainder[position..position + divisor_length + 1];
        let (low, high) = window.split_at_mut(divisor_length);
        let borrow = sub_mul_limb(low, normalized_divisor, estimate as Limb);
        let (difference, underflow) = high[0].overflowing_sub(borrow);
        high[0] = difference;
        let mut estimate = estimate as Limb;
        if underflow {
            estimate -= 1;
            let carry = add_assign(low, normalized_divisor);
            high[0] = high[0].wrapping_add(Limb::from(carry));
        }
        quotient[position] = estimate;
    }
    normalize(&mut quotient);
    remainder.truncate(divisor_length);
    shr_in_place(&mut remainder, shift);
    normalize(&mut remainder);
    (quotient, remainder)
}

fn shr_in_place(value: &mut [Limb], shift: u32) {
    if shift == 0 {
        return;
    }
    let length = value.len();
    for index in 0..length {
        let high = if index + 1 < length {
            value[index + 1] << (LIMB_BITS - shift)
        } else {
            0
        };
        value[index] = (value[index] >> shift) | high;
    }
}

pub(crate) fn twos_complement_in_place(limbs: &mut [Limb]) {
    let mut carry = true;
    for slot in limbs.iter_mut() {
        let (value, overflow) = (!*slot).overflowing_add(Limb::from(carry));
        *slot = value;
        carry = overflow;
    }
}


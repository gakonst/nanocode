use super::*;
use alloc::string::ToString;

fn b(s: &str) -> BigInt {
    let units: Vec<u16> = s.encode_utf16().collect();
    BigInt::parse_string(&units).unwrap_or_else(|| panic!("bad literal {s}"))
}

fn i(v: i128) -> BigInt {
    let neg = v < 0;
    let m = v.unsigned_abs();
    BigInt::new(
        neg,
        alloc::vec![
            m as u32,
            (m >> 32) as u32,
            (m >> 64) as u32,
            (m >> 96) as u32
        ],
    )
}

fn to_i128(x: &BigInt) -> i128 {
    assert!(x.mag.len() <= 4);
    let mut m: u128 = 0;
    for (k, &l) in x.mag.iter().enumerate() {
        m |= (l as u128) << (32 * k);
    }
    if x.neg { -(m as i128) } else { m as i128 }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A random value with up to `limbs` limbs, biased towards edge limbs.
    fn big(&mut self, limbs: usize) -> BigInt {
        let n = (self.next() as usize) % (limbs + 1);
        let mag = (0..n)
            .map(|_| match self.next() % 4 {
                0 => 0,
                1 => u32::MAX,
                _ => self.next() as u32,
            })
            .collect();
        BigInt::new(self.next() & 1 == 1, mag)
    }

    fn small(&mut self) -> i128 {
        let v = self.next() as i64 as i128;
        match self.next() % 3 {
            0 => v >> (self.next() % 64),
            _ => v,
        }
    }
}

#[test]
fn parse_and_print() {
    for s in [
        "0",
        "1",
        "-1",
        "4294967296",
        "-18446744073709551616",
        "123456789012345678901234567890",
    ] {
        assert_eq!(b(s).to_string_radix(10), s);
    }
    assert_eq!(b("0x10").to_string_radix(10), "16");
    assert_eq!(b("0o17").to_string_radix(10), "15");
    assert_eq!(b("0b101").to_string_radix(10), "5");
    assert_eq!(b("  -12 \n").to_string_radix(10), "-12");
    assert_eq!(b("+12").to_string_radix(10), "12");
    assert_eq!(b("").to_string_radix(10), "0");
    assert_eq!(b("-0").to_string_radix(10), "0");
    assert!(!b("-0").neg);
    for bad in [
        "-0x10", "1.5", "1e3", "1_000", "0x", "12n", "--1", "+", "0xg",
    ] {
        let units: Vec<u16> = bad.encode_utf16().collect();
        assert_eq!(BigInt::parse_string(&units), None, "{bad}");
    }
    assert_eq!(b("255").to_string_radix(16), "ff");
    assert_eq!(b("-255").to_string_radix(2), "-11111111");
    assert_eq!(b("-255").to_string_radix(8), "-377");
    assert_eq!(b("123456789").to_string_radix(36), "21i3v9");
    let big = b("0xfedcba9876543210fedcba9876543210f");
    assert_eq!(big.to_string_radix(16), "fedcba9876543210fedcba9876543210f");
    assert_eq!(
        big.to_string_radix(8),
        "77556272460731241441037667135230354520620417"
    );
}

#[test]
fn radix_round_trip() {
    let mut r = Rng(0x1234_5678);
    for _ in 0..300 {
        let x = r.big(12);
        for radix in [2, 3, 7, 8, 10, 16, 32, 36] {
            let s = x.to_string_radix(radix);
            let (neg, digits) = match s.strip_prefix('-') {
                Some(d) => (true, d),
                None => (false, s.as_str()),
            };
            let y = BigInt::parse_digits(digits.bytes().map(u32::from), radix).unwrap();
            assert_eq!(BigInt::new(neg, y.mag), x, "radix {radix}: {s}");
        }
    }
}

#[test]
fn small_arithmetic_matches_i128() {
    let mut r = Rng(0xDEAD_BEEF);
    for _ in 0..20000 {
        let (x, y) = (r.small(), r.small());
        let (a, c) = (i(x), i(y));
        assert_eq!(to_i128(&a.add(&c).unwrap()), x + y);
        assert_eq!(to_i128(&a.sub(&c).unwrap()), x - y);
        if let Some(p) = x.checked_mul(y) {
            assert_eq!(to_i128(&a.mul(&c).unwrap()), p, "{x} * {y}");
        }
        if y != 0 {
            let (q, m) = a.divrem(&c).unwrap();
            assert_eq!(to_i128(&q), x / y, "{x} / {y}");
            assert_eq!(to_i128(&m), x % y, "{x} % {y}");
        }
        assert_eq!(to_i128(&a.and(&c)), x & y);
        assert_eq!(to_i128(&a.or(&c)), x | y);
        assert_eq!(to_i128(&a.xor(&c)), x ^ y);
        assert_eq!(to_i128(&a.not().unwrap()), !x);
        let s = (r.next() % 70) as i64;
        assert_eq!(
            to_i128(&a.shr(&BigInt::from_i64(s)).unwrap()),
            x >> s.min(127)
        );
        if x.unsigned_abs() < 1 << 56 {
            assert_eq!(to_i128(&a.shl(&BigInt::from_i64(s)).unwrap()), x << s);
            assert_eq!(to_i128(&a.shr(&BigInt::from_i64(-s)).unwrap()), x << s);
        }
        assert_eq!(a.cmp(&c), x.cmp(&y));
        assert_eq!(a.to_string_radix(10), x.to_string());
        assert_eq!(a.as_uint_n(64).unwrap().to_u64_wrapping(), x as u64);
        assert_eq!(to_i128(&a.as_int_n(64).unwrap()), x as i64 as i128);
        assert_eq!(to_i128(&a.as_int_n(32).unwrap()), x as i32 as i128);
        assert_eq!(to_i128(&a.as_uint_n(8).unwrap()), x as u8 as i128);
        assert_eq!(a.to_u64_wrapping(), x as u64);
    }
}

#[test]
fn division_identity_on_large_values() {
    let mut r = Rng(0xFEED_F00D);
    for _ in 0..3000 {
        let x = r.big(20);
        let y = r.big(10);
        if y.is_zero() {
            assert_eq!(x.divrem(&y), Err(Error::DivisionByZero));
            continue;
        }
        let (q, m) = x.divrem(&y).unwrap();
        // x = q*y + m, |m| < |y|, m has the sign of x (or is zero).
        assert_eq!(q.mul(&y).unwrap().add(&m).unwrap(), x);
        assert_eq!(cmp_mag(&m.mag, &y.mag), Ordering::Less);
        assert!(m.is_zero() || m.neg == x.neg);
        // Distributivity cross-checks multiplication and addition.
        let z = r.big(8);
        let lhs = x.add(&y).unwrap().mul(&z).unwrap();
        let rhs = x.mul(&z).unwrap().add(&y.mul(&z).unwrap()).unwrap();
        assert_eq!(lhs, rhs);
        // Bitwise identities: x ^ y = (x | y) - (x & y); !x = -x - 1.
        assert_eq!(x.xor(&y), x.or(&y).sub(&x.and(&y)).unwrap());
        // Shifts agree with multiplication and floor division by powers of two.
        let s = r.next() % 100;
        let p = BigInt::from_u64(1).shl_bits(s).unwrap();
        assert_eq!(x.shl(&BigInt::from_u64(s)).unwrap(), x.mul(&p).unwrap());
        let (fq, fm) = x.divrem(&p).unwrap();
        let floor = if fm.is_zero() || !x.neg {
            fq
        } else {
            fq.sub(&BigInt::from_u64(1)).unwrap()
        };
        assert_eq!(x.shr(&BigInt::from_u64(s)).unwrap(), floor);
    }
}

#[test]
fn knuth_d_add_back_case() {
    // Classic operands that exercise the rare add-back step.
    let u = BigInt::from_parts(false, &[0, 0, 0x8000_0000, 0x7FFF_FFFF]);
    let v = BigInt::from_parts(false, &[1, 0, 0x8000_0000]);
    let (q, r) = u.divrem(&v).unwrap();
    assert_eq!(q.mul(&v).unwrap().add(&r).unwrap(), u);
    assert_eq!(cmp_mag(&r.mag, &v.mag), Ordering::Less);
    let u = BigInt::from_parts(false, &[0, 0xFFFF_FFFE, 0, 0x8000_0000]);
    let v = BigInt::from_parts(false, &[0xFFFF_FFFF, 0, 0x8000_0000]);
    let (q, r) = u.divrem(&v).unwrap();
    assert_eq!(q.mul(&v).unwrap().add(&r).unwrap(), u);
}

#[test]
fn powers() {
    assert_eq!(
        b("2").pow(&b("100")).unwrap().to_string_radix(10),
        "1267650600228229401496703205376"
    );
    assert_eq!(b("-3").pow(&b("3")).unwrap(), i(-27));
    assert_eq!(b("-3").pow(&b("4")).unwrap(), i(81));
    assert_eq!(b("7").pow(&b("0")).unwrap(), i(1));
    assert_eq!(b("0").pow(&b("0")).unwrap(), i(1));
    assert_eq!(b("0").pow(&b("10000000")).unwrap(), i(0));
    assert_eq!(b("-1").pow(&b("10000001")).unwrap(), i(-1));
    assert_eq!(b("1").pow(&b("0x100000000000000000000")).unwrap(), i(1));
    assert_eq!(b("2").pow(&b("-1")), Err(Error::NegativeExponent));
    assert_eq!(b("2").pow(&b("10000000")), Err(Error::ExponentTooLarge));
    assert_eq!(b("2").pow(&b("1048578")), Err(Error::ExponentTooLarge));
    assert_eq!(b("2").pow(&b("1048577")), Err(Error::TooLarge));
    assert_eq!(b("3").pow(&b("1000000")), Err(Error::TooLarge));
    assert_eq!(b("2").pow(&b("1048574")).unwrap().bits(), 1048575);
    assert_eq!(b("2").pow(&b("1048575")), Err(Error::TooLarge));
    assert_eq!(b("-2").pow(&b("1048574")).unwrap().bits(), 1048575);
    let x = b("12345678901234567890");
    let mut p = i(1);
    for _ in 0..13 {
        p = p.mul(&x).unwrap();
    }
    assert_eq!(x.pow(&i(13)).unwrap(), p);
}

#[test]
fn size_limits() {
    let one = i(1);
    assert_eq!(one.shl(&i(1048574)).unwrap().bits(), 1048575);
    assert_eq!(one.shl(&i(1048575)), Err(Error::TooLarge));
    assert_eq!(one.shl(&b("0x10000000000000000000")), Err(Error::TooLarge));
    assert_eq!(one.shr(&b("0x10000000000000000000")).unwrap(), i(0));
    assert_eq!(i(-1).shr(&b("0x10000000000000000000")).unwrap(), i(-1));
    assert_eq!(
        i(-5).shl(&b("0x10000000000000000000").neg()).unwrap(),
        i(-1)
    );
    assert_eq!(i(0).shl(&b("0x10000000000000000000")).unwrap(), i(0));
    let big = one.shl(&i(600000)).unwrap();
    assert_eq!(big.mul(&big), Err(Error::TooLarge));
    let max = one.shl(&i(1048574)).unwrap();
    assert_eq!(max.add(&max), Err(Error::TooLarge));
    assert_eq!(i(-1).as_uint_n(1 << 40), Err(Error::TooLarge));
    assert_eq!(i(5).as_uint_n(1 << 40).unwrap(), i(5));
    assert_eq!(i(-5).as_int_n(1 << 40).unwrap(), i(-5));
}

#[test]
fn as_int_n_edges() {
    assert_eq!(i(255).as_int_n(8).unwrap(), i(-1));
    assert_eq!(i(128).as_int_n(8).unwrap(), i(-128));
    assert_eq!(i(127).as_int_n(8).unwrap(), i(127));
    assert_eq!(i(-128).as_int_n(8).unwrap(), i(-128));
    assert_eq!(i(-129).as_int_n(8).unwrap(), i(127));
    assert_eq!(i(5).as_int_n(0).unwrap(), i(0));
    assert_eq!(i(-1).as_uint_n(0).unwrap(), i(0));
    assert_eq!(i(1).as_int_n(1).unwrap(), i(-1));
    assert_eq!(i(-1).as_uint_n(65).unwrap(), i((1i128 << 65) - 1));
    assert_eq!(i(1 << 63).as_int_n(64).unwrap(), i(i64::MIN as i128));
    assert_eq!(i(-(1 << 64)).as_int_n(65).unwrap(), i(-(1 << 64)));
    assert_eq!(i(1 << 64).as_int_n(65).unwrap(), i(-(1 << 64)));
}

#[test]
fn floats() {
    for f in [
        0.0,
        -0.0,
        1.0,
        -1.0,
        2f64.powi(53),
        2f64.powi(53) + 2.0,
        1e21,
        -1e300,
        f64::MAX,
    ] {
        let x = BigInt::from_f64(f);
        assert_eq!(x.to_f64(), if f == 0.0 { 0.0 } else { f });
        assert_eq!(x.cmp_f64(f), Some(Ordering::Equal));
    }
    assert_eq!(
        BigInt::from_f64(1e21).to_string_radix(10),
        "1000000000000000000000"
    );
    assert_eq!(BigInt::from_f64(f64::MAX).bits(), 1024);
    // Rounding: 2^53 + 1 is a tie (to even, 2^53); 2^53 + 3 rounds up.
    assert_eq!(b("9007199254740993").to_f64(), 9007199254740992.0);
    assert_eq!(b("9007199254740995").to_f64(), 9007199254740996.0);
    // A tie broken by a sticky bit far below the top 64 bits.
    let x = b("0x20000000000001")
        .shl(&i(100))
        .unwrap()
        .add(&i(1))
        .unwrap();
    assert_eq!(x.to_f64(), (9007199254740994.0f64) * 2f64.powi(100));
    let y = b("0x20000000000001").shl(&i(100)).unwrap();
    assert_eq!(y.to_f64(), (9007199254740992.0f64) * 2f64.powi(100));
    assert_eq!(i(1).shl(&i(1024)).unwrap().to_f64(), f64::INFINITY);
    assert_eq!(i(-1).shl(&i(5000)).unwrap().to_f64(), f64::NEG_INFINITY);
    assert_eq!(
        BigInt::from_f64(f64::MAX).add(&i(1)).unwrap().to_f64(),
        f64::MAX
    );
    assert_eq!(i(1).cmp_f64(1.5), Some(Ordering::Less));
    assert_eq!(i(2).cmp_f64(1.5), Some(Ordering::Greater));
    assert_eq!(i(-2).cmp_f64(-1.5), Some(Ordering::Less));
    assert_eq!(i(-1).cmp_f64(-1.5), Some(Ordering::Greater));
    assert_eq!(i(-1).cmp_f64(-(2f64.powi(63))), Some(Ordering::Greater));
    assert_eq!(
        i(-1).shl(&i(100)).unwrap().cmp_f64(-1e20),
        Some(Ordering::Less)
    );
    assert_eq!(i(1).cmp_f64(f64::NAN), None);
    assert_eq!(i(1).cmp_f64(f64::INFINITY), Some(Ordering::Less));
    assert_eq!(i(1).cmp_f64(f64::NEG_INFINITY), Some(Ordering::Greater));
    assert_eq!(
        b("9007199254740993").cmp_f64(9007199254740992.0),
        Some(Ordering::Greater)
    );
}

#[test]
fn conversions_to_i64() {
    assert_eq!(i(i64::MIN as i128).to_i64(), Some(i64::MIN));
    assert_eq!(i(i64::MAX as i128).to_i64(), Some(i64::MAX));
    assert_eq!(i(i64::MAX as i128 + 1).to_i64(), None);
    assert_eq!(i(i64::MIN as i128 - 1).to_i64(), None);
    assert_eq!(i(-1).to_u64_wrapping(), u64::MAX);
    assert_eq!(BigInt::from_i64(i64::MIN), i(i64::MIN as i128));
}

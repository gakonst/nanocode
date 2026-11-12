//! math.integer
//!
//! Integer-related mathematical functions.
//! This module requires either `num-bigint` or `malachite-bigint` feature.

use super::bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

/// Return the greatest common divisor of the integer arguments.
///
/// gcd() with no arguments returns 0.
/// gcd(a) returns abs(a).
#[inline]
pub fn gcd(args: &[&BigInt]) -> BigInt {
    if args.is_empty() {
        return BigInt::zero();
    }
    let mut result = args[0].abs();
    for arg in &args[1..] {
        result = result.gcd(*arg);
        if result.is_one() {
            return result;
        }
    }
    result
}

/// Return the least common multiple of the integer arguments.
///
/// lcm() with no arguments returns 1.
/// lcm(a) returns abs(a).
#[inline]
pub fn lcm(args: &[&BigInt]) -> BigInt {
    if args.is_empty() {
        return BigInt::one();
    }
    let mut result = args[0].abs();
    for arg in &args[1..] {
        if result.is_zero() || arg.is_zero() {
            return BigInt::zero();
        }
        let g = result.gcd(*arg);
        result = result / &g * arg.abs();
    }
    result
}

/// Approximate square roots for 16-bit integers.
/// For any n in range 2**14 <= n < 2**16, the value
///     a = APPROXIMATE_ISQRT_TAB[(n >> 8) - 64]
/// is an approximate square root of n, satisfying (a - 1)**2 < n < (a + 1)**2.
///
/// The table was computed in Python using:
///     [min(round(sqrt(256*n + 128)), 255) for n in range(64, 256)]
const APPROXIMATE_ISQRT_TAB: [u8; 192] = [
    128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 144, 145,
    146, 147, 148, 149, 150, 151, 151, 152, 153, 154, 155, 156, 156, 157, 158, 159, 160, 160, 161,
    162, 163, 164, 164, 165, 166, 167, 167, 168, 169, 170, 170, 171, 172, 173, 173, 174, 175, 176,
    176, 177, 178, 179, 179, 180, 181, 181, 182, 183, 183, 184, 185, 186, 186, 187, 188, 188, 189,
    190, 190, 191, 192, 192, 193, 194, 194, 195, 196, 196, 197, 198, 198, 199, 200, 200, 201, 201,
    202, 203, 203, 204, 205, 205, 206, 206, 207, 208, 208, 209, 210, 210, 211, 211, 212, 213, 213,
    214, 214, 215, 216, 216, 217, 217, 218, 219, 219, 220, 220, 221, 221, 222, 223, 223, 224, 224,
    225, 225, 226, 227, 227, 228, 228, 229, 229, 230, 230, 231, 232, 232, 233, 233, 234, 234, 235,
    235, 236, 237, 237, 238, 238, 239, 239, 240, 240, 241, 241, 242, 242, 243, 243, 244, 244, 245,
    246, 246, 247, 247, 248, 248, 249, 249, 250, 250, 251, 251, 252, 252, 253, 253, 254, 254, 255,
    255, 255,
];

/// Approximate square root of a large 64-bit integer.
///
/// Given `n` satisfying `2**62 <= n < 2**64`, return `a`
/// satisfying `(a - 1)**2 < n < (a + 1)**2`.
#[inline]
fn approximate_isqrt(n: u64) -> u32 {
    let u = APPROXIMATE_ISQRT_TAB[((n >> 56) - 64) as usize] as u32;
    let u = (u << 7) + (n >> 41) as u32 / u;
    (u << 15) + ((n >> 17) / u as u64) as u32
}

/// Return the integer part of the square root of a non-negative integer.
///
/// Returns Err(EDOM) if n is negative.
pub fn isqrt(n: &BigInt) -> crate::Result<BigInt> {
    if n.is_negative() {
        return Err(crate::Error::EDOM);
    }
    Ok(isqrt_unsigned(n.magnitude()).into())
}

/// Return the integer part of the square root of the input.
///
/// This is an adaptive-precision pure-integer version of Newton's iteration.
///
/// TODO: regard to expose as public API
fn isqrt_unsigned(n: &BigUint) -> BigUint {
    if n.is_zero() {
        return BigUint::zero();
    }

    // c = (n.bit_length() - 1) // 2
    let c = (n.bits() - 1) / 2;

    // Fast path: if c <= 31 then n < 2**64 and we can compute directly
    if c <= 31 {
        let shift = 31 - c as u32;
        let m = n.to_u64().unwrap();
        let mut u = approximate_isqrt(m << (2 * shift)) >> shift;
        if (u as u64) * (u as u64) > m {
            u -= 1;
        }
        return BigUint::from(u);
    }

    // Slow path: n >= 2**64
    // We perform the first five iterations in u64 arithmetic,
    // then switch to using BigUint.

    // From n >= 2**64 it follows that c.bit_length() >= 6
    let mut c_bit_length = 6u64;
    while (c >> c_bit_length) > 0 {
        c_bit_length += 1;
    }

    // Initialize d and a
    let d = c >> (c_bit_length - 5);
    let m = (n >> (2 * c - 62)).to_u64().unwrap();
    let u = approximate_isqrt(m) >> (31 - d as u32);
    let mut a = BigUint::from(u);

    let mut prev_d = d;
    for s in (0..=(c_bit_length - 6)).rev() {
        let e = prev_d;
        let d = c >> s;

        // q = (n >> 2*c - e - d + 1) // a
        let shift = 2 * c - d - e + 1;
        let q = (n >> shift) / &a;

        // a = (a << d - 1 - e) + q
        a = (a << (d - 1 - e) as usize) + q;

        prev_d = d;
    }

    // The correct result is either a or a - 1
    if &a * &a > *n {
        a -= 1u32;
    }

    a
}

// FACTORIAL

/// Lookup table for small factorial values
const SMALL_FACTORIALS: [u64; 21] = [
    1,
    1,
    2,
    6,
    24,
    120,
    720,
    5040,
    40320,
    362880,
    3628800,
    39916800,
    479001600,
    6227020800,
    87178291200,
    1307674368000,
    20922789888000,
    355687428096000,
    6402373705728000,
    121645100408832000,
    2432902008176640000,
];

/// Count the number of set bits in n
#[inline]
fn count_set_bits(n: u64) -> u64 {
    n.count_ones() as u64
}

/// Compute product(range(start, stop, 2)) using divide and conquer.
/// Assumes start and stop are odd and stop > start.
fn factorial_partial_product(start: u64, stop: u64, max_bits: u32) -> BigUint {
    let num_operands = (stop - start) / 2;

    // If the result fits in a u64, multiply directly
    if num_operands <= 64 && num_operands * (max_bits as u64) <= 64 {
        let mut total = start;
        let mut j = start + 2;
        while j < stop {
            total *= j;
            j += 2;
        }
        return BigUint::from(total);
    }

    // Find midpoint of range(start, stop), rounded up to next odd number
    let midpoint = (start + num_operands) | 1;
    let left = factorial_partial_product(start, midpoint, 64 - (midpoint - 2).leading_zeros());
    let right = factorial_partial_product(midpoint, stop, max_bits);
    left * right
}

/// Compute the odd part of factorial(n).
fn factorial_odd_part(n: u64) -> BigUint {
    let mut inner = BigUint::one();
    let mut outer = BigUint::one();

    let mut upper = 3u64;
    let n_bit_length = 64 - n.leading_zeros();

    for i in (0..=(n_bit_length.saturating_sub(2))).rev() {
        let v = n >> i;
        if v <= 2 {
            continue;
        }
        let lower = upper;
        // (v + 1) | 1 = least odd integer strictly larger than n / 2**i
        upper = (v + 1) | 1;
        let partial = factorial_partial_product(lower, upper, 64 - (upper - 2).leading_zeros());
        inner *= partial;
        outer *= &inner;
    }

    outer
}

/// Return n factorial (n!).
///
/// Returns Err(EDOM) if n is negative.
/// Uses the divide-and-conquer algorithm.
/// Based on: http://www.luschny.de/math/factorial/binarysplitfact.html
pub fn factorial(n: i64) -> crate::Result<BigUint> {
    if n < 0 {
        return Err(crate::Error::EDOM);
    }
    let n = n as u64;
    // Use lookup table for small values
    if n < SMALL_FACTORIALS.len() as u64 {
        return Ok(BigUint::from(SMALL_FACTORIALS[n as usize]));
    }

    // Express as odd_part * 2**two_valuation
    let odd_part = factorial_odd_part(n);
    let two_valuation = n - count_set_bits(n);
    Ok(odd_part << two_valuation as usize)
}

// COMB / PERM

/// Least significant 64 bits of the odd part of factorial(n), for n in range(128).
const REDUCED_FACTORIAL_ODD_PART: [u64; 128] = [
    0x0000000000000001,
    0x0000000000000001,
    0x0000000000000001,
    0x0000000000000003,
    0x0000000000000003,
    0x000000000000000f,
    0x000000000000002d,
    0x000000000000013b,
    0x000000000000013b,
    0x0000000000000b13,
    0x000000000000375f,
    0x0000000000026115,
    0x000000000007233f,
    0x00000000005cca33,
    0x0000000002898765,
    0x00000000260eeeeb,
    0x00000000260eeeeb,
    0x0000000286fddd9b,
    0x00000016beecca73,
    0x000001b02b930689,
    0x00000870d9df20ad,
    0x0000b141df4dae31,
    0x00079dd498567c1b,
    0x00af2e19afc5266d,
    0x020d8a4d0f4f7347,
    0x335281867ec241ef,
    0x9b3093d46fdd5923,
    0x5e1f9767cc5866b1,
    0x92dd23d6966aced7,
    0xa30d0f4f0a196e5b,
    0x8dc3e5a1977d7755,
    0x2ab8ce915831734b,
    0x2ab8ce915831734b,
    0x81d2a0bc5e5fdcab,
    0x9efcac82445da75b,
    0xbc8b95cf58cde171,
    0xa0e8444a1f3cecf9,
    0x4191deb683ce3ffd,
    0xddd3878bc84ebfc7,
    0xcb39a64b83ff3751,
    0xf8203f7993fc1495,
    0xbd2a2a78b35f4bdd,
    0x84757be6b6d13921,
    0x3fbbcfc0b524988b,
    0xbd11ed47c8928df9,
    0x3c26b59e41c2f4c5,
    0x677a5137e883fdb3,
    0xff74e943b03b93dd,
    0xfe5ebbcb10b2bb97,
    0xb021f1de3235e7e7,
    0x33509eb2e743a58f,
    0x390f9da41279fb7d,
    0xe5cb0154f031c559,
    0x93074695ba4ddb6d,
    0x81c471caa636247f,
    0xe1347289b5a1d749,
    0x286f21c3f76ce2ff,
    0x00be84a2173e8ac7,
    0x1595065ca215b88b,
    0xf95877595b018809,
    0x9c2efe3c5516f887,
    0x373294604679382b,
    0xaf1ff7a888adcd35,
    0x18ddf279a2c5800b,
    0x18ddf279a2c5800b,
    0x505a90e2542582cb,
    0x5bacad2cd8d5dc2b,
    0xfe3152bcbff89f41,
    0xe1467e88bf829351,
    0xb8001adb9e31b4d5,
    0x2803ac06a0cbb91f,
    0x1904b5d698805799,
    0xe12a648b5c831461,
    0x3516abbd6160cfa9,
    0xac46d25f12fe036d,
    0x78bfa1da906b00ef,
    0xf6390338b7f111bd,
    0x0f25f80f538255d9,
    0x4ec8ca55b8db140f,
    0x4ff670740b9b30a1,
    0x8fd032443a07f325,
    0x80dfe7965c83eeb5,
    0xa3dc1714d1213afd,
    0x205b7bbfcdc62007,
    0xa78126bbe140a093,
    0x9de1dc61ca7550cf,
    0x84f0046d01b492c5,
    0x2d91810b945de0f3,
    0xf5408b7f6008aa71,
    0x43707f4863034149,
    0xdac65fb9679279d5,
    0xc48406e7d1114eb7,
    0xa7dc9ed3c88e1271,
    0xfb25b2efdb9cb30d,
    0x1bebda0951c4df63,
    0x5c85e975580ee5bd,
    0x1591bc60082cb137,
    0x2c38606318ef25d7,
    0x76ca72f7c5c63e27,
    0xf04a75d17baa0915,
    0x77458175139ae30d,
    0x0e6c1330bc1b9421,
    0xdf87d2b5797e8293,
    0xefa5c703e1e68925,
    0x2b6b1b3278b4f6e1,
    0xceee27b382394249,
    0xd74e3829f5dab91d,
    0xfdb17989c26b5f1f,
    0xc1b7d18781530845,
    0x7b4436b2105a8561,
    0x7ba7c0418372a7d7,
    0x9dbc5c67feb6c639,
    0x502686d7f6ff6b8f,
    0x6101855406be7a1f,
    0x9956afb5806930e7,
    0xe1f0ee88af40f7c5,
    0x984b057bda5c1151,
    0x9a49819acc13ea05,
    0x8ef0dead0896ef27,
    0x71f7826efe292b21,
    0xad80a480e46986ef,
    0x01cdc0ebf5e0c6f7,
    0x6e06f839968f68db,
    0xdd5943ab56e76139,
    0xcdcf31bf8604c5e7,
    0x7e2b4a847054a1cb,
    0x0ca75697a4d3d0f5,
    0x4703f53ac514a98b,
];

/// Inverses of reduced_factorial_odd_part values modulo 2**64.
const INVERTED_FACTORIAL_ODD_PART: [u64; 128] = [
    0x0000000000000001,
    0x0000000000000001,
    0x0000000000000001,
    0xaaaaaaaaaaaaaaab,
    0xaaaaaaaaaaaaaaab,
    0xeeeeeeeeeeeeeeef,
    0x4fa4fa4fa4fa4fa5,
    0x2ff2ff2ff2ff2ff3,
    0x2ff2ff2ff2ff2ff3,
    0x938cc70553e3771b,
    0xb71c27cddd93e49f,
    0xb38e3229fcdee63d,
    0xe684bb63544a4cbf,
    0xc2f684917ca340fb,
    0xf747c9cba417526d,
    0xbb26eb51d7bd49c3,
    0xbb26eb51d7bd49c3,
    0xb0a7efb985294093,
    0xbe4b8c69f259eabb,
    0x6854d17ed6dc4fb9,
    0xe1aa904c915f4325,
    0x3b8206df131cead1,
    0x79c6009fea76fe13,
    0xd8c5d381633cd365,
    0x4841f12b21144677,
    0x4a91ff68200b0d0f,
    0x8f9513a58c4f9e8b,
    0x2b3e690621a42251,
    0x4f520f00e03c04e7,
    0x2edf84ee600211d3,
    0xadcaa2764aaacdfd,
    0x161f4f9033f4fe63,
    0x161f4f9033f4fe63,
    0xbada2932ea4d3e03,
    0xcec189f3efaa30d3,
    0xf7475bb68330bf91,
    0x37eb7bf7d5b01549,
    0x46b35660a4e91555,
    0xa567c12d81f151f7,
    0x4c724007bb2071b1,
    0x0f4a0cce58a016bd,
    0xfa21068e66106475,
    0x244ab72b5a318ae1,
    0x366ce67e080d0f23,
    0xd666fdae5dd2a449,
    0xd740ddd0acc06a0d,
    0xb050bbbb28e6f97b,
    0x70b003fe890a5c75,
    0xd03aabff83037427,
    0x13ec4ca72c783bd7,
    0x90282c06afdbd96f,
    0x4414ddb9db4a95d5,
    0xa2c68735ae6832e9,
    0xbf72d71455676665,
    0xa8469fab6b759b7f,
    0xc1e55b56e606caf9,
    0x40455630fc4a1cff,
    0x0120a7b0046d16f7,
    0xa7c3553b08faef23,
    0x9f0bfd1b08d48639,
    0xa433ffce9a304d37,
    0xa22ad1d53915c683,
    0xcb6cbc723ba5dd1d,
    0x547fb1b8ab9d0ba3,
    0x547fb1b8ab9d0ba3,
    0x8f15a826498852e3,
    0x32e1a03f38880283,
    0x3de4cce63283f0c1,
    0x5dfe6667e4da95b1,
    0xfda6eeeef479e47d,
    0xf14de991cc7882df,
    0xe68db79247630ca9,
    0xa7d6db8207ee8fa1,
    0x255e1f0fcf034499,
    0xc9a8990e43dd7e65,
    0x3279b6f289702e0f,
    0xe7b5905d9b71b195,
    0x03025ba41ff0da69,
    0xb7df3d6d3be55aef,
    0xf89b212ebff2b361,
    0xfe856d095996f0ad,
    0xd6e533e9fdf20f9d,
    0xf8c0e84a63da3255,
    0xa677876cd91b4db7,
    0x07ed4f97780d7d9b,
    0x90a8705f258db62f,
    0xa41bbb2be31b1c0d,
    0x6ec28690b038383b,
    0xdb860c3bb2edd691,
    0x0838286838a980f9,
    0x558417a74b36f77d,
    0x71779afc3646ef07,
    0x743cda377ccb6e91,
    0x7fdf9f3fe89153c5,
    0xdc97d25df49b9a4b,
    0x76321a778eb37d95,
    0x7cbb5e27da3bd487,
    0x9cff4ade1a009de7,
    0x70eb166d05c15197,
    0xdcf0460b71d5fe3d,
    0x5ac1ee5260b6a3c5,
    0xc922dedfdd78efe1,
    0xe5d381dc3b8eeb9b,
    0xd57e5347bafc6aad,
    0x86939040983acd21,
    0x395b9d69740a4ff9,
    0x1467299c8e43d135,
    0x5fe440fcad975cdf,
    0xcaa9a39794a6ca8d,
    0xf61dbd640868dea1,
    0xac09d98d74843be7,
    0x2b103b9e1a6b4809,
    0x2ab92d16960f536f,
    0x6653323d5e3681df,
    0xefd48c1c0624e2d7,
    0xa496fefe04816f0d,
    0x1754a7b07bbdd7b1,
    0x23353c829a3852cd,
    0xbf831261abd59097,
    0x57a8e656df0618e1,
    0x16e9206c3100680f,
    0xadad4c6ee921dac7,
    0x635f2b3860265353,
    0xdd6d0059f44b3d09,
    0xac4dd6b894447dd7,
    0x42ea183eeaa87be3,
    0x15612d1550ee5b5d,
    0x226fa19d656cb623,
];

/// Exponent of the largest power of 2 dividing factorial(n), for n in range(128).
const FACTORIAL_TRAILING_ZEROS: [u8; 128] = [
    0, 0, 1, 1, 3, 3, 4, 4, 7, 7, 8, 8, 10, 10, 11, 11, //  0-15
    15, 15, 16, 16, 18, 18, 19, 19, 22, 22, 23, 23, 25, 25, 26, 26, // 16-31
    31, 31, 32, 32, 34, 34, 35, 35, 38, 38, 39, 39, 41, 41, 42, 42, // 32-47
    46, 46, 47, 47, 49, 49, 50, 50, 53, 53, 54, 54, 56, 56, 57, 57, // 48-63
    63, 63, 64, 64, 66, 66, 67, 67, 70, 70, 71, 71, 73, 73, 74, 74, // 64-79
    78, 78, 79, 79, 81, 81, 82, 82, 85, 85, 86, 86, 88, 88, 89, 89, // 80-95
    94, 94, 95, 95, 97, 97, 98, 98, 101, 101, 102, 102, 104, 104, 105, 105, // 96-111
    109, 109, 110, 110, 112, 112, 113, 113, 116, 116, 117, 117, 119, 119, 120, 120, // 112-127
];

/// Maximal n so that 2*k-1 <= n <= 127 and C(n, k) fits into a u64.
const FAST_COMB_LIMITS1: [u8; 35] = [
    0, 0, 127, 127, 127, 127, 127, 127, // 0-7
    127, 127, 127, 127, 127, 127, 127, 127, // 8-15
    116, 105, 97, 91, 86, 82, 78, 76, // 16-23
    74, 72, 71, 70, 69, 68, 68, 67, // 24-31
    67, 67, 67, // 32-34
];

/// Maximal n so that 2*k-1 <= n <= 127 and C(n, k)*k fits into a u64.
const FAST_COMB_LIMITS2: [u64; 14] = [
    0,
    u64::MAX,
    4294967296,
    3329022,
    102570,
    13467,
    3612,
    1449, // 0-7
    746,
    453,
    308,
    227,
    178,
    147, // 8-13
];

/// Maximal n so that k <= n and P(n, k) fits into a u64.
const FAST_PERM_LIMITS: [u64; 21] = [
    0,
    u64::MAX,
    4294967296,
    2642246,
    65537,
    7133,
    1627,
    568, // 0-7
    259,
    142,
    88,
    61,
    45,
    36,
    30,
    26, // 8-15
    24,
    22,
    21,
    20,
    20, // 16-20
];

/// Calculate C(n, k) or P(n, k) for n in the 63-bit range.
pub(super) fn perm_comb_small(n: u64, k: u64, is_comb: bool) -> BigUint {
    if k == 0 {
        return BigUint::one();
    }

    if is_comb {
        // Fast path 1: use lookup tables for small n
        if (k as usize) < FAST_COMB_LIMITS1.len() && n <= FAST_COMB_LIMITS1[k as usize] as u64 {
            let comb_odd_part = REDUCED_FACTORIAL_ODD_PART[n as usize]
                .wrapping_mul(INVERTED_FACTORIAL_ODD_PART[k as usize])
                .wrapping_mul(INVERTED_FACTORIAL_ODD_PART[(n - k) as usize]);
            let shift = FACTORIAL_TRAILING_ZEROS[n as usize] as i32
                - FACTORIAL_TRAILING_ZEROS[k as usize] as i32
                - FACTORIAL_TRAILING_ZEROS[(n - k) as usize] as i32;
            return BigUint::from(comb_odd_part << shift);
        }

        // Fast path 2: sequential multiplication for medium values
        if (k as usize) < FAST_COMB_LIMITS2.len() && n <= FAST_COMB_LIMITS2[k as usize] {
            let mut result = n;
            let mut n = n;
            let mut i = 1u64;
            while i < k {
                n -= 1;
                result *= n;
                i += 1;
                result /= i;
            }
            return BigUint::from(result);
        }
    } else {
        // Permutation fast paths
        if (k as usize) < FAST_PERM_LIMITS.len() && n <= FAST_PERM_LIMITS[k as usize] {
            if n <= 127 {
                let perm_odd_part = REDUCED_FACTORIAL_ODD_PART[n as usize]
                    .wrapping_mul(INVERTED_FACTORIAL_ODD_PART[(n - k) as usize]);
                let shift = FACTORIAL_TRAILING_ZEROS[n as usize] as i32
                    - FACTORIAL_TRAILING_ZEROS[(n - k) as usize] as i32;
                return BigUint::from(perm_odd_part << shift);
            }

            let mut result = n;
            let mut n = n;
            let mut i = 1u64;
            while i < k {
                n -= 1;
                result *= n;
                i += 1;
            }
            return BigUint::from(result);
        }
    }

    // For larger n use recursive formulas:
    // P(n, k) = P(n, j) * P(n-j, k-j)
    // C(n, k) = C(n, j) * C(n-j, k-j) / C(k, j)
    let j = k / 2;
    let a = perm_comb_small(n, j, is_comb);
    let b = perm_comb_small(n - j, k - j, is_comb);
    let mut result = a * b;
    if is_comb {
        let c = perm_comb_small(k, j, true);
        result /= c;
    }
    result
}

/// Return the number of ways to choose k items from n items (n choose k).
///
/// Returns Err(EDOM) if n or k is negative.
/// Evaluates to n! / (k! * (n - k)!) when k <= n and evaluates
/// to zero when k > n.
pub fn comb(n: i64, k: i64) -> crate::Result<BigUint> {
    if n < 0 || k < 0 {
        return Err(crate::Error::EDOM);
    }
    let (n, k) = (n as u64, k as u64);
    if k > n {
        return Ok(BigUint::zero());
    }

    // Use smaller k for efficiency
    let k = k.min(n - k);

    if k <= 1 {
        if k == 0 {
            return Ok(BigUint::one());
        }
        return Ok(BigUint::from(n));
    }

    Ok(perm_comb_small(n, k, true))
}

/// Return the number of ways to arrange k items from n items.
///
/// Returns Err(EDOM) if n or k is negative.
/// Evaluates to n! / (n - k)! when k <= n and evaluates
/// to zero when k > n.
///
/// If k is not specified (None), then k defaults to n
/// and the function returns n!.
pub fn perm(n: i64, k: Option<i64>) -> crate::Result<BigUint> {
    if n < 0 {
        return Err(crate::Error::EDOM);
    }
    let n = n as u64;
    let k = match k {
        Some(k) if k < 0 => return Err(crate::Error::EDOM),
        Some(k) => k as u64,
        None => n,
    };
    if k > n {
        return Ok(BigUint::zero());
    }

    if k == 0 {
        return Ok(BigUint::one());
    }
    if k == 1 {
        return Ok(BigUint::from(n));
    }

    Ok(perm_comb_small(n, k, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;

    /// Edge i64 values for testing integer math functions (gcd, lcm, isqrt, factorial, comb, perm)
    const EDGE_I64: &[i64] = &[
        // Zero and small values
        0,
        1,
        -1,
        2,
        -2,
        // Prime numbers
        7,
        13,
        97,
        127, // table boundary in comb/perm
        128, // Table boundary + 1
        // Powers of 2 and boundaries
        64,
        63, // 2^6 - 1
        65, // 2^6 + 1
        1024,
        65535, // 2^16 - 1
        65536, // 2^16
        65537, // 2^16 + 1 (Fermat prime)
        // Factorial-relevant
        12,  // 12! = 479001600 fits in u32
        13,  // 13! overflows u32
        20,  // 20! fits in u64
        21,  // 21! overflows u64
        170, // factorial(170) is the largest that fits in f64
        171, // factorial(171) overflows f64
        // Comb/perm algorithm switching points
        34, // FAST_COMB_LIMITS1 boundary
        35,
        // Large values
        1_000_000,
        -1_000_000,
        i32::MAX as i64,
        i32::MAX as i64 + 1,
        i32::MIN as i64,
        i32::MIN as i64 - 1,
        // Near i64 bounds
        i64::MAX,
        i64::MIN,
        i64::MAX - 1,
        i64::MIN + 1,
        // Square root boundaries
        (1i64 << 15) - 1, // 32767, sqrt = 181
        1i64 << 16,       // 65536, sqrt = 256 (exact)
        (1i64 << 31) - 1, // sqrt fits in u16
        1i64 << 32,       // sqrt boundary (exact power of 2)
        (1i64 << 32) - 1, // near sqrt boundary
        (1i64 << 32) + 1, // just above sqrt boundary
        (1i64 << 62) - 1, // large but valid for isqrt
        1i64 << 62,       // exact power of 2
        // Near perfect squares
        99,  // sqrt(99) = 9.949...
        101, // sqrt(101) = 10.049...
    ];

    fn test_gcd_impl(args: &[i64]) {
        use std::str::FromStr;
        let bigints: Vec<BigInt> = args.iter().map(|&x| BigInt::from(x)).collect();
        let refs: Vec<_> = bigints.iter().collect();
        let rs = gcd(&refs);
        crate::test::with_py_math(|py, math| {
            let py_args = pyo3::types::PyTuple::new(py, args).unwrap();
            let py_result = math.getattr("gcd").unwrap().call1(py_args).unwrap();
            let py_str: String = py_result.str().unwrap().extract().unwrap();
            let py = BigInt::from_str(&py_str).unwrap();
            assert_eq!(rs, py, "gcd({args:?}): py={py} vs rs={rs}");
        });
    }

    fn test_lcm_impl(args: &[i64]) {
        use std::str::FromStr;
        let bigints: Vec<BigInt> = args.iter().map(|&x| BigInt::from(x)).collect();
        let refs: Vec<_> = bigints.iter().collect();
        let rs = lcm(&refs);
        crate::test::with_py_math(|py, math| {
            let py_args = pyo3::types::PyTuple::new(py, args).unwrap();
            let py_result = math.getattr("lcm").unwrap().call1(py_args).unwrap();
            let py_str: String = py_result.str().unwrap().extract().unwrap();
            let py = BigInt::from_str(&py_str).unwrap();
            assert_eq!(rs, py, "lcm({args:?}): py={py} vs rs={rs}");
        });
    }

    fn test_isqrt_impl(n: i64) {
        let rs = isqrt(&BigInt::from(n));
        crate::test::with_py_math(|_py, math| {
            let py_result = math.getattr("isqrt").unwrap().call1((n,));
            match py_result {
                Ok(result) => {
                    let py: i64 = result.extract().unwrap();
                    assert_eq!(rs, Ok(BigInt::from(py)), "isqrt({n}): py={py} vs rs={rs:?}");
                }
                Err(_) => {
                    assert!(rs.is_err(), "isqrt({n}): py raised error but rs={rs:?}");
                }
            }
        });
    }

    fn test_factorial_impl(n: i64) {
        use std::str::FromStr;
        let rs = factorial(n);
        crate::test::with_py_math(|_py, math| {
            let py_result = math.getattr("factorial").unwrap().call1((n,));
            match py_result {
                Ok(result) => {
                    let py_str: String = result.str().unwrap().extract().unwrap();
                    let py = BigUint::from_str(&py_str).unwrap();
                    assert_eq!(rs, Ok(py.clone()), "factorial({n}): py={py} vs rs={rs:?}");
                }
                Err(_) => {
                    assert!(rs.is_err(), "factorial({n}): py raised error but rs={rs:?}");
                }
            }
        });
    }

    fn test_comb_impl(n: i64, k: i64) {
        use std::str::FromStr;
        let rs = comb(n, k);
        crate::test::with_py_math(|_py, math| {
            let py_result = math.getattr("comb").unwrap().call1((n, k));
            match py_result {
                Ok(result) => {
                    let py_str: String = result.str().unwrap().extract().unwrap();
                    let py = BigUint::from_str(&py_str).unwrap();
                    assert_eq!(rs, Ok(py.clone()), "comb({n}, {k}): py={py} vs rs={rs:?}");
                }
                Err(_) => {
                    assert!(rs.is_err(), "comb({n}, {k}): py raised error but rs={rs:?}");
                }
            }
        });
    }

    fn test_perm_impl(n: i64, k: Option<i64>) {
        use std::str::FromStr;
        let rs = perm(n, k);
        crate::test::with_py_math(|_py, math| {
            let py_func = math.getattr("perm").unwrap();
            let py_result = match k {
                Some(k) => py_func.call1((n, k)),
                None => py_func.call1((n,)),
            };
            match py_result {
                Ok(result) => {
                    let py_str: String = result.str().unwrap().extract().unwrap();
                    let py = BigUint::from_str(&py_str).unwrap();
                    assert_eq!(rs, Ok(py.clone()), "perm({n}, {k:?}): py={py} vs rs={rs:?}");
                }
                Err(_) => {
                    assert!(
                        rs.is_err(),
                        "perm({n}, {k:?}): py raised error but rs={rs:?}"
                    );
                }
            }
        });
    }

    #[test]
    fn test_gcd() {
        // No arguments
        test_gcd_impl(&[]);
        // Single argument
        test_gcd_impl(&[5]);
        test_gcd_impl(&[-5]);
        test_gcd_impl(&[0]);
        // Two arguments
        test_gcd_impl(&[12, 8]);
        test_gcd_impl(&[8, 12]);
        test_gcd_impl(&[0, 5]);
        test_gcd_impl(&[5, 0]);
        test_gcd_impl(&[0, 0]);
        test_gcd_impl(&[-12, 8]);
        test_gcd_impl(&[12, -8]);
        test_gcd_impl(&[-12, -8]);
        test_gcd_impl(&[17, 13]);
        // Multiple arguments
        test_gcd_impl(&[12, 8, 4]);
        test_gcd_impl(&[12, 8, 6]);
        test_gcd_impl(&[100, 150, 200, 250]);
    }

    #[test]
    fn test_lcm() {
        // No arguments
        test_lcm_impl(&[]);
        // Single argument
        test_lcm_impl(&[5]);
        test_lcm_impl(&[-5]);
        // Two arguments
        test_lcm_impl(&[12, 8]);
        test_lcm_impl(&[8, 12]);
        test_lcm_impl(&[0, 5]);
        test_lcm_impl(&[5, 0]);
        test_lcm_impl(&[0, 0]);
        test_lcm_impl(&[-12, 8]);
        test_lcm_impl(&[12, -8]);
        test_lcm_impl(&[17, 13]);
        // Multiple arguments
        test_lcm_impl(&[2, 3, 4]);
        test_lcm_impl(&[6, 8, 12]);
    }

    #[test]
    fn test_isqrt() {
        test_isqrt_impl(0);
        test_isqrt_impl(1);
        test_isqrt_impl(4);
        test_isqrt_impl(9);
        test_isqrt_impl(10);
        test_isqrt_impl(15);
        test_isqrt_impl(16);
        test_isqrt_impl(100);
        test_isqrt_impl(1000000);
        test_isqrt_impl(-1);
        test_isqrt_impl(-100);
    }

    #[test]
    fn test_factorial() {
        test_factorial_impl(0);
        test_factorial_impl(1);
        test_factorial_impl(5);
        test_factorial_impl(10);
        test_factorial_impl(20);
        test_factorial_impl(50);
        test_factorial_impl(100);
        test_factorial_impl(-1);
        test_factorial_impl(-10);
    }

    #[test]
    fn test_comb() {
        test_comb_impl(5, 0);
        test_comb_impl(5, 5);
        test_comb_impl(5, 2);
        test_comb_impl(10, 3);
        test_comb_impl(3, 5); // k > n
        test_comb_impl(100, 30);
        test_comb_impl(100, 70);
        test_comb_impl(-1, 0);
        test_comb_impl(5, -1);
        test_comb_impl(-5, -3);
    }

    #[test]
    fn test_perm() {
        test_perm_impl(5, Some(0));
        test_perm_impl(5, Some(5));
        test_perm_impl(5, Some(2));
        test_perm_impl(5, None); // 5!
        test_perm_impl(3, Some(5)); // k > n
        test_perm_impl(10, Some(3));
        test_perm_impl(20, None);
        test_perm_impl(-1, None);
        test_perm_impl(5, Some(-1));
        test_perm_impl(-5, Some(-3));
    }

    // Edge case tests using EDGE_I64
    #[test]
    fn edgetest_gcd() {
        // Test all edge values - gcd handles arbitrary large integers
        for &a in EDGE_I64 {
            test_gcd_impl(&[a]);
            for &b in EDGE_I64 {
                test_gcd_impl(&[a, b]);
            }
        }
        // Additional large value tests
        test_gcd_impl(&[i64::MAX, i64::MAX - 1]);
        test_gcd_impl(&[i64::MIN, i64::MIN + 1]);
        test_gcd_impl(&[i64::MAX, i64::MIN]);
    }

    #[test]
    fn edgetest_lcm() {
        // Test all edge values - lcm handles arbitrary large integers
        for &a in EDGE_I64 {
            test_lcm_impl(&[a]);
            for &b in EDGE_I64 {
                test_lcm_impl(&[a, b]);
            }
        }
        // Additional boundary tests
        test_lcm_impl(&[i64::MAX, 1]);
        test_lcm_impl(&[i64::MIN, 1]);
        test_lcm_impl(&[i64::MIN, -1]);
    }

    #[test]
    fn edgetest_isqrt() {
        for &n in EDGE_I64 {
            test_isqrt_impl(n);
        }
        // Additional boundary cases
        test_isqrt_impl(i64::MAX);
        test_isqrt_impl((1i64 << 62) - 1);
        test_isqrt_impl(1i64 << 62);
        // Perfect squares
        for i in 0..20 {
            let sq = i * i;
            test_isqrt_impl(sq);
            if sq > 0 {
                test_isqrt_impl(sq - 1);
                test_isqrt_impl(sq + 1);
            }
        }
    }

    #[test]
    fn edgetest_factorial() {
        for &n in EDGE_I64 {
            // factorial only makes sense for reasonable n values
            if (-10..=170).contains(&n) {
                test_factorial_impl(n);
            }
        }
        // Boundary cases
        for n in 0..=25 {
            test_factorial_impl(n);
        }
        test_factorial_impl(50);
        test_factorial_impl(100);
        test_factorial_impl(170);
    }

    #[test]
    fn edgetest_comb() {
        let vals: Vec<i64> = EDGE_I64
            .iter()
            .copied()
            .filter(|&x| (-10..=200).contains(&x))
            .collect();
        for &n in &vals {
            for &k in &vals {
                test_comb_impl(n, k);
            }
        }
        // Large comb values
        test_comb_impl(1000, 3);
        test_comb_impl(1000, 500);
        test_comb_impl(500, 250);
        test_comb_impl(200, 100);
    }

    #[test]
    fn edgetest_perm() {
        let vals: Vec<i64> = EDGE_I64
            .iter()
            .copied()
            .filter(|&x| (-10..=100).contains(&x))
            .collect();
        for &n in &vals {
            for &k in &vals {
                test_perm_impl(n, Some(k));
            }
            test_perm_impl(n, None);
        }
        // Large perm values
        test_perm_impl(100, Some(5));
        test_perm_impl(1000, Some(3));
    }

    proptest::proptest! {
        #[test]
        fn proptest_gcd_2(a: i64, b: i64) {
            test_gcd_impl(&[a, b]);
        }

        #[test]
        fn proptest_gcd_3(a: i64, b: i64, c: i64) {
            test_gcd_impl(&[a, b, c]);
        }

        #[test]
        fn proptest_lcm_2(a in -100000i64..100000i64, b in -100000i64..100000i64) {
            test_lcm_impl(&[a, b]);
        }

        #[test]
        fn proptest_lcm_3(a in -1000i64..1000i64, b in -1000i64..1000i64, c in -1000i64..1000i64) {
            test_lcm_impl(&[a, b, c]);
        }

        #[test]
        fn proptest_isqrt_small(n in 0i64..1_000_000i64) {
            test_isqrt_impl(n);
        }

        #[test]
        fn proptest_isqrt_medium(n in 0i64..(1i64 << 32)) {
            test_isqrt_impl(n);
        }

        #[test]
        fn proptest_isqrt_large(n in 0i64..i64::MAX) {
            test_isqrt_impl(n);
        }

        #[test]
        fn proptest_isqrt_negative(n in i64::MIN..0i64) {
            test_isqrt_impl(n);
        }

        #[test]
        fn proptest_factorial(n in -10i64..50i64) {
            test_factorial_impl(n);
        }

        #[test]
        fn proptest_comb_small(n in -10i64..100i64, k in -10i64..100i64) {
            test_comb_impl(n, k);
        }

        #[test]
        fn proptest_comb_large(n in 100i64..1000i64, k in 0i64..50i64) {
            test_comb_impl(n, k);
        }

        #[test]
        fn proptest_perm_small(n in -10i64..50i64, k in -10i64..50i64) {
            test_perm_impl(n, Some(k));
        }

        #[test]
        fn proptest_perm_large(n in 50i64..200i64, k in 0i64..20i64) {
            test_perm_impl(n, Some(k));
        }

        #[test]
        fn proptest_perm_none(n in -10i64..25i64) {
            test_perm_impl(n, None);
        }
    }
}

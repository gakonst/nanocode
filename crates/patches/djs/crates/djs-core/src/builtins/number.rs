//! Number and Math leaf intrinsics: formatting and parsing through `crate::num`, the
//! libm functions (djs-libm, D1150, D3174), and `Math.random`'s generator.

use crate::heap::{Heap, StrView};
use crate::interp::conv::{Fault, to_int32};
use crate::interp::{ErrorKind, Intrinsic, realm};
use crate::num;
use crate::value::{Str, Value};

fn range_error<T>(e: num::RangeError) -> Result<T, Fault> {
    Err(Fault::error(ErrorKind::RangeError, e.message().into()))
}

/// ToIntegerOrInfinity results saturated to i32 (as `crate::num` wants them).
fn saturate(n: f64) -> i32 {
    n as i32
}

/// `NumberToString(x, radix)`, `NumberToFixed`, `NumberToExponential`,
/// `NumberToPrecision`: `op` 0..=3. `arg` is the (integral) radix, digit count or
/// precision, or undefined.
pub(crate) fn format(heap: &mut Heap, op: u8, x: f64, arg: Value) -> Result<Str, Fault> {
    let digits = arg.as_number().map(saturate);
    let r = match op {
        0 => num::to_string_radix(x, digits.unwrap_or(10)).map(|s| heap.alloc_latin1(s.as_bytes())),
        1 => num::to_fixed(x, digits.unwrap_or(0)).map(|s| heap.alloc_latin1(s.as_bytes())),
        2 => num::to_exponential(x, digits).map(|s| heap.alloc_latin1(s.as_bytes())),
        _ => num::to_precision(x, digits).map(|s| heap.alloc_latin1(s.as_bytes())),
    };
    match r {
        Ok(s) => Ok(s?),
        Err(e) => range_error(e),
    }
}

/// `parseFloat` (radix None) or `parseInt` of a string.
pub(crate) fn parse(heap: &mut Heap, s: Str, radix: Option<i32>) -> Result<f64, Fault> {
    let flat = heap.contiguous(s)?;
    let run = |units: &[u8]| match radix {
        None => num::parse_float(units),
        Some(r) => num::parse_int(units, r),
    };
    if let Some(StrView::Latin1(b)) = heap.str_view(flat) {
        return Ok(run(b));
    }
    let b = num::narrow_units(heap.str_view_flat(flat)?.units());
    Ok(run(&b))
}

/// ToUint32 of a number.
pub(crate) fn to_uint32(n: f64) -> u32 {
    to_int32(n) as u32
}

/// `Math.round`: QuickJS's bit-level rounding (half up, -0 and -0.5 give -0).
pub(crate) fn round(a: f64) -> f64 {
    let mut u = a.to_bits();
    let e = ((u >> 52) & 0x7ff) as u32;
    if e < 1023 {
        // |a| < 1
        if e == 1023 - 1 && u != 0xbfe0_0000_0000_0000 {
            // |a| > 0.5, or a = 0.5: ±1
            u = (u & (1 << 63)) | (1023u64 << 52);
        } else {
            u &= 1 << 63;
        }
    } else if e < 1023 + 52 {
        let s = u >> 63;
        let one = 1u64 << (52 - (e - 1023));
        u = u.wrapping_add((one >> 1) - s);
        u &= !(one - 1);
    }
    f64::from_bits(u)
}

/// `Math.sign`.
pub(crate) fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else if x < 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// The default seed of `Math.random` (any nonzero value; the embedder may set another
/// with `Vm::set_random_seed`).
pub const DEFAULT_RANDOM_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Stores a seed in the realm (zero is not a valid xorshift state), and keys the VM's
/// string hash from it (D2433): a host that seeds from entropy gets a secret key, one in
/// deterministic mode the same layout for the same seed.
pub(crate) fn set_seed(heap: &mut Heap, seed: u64) {
    let seed = set_math_state(heap, seed, false);
    heap.set_hash_key(crate::heap::string::hash_key_for_seed(seed));
}

/// Stores a seed for `Math.random` alone; the hash key stays (`Vm::set_math_random_seed`).
/// With `restore`, a dumped run's state (`$djs.seed`, `Vm::restore_math_random`): one
/// that ran, so that the next dump writes it again, its seed recorded as another value
/// (D2837). Beside the state the record holds the seed it is taken to start from:
/// `Math.random` ran where they differ (xorshift never returns to its seed in practice).
/// The state stored.
pub(crate) fn set_math_state(heap: &mut Heap, s: u64, restore: bool) -> u64 {
    let s = if s == 0 { DEFAULT_RANDOM_SEED } else { s };
    store_state(heap, s, 0);
    store_state(heap, if restore { !s } else { s }, 2);
    s
}

/// The `Math.random` state as its two int32 halves (low, high), for dumps (D2624), and
/// whether `Math.random` ran since the VM was seeded (D2837).
#[cfg(feature = "dump")]
pub(crate) fn random_state(heap: &Heap) -> (i32, i32, bool) {
    let a = realm::intrinsic(heap, Intrinsic::RandomState)
        .as_internal()
        .expect("random state");
    let half = |i| heap.value_array_get(a, i).as_int().unwrap_or(0);
    (half(0), half(1), (half(0), half(1)) != (half(2), half(3)))
}

fn store_state(heap: &mut Heap, s: u64, at: u32) {
    // Two int32 halves in an internal array: heap data, so it survives snapshots and is
    // the same in every copy.
    let a = realm::intrinsic(heap, Intrinsic::RandomState)
        .as_internal()
        .expect("random state");
    heap.value_array_set(a, at, Value::int(s as u32 as i32));
    heap.value_array_set(a, at + 1, Value::int((s >> 32) as u32 as i32));
}

/// The next `Math.random()` value: xorshift64* (as QuickJS), 52 random mantissa bits
/// in [1, 2), minus 1.
pub(crate) fn random(heap: &mut Heap) -> f64 {
    let a = realm::intrinsic(heap, Intrinsic::RandomState)
        .as_internal()
        .expect("random state");
    let lo = heap.value_array_get(a, 0).as_int().unwrap_or(0) as u32 as u64;
    let hi = heap.value_array_get(a, 1).as_int().unwrap_or(0) as u32 as u64;
    let mut x = lo | (hi << 32);
    if x == 0 {
        x = DEFAULT_RANDOM_SEED;
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    store_state(heap, x, 0);
    let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    f64::from_bits((0x3ffu64 << 52) | (v >> 12)) - 1.0
}

//! Utilities for working with hex float formats.

use super::{Round, Status, f64_from_bits};

/// Construct a 64-bit float from hex float representation (C-style)
pub const fn hf64(s: &str) -> f64 {
    match parse_hex_exact(s, 64, 52) {
        Ok(bits) => f64_from_bits(bits as u64),
        Err(HexFloatParseError(s)) => panic!("{}", s),
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HexFloatParseError(&'static str);

/// Parses any float to its bitwise representation, returning an error if it cannot be represented exactly
pub const fn parse_hex_exact(
    s: &str,
    bits: u32,
    sig_bits: u32,
) -> Result<u128, HexFloatParseError> {
    match parse_any(s, bits, sig_bits, Round::Nearest) {
        Err(e) => Err(e),
        Ok((bits, Status::OK)) => Ok(bits),
        Ok((_, status)) if status.overflow() => Err(HexFloatParseError("the value is too huge")),
        Ok((_, status)) if status.underflow() => Err(HexFloatParseError("the value is too tiny")),
        Ok((_, status)) if status.inexact() => Err(HexFloatParseError("the value is too precise")),
        Ok(_) => unreachable!(),
    }
}

/// Parse any float from hex to its bitwise representation.
pub const fn parse_any(
    s: &str,
    bits: u32,
    sig_bits: u32,
    round: Round,
) -> Result<(u128, Status), HexFloatParseError> {
    let mut b = s.as_bytes();

    if sig_bits > 119 || bits > 128 || bits < sig_bits + 3 || bits > sig_bits + 30 {
        return Err(HexFloatParseError("unsupported target float configuration"));
    }

    let neg = matches!(b, [b'-', ..]);
    if let &[b'-' | b'+', ref rest @ ..] = b {
        b = rest;
    }

    let sign_bit = 1 << (bits - 1);
    let quiet_bit = 1 << (sig_bits - 1);
    let nan = sign_bit - quiet_bit;
    let inf = nan - quiet_bit;

    let (mut x, status) = match *b {
        [b'i' | b'I', b'n' | b'N', b'f' | b'F'] => (inf, Status::OK),
        [b'n' | b'N', b'a' | b'A', b'n' | b'N'] => (nan, Status::OK),
        [b'0', b'x' | b'X', ref rest @ ..] => {
            let round = match (neg, round) {
                // parse("-x", Round::Positive) == -parse("x", Round::Negative)
                (true, Round::Positive) => Round::Negative,
                (true, Round::Negative) => Round::Positive,
                // rounding toward nearest or zero are symmetric
                (true, Round::Nearest | Round::Zero) | (false, _) => round,
            };
            match parse_finite(rest, bits, sig_bits, round) {
                Err(e) => return Err(e),
                Ok(res) => res,
            }
        }
        _ => return Err(HexFloatParseError("no hex indicator")),
    };

    if neg {
        x ^= sign_bit;
    }

    Ok((x, status))
}

const fn parse_finite(
    b: &[u8],
    bits: u32,
    sig_bits: u32,
    rounding_mode: Round,
) -> Result<(u128, Status), HexFloatParseError> {
    let exp_bits: u32 = bits - sig_bits - 1;
    let max_msb: i32 = (1 << (exp_bits - 1)) - 1;
    // The exponent of one ULP in the subnormals
    let min_lsb: i32 = 1 - max_msb - sig_bits as i32;

    let (mut sig, mut exp) = match parse_hex(b) {
        Err(e) => return Err(e),
        Ok(Parsed { sig: 0, .. }) => return Ok((0, Status::OK)),
        Ok(Parsed { sig, exp }) => (sig, exp),
    };

    let mut round_bits = u128_ilog2(sig) as i32 - sig_bits as i32;

    // Round at least up to min_lsb
    if exp < min_lsb - round_bits {
        round_bits = min_lsb - exp;
    }

    let mut status = Status::OK;

    exp += round_bits;

    if round_bits > 0 {
        // first, prepare for rounding exactly two bits
        if round_bits == 1 {
            sig <<= 1;
        } else if round_bits > 2 {
            sig = shr_odd_rounding(sig, (round_bits - 2) as u32);
        }

        if sig & 0b11 != 0 {
            status = Status::INEXACT;
        }

        sig = shr2_round(sig, rounding_mode);
    } else if round_bits < 0 {
        sig <<= -round_bits;
    }

    // The parsed value is X = sig * 2^exp
    // Expressed as a multiple U of the smallest subnormal value:
    // X = U * 2^min_lsb, so U = sig * 2^(exp-min_lsb)
    let uexp = (exp - min_lsb) as u128;
    let uexp = uexp << sig_bits;

    // Note that it is possible for the exponent bits to equal 2 here
    // if the value rounded up, but that means the mantissa is all zeroes
    // so the value is still correct
    debug_assert!(sig <= 2 << sig_bits);

    let inf = ((1 << exp_bits) - 1) << sig_bits;

    let bits = match sig.checked_add(uexp) {
        Some(bits) if bits < inf => {
            // inexact subnormal or zero?
            if status.inexact() && bits < (1 << sig_bits) {
                status = status.with(Status::UNDERFLOW);
            }
            bits
        }
        _ => {
            // overflow to infinity
            status = status.with(Status::OVERFLOW).with(Status::INEXACT);
            match rounding_mode {
                Round::Positive | Round::Nearest => inf,
                Round::Negative | Round::Zero => inf - 1,
            }
        }
    };
    Ok((bits, status))
}

/// Shift right, rounding all inexact divisions to the nearest odd number
/// E.g. (0 >> 4) -> 0, (1..=31 >> 4) -> 1, (32 >> 4) -> 2, ...
///
/// Useful for reducing a number before rounding the last two bits, since
/// the result of the final rounding is preserved for all rounding modes.
const fn shr_odd_rounding(x: u128, k: u32) -> u128 {
    if k < 128 {
        let inexact = x.trailing_zeros() < k;
        (x >> k) | (inexact as u128)
    } else {
        (x != 0) as u128
    }
}

/// Divide by 4, rounding with the given mode
const fn shr2_round(mut x: u128, round: Round) -> u128 {
    let t = (x as u32) & 0b111;
    x >>= 2;
    match round {
        // Look-up-table on the last three bits for when to round up
        Round::Nearest => x + ((0b11001000_u8 >> t) & 1) as u128,

        Round::Negative => x,
        Round::Zero => x,
        Round::Positive => x + (t & 0b11 != 0) as u128,
    }
}

/// A parsed finite and unsigned floating point number.
struct Parsed {
    /// Absolute value sig * 2^exp
    sig: u128,
    exp: i32,
}

/// Parse a hexadecimal float x
const fn parse_hex(mut b: &[u8]) -> Result<Parsed, HexFloatParseError> {
    let mut sig: u128 = 0;
    let mut exp: i32 = 0;

    let mut seen_point = false;
    let mut some_digits = false;
    let mut inexact = false;

    while let &[c, ref rest @ ..] = b {
        b = rest;

        match c {
            b'.' => {
                if seen_point {
                    return Err(HexFloatParseError(
                        "unexpected '.' parsing fractional digits",
                    ));
                }
                seen_point = true;
                continue;
            }
            b'p' | b'P' => break,
            c => {
                let digit = match hex_digit(c) {
                    Some(d) => d,
                    None => return Err(HexFloatParseError("expected hexadecimal digit")),
                };
                some_digits = true;

                if (sig >> 124) == 0 {
                    sig <<= 4;
                    sig |= digit as u128;
                } else {
                    // FIXME: it is technically possible for exp to overflow if parsing a string with >500M digits
                    exp += 4;
                    inexact |= digit != 0;
                }
                // Up until the fractional point, the value grows
                // with more digits, but after it the exponent is
                // compensated to match.
                if seen_point {
                    exp -= 4;
                }
            }
        }
    }
    // If we've set inexact, the exact value has more than 125
    // significant bits, and lies somewhere between sig and sig + 1.
    // Because we'll round off at least two of the trailing bits,
    // setting the last bit gives correct rounding for inexact values.
    sig |= inexact as u128;

    if !some_digits {
        return Err(HexFloatParseError("at least one digit is required"));
    };

    some_digits = false;

    let negate_exp = matches!(b, [b'-', ..]);
    if let &[b'-' | b'+', ref rest @ ..] = b {
        b = rest;
    }

    let mut pexp: u32 = 0;
    while let &[c, ref rest @ ..] = b {
        b = rest;
        let digit = match dec_digit(c) {
            Some(d) => d,
            None => return Err(HexFloatParseError("expected decimal digit")),
        };
        some_digits = true;
        pexp = pexp.saturating_mul(10);
        pexp += digit as u32;
    }

    if !some_digits {
        return Err(HexFloatParseError(
            "at least one exponent digit is required",
        ));
    };

    {
        let e;
        if negate_exp {
            e = (exp as i64) - (pexp as i64);
        } else {
            e = (exp as i64) + (pexp as i64);
        };

        exp = if e < i32::MIN as i64 {
            i32::MIN
        } else if e > i32::MAX as i64 {
            i32::MAX
        } else {
            e as i32
        };
    }
    /* FIXME(msrv): once MSRV >= 1.66, replace the above workaround block with:
    if negate_exp {
        exp = exp.saturating_sub_unsigned(pexp);
    } else {
        exp = exp.saturating_add_unsigned(pexp);
    };
    */

    Ok(Parsed { sig, exp })
}

const fn dec_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        _ => None,
    }
}

const fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/* FIXME(msrv): vendor some things that are not const stable at our MSRV */

/// `u128::ilog2`
const fn u128_ilog2(v: u128) -> u32 {
    assert!(v != 0);
    u128::BITS - 1 - v.leading_zeros()
}

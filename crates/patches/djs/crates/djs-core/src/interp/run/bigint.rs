//! BigInt operators and leaf intrinsics (0046, D1460-D1462). Kept apart from the main
//! dispatch: `exec` calls [`Exec::bigint_operands`] for the numeric operators, and the
//! intrinsic fallback chain continues in [`Exec::bigint_intrinsic`].
//!
//! Relational operators, `==`, `===`, `typeof`, ToBoolean and ToString handle BigInts in
//! `conv`, since they need no fuel or new errors.

use super::intrinsics::{bad_argument, id};
use super::*;
use crate::FULL;
use crate::bigint::{self, BigInt};

const TO_BIGINT_PRIMITIVE: u16 = id("ToBigIntPrimitive");
const NUMBER_TO_BIGINT: u16 = id("NumberToBigInt");
const IS_BIGINT: u16 = id("IsBigInt");
const BIGINT_TO_STRING: u16 = id("BigIntToString");
const BIGINT_AS_INT_N: u16 = id("BigIntAsIntN");
const BIGINT_AS_UINT_N: u16 = id("BigIntAsUintN");
const BIGINT_TO_NUMBER: u16 = id("BigIntToNumber");

/// The JS error for a failed BigInt operation (QuickJS's messages).
pub(crate) fn bigint_fault(e: bigint::Error) -> Fault {
    let msg = match e {
        bigint::Error::TooLarge => "BigInt is too large to allocate",
        bigint::Error::DivisionByZero => "BigInt division by zero",
        bigint::Error::NegativeExponent => "BigInt negative exponent",
        bigint::Error::ExponentTooLarge => "BigInt is too large",
    };
    Fault::error(ErrorKind::RangeError, String::from(msg))
}

impl Exec<'_> {
    /// A numeric operator with a BigInt operand: computes it and returns `Some(result)`
    /// (the caller advances `pc`). `None` when no operand is a BigInt, or for `+` with a
    /// string (concatenation). Object operands were converted by `primitive_operands`.
    #[inline]
    pub(super) fn bigint_operands(&mut self, op: Op) -> Option<Result<(), Fault>> {
        match op {
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Mod
            | Op::Pow
            | Op::Shl
            | Op::Sar
            | Op::Shr
            | Op::BAnd
            | Op::BOr
            | Op::BXor => {
                let (a, b) = (self.top(1), self.top(0));
                if a.as_bigint().is_none() && b.as_bigint().is_none()
                    || op == Op::Add && (a.as_string().is_some() || b.as_string().is_some())
                {
                    return None;
                }
                Some(self.bigint_binary(op, a, b))
            }
            Op::Neg | Op::BNot | Op::Inc | Op::Dec | Op::Plus | Op::ToNumeric => {
                let a = self.top(0);
                a.as_bigint()?;
                Some(self.bigint_unary(op, a))
            }
            _ => None,
        }
    }

    #[cold]
    fn bigint_binary(&mut self, op: Op, a: Value, b: Value) -> Result<(), Fault> {
        // ToNumeric of the other operand first (a symbol throws), then both must be
        // BigInts.
        for v in [a, b] {
            if v.as_bigint().is_none() {
                conv::to_number(self.heap, v)?;
            }
        }
        if op == Op::Shr {
            return type_error("bigint operands are forbidden for >>>");
        }
        if a.as_bigint().is_none() || b.as_bigint().is_none() {
            return type_error("cannot convert bigint to number");
        }
        let (x, y) = (self.heap.bigint(a), self.heap.bigint(b));
        let (n, m) = (x.mag.len(), y.mag.len());
        let r = match op {
            Op::Add => x.add(&y),
            Op::Sub => x.sub(&y),
            Op::Mul => {
                self.fuel -= bigint::mul_cost(n, m) as i64;
                x.mul(&y)
            }
            Op::Div | Op::Mod => {
                self.fuel -= bigint::mul_cost(n.saturating_sub(m) + 1, m) as i64;
                x.divrem(&y).map(|(q, r)| if op == Op::Div { q } else { r })
            }
            Op::Pow => x.pow(&y).inspect(|r| {
                // Squarings up to the result size cost about a third of its square.
                let k = r.mag.len();
                self.fuel -= (bigint::mul_cost(k, k) / 3) as i64;
            }),
            Op::Shl => x.shl(&y),
            Op::Sar => x.shr(&y),
            Op::BAnd => Ok(x.and(&y)),
            Op::BOr => Ok(x.or(&y)),
            Op::BXor => Ok(x.xor(&y)),
            _ => unreachable!("not a BigInt binary operator"),
        }
        .map_err(bigint_fault)?;
        self.fuel -= bigint::linear_cost(n.max(m).max(r.mag.len())) as i64;
        let v = self.heap.alloc_bigint(&r)?;
        self.sp -= 1;
        self.set(self.sp - 1, v);
        Ok(())
    }

    /// `a + d` for a BigInt `a` and `d` = ±1 (`inc_loc`, `dec_loc`).
    #[cold]
    pub(super) fn bigint_add_small(&mut self, a: Value, d: i32) -> Result<Value, Fault> {
        let x = self.heap.bigint(a);
        let one = BigInt::from_u64(1);
        let r = if d > 0 { x.add(&one) } else { x.sub(&one) }.map_err(bigint_fault)?;
        self.fuel -= bigint::linear_cost(r.mag.len()) as i64;
        Ok(self.heap.alloc_bigint(&r)?)
    }

    #[cold]
    fn bigint_unary(&mut self, op: Op, a: Value) -> Result<(), Fault> {
        let x = self.heap.bigint(a);
        let one = BigInt::from_u64(1);
        let r = match op {
            Op::ToNumeric => return Ok(()),
            Op::Plus => return type_error("bigint argument with unary +"),
            Op::Neg => Ok(x.neg()),
            Op::BNot => x.not(),
            Op::Inc => x.add(&one),
            Op::Dec => x.sub(&one),
            _ => unreachable!("not a BigInt unary operator"),
        }
        .map_err(bigint_fault)?;
        self.fuel -= bigint::linear_cost(r.mag.len()) as i64;
        let v = self.heap.alloc_bigint(&r)?;
        self.set(self.sp - 1, v);
        Ok(())
    }

    /// `call_intrinsic` for the BigInt leaves; falls back to `late_intrinsic`.
    pub(super) fn bigint_intrinsic(&mut self, id: u16, base: u32, argc: u32, next: u32) -> Step {
        let arg = |ex: &Self, i: u32| {
            if i < argc {
                ex.get(base + i)
            } else {
                Value::UNDEFINED
            }
        };
        let a0 = arg(self, 0);
        let v = match id {
            IS_BIGINT => Value::bool(a0.as_bigint().is_some()),
            TO_BIGINT_PRIMITIVE if FULL => self.bigint_of_primitive(a0)?,
            NUMBER_TO_BIGINT if FULL => {
                let Some(n) = a0.as_number() else {
                    return bad_argument(id);
                };
                let msg = if !n.is_finite() {
                    "cannot convert NaN or Infinity to BigInt"
                } else if djs_libm::trunc(n) != n {
                    "cannot convert to BigInt: not an integer"
                } else {
                    ""
                };
                if !msg.is_empty() {
                    return Err(Fault::error(ErrorKind::RangeError, String::from(msg)));
                }
                self.heap.alloc_bigint(&BigInt::from_f64(n))?
            }
            BIGINT_TO_STRING if FULL => {
                let (Some(_), Some(radix)) = (a0.as_bigint(), arg(self, 1).as_int()) else {
                    return bad_argument(id);
                };
                let x = self.heap.bigint(a0);
                self.fuel -= x.to_string_cost(radix as u32) as i64;
                let s = x.to_string_radix(radix as u32);
                Value::string(self.heap.alloc_latin1(s.as_bytes())?)
            }
            BIGINT_AS_INT_N | BIGINT_AS_UINT_N if FULL => {
                let (Some(bits), Some(_)) = (arg(self, 0).as_number(), arg(self, 1).as_bigint())
                else {
                    return bad_argument(id);
                };
                let x = self.heap.bigint(arg(self, 1));
                let bits = bits as u64;
                let r = if id == BIGINT_AS_INT_N {
                    x.as_int_n(bits)
                } else {
                    x.as_uint_n(bits)
                }
                .map_err(bigint_fault)?;
                self.fuel -= bigint::linear_cost(r.mag.len().max(x.mag.len())) as i64;
                self.heap.alloc_bigint(&r)?
            }
            BIGINT_TO_NUMBER => {
                if a0.as_bigint().is_none() {
                    return bad_argument(id);
                }
                conv::number(self.heap.bigint(a0).to_f64())
            }
            _ => return self.late_intrinsic(id, base, argc, next),
        };
        self.sp = base;
        self.push(v);
        self.pc = next;
        Ok(None)
    }

    /// ToBigInt (7.1.13) of a primitive.
    fn bigint_of_primitive(&mut self, v: Value) -> Result<Value, Fault> {
        let syntax = || {
            Err(Fault::error(
                ErrorKind::SyntaxError,
                String::from("invalid bigint literal"),
            ))
        };
        Ok(match v.kind() {
            Kind::BigInt => v,
            Kind::Bool => self.heap.alloc_bigint_i64(v.as_bool().unwrap() as i64)?,
            Kind::String => {
                let s = v.as_string().unwrap();
                let Some(b) = conv::string_to_bigint(self.heap, s)? else {
                    return syntax();
                };
                if b.bits() > bigint::MAX_BITS {
                    return Err(bigint_fault(bigint::Error::TooLarge));
                }
                self.fuel -= bigint::mul_cost(b.mag.len(), b.mag.len()) as i64 / 2;
                self.heap.alloc_bigint(&b)?
            }
            _ => return type_error("cannot convert to bigint"),
        })
    }
}

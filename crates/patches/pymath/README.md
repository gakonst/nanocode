pymath
======

**0 ULP (bit-exact) compatibility with CPython's math and cmath modules.**

Every function produces identical results to Python at the binary representation level - not just "close enough", but exactly the same bits.

## Overview

`pymath` is a strict port of CPython's math library to Rust. Each function has been carefully translated from CPython's C implementation, preserving the same algorithms, constants, and corner case handling.

## Compatibility Status

### math (56/56)

- [x] `acos`
- [x] `acosh`
- [x] `asin`
- [x] `asinh`
- [x] `atan`
- [x] `atan2`
- [x] `atanh`
- [x] `cbrt`
- [x] `ceil`
- [x] `copysign`
- [x] `cos`
- [x] `cosh`
- [x] `degrees`
- [x] `dist`
- [x] `e`
- [x] `erf`
- [x] `erfc`
- [x] `exp`
- [x] `exp2`
- [x] `expm1`
- [x] `fabs`
- [x] `floor`
- [x] `fma`
- [x] `fmod`
- [x] `frexp`
- [x] `fsum`
- [x] `gamma`
- [x] `hypot` (n-dimensional)
- [x] `inf`
- [x] `isclose`
- [x] `isfinite`
- [x] `isinf`
- [x] `isnan`
- [x] `ldexp`
- [x] `lgamma`
- [x] `log`
- [x] `log10`
- [x] `log1p`
- [x] `log2`
- [x] `modf`
- [x] `nan`
- [x] `nextafter`
- [x] `pi`
- [x] `pow`
- [x] `prod` (`prod`, `prod_int`)
- [x] `radians`
- [x] `remainder`
- [x] `sin`
- [x] `sinh`
- [x] `sqrt`
- [x] `sumprod` (`sumprod`, `sumprod_int`)
- [x] `tan`
- [x] `tanh`
- [x] `tau`
- [x] `trunc`
- [x] `ulp`

### math.integer (6/6, requires `num-bigint` or `malachite-bigint` feature)

- [x] `comb`
- [x] `factorial`
- [x] `gcd`
- [x] `isqrt`
- [x] `lcm`
- [x] `perm`

### cmath (31/31, requires `complex` feature)

- [x] `abs`
- [x] `acos`
- [x] `acosh`
- [x] `asin`
- [x] `asinh`
- [x] `atan`
- [x] `atanh`
- [x] `cos`
- [x] `cosh`
- [x] `e`
- [x] `exp`
- [x] `inf`
- [x] `infj`
- [x] `isclose`
- [x] `isfinite`
- [x] `isinf`
- [x] `isnan`
- [x] `log`
- [x] `log10`
- [x] `nan`
- [x] `nanj`
- [x] `phase`
- [x] `pi`
- [x] `polar`
- [x] `rect`
- [x] `sin`
- [x] `sinh`
- [x] `sqrt`
- [x] `tan`
- [x] `tanh`
- [x] `tau`

## Usage

```rust
use pymath::math::{sqrt, gamma, lgamma};

fn main() {
    let sqrt_result = sqrt(2.0).unwrap();
    let gamma_result = gamma(4.5).unwrap();
    let lgamma_result = lgamma(4.5).unwrap();
}
```

### Bit-exact verification

```python
# Python 3.14
>>> import math
>>> math.gamma(0.5).hex()
'0x1.c5bf891b4ef6ap+0'
```

```rust
// Rust - identical bits
assert_eq!(
    pymath::math::gamma(0.5).unwrap().to_bits(),
    0x3ffc5bf891b4ef6a
);
```

*Bit representation may vary across platforms, but CPython and pymath built on the same environment will always produce identical results.*

## Features

- `complex` (default) - Enable cmath module for complex number functions
- `num-bigint` - Enable integer functions using num-bigint
- `malachite-bigint` - Enable integer functions using malachite-bigint
- `mul_add` - Use hardware FMA for bit-exact macOS compatibility

## Module Structure

- `pymath::math` - Real number math functions (Python's `math` module)
- `pymath::cmath` - Complex number functions (Python's `cmath` module)
- `pymath::m` - Direct libm bindings

## Important Note

This library guarantees **compatibility with Python's math module**, not mathematical correctness.

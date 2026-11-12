use crate::field::{self, BYTES, Limbs, Modulus};

const WINDOW_BITS: u32 = 4;
const WINDOW_ENTRIES: usize = 1 << WINDOW_BITS;
const WINDOW_MASK: u8 = (WINDOW_ENTRIES - 1) as u8;
const WINDOWS_PER_BYTE: u32 = u8::BITS / WINDOW_BITS;
const SECP256K1_B: u64 = 7;
const P256_A_NEGATED: u64 = 3;

pub(crate) struct Curve {
    pub(crate) field: Modulus,
    pub(crate) order: Modulus,
    a: Limbs,
    b: Limbs,
    b3: Limbs,
    generator: Point,
}

#[derive(Clone, Copy)]
pub(crate) struct Point {
    x: Limbs,
    y: Limbs,
    z: Limbs,
}

pub(crate) struct Affine {
    pub(crate) x: [u8; BYTES],
    pub(crate) y: [u8; BYTES],
    pub(crate) y_odd: bool,
}

pub(crate) const SECP256K1: Curve = {
    let field = Modulus::new(field::from_hex(
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F",
    ));
    let order = Modulus::new(field::from_hex(
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
    ));
    let b = field.small(SECP256K1_B);
    Curve {
        field,
        order,
        a: [0; field::LIMBS],
        b,
        b3: field.add(&field.add(&b, &b), &b),
        generator: Point {
            x: field.encode(&field::from_hex(
                "79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798",
            )),
            y: field.encode(&field::from_hex(
                "483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8",
            )),
            z: field.one,
        },
    }
};

pub(crate) const P256: Curve = {
    let field = Modulus::new(field::from_hex(
        "FFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF",
    ));
    let order = Modulus::new(field::from_hex(
        "FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551",
    ));
    let b = field.encode(&field::from_hex(
        "5AC635D8AA3A93E7B3EBBD55769886BC651D06B0CC53B0F63BCE3C3E27D2604B",
    ));
    Curve {
        field,
        order,
        a: field.negate(&field.small(P256_A_NEGATED)),
        b,
        b3: field.add(&field.add(&b, &b), &b),
        generator: Point {
            x: field.encode(&field::from_hex(
                "6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296",
            )),
            y: field.encode(&field::from_hex(
                "4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5",
            )),
            z: field.one,
        },
    }
};

impl Curve {
    const fn identity(&self) -> Point {
        Point {
            x: [0; field::LIMBS],
            y: self.field.one,
            z: [0; field::LIMBS],
        }
    }

    pub(crate) const fn add(&self, p: &Point, q: &Point) -> Point {
        let f = &self.field;
        let mut t0 = f.mul(&p.x, &q.x);
        let mut t1 = f.mul(&p.y, &q.y);
        let mut t2 = f.mul(&p.z, &q.z);
        let mut t3 = f.add(&p.x, &p.y);
        let mut t4 = f.add(&q.x, &q.y);
        t3 = f.mul(&t3, &t4);
        t4 = f.add(&t0, &t1);
        t3 = f.sub(&t3, &t4);
        t4 = f.add(&p.x, &p.z);
        let mut t5 = f.add(&q.x, &q.z);
        t4 = f.mul(&t4, &t5);
        t5 = f.add(&t0, &t2);
        t4 = f.sub(&t4, &t5);
        t5 = f.add(&p.y, &p.z);
        let mut x3 = f.add(&q.y, &q.z);
        t5 = f.mul(&t5, &x3);
        x3 = f.add(&t1, &t2);
        t5 = f.sub(&t5, &x3);
        let mut z3 = f.mul(&self.a, &t4);
        x3 = f.mul(&self.b3, &t2);
        z3 = f.add(&x3, &z3);
        x3 = f.sub(&t1, &z3);
        z3 = f.add(&t1, &z3);
        let mut y3 = f.mul(&x3, &z3);
        t1 = f.add(&t0, &t0);
        t1 = f.add(&t1, &t0);
        t2 = f.mul(&self.a, &t2);
        t4 = f.mul(&self.b3, &t4);
        t1 = f.add(&t1, &t2);
        t2 = f.sub(&t0, &t2);
        t2 = f.mul(&self.a, &t2);
        t4 = f.add(&t4, &t2);
        t0 = f.mul(&t1, &t4);
        y3 = f.add(&y3, &t0);
        t0 = f.mul(&t5, &t4);
        x3 = f.mul(&t3, &x3);
        x3 = f.sub(&x3, &t0);
        t0 = f.mul(&t3, &t1);
        z3 = f.mul(&t5, &z3);
        z3 = f.add(&z3, &t0);
        Point {
            x: x3,
            y: y3,
            z: z3,
        }
    }

    const fn select(when_one: &Point, when_zero: &Point, choice: u64) -> Point {
        Point {
            x: field::select(&when_one.x, &when_zero.x, choice),
            y: field::select(&when_one.y, &when_zero.y, choice),
            z: field::select(&when_one.z, &when_zero.z, choice),
        }
    }

    pub(crate) fn multiply(&self, point: &Point, scalar: &[u8; BYTES]) -> Point {
        let mut table = [self.identity(); WINDOW_ENTRIES];
        table[1] = *point;
        for entry in 2..WINDOW_ENTRIES {
            table[entry] = self.add(&table[entry - 1], point);
        }
        let mut accumulator = self.identity();
        for byte in scalar {
            for window in (0..WINDOWS_PER_BYTE).rev() {
                for _ in 0..WINDOW_BITS {
                    accumulator = self.add(&accumulator, &accumulator);
                }
                let index = (byte >> (window * WINDOW_BITS)) & WINDOW_MASK;
                let mut selected = self.identity();
                for (entry, candidate) in table.iter().enumerate() {
                    let hit = field::equal(&[entry as u64, 0, 0, 0], &[index as u64, 0, 0, 0]);
                    selected = Self::select(candidate, &selected, hit);
                }
                accumulator = self.add(&accumulator, &selected);
            }
        }
        accumulator
    }

    pub(crate) fn multiply_generator(&self, scalar: &[u8; BYTES]) -> Point {
        self.multiply(&self.generator, scalar)
    }

    pub(crate) const fn is_identity(&self, point: &Point) -> bool {
        field::is_zero(&point.z) == 1
    }

    pub(crate) const fn to_affine(&self, point: &Point) -> Option<Affine> {
        if self.is_identity(point) {
            return None;
        }
        let f = &self.field;
        let inverse = f.invert(&point.z);
        let x = field::to_be_bytes(&f.decode(&f.mul(&point.x, &inverse)));
        let y = f.decode(&f.mul(&point.y, &inverse));
        Some(Affine {
            x,
            y: field::to_be_bytes(&y),
            y_odd: y[0] & 1 == 1,
        })
    }

    const fn right_side(&self, x: &Limbs) -> Limbs {
        let f = &self.field;
        let cube = f.mul(&f.square(x), x);
        f.add(&f.add(&cube, &f.mul(&self.a, x)), &self.b)
    }

    pub(crate) const fn lift(&self, x: &[u8; BYTES], y: &[u8; BYTES]) -> Option<Point> {
        let f = &self.field;
        let (x, y) = (field::from_be_bytes(x), field::from_be_bytes(y));
        if f.is_canonical(&x) == 0 || f.is_canonical(&y) == 0 {
            return None;
        }
        let (x, y) = (f.encode(&x), f.encode(&y));
        if field::equal(&f.square(&y), &self.right_side(&x)) == 0 {
            return None;
        }
        Some(Point { x, y, z: f.one })
    }

    pub(crate) const fn decompress(&self, x: &[u8; BYTES], y_odd: bool) -> Option<Point> {
        let f = &self.field;
        let x = field::from_be_bytes(x);
        if f.is_canonical(&x) == 0 {
            return None;
        }
        let x = f.encode(&x);
        let square = self.right_side(&x);
        let exponent = field::add_carry(&f.value, &[1, 0, 0, 0]).0;
        let quarter = [
            (exponent[0] >> 2) | (exponent[1] << 62),
            (exponent[1] >> 2) | (exponent[2] << 62),
            (exponent[2] >> 2) | (exponent[3] << 62),
            exponent[3] >> 2,
        ];
        let root = f.pow(&square, &quarter);
        if field::equal(&f.square(&root), &square) == 0 {
            return None;
        }
        let odd = f.decode(&root)[0] & 1 == 1;
        let y = if odd == y_odd { root } else { f.negate(&root) };
        Some(Point { x, y, z: f.one })
    }
}

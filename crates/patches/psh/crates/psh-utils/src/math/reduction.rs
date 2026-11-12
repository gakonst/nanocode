use super::rounding::scalbn;
use super::{
    EXPONENT_BIAS, HIGH_MAGNITUDE_MASK, HIGH_SIGNIFICAND_BITS, HIGH_SIGNIFICAND_MASK,
    NON_FINITE_HIGH_WORD, SIGNIFICAND_BITS, SIGNIFICAND_MASK, biased_exponent, high_word,
    power_of_two,
};

pub(super) const PI_OVER_4_HIGH_WORD: u32 = 0x3fe9_21fb;
const PI_OVER_2_SIGNIFICAND_HIGH: u32 = 0x9_21fb;
const THREE_PI_OVER_4_HIGH_WORD: u32 = 0x4002_d97c;
const FIVE_PI_OVER_4_HIGH_WORD: u32 = 0x400f_6a7a;
const THREE_PI_OVER_2_HIGH_WORD: u32 = 0x4012_d97c;
const SEVEN_PI_OVER_4_HIGH_WORD: u32 = 0x4015_fdbc;
const TWO_PI_HIGH_WORD: u32 = 0x4019_21fb;
const NINE_PI_OVER_4_HIGH_WORD: u32 = 0x401c_463b;
const MEDIUM_LIMIT_HIGH_WORD: u32 = 0x4139_21fb;
const TO_INTEGER: f64 = 1.5 / f64::EPSILON;
const PI_OVER_2_FIRST: f64 = 1.5707963267341256;
const PI_OVER_2_FIRST_TAIL: f64 = 6.077100506506192e-11;
const PI_OVER_2_SECOND: f64 = 6.077100506303966e-11;
const PI_OVER_2_SECOND_TAIL: f64 = 2.0222662487959506e-21;
const PI_OVER_2_THIRD: f64 = 2.0222662487111665e-21;
const PI_OVER_2_THIRD_TAIL: f64 = 8.4784276603689e-32;
const SECOND_ROUND_LOST_BITS: i32 = 16;
const THIRD_ROUND_LOST_BITS: i32 = 49;
const CHUNK_BITS: i32 = 24;
const CHUNK_SCALE: f64 = power_of_two(CHUNK_BITS);
const CHUNK_SCALE_INVERSE: f64 = power_of_two(-CHUNK_BITS);
const CHUNK_MAX: i32 = (1 << CHUNK_BITS) - 1;
const INPUT_CHUNKS: usize = 3;
const TERMS_BEYOND_INPUT: usize = 4;
const WORK_TERMS: usize = 20;
const TWO_OVER_PI_CHUNKS: [i32; 66] = [
    0xa2f983, 0x6e4e44, 0x1529fc, 0x2757d1, 0xf534dd, 0xc0db62, 0x95993c, 0x439041, 0xfe5163,
    0xabdebb, 0xc561b7, 0x246e3a, 0x424dd2, 0xe00649, 0x2eea09, 0xd1921c, 0xfe1deb, 0x1cb129,
    0xa73ee8, 0x8235f5, 0x2ebb44, 0x84e99c, 0x7026b4, 0x5f7e41, 0x3991d6, 0x398353, 0x39f49c,
    0x845f8b, 0xbdf928, 0x3b1ff8, 0x97ffde, 0x05980f, 0xef2f11, 0x8b5a0a, 0x6d1f6d, 0x367ecf,
    0x27cb09, 0xb74f46, 0x3f669e, 0x5fea2d, 0x7527ba, 0xc7ebe5, 0xf17b3d, 0x0739f7, 0x8a5292,
    0xea6bfb, 0x5fb11f, 0x8d5d08, 0x560330, 0x46fc7b, 0x6babf0, 0xcfbc20, 0x9af436, 0x1da9e3,
    0x91615e, 0xe61b08, 0x659985, 0x5f14a0, 0x68408d, 0xffd880, 0x4d7327, 0x310606, 0x1556ca,
    0x73a8c9, 0x60e27b, 0xc08c6b,
];
const PI_OVER_2_CHUNKS: [f64; 8] = [
    1.570796251296997,
    7.549789415861596e-08,
    5.390302529957765e-15,
    3.282003415807913e-22,
    1.270655753080676e-29,
    1.2293330898111133e-36,
    2.7337005381646456e-44,
    2.1674168387780482e-51,
];

pub(super) struct Reduced {
    pub quadrant: i32,
    pub high: f64,
    pub low: f64,
}

fn subtract_multiple(x: f64, quadrant: i32) -> Reduced {
    let multiple = f64::from(quadrant);
    let z = x - multiple * PI_OVER_2_FIRST;
    let high = z - multiple * PI_OVER_2_FIRST_TAIL;
    let low = (z - high) - multiple * PI_OVER_2_FIRST_TAIL;
    Reduced {
        quadrant,
        high,
        low,
    }
}

fn refine(r: f64, f_n: f64, part: f64, tail: f64) -> (f64, f64, f64) {
    let w_part = f_n * part;
    let reduced = r - w_part;
    let w = f_n * tail - ((r - reduced) - w_part);
    (reduced, w, reduced - w)
}

fn medium(x: f64, magnitude: u32) -> Reduced {
    let f_n = x * std::f64::consts::FRAC_2_PI + TO_INTEGER - TO_INTEGER;
    let quadrant = f_n as i32;
    let mut r = x - f_n * PI_OVER_2_FIRST;
    let mut w = f_n * PI_OVER_2_FIRST_TAIL;
    let mut high = r - w;
    let x_exponent = (magnitude >> HIGH_SIGNIFICAND_BITS) as i32;
    if x_exponent - biased_exponent(high) as i32 > SECOND_ROUND_LOST_BITS {
        (r, w, high) = refine(r, f_n, PI_OVER_2_SECOND, PI_OVER_2_SECOND_TAIL);
        if x_exponent - biased_exponent(high) as i32 > THIRD_ROUND_LOST_BITS {
            (r, w, high) = refine(r, f_n, PI_OVER_2_THIRD, PI_OVER_2_THIRD_TAIL);
        }
    }
    Reduced {
        quadrant,
        high,
        low: (r - high) - w,
    }
}

pub(super) fn reduce(x: f64) -> Reduced {
    let negative = x.is_sign_negative();
    let magnitude = high_word(x) & HIGH_MAGNITUDE_MASK;
    let sign = if negative { -1 } else { 1 };
    if magnitude <= FIVE_PI_OVER_4_HIGH_WORD {
        if magnitude & HIGH_SIGNIFICAND_MASK == PI_OVER_2_SIGNIFICAND_HIGH {
            return medium(x, magnitude);
        }
        if magnitude <= THREE_PI_OVER_4_HIGH_WORD {
            return subtract_multiple(x, sign);
        }
        return subtract_multiple(x, 2 * sign);
    }
    if magnitude <= NINE_PI_OVER_4_HIGH_WORD {
        if magnitude <= SEVEN_PI_OVER_4_HIGH_WORD {
            if magnitude == THREE_PI_OVER_2_HIGH_WORD {
                return medium(x, magnitude);
            }
            return subtract_multiple(x, 3 * sign);
        }
        if magnitude == TWO_PI_HIGH_WORD {
            return medium(x, magnitude);
        }
        return subtract_multiple(x, 4 * sign);
    }
    if magnitude < MEDIUM_LIMIT_HIGH_WORD {
        return medium(x, magnitude);
    }
    if magnitude >= NON_FINITE_HIGH_WORD {
        let nan = super::difference(x, x);
        return Reduced {
            quadrant: 0,
            high: nan,
            low: nan,
        };
    }
    let reduced = reduce_large(x, magnitude);
    if negative {
        return Reduced {
            quadrant: -reduced.quadrant,
            high: -reduced.high,
            low: -reduced.low,
        };
    }
    reduced
}

struct LargeReduction {
    inputs: [f64; INPUT_CHUNKS],
    input_count: usize,
    first_chunk: usize,
    factors: [f64; WORK_TERMS],
    products: [f64; WORK_TERMS],
    chunks: [i32; WORK_TERMS],
    top: usize,
    exponent: i32,
}

impl LargeReduction {
    fn product(&self, index: usize) -> f64 {
        let last = self.input_count - 1;
        let mut sum = 0.0;
        for j in 0..=last {
            sum += self.inputs[j] * self.factors[last + index - j];
        }
        sum
    }

    fn distill(&mut self) -> f64 {
        let mut z = self.products[self.top];
        for (i, j) in (1..=self.top).rev().enumerate() {
            let high = f64::from((CHUNK_SCALE_INVERSE * z) as i32);
            self.chunks[i] = (z - CHUNK_SCALE * high) as i32;
            z = self.products[j - 1] + high;
        }
        z
    }

    fn complement(&mut self) -> bool {
        let mut carry = false;
        for i in 0..self.top {
            let chunk = self.chunks[i];
            if carry {
                self.chunks[i] = CHUNK_MAX - chunk;
            } else if chunk != 0 {
                carry = true;
                self.chunks[i] = CHUNK_MAX + 1 - chunk;
            }
        }
        if self.exponent > 0 && self.exponent <= 2 {
            self.chunks[self.top - 1] &= CHUNK_MAX >> self.exponent;
        }
        carry
    }

    fn needs_more_terms(&self) -> Option<usize> {
        let zero = (TERMS_BEYOND_INPUT..self.top).all(|i| self.chunks[i] == 0);
        if !zero {
            return None;
        }
        let mut more = 1;
        while self.chunks[TERMS_BEYOND_INPUT - more] == 0 {
            more += 1;
        }
        Some(more)
    }

    fn add_terms(&mut self, more: usize) {
        let last = self.input_count - 1;
        for i in self.top + 1..=self.top + more {
            self.factors[last + i] = f64::from(TWO_OVER_PI_CHUNKS[self.first_chunk + i]);
            self.products[i] = self.product(i);
        }
        self.top += more;
    }

    fn quotient(&mut self) -> (i32, i32, f64) {
        loop {
            let mut z = scalbn(self.distill(), self.exponent);
            z -= 8.0 * (z * 0.125).floor();
            let mut n = z as i32;
            z -= f64::from(n);
            let mut half = 0;
            let exponent = self.exponent;
            if exponent > 0 {
                let carried = self.chunks[self.top - 1] >> (CHUNK_BITS - exponent);
                n += carried;
                self.chunks[self.top - 1] -= carried << (CHUNK_BITS - exponent);
                half = self.chunks[self.top - 1] >> (CHUNK_BITS - 1 - exponent);
            } else if exponent == 0 {
                half = self.chunks[self.top - 1] >> (CHUNK_BITS - 1);
            } else if z >= 0.5 {
                half = 2;
            }
            if half > 0 {
                n += 1;
                let carry = self.complement();
                if half == 2 {
                    z = 1.0 - z;
                    if carry {
                        z -= scalbn(1.0, exponent);
                    }
                }
            }
            if z == 0.0
                && let Some(more) = self.needs_more_terms()
            {
                self.add_terms(more);
                continue;
            }
            return (n, half, z);
        }
    }

    fn finish(&mut self, z: f64) -> [f64; WORK_TERMS] {
        if z == 0.0 {
            self.top -= 1;
            self.exponent -= CHUNK_BITS;
            while self.chunks[self.top] == 0 {
                self.top -= 1;
                self.exponent -= CHUNK_BITS;
            }
        } else {
            let z = scalbn(z, -self.exponent);
            if z >= CHUNK_SCALE {
                let high = f64::from((CHUNK_SCALE_INVERSE * z) as i32);
                self.chunks[self.top] = (z - CHUNK_SCALE * high) as i32;
                self.top += 1;
                self.exponent += CHUNK_BITS;
                self.chunks[self.top] = high as i32;
            } else {
                self.chunks[self.top] = z as i32;
            }
        }
        let mut scale = scalbn(1.0, self.exponent);
        let mut values = [0.0; WORK_TERMS];
        for i in (0..=self.top).rev() {
            values[i] = scale * f64::from(self.chunks[i]);
            scale *= CHUNK_SCALE_INVERSE;
        }
        let mut sums = [0.0; WORK_TERMS];
        for i in (0..=self.top).rev() {
            let mut sum = 0.0;
            let mut k = 0;
            while k <= TERMS_BEYOND_INPUT && k <= self.top - i {
                sum += PI_OVER_2_CHUNKS[k] * values[i + k];
                k += 1;
            }
            sums[self.top - i] = sum;
        }
        sums
    }
}

fn reduce_large(x: f64, magnitude: u32) -> Reduced {
    let scaled_exponent = EXPONENT_BIAS + CHUNK_BITS - 1;
    let bits = x.to_bits() & SIGNIFICAND_MASK | (scaled_exponent as u64) << SIGNIFICAND_BITS;
    let mut z = f64::from_bits(bits);
    let mut inputs = [0.0; INPUT_CHUNKS];
    for input in inputs.iter_mut().take(INPUT_CHUNKS - 1) {
        *input = f64::from(z as i32);
        z = (z - *input) * CHUNK_SCALE;
    }
    inputs[INPUT_CHUNKS - 1] = z;
    let mut last = INPUT_CHUNKS - 1;
    while last != 0 && inputs[last] == 0.0 {
        last -= 1;
    }
    let exponent = (magnitude >> HIGH_SIGNIFICAND_BITS) as i32 - scaled_exponent;
    let first_chunk = ((exponent - 3) / CHUNK_BITS).max(0);
    let mut state = LargeReduction {
        inputs,
        input_count: last + 1,
        first_chunk: first_chunk as usize,
        factors: [0.0; WORK_TERMS],
        products: [0.0; WORK_TERMS],
        chunks: [0; WORK_TERMS],
        top: TERMS_BEYOND_INPUT,
        exponent: exponent - CHUNK_BITS * (first_chunk + 1),
    };
    let factors = state.factors.iter_mut().take(last + TERMS_BEYOND_INPUT + 1);
    for (index, factor) in (first_chunk - last as i32..).zip(factors) {
        *factor = if index < 0 {
            0.0
        } else {
            f64::from(TWO_OVER_PI_CHUNKS[index as usize])
        };
    }
    for i in 0..=TERMS_BEYOND_INPUT {
        state.products[i] = state.product(i);
    }
    let (n, half, z) = state.quotient();
    let sums = state.finish(z);
    let mut high = 0.0;
    for i in (0..=state.top).rev() {
        high += sums[i];
    }
    let mut low = sums[0] - high;
    for sum in sums.iter().take(state.top + 1).skip(1) {
        low += sum;
    }
    let (high, low) = if half == 0 {
        (high, low)
    } else {
        (-high, -low)
    };
    Reduced {
        quadrant: n & 7,
        high,
        low,
    }
}

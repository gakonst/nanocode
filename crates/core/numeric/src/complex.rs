use core::fmt;
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(PartialEq, Copy, Clone, Debug, Default)]
pub struct Complex<T> {
    pub re: T,
    pub im: T,
}

pub type Complex64 = Complex<f64>;

impl<T> Complex<T> {
    pub const fn new(re: T, im: T) -> Self {
        Self { re, im }
    }
}

impl Complex<f64> {
    pub fn norm_sqr(&self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn conj(&self) -> Self {
        Self::new(self.re, -self.im)
    }

    pub fn inv(&self) -> Self {
        let norm_sqr = self.norm_sqr();
        Self::new(self.re / norm_sqr, -self.im / norm_sqr)
    }

    pub fn scale(&self, factor: f64) -> Self {
        Self::new(self.re * factor, self.im * factor)
    }

    pub fn unscale(&self, divisor: f64) -> Self {
        Self::new(self.re / divisor, self.im / divisor)
    }

    pub fn norm(self) -> f64 {
        self.re.hypot(self.im)
    }

    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    pub fn to_polar(self) -> (f64, f64) {
        (self.norm(), self.arg())
    }

    pub fn from_polar(radius: f64, angle: f64) -> Self {
        Self::new(radius * angle.cos(), radius * angle.sin())
    }

    pub fn exp(self) -> Self {
        let Self { re, mut im } = self;
        if re.is_infinite() {
            if re < 0.0 {
                if !im.is_finite() {
                    return Self::new(0.0, 0.0);
                }
            } else if im == 0.0 || !im.is_finite() {
                if im.is_infinite() {
                    im = f64::NAN;
                }
                return Self::new(re, im);
            }
        } else if re.is_nan() && im == 0.0 {
            return self;
        }
        Self::from_polar(re.exp(), im)
    }

    pub fn ln(self) -> Self {
        let (radius, angle) = self.to_polar();
        Self::new(radius.ln(), angle)
    }

    pub fn powf(self, exponent: f64) -> Self {
        if exponent == 0.0 {
            return Self::new(1.0, 0.0);
        }
        let (radius, angle) = self.to_polar();
        Self::from_polar(radius.powf(exponent), angle * exponent)
    }

    pub fn powc(self, exponent: Self) -> Self {
        if exponent.re == 0.0 && exponent.im == 0.0 {
            return Self::new(1.0, 0.0);
        }
        (exponent * self.ln()).exp()
    }

    pub fn powi(&self, exponent: i32) -> Self {
        if exponent < 0 {
            self.inv().powu(exponent.wrapping_neg() as u32)
        } else {
            self.powu(exponent as u32)
        }
    }

    pub fn powu(&self, exponent: u32) -> Self {
        let mut exponent = exponent;
        if exponent == 0 {
            return Self::new(1.0, 0.0);
        }
        let mut base = *self;
        while exponent & 1 == 0 {
            base = base * base;
            exponent >>= 1;
        }
        if exponent == 1 {
            return base;
        }
        let mut accumulator = base;
        while exponent > 1 {
            exponent >>= 1;
            base = base * base;
            if exponent & 1 == 1 {
                accumulator *= base;
            }
        }
        accumulator
    }

    pub const fn is_nan(self) -> bool {
        self.re.is_nan() || self.im.is_nan()
    }

    pub const fn is_infinite(self) -> bool {
        !self.is_nan() && (self.re.is_infinite() || self.im.is_infinite())
    }

    pub const fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

impl From<f64> for Complex<f64> {
    fn from(re: f64) -> Self {
        Self::new(re, 0.0)
    }
}

impl Add for Complex<f64> {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }
}

impl Sub for Complex<f64> {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }
}

impl Mul for Complex<f64> {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        let re = self.re * other.re - self.im * other.im;
        let im = self.re * other.im + self.im * other.re;
        Self::new(re, im)
    }
}

impl Div for Complex<f64> {
    type Output = Self;

    fn div(self, other: Self) -> Self {
        let norm_sqr = other.norm_sqr();
        let re = self.re * other.re + self.im * other.im;
        let im = self.im * other.re - self.re * other.im;
        Self::new(re / norm_sqr, im / norm_sqr)
    }
}

impl Neg for Complex<f64> {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}

impl Neg for &Complex<f64> {
    type Output = Complex<f64>;

    fn neg(self) -> Complex<f64> {
        Complex::new(-self.re, -self.im)
    }
}

impl Add<f64> for Complex<f64> {
    type Output = Self;

    fn add(self, other: f64) -> Self {
        Self::new(self.re + other, self.im)
    }
}

impl Sub<f64> for Complex<f64> {
    type Output = Self;

    fn sub(self, other: f64) -> Self {
        Self::new(self.re - other, self.im)
    }
}

impl Mul<f64> for Complex<f64> {
    type Output = Self;

    fn mul(self, other: f64) -> Self {
        Self::new(self.re * other, self.im * other)
    }
}

impl Div<f64> for Complex<f64> {
    type Output = Self;

    fn div(self, other: f64) -> Self {
        Self::new(self.re / other, self.im / other)
    }
}

impl Add<Complex<Self>> for f64 {
    type Output = Complex<Self>;

    fn add(self, other: Complex<Self>) -> Complex<Self> {
        Complex::new(self + other.re, other.im)
    }
}

impl Sub<Complex<Self>> for f64 {
    type Output = Complex<Self>;

    fn sub(self, other: Complex<Self>) -> Complex<Self> {
        Complex::new(self - other.re, 0.0 - other.im)
    }
}

impl Mul<Complex<Self>> for f64 {
    type Output = Complex<Self>;

    fn mul(self, other: Complex<Self>) -> Complex<Self> {
        Complex::new(self * other.re, self * other.im)
    }
}

impl Div<Complex<Self>> for f64 {
    type Output = Complex<Self>;

    fn div(self, other: Complex<Self>) -> Complex<Self> {
        let norm_sqr = other.norm_sqr();
        Complex::new(self * other.re / norm_sqr, 0.0 - self * other.im / norm_sqr)
    }
}

impl AddAssign for Complex<f64> {
    fn add_assign(&mut self, other: Self) {
        self.re += other.re;
        self.im += other.im;
    }
}

impl SubAssign for Complex<f64> {
    fn sub_assign(&mut self, other: Self) {
        self.re -= other.re;
        self.im -= other.im;
    }
}

impl MulAssign for Complex<f64> {
    fn mul_assign(&mut self, other: Self) {
        *self = *self * other;
    }
}

impl DivAssign for Complex<f64> {
    fn div_assign(&mut self, other: Self) {
        *self = *self / other;
    }
}

impl crate::Zero for Complex<f64> {
    fn zero() -> Self {
        Self::new(0.0, 0.0)
    }

    fn is_zero(&self) -> bool {
        self.re == 0.0 && self.im == 0.0
    }
}

impl crate::One for Complex<f64> {
    fn one() -> Self {
        Self::new(1.0, 0.0)
    }

    fn is_one(&self) -> bool {
        self.re == 1.0 && self.im == 0.0
    }
}


impl fmt::Display for Complex<f64> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let real_negative = self.re < 0.0;
        let imaginary_negative = self.im < 0.0;
        let real = if real_negative { 0.0 - self.re } else { self.re };
        let imaginary = if imaginary_negative { 0.0 - self.im } else { self.im };
        let sign = if real_negative {
            "-"
        } else if formatter.sign_plus() {
            "+"
        } else {
            ""
        };
        let between = if imaginary_negative { "-" } else { "+" };
        let text = match formatter.precision() {
            Some(precision) => alloc::format!("{sign}{real:.precision$}{between}{imaginary:.precision$}i"),
            None => alloc::format!("{sign}{real}{between}{imaginary}i"),
        };
        match formatter.width() {
            Some(width) => write!(formatter, "{text: >width$}"),
            None => formatter.write_str(&text),
        }
    }
}

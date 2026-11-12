pub trait Zero: Sized {
    fn zero() -> Self;
    fn is_zero(&self) -> bool;
}

pub trait One: Sized {
    fn one() -> Self;
    fn is_one(&self) -> bool;
}

pub trait Signed: Sized {
    fn abs(&self) -> Self;
    fn abs_sub(&self, other: &Self) -> Self;
    fn signum(&self) -> Self;
    fn is_positive(&self) -> bool;
    fn is_negative(&self) -> bool;
}

pub trait Num: Sized {
    type FromStrRadixErr;

    fn from_str_radix(text: &str, radix: u32) -> Result<Self, Self::FromStrRadixErr>;
}

pub trait Pow<Exponent> {
    type Output;

    fn pow(self, exponent: Exponent) -> Self::Output;
}

pub trait PrimInt: Copy + Ord + Zero + One {
    fn min_value() -> Self;
    fn max_value() -> Self;
    fn to_be(self) -> Self;
    fn to_le(self) -> Self;
}

pub trait ToPrimitive {
    fn to_i8(&self) -> Option<i8>;
    fn to_i16(&self) -> Option<i16>;
    fn to_i32(&self) -> Option<i32>;
    fn to_i64(&self) -> Option<i64>;
    fn to_i128(&self) -> Option<i128>;
    fn to_isize(&self) -> Option<isize>;
    fn to_u8(&self) -> Option<u8>;
    fn to_u16(&self) -> Option<u16>;
    fn to_u32(&self) -> Option<u32>;
    fn to_u64(&self) -> Option<u64>;
    fn to_u128(&self) -> Option<u128>;
    fn to_usize(&self) -> Option<usize>;
    fn to_f32(&self) -> Option<f32>;
    fn to_f64(&self) -> Option<f64>;
}

pub trait FromPrimitive: Sized {
    fn from_i8(value: i8) -> Option<Self>;
    fn from_i16(value: i16) -> Option<Self>;
    fn from_i32(value: i32) -> Option<Self>;
    fn from_i64(value: i64) -> Option<Self>;
    fn from_i128(value: i128) -> Option<Self>;
    fn from_isize(value: isize) -> Option<Self>;
    fn from_u8(value: u8) -> Option<Self>;
    fn from_u16(value: u16) -> Option<Self>;
    fn from_u32(value: u32) -> Option<Self>;
    fn from_u64(value: u64) -> Option<Self>;
    fn from_u128(value: u128) -> Option<Self>;
    fn from_usize(value: usize) -> Option<Self>;
    fn from_f32(value: f32) -> Option<Self>;
    fn from_f64(value: f64) -> Option<Self>;
}

pub struct ExtendedGcd<T> {
    pub gcd: T,
    pub x: T,
    pub y: T,
}

pub trait Integer: Sized {
    fn div_floor(&self, other: &Self) -> Self;
    fn mod_floor(&self, other: &Self) -> Self;
    fn div_mod_floor(&self, other: &Self) -> (Self, Self);
    fn div_ceil(&self, other: &Self) -> Self;
    fn gcd(&self, other: &Self) -> Self;
    fn lcm(&self, other: &Self) -> Self;
    fn is_multiple_of(&self, other: &Self) -> bool;
    fn is_even(&self) -> bool;
    fn is_odd(&self) -> bool;
    fn div_rem(&self, other: &Self) -> (Self, Self);
}

macro_rules! convert_methods {
    ($convert:ident) => {
        fn to_i8(&self) -> Option<i8> {
            $convert!(*self, i8)
        }

        fn to_i16(&self) -> Option<i16> {
            $convert!(*self, i16)
        }

        fn to_i32(&self) -> Option<i32> {
            $convert!(*self, i32)
        }

        fn to_i64(&self) -> Option<i64> {
            $convert!(*self, i64)
        }

        fn to_i128(&self) -> Option<i128> {
            $convert!(*self, i128)
        }

        fn to_isize(&self) -> Option<isize> {
            $convert!(*self, isize)
        }

        fn to_u8(&self) -> Option<u8> {
            $convert!(*self, u8)
        }

        fn to_u16(&self) -> Option<u16> {
            $convert!(*self, u16)
        }

        fn to_u32(&self) -> Option<u32> {
            $convert!(*self, u32)
        }

        fn to_u64(&self) -> Option<u64> {
            $convert!(*self, u64)
        }

        fn to_u128(&self) -> Option<u128> {
            $convert!(*self, u128)
        }

        fn to_usize(&self) -> Option<usize> {
            $convert!(*self, usize)
        }
    };
}

macro_rules! integer_to_integer {
    ($value:expr, $target:ty) => {
        <$target>::try_from($value).ok()
    };
}

macro_rules! float_to_integer {
    ($value:expr, $target:ty) => {
        float_to_integer_value::<$target>(f64::from($value), <$target>::MIN as f64, <$target>::BITS)
    };
}

trait FromTruncated: Sized {
    fn from_truncated(value: f64) -> Self;
}

macro_rules! from_truncated {
    ($($primitive:ty),*) => {$(
        impl FromTruncated for $primitive {
            fn from_truncated(value: f64) -> Self {
                value as $primitive
            }
        }
    )*};
}

from_truncated!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

fn float_to_integer_value<T: FromTruncated>(value: f64, minimum: f64, bits: u32) -> Option<T> {
    if value.is_nan() {
        return None;
    }
    let truncated = value.trunc();
    let exclusive_maximum = if minimum < 0.0 {
        -minimum
    } else {
        2f64.powi(i32::try_from(bits).expect("an integer width fits"))
    };
    if truncated >= minimum && truncated < exclusive_maximum {
        Some(T::from_truncated(truncated))
    } else {
        None
    }
}

macro_rules! primitive_integer {
    ($($primitive:ty),*) => {$(
        impl Zero for $primitive {
            fn zero() -> Self {
                0
            }

            fn is_zero(&self) -> bool {
                *self == 0
            }
        }

        impl One for $primitive {
            fn one() -> Self {
                1
            }

            fn is_one(&self) -> bool {
                *self == 1
            }
        }

        impl PrimInt for $primitive {
            fn min_value() -> Self {
                <$primitive>::MIN
            }

            fn max_value() -> Self {
                <$primitive>::MAX
            }

            fn to_be(self) -> Self {
                <$primitive>::to_be(self)
            }

            fn to_le(self) -> Self {
                <$primitive>::to_le(self)
            }
        }

        impl ToPrimitive for $primitive {
            convert_methods!(integer_to_integer);

            fn to_f32(&self) -> Option<f32> {
                Some(*self as f32)
            }

            fn to_f64(&self) -> Option<f64> {
                Some(*self as f64)
            }
        }

        impl FromPrimitive for $primitive {
            fn from_i8(value: i8) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_i16(value: i16) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_i32(value: i32) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_i64(value: i64) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_i128(value: i128) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_isize(value: isize) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_u8(value: u8) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_u16(value: u16) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_u32(value: u32) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_u64(value: u64) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_u128(value: u128) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_usize(value: usize) -> Option<Self> {
                Self::try_from(value).ok()
            }

            fn from_f32(value: f32) -> Option<Self> {
                float_to_integer!(value, $primitive)
            }

            fn from_f64(value: f64) -> Option<Self> {
                float_to_integer!(value, $primitive)
            }
        }
    )*};
}

primitive_integer!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

macro_rules! primitive_signed {
    ($($primitive:ty),*) => {$(
        impl Signed for $primitive {
            fn abs(&self) -> Self {
                if *self < 0 { -*self } else { *self }
            }

            fn abs_sub(&self, other: &Self) -> Self {
                if *self <= *other { 0 } else { *self - *other }
            }

            fn signum(&self) -> Self {
                match *self {
                    value if value > 0 => 1,
                    0 => 0,
                    _ => -1,
                }
            }

            fn is_positive(&self) -> bool {
                *self > 0
            }

            fn is_negative(&self) -> bool {
                *self < 0
            }
        }
    )*};
}

primitive_signed!(i8, i16, i32, i64, i128, isize);

macro_rules! primitive_float {
    ($($primitive:ty),*) => {$(
        impl Zero for $primitive {
            fn zero() -> Self {
                0.0
            }

            fn is_zero(&self) -> bool {
                *self == 0.0
            }
        }

        impl One for $primitive {
            fn one() -> Self {
                1.0
            }

            fn is_one(&self) -> bool {
                *self == 1.0
            }
        }

        impl Signed for $primitive {
            fn abs(&self) -> Self {
                <$primitive>::abs(*self)
            }

            fn abs_sub(&self, other: &Self) -> Self {
                if *self <= *other { 0.0 } else { *self - *other }
            }

            fn signum(&self) -> Self {
                <$primitive>::signum(*self)
            }

            fn is_positive(&self) -> bool {
                self.is_sign_positive()
            }

            fn is_negative(&self) -> bool {
                self.is_sign_negative()
            }
        }

        impl ToPrimitive for $primitive {
            convert_methods!(float_to_integer);

            fn to_f32(&self) -> Option<f32> {
                Some(*self as f32)
            }

            fn to_f64(&self) -> Option<f64> {
                Some(*self as f64)
            }
        }

        impl FromPrimitive for $primitive {
            fn from_i8(value: i8) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_i16(value: i16) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_i32(value: i32) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_i64(value: i64) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_i128(value: i128) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_isize(value: isize) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_u8(value: u8) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_u16(value: u16) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_u32(value: u32) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_u64(value: u64) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_u128(value: u128) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_usize(value: usize) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_f32(value: f32) -> Option<Self> {
                Some(value as $primitive)
            }

            fn from_f64(value: f64) -> Option<Self> {
                Some(value as $primitive)
            }
        }
    )*};
}

primitive_float!(f32, f64);


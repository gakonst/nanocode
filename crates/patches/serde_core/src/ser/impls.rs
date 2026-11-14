use crate::lib::*;

use crate::ser::{Error, Serialize, SerializeTuple, Serializer};

macro_rules! primitive_impl {
    ($ty:ident, $method:ident $($cast:tt)*) => {
        impl Serialize for $ty {
            #[inline]
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.$method(*self $($cast)*)
            }
        }
    }
}

primitive_impl!(bool, serialize_bool);
primitive_impl!(isize, serialize_i64 as i64);
primitive_impl!(i8, serialize_i8);
primitive_impl!(i16, serialize_i16);
primitive_impl!(i32, serialize_i32);
primitive_impl!(i64, serialize_i64);
primitive_impl!(i128, serialize_i128);
primitive_impl!(usize, serialize_u64 as u64);
primitive_impl!(u8, serialize_u8);
primitive_impl!(u16, serialize_u16);
primitive_impl!(u32, serialize_u32);
primitive_impl!(u64, serialize_u64);
primitive_impl!(u128, serialize_u128);
primitive_impl!(f32, serialize_f32);
primitive_impl!(f64, serialize_f64);
primitive_impl!(char, serialize_char);

impl Serialize for str {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self)
    }
}

impl Serialize for String {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self)
    }
}

impl<'a> Serialize for fmt::Arguments<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl Serialize for CStr {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.to_bytes())
    }
}

impl Serialize for CString {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.to_bytes())
    }
}

impl<T> Serialize for Option<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Some(ref value) => serializer.serialize_some(value),
            None => serializer.serialize_none(),
        }
    }
}

impl<T> Serialize for PhantomData<T>
where
    T: ?Sized,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_unit_struct("PhantomData")
    }
}

impl<T> Serialize for [T; 0] {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        tri!(serializer.serialize_tuple(0)).end()
    }
}

macro_rules! array_impls {
    ($($len:tt)+) => {
        $(
            impl<T> Serialize for [T; $len]
            where
                T: Serialize,
            {
                #[inline]
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    let mut seq = tri!(serializer.serialize_tuple($len));
                    for e in self {
                        tri!(seq.serialize_element(e));
                    }
                    seq.end()
                }
            }
        )+
    }
}

array_impls! {
    01 02 03 04 05 06 07 08 09 10
    11 12 13 14 15 16 17 18 19 20
    21 22 23 24 25 26 27 28 29 30
    31 32
}

impl<T> Serialize for [T]
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self)
    }
}

macro_rules! seq_impl {
    (
        $(#[$attr:meta])*
        $ty:ident <T $(, $typaram:ident : $bound:ident)*>
    ) => {
        $(#[$attr])*
        impl<T $(, $typaram)*> Serialize for $ty<T $(, $typaram)*>
        where
            T: Serialize,
        {
            #[inline]
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.collect_seq(self)
            }
        }
    }
}

seq_impl! {
    BinaryHeap<T>
}

seq_impl! {
    BTreeSet<T>
}

seq_impl! {
    HashSet<T, H: BuildHasher>
}

seq_impl! {
    LinkedList<T>
}

seq_impl! {
    Vec<T>
}

seq_impl! {
    VecDeque<T>
}

impl<Idx> Serialize for Range<Idx>
where
    Idx: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let mut state = tri!(serializer.serialize_struct("Range", 2));
        tri!(state.serialize_field("start", &self.start));
        tri!(state.serialize_field("end", &self.end));
        state.end()
    }
}

impl<Idx> Serialize for RangeFrom<Idx>
where
    Idx: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let mut state = tri!(serializer.serialize_struct("RangeFrom", 1));
        tri!(state.serialize_field("start", &self.start));
        state.end()
    }
}

impl<Idx> Serialize for RangeInclusive<Idx>
where
    Idx: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let mut state = tri!(serializer.serialize_struct("RangeInclusive", 2));
        tri!(state.serialize_field("start", &self.start()));
        tri!(state.serialize_field("end", &self.end()));
        state.end()
    }
}

impl<Idx> Serialize for RangeTo<Idx>
where
    Idx: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let mut state = tri!(serializer.serialize_struct("RangeTo", 1));
        tri!(state.serialize_field("end", &self.end));
        state.end()
    }
}

impl<T> Serialize for Bound<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Bound::Unbounded => serializer.serialize_unit_variant("Bound", 0, "Unbounded"),
            Bound::Included(ref value) => {
                serializer.serialize_newtype_variant("Bound", 1, "Included", value)
            }
            Bound::Excluded(ref value) => {
                serializer.serialize_newtype_variant("Bound", 2, "Excluded", value)
            }
        }
    }
}

impl Serialize for () {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_unit()
    }
}

macro_rules! tuple_impls {
    ($($len:expr => ($($n:tt $name:ident)+))+) => {
        $(
            impl<$($name),+> Serialize for ($($name,)+)
            where
                $($name: Serialize,)+
            {
                tuple_impl_body!($len => ($($n)+));
            }
        )+
    };
}

macro_rules! tuple_impl_body {
    ($len:expr => ($($n:tt)+)) => {
        #[inline]
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut tuple = tri!(serializer.serialize_tuple($len));
            $(
                tri!(tuple.serialize_element(&self.$n));
            )+
            tuple.end()
        }
    };
}

impl<T> Serialize for (T,)
where
    T: Serialize,
{
    tuple_impl_body!(1 => (0));
}

tuple_impls! {
    2 => (0 T0 1 T1)
    3 => (0 T0 1 T1 2 T2)
    4 => (0 T0 1 T1 2 T2 3 T3)
    5 => (0 T0 1 T1 2 T2 3 T3 4 T4)
    6 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5)
    7 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6)
    8 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7)
    9 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8)
    10 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9)
    11 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10)
    12 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11)
    13 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12)
    14 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13)
    15 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14)
    16 => (0 T0 1 T1 2 T2 3 T3 4 T4 5 T5 6 T6 7 T7 8 T8 9 T9 10 T10 11 T11 12 T12 13 T13 14 T14 15 T15)
}

macro_rules! map_impl {
    (
        $(#[$attr:meta])*
        $ty:ident <K $(: $kbound1:ident $(+ $kbound2:ident)*)*, V $(, $typaram:ident : $bound:ident)*>
    ) => {
        $(#[$attr])*
        impl<K, V $(, $typaram)*> Serialize for $ty<K, V $(, $typaram)*>
        where
            K: Serialize,
            V: Serialize,
        {
            #[inline]
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.collect_map(self)
            }
        }
    }
}

map_impl! {
    BTreeMap<K: Ord, V>
}

map_impl! {
    HashMap<K: Eq + Hash, V, H: BuildHasher>
}

macro_rules! deref_impl {
    (
        $(#[$attr:meta])*
        <$($desc:tt)+
    ) => {
        $(#[$attr])*
        impl <$($desc)+ {
            #[inline]
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                (**self).serialize(serializer)
            }
        }
    };
}

deref_impl! {
    <'a, T> Serialize for &'a T where T: ?Sized + Serialize
}

deref_impl! {
    <'a, T> Serialize for &'a mut T where T: ?Sized + Serialize
}

deref_impl! {
    <T> Serialize for Box<T> where T: ?Sized + Serialize
}

deref_impl! {
    #[cfg(feature = "rc")]
    <T> Serialize for Rc<T> where T: ?Sized + Serialize
}

deref_impl! {
    #[cfg(feature = "rc")]
    <T> Serialize for Arc<T> where T: ?Sized + Serialize
}

deref_impl! {
    <'a, T> Serialize for Cow<'a, T> where T: ?Sized + Serialize + ToOwned
}

#[cfg(feature = "rc")]
impl<T> Serialize for RcWeak<T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.upgrade().serialize(serializer)
    }
}

#[cfg(feature = "rc")]
impl<T> Serialize for ArcWeak<T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.upgrade().serialize(serializer)
    }
}

macro_rules! nonzero_integers {
    ($($T:ident,)+) => {
        $(
            impl Serialize for num::$T {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    self.get().serialize(serializer)
                }
            }
        )+
    }
}

nonzero_integers! {
    NonZeroI8,
    NonZeroI16,
    NonZeroI32,
    NonZeroI64,
    NonZeroI128,
    NonZeroIsize,
    NonZeroU8,
    NonZeroU16,
    NonZeroU32,
    NonZeroU64,
    NonZeroU128,
    NonZeroUsize,
}

impl<T> Serialize for Cell<T>
where
    T: Serialize + Copy,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.get().serialize(serializer)
    }
}

impl<T> Serialize for RefCell<T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.try_borrow() {
            Ok(value) => value.serialize(serializer),
            Err(_) => Err(S::Error::custom("already mutably borrowed")),
        }
    }
}

impl<T> Serialize for Mutex<T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.lock() {
            Ok(locked) => locked.serialize(serializer),
            Err(_) => Err(S::Error::custom("lock poison error while serializing")),
        }
    }
}

impl<T> Serialize for RwLock<T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.read() {
            Ok(locked) => locked.serialize(serializer),
            Err(_) => Err(S::Error::custom("lock poison error while serializing")),
        }
    }
}

impl<T, E> Serialize for Result<T, E>
where
    T: Serialize,
    E: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Result::Ok(ref value) => serializer.serialize_newtype_variant("Result", 0, "Ok", value),
            Result::Err(ref value) => {
                serializer.serialize_newtype_variant("Result", 1, "Err", value)
            }
        }
    }
}

impl Serialize for Duration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let mut state = tri!(serializer.serialize_struct("Duration", 2));
        tri!(state.serialize_field("secs", &self.as_secs()));
        tri!(state.serialize_field("nanos", &self.subsec_nanos()));
        state.end()
    }
}

impl Serialize for SystemTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use super::SerializeStruct;
        let duration_since_epoch = match self.duration_since(UNIX_EPOCH) {
            Ok(duration_since_epoch) => duration_since_epoch,
            Err(_) => return Err(S::Error::custom("SystemTime must be later than UNIX_EPOCH")),
        };
        let mut state = tri!(serializer.serialize_struct("SystemTime", 2));
        tri!(state.serialize_field("secs_since_epoch", &duration_since_epoch.as_secs()));
        tri!(state.serialize_field("nanos_since_epoch", &duration_since_epoch.subsec_nanos()));
        state.end()
    }
}

macro_rules! serialize_display_bounded_length {
    ($value:expr, $max:expr, $serializer:expr) => {{
        let mut buffer = [0u8; $max];
        let mut writer = crate::format::Buf::new(&mut buffer);
        write!(&mut writer, "{}", $value).unwrap();
        $serializer.serialize_str(writer.as_str())
    }};
}

impl Serialize for net::IpAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            match *self {
                net::IpAddr::V4(ref a) => a.serialize(serializer),
                net::IpAddr::V6(ref a) => a.serialize(serializer),
            }
        } else {
            match *self {
                net::IpAddr::V4(ref a) => {
                    serializer.serialize_newtype_variant("IpAddr", 0, "V4", a)
                }
                net::IpAddr::V6(ref a) => {
                    serializer.serialize_newtype_variant("IpAddr", 1, "V6", a)
                }
            }
        }
    }
}

const DEC_DIGITS_LUT: &[u8] = b"\
      0001020304050607080910111213141516171819\
      2021222324252627282930313233343536373839\
      4041424344454647484950515253545556575859\
      6061626364656667686970717273747576777879\
      8081828384858687888990919293949596979899";

#[inline]
fn format_u8(mut n: u8, out: &mut [u8]) -> usize {
    if n >= 100 {
        let d1 = ((n % 100) << 1) as usize;
        n /= 100;
        out[0] = b'0' + n;
        out[1] = DEC_DIGITS_LUT[d1];
        out[2] = DEC_DIGITS_LUT[d1 + 1];
        3
    } else if n >= 10 {
        let d1 = (n << 1) as usize;
        out[0] = DEC_DIGITS_LUT[d1];
        out[1] = DEC_DIGITS_LUT[d1 + 1];
        2
    } else {
        out[0] = b'0' + n;
        1
    }
}

#[test]
fn test_format_u8() {
    let mut i = 0u8;

    loop {
        let mut buf = [0u8; 3];
        let written = format_u8(i, &mut buf);
        assert_eq!(i.to_string().as_bytes(), &buf[..written]);

        match i.checked_add(1) {
            Some(next) => i = next,
            None => break,
        }
    }
}

impl Serialize for net::Ipv4Addr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            const MAX_LEN: usize = 15;
            debug_assert_eq!(MAX_LEN, "101.102.103.104".len());
            let mut buf = [b'.'; MAX_LEN];
            let mut written = format_u8(self.octets()[0], &mut buf);
            for oct in &self.octets()[1..] {
                written += format_u8(*oct, &mut buf[written + 1..]) + 1;
            }
            let buf = unsafe { str::from_utf8_unchecked(&buf[..written]) };
            serializer.serialize_str(buf)
        } else {
            self.octets().serialize(serializer)
        }
    }
}

impl Serialize for net::Ipv6Addr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            const MAX_LEN: usize = 39;
            debug_assert_eq!(MAX_LEN, "1001:1002:1003:1004:1005:1006:1007:1008".len());
            serialize_display_bounded_length!(self, MAX_LEN, serializer)
        } else {
            self.octets().serialize(serializer)
        }
    }
}

impl Serialize for net::SocketAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            match *self {
                net::SocketAddr::V4(ref addr) => addr.serialize(serializer),
                net::SocketAddr::V6(ref addr) => addr.serialize(serializer),
            }
        } else {
            match *self {
                net::SocketAddr::V4(ref addr) => {
                    serializer.serialize_newtype_variant("SocketAddr", 0, "V4", addr)
                }
                net::SocketAddr::V6(ref addr) => {
                    serializer.serialize_newtype_variant("SocketAddr", 1, "V6", addr)
                }
            }
        }
    }
}

impl Serialize for net::SocketAddrV4 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            const MAX_LEN: usize = 21;
            debug_assert_eq!(MAX_LEN, "101.102.103.104:65000".len());
            serialize_display_bounded_length!(self, MAX_LEN, serializer)
        } else {
            (self.ip(), self.port()).serialize(serializer)
        }
    }
}

impl Serialize for net::SocketAddrV6 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            const MAX_LEN: usize = 58;
            debug_assert_eq!(
                MAX_LEN,
                "[1001:1002:1003:1004:1005:1006:1007:1008%4294967295]:65000".len()
            );
            serialize_display_bounded_length!(self, MAX_LEN, serializer)
        } else {
            (self.ip(), self.port()).serialize(serializer)
        }
    }
}

impl Serialize for Path {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.to_str() {
            Some(s) => s.serialize(serializer),
            None => Err(Error::custom("path contains invalid UTF-8 characters")),
        }
    }
}

impl Serialize for PathBuf {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_path().serialize(serializer)
    }
}

#[cfg(any(unix, windows))]
impl Serialize for OsStr {
    #[cfg(unix)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::os::unix::ffi::OsStrExt;
        serializer.serialize_newtype_variant("OsString", 0, "Unix", self.as_bytes())
    }

    #[cfg(windows)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::os::windows::ffi::OsStrExt;
        let val = self.encode_wide().collect::<Vec<_>>();
        serializer.serialize_newtype_variant("OsString", 1, "Windows", &val)
    }
}

#[cfg(any(unix, windows))]
impl Serialize for OsString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_os_str().serialize(serializer)
    }
}

impl<T> Serialize for Wrapping<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<T> Serialize for Saturating<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<T> Serialize for Reverse<T>
where
    T: Serialize,
{
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

macro_rules! atomic_impl {
    ($($ty:ident $size:expr)*) => {
        $(
            #[cfg(target_has_atomic = $size)]
            impl Serialize for $ty {
                fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
                where
                    S: Serializer,
                {
                    self.load(Ordering::Relaxed).serialize(serializer)
                }
            }
        )*
    }
}

atomic_impl! {
    AtomicBool "8"
    AtomicI8 "8"
    AtomicI16 "16"
    AtomicI32 "32"
    AtomicIsize "ptr"
    AtomicU8 "8"
    AtomicU16 "16"
    AtomicU32 "32"
    AtomicUsize "ptr"
}

atomic_impl! {
    AtomicI64 "64"
    AtomicU64 "64"
}

pub const LITTLE_ENDIAN: u8 = b'l';
pub const BIG_ENDIAN: u8 = b'B';
const ARRAY_BYTES_MAX: usize = 1 << 26;
const SIGNATURE_BYTES_MAX: usize = 255;
const ARRAY_DEPTH_MAX: usize = 32;
const STRUCT_DEPTH_MAX: usize = 32;
const NESTING_DEPTH_MAX: usize = ARRAY_DEPTH_MAX + STRUCT_DEPTH_MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Byte(u8),
    Bool(bool),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F64(f64),
    Str(String),
    Path(String),
    Signature(String),
    Fd(u32),
    Array(Vec<Self>),
    Entry(Box<(Self, Self)>),
    Struct(Vec<Self>),
    Variant(Box<Self>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl Value {
    pub fn str(&self) -> Option<&str> {
        match self {
            Self::Str(text) | Self::Path(text) | Self::Signature(text) => Some(text),
            _ => None,
        }
    }

    pub const fn i32(&self) -> Option<i32> {
        match self {
            Self::I32(value) => Some(*value),
            _ => None,
        }
    }

    pub const fn u32(&self) -> Option<u32> {
        match self {
            Self::U32(value) => Some(*value),
            _ => None,
        }
    }

    pub const fn f64(&self) -> Option<f64> {
        match self {
            Self::F64(value) => Some(*value),
            _ => None,
        }
    }

    pub const fn bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn fields(&self) -> Option<&[Self]> {
        match self {
            Self::Struct(fields) => Some(fields),
            _ => None,
        }
    }

    pub fn inner(&self) -> Option<&Self> {
        match self {
            Self::Variant(inner) => Some(inner),
            _ => None,
        }
    }

    fn signature(&self, out: &mut String) {
        match self {
            Self::Byte(_) => out.push('y'),
            Self::Bool(_) => out.push('b'),
            Self::I16(_) => out.push('n'),
            Self::U16(_) => out.push('q'),
            Self::I32(_) => out.push('i'),
            Self::U32(_) => out.push('u'),
            Self::I64(_) => out.push('x'),
            Self::U64(_) => out.push('t'),
            Self::F64(_) => out.push('d'),
            Self::Str(_) => out.push('s'),
            Self::Path(_) => out.push('o'),
            Self::Signature(_) => out.push('g'),
            Self::Fd(_) => out.push('h'),
            Self::Variant(_) => out.push('v'),
            Self::Struct(fields) => {
                out.push('(');
                fields.iter().for_each(|field| field.signature(out));
                out.push(')');
            }
            Self::Array(_) | Self::Entry(_) => {
                panic!("nanocode-computer sends no D-Bus arrays, so none is ever encoded")
            }
        }
    }
}

pub fn signature(values: &[Value]) -> String {
    let mut out = String::new();
    values.iter().for_each(|value| value.signature(&mut out));
    out
}

#[derive(Default)]
pub struct Writer {
    pub bytes: Vec<u8>,
}

impl Writer {
    pub fn pad(&mut self, alignment: usize) {
        let padded = self.bytes.len().next_multiple_of(alignment);
        self.bytes.resize(padded, 0);
    }

    pub fn byte(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn u32(&mut self, value: u32) {
        self.pad(4);
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    pub fn text(&mut self, text: &str) {
        let length = u32::try_from(text.len()).expect("a D-Bus string we send fits in 32 bits");
        self.u32(length);
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(0);
    }

    pub fn signature(&mut self, signature: &str) {
        assert!(signature.len() <= SIGNATURE_BYTES_MAX);
        self.bytes.push(signature.len() as u8);
        self.bytes.extend_from_slice(signature.as_bytes());
        self.bytes.push(0);
    }

    pub fn value(&mut self, value: &Value) {
        match value {
            Value::Byte(value) => self.byte(*value),
            Value::Bool(value) => self.u32(u32::from(*value)),
            Value::I16(value) => self.fixed(2, &value.to_le_bytes()),
            Value::U16(value) => self.fixed(2, &value.to_le_bytes()),
            Value::I32(value) => self.fixed(4, &value.to_le_bytes()),
            Value::U32(value) | Value::Fd(value) => self.u32(*value),
            Value::I64(value) => self.fixed(8, &value.to_le_bytes()),
            Value::U64(value) => self.fixed(8, &value.to_le_bytes()),
            Value::F64(value) => self.fixed(8, &value.to_le_bytes()),
            Value::Str(text) | Value::Path(text) => self.text(text),
            Value::Signature(text) => self.signature(text),
            Value::Variant(inner) => {
                self.signature(&signature(std::slice::from_ref(inner)));
                self.value(inner);
            }
            Value::Struct(fields) => {
                self.pad(8);
                fields.iter().for_each(|field| self.value(field));
            }
            Value::Array(_) | Value::Entry(_) => {
                panic!("nanocode-computer sends no D-Bus arrays, so none is ever encoded")
            }
        }
    }

    fn fixed(&mut self, alignment: usize, bytes: &[u8]) {
        self.pad(alignment);
        self.bytes.extend_from_slice(bytes);
    }
}

pub fn complete(signature: &[u8]) -> Result<usize, Malformed> {
    complete_at(signature, 0, 0, 0)
}

fn complete_at(
    signature: &[u8],
    at: usize,
    arrays: usize,
    structs: usize,
) -> Result<usize, Malformed> {
    if arrays > ARRAY_DEPTH_MAX || structs > STRUCT_DEPTH_MAX {
        return Err(Malformed("a signature nests deeper than D-Bus allows"));
    }
    match signature.get(at) {
        None => Err(Malformed("a signature ends inside a type")),
        Some(
            b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b's' | b'o' | b'g'
            | b'h' | b'v',
        ) => Ok(at + 1),
        Some(b'a') if signature.get(at + 1) == Some(&b'{') => {
            let key = signature.get(at + 2).copied().unwrap_or_default();
            if !b"ybnqiuxtdsogh".contains(&key) {
                return Err(Malformed("a dictionary key is not a basic type"));
            }
            let end = complete_at(signature, at + 3, arrays + 1, structs + 1)?;
            if signature.get(end) != Some(&b'}') {
                return Err(Malformed("a dictionary entry holds other than two types"));
            }
            Ok(end + 1)
        }
        Some(b'a') => complete_at(signature, at + 1, arrays + 1, structs),
        Some(b'(') => {
            let mut end = at + 1;
            if signature.get(end) == Some(&b')') {
                return Err(Malformed("a signature has an empty struct"));
            }
            while signature.get(end) != Some(&b')') {
                end = complete_at(signature, end, arrays, structs + 1)?;
            }
            Ok(end + 1)
        }
        Some(_) => Err(Malformed("a signature names an unknown type")),
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    big: bool,
}

impl<'a> Reader<'a> {
    pub const fn new(bytes: &'a [u8], at: usize, big: bool) -> Self {
        Self { bytes, at, big }
    }

    pub const fn at(&self) -> usize {
        self.at
    }

    pub fn values(&mut self, signature: &str) -> Result<Vec<Value>, Malformed> {
        let signature = signature.as_bytes();
        let mut values = Vec::new();
        let mut at = 0;
        while at < signature.len() {
            let end = complete(&signature[at..])? + at;
            values.push(self.value(&signature[at..end], 0)?);
            at = end;
        }
        Ok(values)
    }

    pub fn pad(&mut self, alignment: usize) -> Result<(), Malformed> {
        let padded = self.at.next_multiple_of(alignment);
        let padding = self
            .bytes
            .get(self.at..padded)
            .ok_or(Malformed("a message ends inside padding"))?;
        if padding.iter().any(|&byte| byte != 0) {
            return Err(Malformed("a message has nonzero padding"));
        }
        self.at = padded;
        Ok(())
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        self.pad(N)?;
        let bytes: [u8; N] = self
            .bytes
            .get(self.at..self.at + N)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(Malformed("a message ends inside a value"))?;
        self.at += N;
        Ok(if self.big {
            let mut swapped = bytes;
            swapped.reverse();
            swapped
        } else {
            bytes
        })
    }

    pub fn byte(&mut self) -> Result<u8, Malformed> {
        Ok(self.take::<1>()?[0])
    }

    pub fn u32(&mut self) -> Result<u32, Malformed> {
        self.take::<4>().map(u32::from_le_bytes)
    }

    fn text(&mut self, length: usize) -> Result<String, Malformed> {
        let bytes = self
            .bytes
            .get(self.at..self.at + length + 1)
            .ok_or(Malformed("a message ends inside a string"))?;
        if bytes[length] != 0 || bytes[..length].contains(&0) {
            return Err(Malformed(
                "a string is not terminated by its only zero byte",
            ));
        }
        let text = std::str::from_utf8(&bytes[..length])
            .map_err(|_| Malformed("a string is not UTF-8"))?
            .to_owned();
        self.at += length + 1;
        Ok(text)
    }

    pub fn string(&mut self) -> Result<String, Malformed> {
        let length = self.u32()? as usize;
        self.text(length)
    }

    pub fn signature(&mut self) -> Result<String, Malformed> {
        let length = usize::from(self.byte()?);
        self.text(length)
    }

    fn value(&mut self, signature: &[u8], depth: usize) -> Result<Value, Malformed> {
        if depth > NESTING_DEPTH_MAX {
            return Err(Malformed("a value nests deeper than D-Bus allows"));
        }
        Ok(match signature[0] {
            b'y' => Value::Byte(self.byte()?),
            b'b' => match self.u32()? {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => return Err(Malformed("a boolean is neither 0 nor 1")),
            },
            b'n' => Value::I16(self.take::<2>().map(i16::from_le_bytes)?),
            b'q' => Value::U16(self.take::<2>().map(u16::from_le_bytes)?),
            b'i' => Value::I32(self.take::<4>().map(i32::from_le_bytes)?),
            b'u' => Value::U32(self.u32()?),
            b'h' => Value::Fd(self.u32()?),
            b'x' => Value::I64(self.take::<8>().map(i64::from_le_bytes)?),
            b't' => Value::U64(self.take::<8>().map(u64::from_le_bytes)?),
            b'd' => Value::F64(self.take::<8>().map(f64::from_le_bytes)?),
            b's' => Value::Str(self.string()?),
            b'o' => Value::Path(self.string()?),
            b'g' => Value::Signature(self.signature()?),
            b'v' => {
                let inner = self.signature()?;
                if inner.is_empty() || complete(inner.as_bytes())? != inner.len() {
                    return Err(Malformed("a variant holds other than one type"));
                }
                Value::Variant(Box::new(self.value(inner.as_bytes(), depth + 1)?))
            }
            b'(' => {
                self.pad(8)?;
                let end = signature.len() - 1;
                let mut fields = Vec::new();
                let mut at = 1;
                while at < end {
                    let next = complete(&signature[at..])? + at;
                    fields.push(self.value(&signature[at..next], depth + 1)?);
                    at = next;
                }
                Value::Struct(fields)
            }
            b'{' => {
                self.pad(8)?;
                let key_end = complete(&signature[1..])? + 1;
                let key = self.value(&signature[1..key_end], depth + 1)?;
                let item = self.value(&signature[key_end..signature.len() - 1], depth + 1)?;
                Value::Entry(Box::new((key, item)))
            }
            b'a' => self.array(&signature[1..], depth)?,
            _ => return Err(Malformed("a signature names an unknown type")),
        })
    }

    fn array(&mut self, item: &[u8], depth: usize) -> Result<Value, Malformed> {
        let length = self.u32()? as usize;
        if length > ARRAY_BYTES_MAX {
            return Err(Malformed("an array is longer than D-Bus allows"));
        }
        self.pad(alignment(item[0]))?;
        let end = self.at + length;
        if end > self.bytes.len() {
            return Err(Malformed("a message ends inside an array"));
        }
        let mut items = Vec::new();
        while self.at < end {
            items.push(self.value(item, depth + 1)?);
        }
        if self.at != end {
            return Err(Malformed("an array's items overrun its length"));
        }
        Ok(Value::Array(items))
    }
}

const fn alignment(code: u8) -> usize {
    match code {
        b'n' | b'q' => 2,
        b'b' | b'i' | b'u' | b'h' | b's' | b'o' | b'a' => 4,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 1,
    }
}

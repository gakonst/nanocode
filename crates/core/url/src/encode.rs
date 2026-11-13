use std::borrow::Cow;

const HEX_DIGITS_UPPER: &[u8; 16] = b"0123456789ABCDEF";
const C0_CONTROL_END: u8 = 0x20;
const DELETE: u8 = 0x7f;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EncodeSet {
    Controls,
    Fragment,
    Query,
    SpecialQuery,
    Path,
    Userinfo,
}

impl EncodeSet {
    const fn contains(self, byte: u8) -> bool {
        if byte < C0_CONTROL_END || byte >= DELETE {
            return true;
        }
        match self {
            Self::Controls => false,
            Self::Fragment => matches!(byte, b' ' | b'"' | b'<' | b'>' | b'`'),
            Self::Query => matches!(byte, b' ' | b'"' | b'#' | b'<' | b'>'),
            Self::SpecialQuery => Self::Query.contains(byte) || byte == b'\'',
            Self::Path => {
                Self::Fragment.contains(byte) || matches!(byte, b'#' | b'?' | b'{' | b'}')
            }
            Self::Userinfo => {
                Self::Path.contains(byte)
                    || matches!(
                        byte,
                        b'/' | b':' | b';' | b'=' | b'@' | b'[' | b'\\' | b']' | b'^' | b'|'
                    )
            }
        }
    }
}

pub(crate) fn push_byte_encoded(out: &mut String, byte: u8) {
    out.push('%');
    out.push(char::from(HEX_DIGITS_UPPER[usize::from(byte >> 4)]));
    out.push(char::from(HEX_DIGITS_UPPER[usize::from(byte & 0x0f)]));
}

pub(crate) fn encode_str(out: &mut String, text: &str, set: EncodeSet) {
    for byte in text.bytes() {
        if set.contains(byte) {
            push_byte_encoded(out, byte);
        } else {
            out.push(char::from(byte));
        }
    }
}

pub(crate) fn encode_char(out: &mut String, value: char, set: EncodeSet) {
    let mut bytes = [0; 4];
    encode_str(out, value.encode_utf8(&mut bytes), set);
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn percent_decode(input: &[u8]) -> Cow<'_, [u8]> {
    if !input.contains(&b'%') {
        return Cow::Borrowed(input);
    }
    let mut out = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let byte = input[index];
        if byte == b'%'
            && let Some(high) = input.get(index + 1).and_then(|byte| hex_value(*byte))
            && let Some(low) = input.get(index + 2).and_then(|byte| hex_value(*byte))
        {
            out.push(high << 4 | low);
            index += 3;
            continue;
        }
        out.push(byte);
        index += 1;
    }
    Cow::Owned(out)
}

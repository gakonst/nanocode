const LOCAL_PART_BYTES_MAX: usize = 64;
const DOMAIN_BYTES_MAX: usize = 254;
const LABEL_BYTES_MAX: usize = 63;
const DISPLAY_SEPARATOR: &str = " <";
const DISPLAY_START: char = '<';
const DISPLAY_END: char = '>';
const QUOTE: char = '"';
const ESCAPE: char = '\\';

pub(crate) fn address_domain(address: &str) -> Option<&str> {
    let (display, email) = split_display(address)?;
    let (local_part, domain) = email.rsplit_once('@')?;
    if display.is_empty() && local_part.starts_with(DISPLAY_START) {
        return None;
    }
    if !valid_local_part(local_part) || !valid_domain(domain) {
        return None;
    }
    Some(domain)
}

fn split_display(text: &str) -> Option<(&str, &str)> {
    let Some((left, right)) = text.rsplit_once(DISPLAY_SEPARATOR) else {
        return Some(("", text));
    };
    let right = right.trim();
    let email = right.strip_suffix(DISPLAY_END)?;
    Some((left.trim(), email))
}

fn valid_local_part(part: &str) -> bool {
    if part.is_empty() || part.len() > LOCAL_PART_BYTES_MAX {
        return false;
    }
    if part.starts_with(QUOTE) && part.ends_with(QUOTE) {
        return part.len() > 2 && is_quoted_content(&part[1..part.len() - 1]);
    }
    part.split('.').all(is_atom)
}

fn valid_domain(part: &str) -> bool {
    if part.is_empty() || part.len() > DOMAIN_BYTES_MAX {
        return false;
    }
    if part.starts_with('[') && part.ends_with(']') {
        return part[1..part.len() - 1].chars().all(is_domain_text);
    }
    part.split('.').all(|label| {
        label.starts_with(char::is_alphanumeric)
            && label.ends_with(char::is_alphanumeric)
            && label.len() <= LABEL_BYTES_MAX
            && is_atom(label)
    })
}

fn is_atom(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_atom_text)
}

fn is_atom_text(c: char) -> bool {
    c.is_alphanumeric() || "!#$%&'*+-/=?^_\x60{|}~".contains(c) || is_encoded_non_ascii(c)
}

const fn is_encoded_non_ascii(c: char) -> bool {
    let bytes = (c as u32).to_be_bytes();
    matches!(
        (bytes[0], bytes[1], bytes[2], bytes[3]),
        (0x00, 0x00, 0xC2..=0xDF, 0x80..=0xBF)
            | (0x00, 0xE0, 0xA0..=0xBF, 0x80..=0xBF)
            | (0x00, 0xE1..=0xEC, 0x80..=0xBF, 0x80..=0xBF)
            | (0x00, 0xED, 0x80..=0x9F, 0x80..=0xBF)
            | (0x00, 0xEE..=0xEF, 0x80..=0xBF, 0x80..=0xBF)
            | (0xF0, 0x90..=0xBF, 0x80..=0xBF, 0x80..=0xBF)
            | (0xF1..=0xF3, 0x80..=0xBF, 0x80..=0xBF, 0x80..=0xBF)
            | (0xF4, 0x80..=0x8F, 0x80..=0xBF, 0x80..=0xBF)
    )
}

const fn is_visible(c: char) -> bool {
    matches!(c, '\x21'..='\x7E')
}

const fn is_quoted_text(c: char) -> bool {
    matches!(c, '\x21' | '\x23'..='\x5B' | '\x5D'..='\x7E') || is_encoded_non_ascii(c)
}

fn is_quoted_content(text: &str) -> bool {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == ESCAPE {
            if !chars.next().is_some_and(is_visible) {
                return false;
            }
        } else if !(c == ' ' || c == '\t' || is_quoted_text(c)) {
            return false;
        }
    }
    true
}

const fn is_domain_text(c: char) -> bool {
    matches!(c, '\x21'..='\x5A' | '\x5E'..='\x7E') || is_encoded_non_ascii(c)
}

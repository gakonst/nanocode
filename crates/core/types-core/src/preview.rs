use serde_json::{Map, Value};

pub const RESULT_BYTES: usize = 32 * 1024;
pub const PREVIEW_BYTES: usize = 16 * 1024;
pub const METADATA_BYTES: usize = 4096;

struct CountingWriter(usize);

impl std::io::Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[must_use]
pub fn encoded_len(value: &Value) -> usize {
    match value {
        Value::Null => 0,
        Value::String(text) => text.len(),
        value => {
            let mut counter = CountingWriter(0);
            serde_json::to_writer(&mut counter, value).map_or(0, |()| counter.0)
        }
    }
}

#[must_use]
pub fn utf8_prefix(text: &str, bytes: usize) -> &str {
    &text[..text.floor_char_boundary(bytes)]
}

#[must_use]
pub fn image_reference(value: &Value) -> Value {
    let Value::Object(object) = value else {
        return Value::Null;
    };
    let reference: Map<String, Value> = ["image_url", "file_id"]
        .into_iter()
        .filter_map(|key| {
            object
                .get(key)
                .filter(|field| field.is_string())
                .map(|field| (key.to_owned(), field.clone()))
        })
        .collect();
    if reference.is_empty() {
        Value::Null
    } else {
        Value::Object(reference)
    }
}

#[must_use]
pub fn shortened(input: &Value) -> Option<(Value, usize)> {
    fn lengths(value: &Value, found: &mut Vec<usize>) {
        match value {
            Value::String(text) => found.push(text.len()),
            Value::Array(items) => items.iter().for_each(|item| lengths(item, found)),
            Value::Object(fields) => fields.values().for_each(|item| lengths(item, found)),
            _ => {}
        }
    }
    fn cut(value: &Value, cap: usize) -> Value {
        match value {
            Value::String(text) => Value::String(utf8_prefix(text, cap).to_owned()),
            Value::Array(items) => Value::Array(items.iter().map(|item| cut(item, cap)).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, item)| (key.clone(), cut(item, cap)))
                    .collect(),
            ),
            value => value.clone(),
        }
    }
    let size = encoded_len(input);
    if size <= RESULT_BYTES {
        return None;
    }
    let mut found = Vec::new();
    lengths(input, &mut found);
    let Some(mut left) = PREVIEW_BYTES.checked_sub(size - found.iter().sum::<usize>()) else {
        return Some((Value::Null, size));
    };
    found.sort_unstable();
    let mut cap = usize::MAX;
    for (index, length) in found.iter().enumerate() {
        let remaining = found.len() - index;
        if length * remaining > left {
            cap = left / remaining;
            break;
        }
        left -= length;
    }
    Some((cut(input, cap), size))
}

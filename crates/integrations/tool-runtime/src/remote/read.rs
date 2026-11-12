use base64::Engine as _;
use serde_json::{Value, json};
use std::future::Future;

#[derive(Debug)]
pub enum ReadError<E> {
    Unreadable(String),
    Changed(&'static str),
    Call(E),
}

impl<E: std::fmt::Display> std::fmt::Display for ReadError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(value) => write!(formatter, "{value}"),
            Self::Changed(value) => write!(formatter, "{value}"),
            Self::Call(value) => std::fmt::Display::fmt(value, formatter),
        }
    }
}

impl<E: std::error::Error> std::error::Error for ReadError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Call(value) => std::error::Error::source(value),
            _ => None,
        }
    }
}

pub async fn read_whole<E, F, Fut>(
    path: &str,
    chunk: u64,
    limit: u64,
    mut read: F,
) -> Result<Option<Vec<u8>>, ReadError<E>>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, E>>,
{
    let (mut bytes, mut version) = (Vec::new(), None);
    loop {
        let input = json!({"path": path, "offset": bytes.len(), "length": chunk});
        let outcome = read(input).await.map_err(ReadError::Call)?;
        if outcome["status"] != "completed" {
            return Err(ReadError::Changed("Hand read failed"));
        }
        let output = &outcome["output"];
        if output["success"] != true {
            let refused = output["output"].as_str().unwrap_or("Hand read refused");
            return Err(ReadError::Unreadable(refused.into()));
        }
        let piece = &output["structured_result"];
        if piece["exists"] == false {
            return match version {
                None => Ok(None),
                Some(_) => Err(ReadError::Changed("file removed while reading")),
            };
        }
        if piece["link"].is_string() {
            return Err(ReadError::Unreadable(
                "symbolic links are not followed".into(),
            ));
        }
        let size = piece["size"]
            .as_u64()
            .ok_or(ReadError::Changed("missing size"))?;
        if size > limit {
            return Err(ReadError::Unreadable("file too large".into()));
        }
        let current = (size, piece["modified_ns"].clone());
        if *version.get_or_insert_with(|| current.clone()) != current {
            return Err(ReadError::Changed("file changed while reading"));
        }
        if piece["offset"] != bytes.len() {
            return Err(ReadError::Changed("unexpected offset"));
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(piece["data"].as_str().unwrap_or_default())
            .map_err(|_| ReadError::Changed("chunk is not base64"))?;
        if data.is_empty() && bytes.len() as u64 != size {
            return Err(ReadError::Changed("file changed while reading"));
        }
        bytes.extend_from_slice(&data);
        if bytes.len() as u64 > size {
            return Err(ReadError::Changed("file changed while reading"));
        }
        if bytes.len() as u64 == size {
            return Ok(Some(bytes));
        }
    }
}

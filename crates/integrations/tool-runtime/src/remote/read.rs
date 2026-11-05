use base64::Engine as _;
use serde_json::{Value, json};
use std::future::Future;

#[derive(Debug, thiserror::Error)]
pub enum ReadError<E> {
    #[error("{0}")]
    Unreadable(String),
    #[error("{0}")]
    Changed(&'static str),
    #[error(transparent)]
    Call(E),
}

/// Reads a whole file through a Hand's chunked `read_file`, `chunk` bytes at a time.
/// `read` runs one `read_file` call with the given input and returns its outcome
/// (`{status, output: {success, output, structured_result}}`). A file that changes
/// size or modification time between chunks, shrinks, or moves under the reader is
/// `Changed`; a refusal, a symbolic link or a file larger than `limit` is
/// `Unreadable`; a missing file is `Ok(None)`.
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

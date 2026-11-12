use std::{error::Error, io};

use futures_util::{Stream, StreamExt, stream};
use nanocode_compression::brotli::{Decoder, Status};

const HTTP_READ_CHUNK_SIZE: usize = 4096;

pub(crate) fn stream<S, B, E>(source: S) -> impl Stream<Item = io::Result<Vec<u8>>>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
    E: Error + Send + Sync + 'static,
{
    let state = (
        Box::pin(source),
        Box::new(Decoder::new()),
        None::<(B, usize)>,
        Status::NeedsInput,
        false,
    );
    stream::try_unfold(
        state,
        |(mut source, mut decoder, mut pending, mut status, mut eof)| async move {
            loop {
                if status == Status::Done {
                    return Ok(None);
                }
                if pending.is_none() && status == Status::NeedsInput && !eof {
                    match source.next().await {
                        Some(Ok(bytes)) => pending = Some((bytes, 0)),
                        Some(Err(error)) => return Err(io::Error::other(error)),
                        None => eof = true,
                    }
                }
                let input = pending
                    .as_ref()
                    .map_or(&[][..], |(bytes, offset)| &bytes.as_ref()[*offset..]);
                let mut output = vec![0; HTTP_READ_CHUNK_SIZE];
                let progress = decoder.decompress(input, &mut output).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid Brotli response body")
                })?;
                status = progress.status;
                if let Some((bytes, offset)) = pending.as_mut() {
                    *offset += progress.consumed;
                    if *offset == bytes.as_ref().len() {
                        pending = None;
                    }
                }
                if progress.written != 0 {
                    output.truncate(progress.written);
                    return Ok(Some((output, (source, decoder, pending, status, eof))));
                }
                if eof && status == Status::NeedsInput {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated Brotli response body",
                    ));
                }
            }
        },
    )
}

use std::path::{Path, PathBuf};

use nanocode_tools::ToolDefinition;
use tokio::io::{AsyncRead, AsyncReadExt};

use super::{
    super::{StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolResult},
    host::{DATA_URL_PREFIX, ViewImageArguments, detail, output, resolve},
};

pub(crate) struct ViewImageHandler {
    workspace: PathBuf,
    max_wire_bytes: Option<u64>,
}

impl ViewImageHandler {
    pub(crate) const fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            max_wire_bytes: None,
        }
    }

    pub(crate) const fn with_wire_limit(workspace: PathBuf, max_wire_bytes: u64) -> Self {
        Self {
            workspace,
            max_wire_bytes: Some(max_wire_bytes),
        }
    }
}

#[async_trait::async_trait]
impl Tool for ViewImageHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::ViewImage.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    async fn execute(&self, input: ToolInput, _context: ToolContext<'_>) -> ToolResult {
        let arguments = input.decode_json::<ViewImageArguments>()?;
        let detail = match detail(arguments.detail.as_deref()) {
            Ok(detail) => detail,
            Err(error) => return Ok(error),
        };
        let path = resolve(&self.workspace, Path::new(&arguments.path));
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "unable to locate image at `{}`: {error}",
                    path.display()
                )));
            }
        };
        let metadata = match file.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                return Ok(ToolOutput::error(format!(
                    "image path `{}` is not a file",
                    path.display()
                )));
            }
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "unable to locate image at `{}`: {error}",
                    path.display()
                )));
            }
        };
        let bytes = match self.max_wire_bytes {
            Some(max_wire_bytes) => {
                let max_raw_bytes = max_raw_bytes(max_wire_bytes);
                if metadata.len() > max_raw_bytes {
                    return Ok(oversized_image_error(&path, metadata.len(), max_wire_bytes));
                }
                match read_bounded(&mut file, metadata.len(), max_raw_bytes).await {
                    Ok(BoundedRead::Complete(bytes)) => bytes,
                    Ok(BoundedRead::TooLarge) => {
                        return Ok(growing_image_error(&path, max_raw_bytes, max_wire_bytes));
                    }
                    Err(error) => return Ok(read_error(&path, error)),
                }
            }
            None => {
                let mut bytes = Vec::new();
                match file.read_to_end(&mut bytes).await {
                    Ok(_) => bytes,
                    Err(error) => return Ok(read_error(&path, error)),
                }
            }
        };
        Ok(output(&bytes, detail))
    }
}

const DATA_URL_PREFIX_BYTES: u64 = DATA_URL_PREFIX.len() as u64;
const VIEW_IMAGE_WIRE_COPIES: u64 = 2;
const VIEW_IMAGE_WIRE_HEADROOM_BYTES: u64 = 4 * 1024;
const INITIAL_IMAGE_READ_CAPACITY_BYTES: u64 = 64 * 1024;

fn max_raw_bytes(max_wire_bytes: u64) -> u64 {
    let fixed_bytes = DATA_URL_PREFIX_BYTES
        .saturating_mul(VIEW_IMAGE_WIRE_COPIES)
        .saturating_add(VIEW_IMAGE_WIRE_HEADROOM_BYTES);
    max_wire_bytes
        .saturating_sub(fixed_bytes)
        .checked_div(VIEW_IMAGE_WIRE_COPIES)
        .unwrap_or(0)
        .checked_div(4)
        .unwrap_or(0)
        .saturating_mul(3)
}

fn estimated_wire_bytes(raw_bytes: u64) -> u64 {
    raw_bytes
        .div_ceil(3)
        .checked_mul(4)
        .and_then(|encoded| encoded.checked_add(DATA_URL_PREFIX_BYTES))
        .and_then(|data_url| data_url.checked_mul(VIEW_IMAGE_WIRE_COPIES))
        .and_then(|duplicated| duplicated.checked_add(VIEW_IMAGE_WIRE_HEADROOM_BYTES))
        .unwrap_or(u64::MAX)
}

enum BoundedRead {
    Complete(Vec<u8>),
    TooLarge,
}

async fn read_bounded(
    reader: &mut (impl AsyncRead + Unpin),
    expected_bytes: u64,
    max_raw_bytes: u64,
) -> std::io::Result<BoundedRead> {
    let read_limit = max_raw_bytes.saturating_add(1);
    let initial_capacity = usize::try_from(
        expected_bytes
            .min(read_limit)
            .min(INITIAL_IMAGE_READ_CAPACITY_BYTES),
    )
    .unwrap_or(0);
    let mut bytes = Vec::with_capacity(initial_capacity);
    reader.take(read_limit).read_to_end(&mut bytes).await?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_raw_bytes {
        Ok(BoundedRead::TooLarge)
    } else {
        Ok(BoundedRead::Complete(bytes))
    }
}

fn oversized_image_error(path: &Path, raw_bytes: u64, max_wire_bytes: u64) -> ToolOutput {
    let estimated_wire_bytes = estimated_wire_bytes(raw_bytes);
    ToolOutput::error(format!(
        "image at `{}` is {raw_bytes} bytes and its VM transfer would require at least \
         {estimated_wire_bytes} bytes, exceeding the {max_wire_bytes}-byte VM tool frame limit; \
         resize or convert it to a compact PNG, JPEG, or WebP inside the VM (for example with \
         ffmpeg or ImageMagick), then call view_image on the smaller file",
        path.display(),
    ))
}

fn growing_image_error(path: &Path, max_raw_bytes: u64, max_wire_bytes: u64) -> ToolOutput {
    ToolOutput::error(format!(
        "image at `{}` exceeded the {max_raw_bytes}-byte VM raw-image limit while it was read \
         (the file may have grown), so its response cannot fit the {max_wire_bytes}-byte VM tool \
         frame; resize or convert it to a compact PNG, JPEG, or WebP inside the VM (for example \
         with ffmpeg or ImageMagick), then call view_image on the smaller file",
        path.display(),
    ))
}

fn read_error(path: &Path, error: std::io::Error) -> ToolOutput {
    ToolOutput::error(format!(
        "unable to read image at `{}`: {error}",
        path.display()
    ))
}

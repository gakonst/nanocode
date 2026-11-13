use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nanocode_process_contracts::FileHost;
use nanocode_tools::ToolDefinition;
use serde::Deserialize;
use serde_json::json;

use super::super::{
    ImageDetail, StandardTool, Tool, ToolContext, ToolInput, ToolOutput, ToolOutputContent,
    ToolResult,
};

const fn media_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, 0xff, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        _ => "application/octet-stream",
    }
}

pub(crate) struct HostViewImageHandler {
    workspace: PathBuf,
    files: Arc<dyn FileHost>,
}

impl HostViewImageHandler {
    pub(crate) const fn new(workspace: PathBuf, files: Arc<dyn FileHost>) -> Self {
        Self { workspace, files }
    }
}

impl Tool for HostViewImageHandler {
    fn definition(&self) -> ToolDefinition {
        StandardTool::ViewImage.definition()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        _context: ToolContext<'life1>,
    ) -> nanocode_tools::ToolCallFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let output: ToolResult = {
                let arguments = input.decode_json::<ViewImageArguments>()?;
                let detail = match detail(arguments.detail.as_deref()) {
                    Ok(detail) => detail,
                    Err(error) => return Ok(error),
                };
                let path = resolve(&self.workspace, Path::new(&arguments.path));
                let bytes = match self.files.read_bytes(&path).await {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => {
                        return Ok(ToolOutput::error(format!(
                            "unable to locate image at `{}`: No such file or directory",
                            path.display()
                        )));
                    }
                    Err(error) => {
                        return Ok(ToolOutput::error(format!(
                            "unable to read image at `{}`: {error}",
                            path.display()
                        )));
                    }
                };
                Ok(output(&bytes, detail))
            };
            output
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ViewImageArguments {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) detail: Option<String>,
}

pub(crate) fn detail(detail: Option<&str>) -> Result<ImageDetail, ToolOutput> {
    match detail {
        None | Some("high") => Ok(ImageDetail::High),
        Some("original") => Ok(ImageDetail::Original),
        Some(detail) => Err(ToolOutput::error(format!(
            "view_image.detail only supports `high` or `original`; omit `detail` for default high resized behavior, got `{detail}`"
        ))),
    }
}

pub(crate) fn output(bytes: &[u8], detail: ImageDetail) -> ToolOutput {
    let prefix = format!("data:{};base64,", media_type(bytes));
    let mut image_url = String::with_capacity(
        prefix.len()
            + nanocode_sys::base64::encoded_length(bytes.len(), true)
                .expect("base64 length of an in-memory buffer fits in usize"),
    );
    image_url.push_str(&prefix);
    nanocode_sys::base64::STANDARD.encode_string(bytes, &mut image_url);
    ToolOutput::content(vec![ToolOutputContent::InputImage {
        image_url: image_url.clone(),
        detail,
    }])
    .with_structured_result(json!({
        "image_url": image_url,
        "detail": detail,
    }))
}

pub(crate) fn resolve(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    }
}

use std::{
    fmt::Display,
    future::Future,
    path::{Path, PathBuf},
};

use crate::client::responses::{
    ContentItem, FunctionOutputBody, FunctionOutputContent, ImageReference, ResponseItem,
    valid_image_file_id,
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use nanocode_tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ImageDetail, ToolOutput, ToolOutputContent};

mod host;
#[cfg(not(target_family = "wasm"))]
mod native;

pub use host::{image_generation_tool, image_generation_tool_with_images};
#[cfg(not(target_family = "wasm"))]
pub use native::ImageGenerationHandler;

const DESCRIPTION: &str = include_str!("imagegen_description.md");
const IMAGE_MODEL: &str = "gpt-image-2";
const MAX_EDIT_IMAGES: usize = 5;
const MAX_OUTPUT_HINT_BYTES: usize = 1_024;

fn definition() -> ToolDefinition {
    ToolDefinition::function(
        "image_gen__imagegen",
        DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string" },
                "transparent_background": { "type": "boolean", "default": false },
                "referenced_image_paths": {
                    "type": ["array", "null"],
                    "items": {
                        "type": "string",
                        "description": "A path that is guaranteed to be absolute and normalized (though it is not guaranteed to be canonicalized or exist on the filesystem).\n\nIMPORTANT: When deserializing an `AbsolutePathBuf`, a base path must be set using [AbsolutePathBufGuard::new]. If no base path is set, the deserialization will fail unless the path being deserialized is already absolute."
                    }
                },
                "num_last_images_to_include": {
                    "type": ["integer", "null"]
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        }),
    )
}

fn parse_args(input: &str) -> Result<ImagegenArgs, ToolOutput> {
    serde_json::from_str::<ImagegenArgs>(input).map_err(|error| {
        ToolOutput::error(format!(
            "failed to parse image_gen.imagegen arguments: {error}"
        ))
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagegenArgs {
    prompt: String,
    #[serde(default)]
    transparent_background: bool,
    #[serde(default)]
    referenced_image_paths: Option<Vec<PathBuf>>,
    #[serde(default)]
    num_last_images_to_include: Option<usize>,
}

#[derive(Debug, PartialEq, Serialize)]
struct ImageGenerationRequest {
    prompt: String,
    background: &'static str,
    model: &'static str,
    quality: &'static str,
    size: &'static str,
}

#[derive(Debug, PartialEq, Serialize)]
struct ImageEditRequest {
    images: Vec<ImageReference>,
    prompt: String,
    background: &'static str,
    model: &'static str,
    quality: &'static str,
    size: &'static str,
}

#[derive(Deserialize)]
struct ImageResponse {
    #[serde(default)]
    generation_id: Option<String>,
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    generation_id: Option<String>,
}

struct ImageRequestFailure {
    message: String,
    imagegen_request_id: Option<String>,
    generation_id: Option<String>,
}
impl From<String> for ImageRequestFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            imagegen_request_id: None,
            generation_id: None,
        }
    }
}
impl ImageRequestFailure {
    fn output(self) -> ToolOutput {
        let metadata = image_ids(self.imagegen_request_id, self.generation_id);
        let mut result = metadata.clone();
        result["error"] = Value::String(self.message.clone());
        ToolOutput::error(self.message)
            .with_structured_result(result)
            .with_metadata(metadata)
    }
}
fn safe_image_id(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')))
    .then(|| value.to_owned())
}
fn image_ids(request_id: Option<String>, generation_id: Option<String>) -> Value {
    let mut result = json!({});
    if let Some(id) = request_id {
        result["imagegen_request_id"] = Value::String(id);
    }
    if let Some(id) = generation_id {
        result["generation_id"] = Value::String(id);
    }
    result
}

fn decode_response(
    success: bool,
    status: impl Display,
    body: &[u8],
    imagegen_request_id: Option<String>,
    operation: &str,
) -> Result<(ImageResponse, Option<String>), ImageRequestFailure> {
    let generation_id = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("generation_id")
                .and_then(Value::as_str)
                .and_then(safe_image_id)
        });
    if !success {
        return Err(ImageRequestFailure {
            message: format!("{operation} returned HTTP {status}"),
            imagegen_request_id,
            generation_id,
        });
    }
    let parsed = serde_json::from_slice(body).map_err(|_| ImageRequestFailure {
        message: format!("failed to decode {operation} response"),
        imagegen_request_id: imagegen_request_id.clone(),
        generation_id,
    })?;
    Ok((parsed, imagegen_request_id))
}

async fn response_output<F, Fut>(
    response: ImageResponse,
    imagegen_request_id: Option<String>,
    save_root: &Path,
    session_id: &str,
    call_id: &str,
    write: F,
) -> ToolOutput
where
    F: FnOnce(PathBuf, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<PathBuf, String>>,
{
    let generation_id = response
        .generation_id
        .as_deref()
        .and_then(safe_image_id)
        .or_else(|| {
            response
                .data
                .first()
                .and_then(|data| data.generation_id.as_deref())
                .and_then(safe_image_id)
        });
    let metadata = image_ids(imagegen_request_id.clone(), generation_id.clone());
    let Some(data) = response.data.into_iter().next() else {
        return ImageRequestFailure {
            message: "image generation returned no image data".to_owned(),
            imagegen_request_id,
            generation_id,
        }
        .output();
    };
    let mut structured_result = metadata.clone();
    let mut output_items = Vec::new();
    match (data.b64_json, data.file_id) {
        (Some(result), None) if !result.is_empty() => {
            let saved_path = save_result(save_root, session_id, call_id, &result, write)
                .await
                .ok();
            let image_url = format!("data:image/png;base64,{result}");
            output_items.push(ToolOutputContent::InputImage {
                image_url: image_url.clone(),
                detail: ImageDetail::High,
            });
            structured_result["image_url"] = Value::String(image_url);
            if let Some(output_hint) = saved_path.as_ref().and_then(|path| image_output_hint(path))
            {
                output_items.push(ToolOutputContent::InputText {
                    text: output_hint.clone(),
                });
                structured_result["output_hint"] = Value::String(output_hint);
            }
        }
        (None, Some(file_id)) if valid_image_file_id(&file_id) => {
            output_items.push(ToolOutputContent::InputImageFile {
                file_id: file_id.clone(),
                detail: ImageDetail::High,
            });
            structured_result["file_id"] = Value::String(file_id);
        }
        _ => {
            return ImageRequestFailure {
                message: "image generation returned an invalid image reference".to_owned(),
                imagegen_request_id,
                generation_id,
            }
            .output();
        }
    }
    ToolOutput::content(output_items)
        .with_structured_result(structured_result)
        .with_metadata(metadata)
}

#[derive(Debug, PartialEq)]
enum ImageRequest {
    Generate(ImageGenerationRequest),
    Edit(ImageEditRequest),
}

async fn request_for_args<F, Fut, G, GFut>(
    args: &ImagegenArgs,
    history: &[ResponseItem],
    local_image_url: F,
    inline_image: G,
) -> Result<ImageRequest, String>
where
    F: Fn(PathBuf) -> Fut,
    Fut: Future<Output = Result<String, String>>,
    G: Fn(String) -> GFut,
    GFut: Future<Output = Result<String, String>>,
{
    let background = if args.transparent_background {
        "transparent"
    } else {
        "opaque"
    };
    let paths = args.referenced_image_paths.as_deref().unwrap_or_default();
    if paths.len() > MAX_EDIT_IMAGES {
        return Err(format!(
            "`referenced_image_paths` must contain at most {MAX_EDIT_IMAGES} paths"
        ));
    }
    let images = match (paths.is_empty(), args.num_last_images_to_include) {
        (true, None) => {
            return Ok(ImageRequest::Generate(ImageGenerationRequest {
                prompt: args.prompt.clone(),
                background,
                model: IMAGE_MODEL,
                quality: "auto",
                size: "auto",
            }));
        }
        (false, None) => {
            let mut images = Vec::with_capacity(paths.len());
            for path in paths {
                if !(path.is_absolute() || cfg!(target_family = "wasm") && path.has_root()) {
                    return Err(format!(
                        "referenced image path `{}` must be absolute",
                        path.display()
                    ));
                }
                images.push(ImageReference::Inline {
                    image_url: local_image_url(path.clone()).await?,
                });
            }
            images
        }
        (true, Some(count)) => {
            if !(1..=MAX_EDIT_IMAGES).contains(&count) {
                return Err(format!(
                    "`num_last_images_to_include` must be between 1 and {MAX_EDIT_IMAGES}"
                ));
            }
            let mut images = recent_images(history, count);
            if images.len() != count {
                return Err(format!(
                    "requested the last {count} conversation images, but only {} were available",
                    images.len()
                ));
            }
            for image in &mut images {
                if let ImageReference::Inline { image_url } = image {
                    *image_url = inline_image(std::mem::take(image_url)).await?;
                }
            }
            images
        }
        (false, Some(_)) => {
            return Err(
                "provide only one of `referenced_image_paths` or `num_last_images_to_include`"
                    .to_owned(),
            );
        }
    };

    for image in &images {
        match image {
            ImageReference::File { file_id } if valid_image_file_id(file_id) => {}
            ImageReference::Inline { image_url } if image_url.starts_with("data:image/") => {}
            _ => {
                return Err(
                    "selected conversation image has an invalid or unsupported reference"
                        .to_owned(),
                );
            }
        }
    }
    Ok(ImageRequest::Edit(ImageEditRequest {
        images,
        prompt: args.prompt.clone(),
        background,
        model: IMAGE_MODEL,
        quality: "auto",
        size: "auto",
    }))
}

fn recent_images(history: &[ResponseItem], count: usize) -> Vec<ImageReference> {
    let mut images = Vec::with_capacity(count);
    'history: for item in history.iter().rev() {
        let references: Vec<ImageReference> = match item {
            ResponseItem::Message { content, .. } => {
                content.iter().rev().filter_map(content_image).collect()
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => output_images(output).collect(),
            ResponseItem::ImageGenerationCall { result, .. } if !result.is_empty() => {
                vec![ImageReference::Inline {
                    image_url: format!("data:image/png;base64,{result}"),
                }]
            }
            _ => Vec::new(),
        };
        for reference in references {
            images.push(reference);
            if images.len() == count {
                break 'history;
            }
        }
    }
    images.reverse();
    images
}
fn output_images(output: &FunctionOutputBody) -> impl Iterator<Item = ImageReference> + '_ {
    let content = match output {
        FunctionOutputBody::Content(content) => Some(content.as_slice()),
        FunctionOutputBody::Text(_) => None,
    };
    content
        .into_iter()
        .flatten()
        .rev()
        .filter_map(|content| match content {
            FunctionOutputContent::InputImage { image_url, .. } => Some(ImageReference::Inline {
                image_url: image_url.to_string(),
            }),
            FunctionOutputContent::InputImageFile { file_id, .. } => Some(ImageReference::File {
                file_id: file_id.to_string(),
            }),
            _ => None,
        })
}
fn content_image(item: &ContentItem) -> Option<ImageReference> {
    match item {
        ContentItem::InputImage { image_url, .. } => Some(ImageReference::Inline {
            image_url: image_url.to_string(),
        }),
        ContentItem::InputImageFile { file_id, .. } => Some(ImageReference::File {
            file_id: file_id.to_string(),
        }),
        _ => None,
    }
}

async fn save_result<F, Fut>(
    save_root: &Path,
    session_id: &str,
    call_id: &str,
    result: &str,
    write: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(PathBuf, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<PathBuf, String>>,
{
    let bytes = BASE64_STANDARD
        .decode(result.trim().as_bytes())
        .map_err(|error| format!("generated image was not valid base64: {error}"))?;
    write(artifact_path(save_root, session_id, call_id), bytes).await
}

fn artifact_path(save_root: &Path, session_id: &str, call_id: &str) -> PathBuf {
    save_root
        .join("generated_images")
        .join(sanitize_path_component(session_id))
        .join(format!("{}.png", sanitize_path_component(call_id)))
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "generated_image".to_owned()
    } else {
        sanitized
    }
}

fn image_output_hint(path: &Path) -> Option<String> {
    let output_dir = path.parent()?;
    let hint = format!(
        "Generated images are saved to {} as {} by default.\nIf you need to use a generated image at another path, copy it and leave the original in place unless the user explicitly asks you to delete it.\nThe generated image is already displayed to the user. There is no need to render it in the final response as a Markdown image or file link.",
        output_dir.display(),
        path.display()
    );
    (hint.len() <= MAX_OUTPUT_HINT_BYTES).then_some(hint)
}

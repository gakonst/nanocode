use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use nanocode_process_contracts::FileHost;
use nanocode_tool_host::image::ImageHistory;
use nanocode_tools::ToolDefinition;
use serde::Serialize;

use crate::client::transport::host::{HostConnectRequest, HostError, HostTransport};

use super::{
    super::{Tool, ToolContext, ToolInput, ToolOutput, ToolResult},
    ImageRequest, ImageRequestFailure, ImageResponse, decode_response, definition, parse_args,
    request_for_args, response_output,
};

const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

struct HostImageGeneration {
    generation_endpoint: String,
    edit_endpoint: String,
    transport: Arc<dyn HostTransport>,
    files: Arc<dyn FileHost>,
    workspace: PathBuf,
    images: ImageHistory,
}

#[must_use]
pub fn image_generation_tool(
    api_base_url: &str,
    transport: Arc<dyn HostTransport>,
    files: Arc<dyn FileHost>,
    workspace: PathBuf,
) -> impl Tool {
    image_generation_tool_with_images(
        api_base_url,
        transport,
        files,
        workspace,
        ImageHistory::default(),
    )
}

#[must_use]
pub fn image_generation_tool_with_images(
    api_base_url: &str,
    transport: Arc<dyn HostTransport>,
    files: Arc<dyn FileHost>,
    workspace: PathBuf,
    images: ImageHistory,
) -> impl Tool {
    let api_base_url = api_base_url.trim_end_matches('/');
    HostImageGeneration {
        generation_endpoint: format!("{api_base_url}/images/generations"),
        edit_endpoint: format!("{api_base_url}/images/edits"),
        transport,
        files,
        workspace,
        images,
    }
}

impl HostImageGeneration {
    async fn run(&self, input: &str, context: ToolContext<'_>) -> ToolOutput {
        let args = match parse_args(input) {
            Ok(args) => args,
            Err(error) => return error,
        };
        let local_image_url = |path: PathBuf| async move { self.local_image_url(&path).await };
        let inline_image = |url: String| async move {
            match self.images.inline(&url).await {
                Ok(Some(data)) => Ok(data),
                Ok(None) => Ok(url),
                Err(error) => Err(format!("unable to load conversation image: {error}")),
            }
        };
        let request =
            match request_for_args(&args, context.history(), local_image_url, inline_image).await {
                Ok(request) => request,
                Err(error) => return ToolOutput::error(error),
            };
        let session = context.session_id();
        let response = match request {
            ImageRequest::Generate(request) => {
                self.post(
                    session,
                    &self.generation_endpoint,
                    &request,
                    "image generation",
                )
                .await
            }
            ImageRequest::Edit(request) => {
                self.post(session, &self.edit_endpoint, &request, "image edit")
                    .await
            }
        };
        let (response, imagegen_request_id) = match response {
            Ok(response) => response,
            Err(error) => return error.output(),
        };
        let files = &self.files;
        response_output(
            response,
            imagegen_request_id,
            &self.workspace,
            session,
            context.call_id(),
            |path: PathBuf, bytes: Vec<u8>| async move {
                files.write_bytes(&path, &bytes).await?;
                Ok(path)
            },
        )
        .await
    }

    async fn post<R: Serialize + ?Sized>(
        &self,
        session: &str,
        endpoint: &str,
        request: &R,
        operation: &str,
    ) -> Result<(ImageResponse, Option<String>), ImageRequestFailure> {
        let body = serde_json::to_string(request)
            .map_err(|error| format!("image request failed: {error}"))?;
        let request = HostConnectRequest::new(endpoint, "", None, false, session, None);
        let mut response = match self.transport.http(request, &body).await {
            Ok(response) => response,
            Err(HostError::HandshakeRejected { status, body, .. }) => {
                return decode_response(
                    false,
                    status_text(status),
                    body.as_bytes(),
                    None,
                    operation,
                );
            }
            Err(error) => return Err(format!("image request failed: {error}").into()),
        };
        let mut bytes = Vec::new();
        loop {
            match response.body.next().await {
                Ok(Some(chunk)) if bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES => {
                    bytes.extend(chunk);
                }
                Ok(None) => break,
                Ok(Some(_)) | Err(_) => {
                    return Err(format!("failed to read {operation} response").into());
                }
            }
        }
        let status = response.metadata.status();
        decode_response(
            (200..300).contains(&status),
            status_text(status),
            &bytes,
            None,
            operation,
        )
    }

    async fn local_image_url(&self, path: &Path) -> Result<String, String> {
        let bytes = match self.files.read_bytes(path).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return Err(format!(
                    "unable to read referenced image at `{}`: No such file or directory",
                    path.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "unable to read referenced image at `{}`: {error}",
                    path.display()
                ));
            }
        };
        let mime = image::guess_format(&bytes)
            .map_err(|error| {
                format!(
                    "unable to process referenced image at `{}`: {error}",
                    path.display()
                )
            })?
            .to_mime_type();
        let mut url = format!("data:{mime};base64,");
        base64_simd::STANDARD.encode_append(&bytes, &mut url);
        Ok(url)
    }
}

fn status_text(status: u16) -> String {
    http::StatusCode::from_u16(status)
        .map_or_else(|_| status.to_string(), |status| status.to_string())
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]
impl Tool for HostImageGeneration {
    fn definition(&self) -> ToolDefinition {
        definition()
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input = input.function_json()?;
        Ok(self.run(input.get(), context).await)
    }
}

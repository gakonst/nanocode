use std::path::PathBuf;

use crate::client::auth::{OpenAiAuth, OpenAiAuthMode, OpenAiAuthSnapshot};
use nanocode_tools::ToolDefinition;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use serde::Serialize;

use super::{
    super::{
        ImageDetail, ImageGenerationConfig, Tool, ToolContext, ToolInput, ToolOutput, ToolResult,
        image::load_for_prompt_data_url,
    },
    ImageRequest, ImageRequestFailure, ImageResponse, decode_response, definition, parse_args,
    request_for_args, response_output, safe_image_id,
};

pub struct ImageGenerationHandler {
    client: reqwest::Client,
    generation_endpoint: String,
    edit_endpoint: String,
    auth: OpenAiAuth,
    save_root: PathBuf,
    http_layer: Option<std::sync::Arc<dyn nanocode_http::layer::Layer>>,
}

impl ImageGenerationHandler {
    pub fn with_client(config: ImageGenerationConfig, client: reqwest::Client) -> Self {
        let api_base_url = config.api_base_url.trim_end_matches('/');
        Self {
            client,
            generation_endpoint: format!("{api_base_url}/images/generations"),
            edit_endpoint: format!("{api_base_url}/images/edits"),
            auth: config.auth,
            save_root: config.save_root,
            http_layer: config.http_layer,
        }
    }

    async fn run(&self, input: &str, context: ToolContext<'_>) -> ToolOutput {
        let args = match parse_args(input) {
            Ok(args) => args,
            Err(error) => return error,
        };
        let request = match request_for_args(
            &args,
            context.history(),
            local_image_url,
            |url| async move { Ok(url) },
        )
        .await
        {
            Ok(request) => request,
            Err(error) => return ToolOutput::error(error),
        };
        let response = match request {
            ImageRequest::Generate(request) => {
                self.post_image_request(&self.generation_endpoint, &request, "image generation")
                    .await
            }
            ImageRequest::Edit(request) => {
                self.post_image_request(&self.edit_endpoint, &request, "image edit")
                    .await
            }
        };
        let (response, imagegen_request_id) = match response {
            Ok(response) => response,
            Err(error) => return error.output(),
        };
        response_output(
            response,
            imagegen_request_id,
            &self.save_root,
            context.session_id(),
            context.call_id(),
            write_image,
        )
        .await
    }

    async fn post_image_request<R: Serialize + ?Sized>(
        &self,
        endpoint: &str,
        request: &R,
        operation: &str,
    ) -> Result<(ImageResponse, Option<String>), ImageRequestFailure> {
        let auth = self
            .auth
            .snapshot()
            .await
            .map_err(|error| error.to_string())?;
        let response = self.send_authorized(endpoint, request, &auth).await?;
        let response = if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && auth.mode() == OpenAiAuthMode::ChatGpt
        {
            self.auth
                .recover_unauthorized(&auth)
                .await
                .map_err(|error| error.to_string())?;
            let refreshed = self
                .auth
                .snapshot()
                .await
                .map_err(|error| error.to_string())?;
            self.send_authorized(endpoint, request, &refreshed).await?
        } else {
            response
        };
        let status = response.status();
        let imagegen_request_id = response
            .headers()
            .get("x-codex-imagegen-request-id")
            .and_then(|value| value.to_str().ok())
            .and_then(safe_image_id);
        let body = response.bytes().await.map_err(|_| ImageRequestFailure {
            message: format!("failed to read {operation} response"),
            imagegen_request_id: imagegen_request_id.clone(),
            generation_id: None,
        })?;
        decode_response(
            status.is_success(),
            status,
            &body,
            imagegen_request_id,
            operation,
        )
    }

    async fn send_authorized<R: Serialize + ?Sized>(
        &self,
        endpoint: &str,
        body: &R,
        auth: &OpenAiAuthSnapshot,
    ) -> Result<reqwest::Response, String> {
        let mut request = self
            .client
            .post(endpoint)
            .header(USER_AGENT, concat!("nanocode/", env!("CARGO_PKG_VERSION")))
            .header(AUTHORIZATION, format!("Bearer {}", auth.bearer()));
        if let Some(account_id) = auth.account_id() {
            request = request.header("ChatGPT-Account-ID", account_id);
        }
        if auth.is_fedramp() {
            request = request.header("X-OpenAI-Fedramp", "true");
        }
        let request = request
            .json(body)
            .build()
            .map_err(|error| format!("image request failed: {error}"))?;
        nanocode_http::execute(&self.client, self.http_layer.as_deref(), request)
            .await
            .map_err(|error| format!("image request failed: {error}"))
    }
}

#[async_trait::async_trait]
impl Tool for ImageGenerationHandler {
    fn definition(&self) -> ToolDefinition {
        definition()
    }

    async fn execute(&self, input: ToolInput, context: ToolContext<'_>) -> ToolResult {
        let input = input.function_json()?;
        Ok(self.run(input.get(), context).await)
    }
}

async fn write_image(path: PathBuf, bytes: Vec<u8>) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("generated image path `{}` has no parent", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| format!("unable to create `{}`: {error}", parent.display()))?;
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|error| format!("unable to write `{}`: {error}", path.display()))?;
    Ok(path)
}

async fn local_image_url(path: PathBuf) -> Result<String, String> {
    let bytes = tokio::fs::read(&path).await.map_err(|error| {
        format!(
            "unable to read referenced image at `{}`: {error}",
            path.display()
        )
    })?;
    let display_path = path.clone();
    tokio::task::spawn_blocking(move || {
        load_for_prompt_data_url(&path, bytes, ImageDetail::Original)
    })
    .await
    .map_err(|error| {
        format!(
            "unable to process referenced image at `{}`: {error}",
            display_path.display()
        )
    })?
    .map_err(|error| {
        format!(
            "unable to process referenced image at `{}`: {error}",
            display_path.display()
        )
    })
}

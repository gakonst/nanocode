use std::{future::Future, marker::PhantomData, sync::Arc};

pub use nanocode_platform::MaybeSend;
use nanocode_tools::{ToolInputError, ToolOutputContent, schema_for};
use schemars::JsonSchema;
use serde::de::{DeserializeOwned, Error as _};
use serde_json::{Map, Value, json};

use crate::{
    Tool, ToolContext, ToolDefinition, ToolInput, ToolOutput, ToolResult, embedded::HostFuture,
};

pub trait FunctionHandler: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        name: &'a str,
        input: Value,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Value, String>>;

    fn invalid_input(&self, _name: &str, _error: &str) -> Option<Result<Value, String>> {
        None
    }
}

pub trait TypedHandler<Args, Error>: Send + Sync + 'static {
    fn call(
        &self,
        args: Args,
        context: ToolContext<'_>,
    ) -> impl Future<Output = Result<Value, Error>> + MaybeSend;

    fn failed(&self, error: Error) -> Failure;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    Error(String),
    Result(Value),
}

impl From<Failure> for Result<Value, String> {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Error(text) => Err(text),
            Failure::Result(value) => Ok(value),
        }
    }
}

struct Typed<H, Args, Error>(H, PhantomData<fn() -> (Args, Error)>);

trait TypedCall: Send + Sync + 'static {
    fn call<'a>(
        &'a self,
        input: &ToolInput,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Value, String>>;
}

impl<H, Args, Error> TypedCall for Typed<H, Args, Error>
where
    H: TypedHandler<Args, Error>,
    Args: DeserializeOwned + 'static,
    Error: From<serde_json::Error> + 'static,
{
    fn call<'a>(
        &'a self,
        input: &ToolInput,
        context: ToolContext<'a>,
    ) -> HostFuture<'a, Result<Value, String>> {
        let call = match input.decode_json::<Args>() {
            Ok(args) => Ok(self.0.call(args, context)),
            Err(ToolInputError::Decode(error)) => Err(self.0.failed(error.into())),
            Err(error) => Err(self.0.failed(serde_json::Error::custom(error).into())),
        };
        Box::pin(async move {
            match call {
                Ok(call) => call.await.or_else(|error| self.0.failed(error).into()),
                Err(failure) => failure.into(),
            }
        })
    }
}

enum Handler {
    Json(Arc<dyn FunctionHandler>),
    Typed(Arc<dyn TypedCall>),
}

pub struct FunctionTool {
    name: String,
    description: String,
    parameters: Value,
    handler: Handler,
    parallel: bool,
    content_results: bool,
    output_schema: Option<Value>,
}

impl FunctionTool {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: Arc<dyn FunctionHandler>,
    ) -> Self {
        Self::with_handler(name, description, parameters, Handler::Json(handler))
    }

    fn with_handler(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: Handler,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler,
            parallel: false,
            content_results: false,
            output_schema: None,
        }
    }

    #[must_use]
    pub fn typed<Args, Error>(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: impl TypedHandler<Args, Error>,
    ) -> Self
    where
        Args: DeserializeOwned + JsonSchema + 'static,
        Error: From<serde_json::Error> + 'static,
    {
        Self::typed_with_parameters(name, description, schema_for::<Args>(), handler)
    }

    #[must_use]
    pub fn typed_with_parameters<Args, Error>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: impl TypedHandler<Args, Error>,
    ) -> Self
    where
        Args: DeserializeOwned + 'static,
        Error: From<serde_json::Error> + 'static,
    {
        let handler = Handler::Typed(Arc::new(Typed(handler, PhantomData)));
        Self::with_handler(name, description, parameters, handler)
    }

    #[must_use]
    pub const fn parallel_tool_calls(mut self, parallel: bool) -> Self {
        self.parallel = parallel;
        self
    }

    #[must_use]
    pub const fn content_results(mut self, enabled: bool) -> Self {
        self.content_results = enabled;
        self
    }

    #[must_use]
    pub fn output_schema(mut self, schema: Value) -> Self {
        self.output_schema = Some(schema);
        self
    }

    fn output(&self, output: Value) -> ToolOutput {
        if !self.content_results {
            return match output {
                Value::String(output) => ToolOutput::text(output),
                output => ToolOutput::from_json(output, true),
            };
        }
        match output {
            Value::Object(mut fields) if fields.contains_key("content") => {
                let structured = fields.remove("structured_result");
                let content = fields.remove("content").unwrap_or_default();
                if let Some(field) = fields.keys().next() {
                    return ToolOutput::error(format!(
                        "{} returned a content result with the unknown field {field}",
                        self.name
                    ));
                }
                match serde_json::from_value::<Vec<ToolOutputContent>>(content) {
                    Ok(content) => {
                        let result = call_tool_result(&content, structured);
                        ToolOutput::content(content).with_structured_result(result)
                    }
                    Err(error) => ToolOutput::error(format!(
                        "{} returned an invalid content result: {error}",
                        self.name
                    )),
                }
            }
            Value::String(text) => {
                let result =
                    call_tool_result(&[ToolOutputContent::InputText { text: text.clone() }], None);
                ToolOutput::text(text).with_structured_result(result)
            }
            output => {
                let result = call_tool_result(
                    &[ToolOutputContent::InputText {
                        text: output.to_string(),
                    }],
                    Some(output.clone()),
                );
                ToolOutput::from_json(output, true).with_structured_result(result)
            }
        }
    }
}

fn call_tool_result(content: &[ToolOutputContent], structured: Option<Value>) -> Value {
    let blocks = content
        .iter()
        .filter_map(|item| match item {
            ToolOutputContent::InputText { text } => Some(json!({"type": "text", "text": text})),
            ToolOutputContent::InputImage { image_url, .. } => {
                Some(media_block("image", image_url))
            }
            ToolOutputContent::InputAudio { audio_url } => Some(media_block("audio", audio_url)),
            ToolOutputContent::InputImageFile { file_id, detail } => {
                Some(json!({"type": "input_image", "file_id": file_id, "detail": detail}))
            }
            ToolOutputContent::EncryptedContent { .. } => None,
        })
        .collect::<Vec<_>>();
    let mut result = Map::new();
    result.insert("content".to_owned(), Value::Array(blocks));
    if let Some(structured) = structured {
        result.insert("structuredContent".to_owned(), structured);
    }
    Value::Object(result)
}

fn media_block(kind: &str, url: &str) -> Value {
    let encoded = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .and_then(|(header, data)| Some((header.strip_suffix(";base64")?, data)));
    match encoded {
        Some((mime_type, data)) => json!({"type": kind, "data": data, "mimeType": mime_type}),
        None => json!({"type": "text", "text": url}),
    }
}

impl Tool for FunctionTool {
    fn definition(&self) -> ToolDefinition {
        let definition = ToolDefinition::function(
            self.name.as_str(),
            self.description.as_str(),
            self.parameters.clone(),
        );
        if !self.content_results {
            return match &self.output_schema {
                Some(schema) => definition.with_output_schema(schema.clone()),
                None => definition,
            };
        }
        definition.with_output_schema(json!({
            "type": "object",
            "properties": {
                "content": {"type": "array", "items": {"type": "object"}},
                "structuredContent": {},
                "isError": {"type": "boolean"},
                "_meta": {"type": "object"}
            },
            "required": ["content"],
            "additionalProperties": false
        }))
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.parallel
    }

    fn execute<'life0, 'life1, 'async_trait>(
        &'life0 self,
        input: ToolInput,
        context: ToolContext<'life1>,
    ) -> HostFuture<'async_trait, ToolResult>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let result = match &self.handler {
                Handler::Typed(handler) => handler.call(&input, context).await,
                Handler::Json(handler) => match input.decode_json() {
                    Ok(input) => handler.call(&self.name, input, context).await,
                    Err(error) => match handler.invalid_input(&self.name, &error.to_string()) {
                        Some(result) => result,
                        None => return Err(error.into()),
                    },
                },
            };
            Ok(match result {
                Ok(output) => self.output(output),
                Err(error) => ToolOutput::error(error),
            })
        })
    }
}

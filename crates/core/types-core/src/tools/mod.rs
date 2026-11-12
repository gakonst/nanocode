mod image_validation;
pub use image_validation::valid_tool_image_data_url;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{
    Value,
    value::{RawValue, to_raw_value},
};

pub use crate::responses::ToolDefinition;

use crate::ImageDetail;

pub const DEFAULT_TOOL_OUTPUT_TOKENS: usize = usize::MAX;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ToolOutputBody {
    Text(String),

    Content(Vec<ToolOutputContent>),
}

impl ToolOutputBody {
    pub fn replace_invalid_image_envelopes(&mut self) {
        let Self::Content(content) = self else {
            return;
        };
        for item in content {
            let invalid = match item {
                ToolOutputContent::InputImage { image_url, .. } => {
                    !valid_tool_image_data_url(image_url)
                }
                ToolOutputContent::InputImageFile { file_id, .. } => {
                    !crate::responses::valid_image_file_id(file_id)
                }
                _ => false,
            };
            if invalid {
                let text = if matches!(item, ToolOutputContent::InputImageFile { .. }) {
                    "image content omitted because its file ID was malformed"
                } else {
                    "image content omitted because its data URL was malformed"
                };
                *item = ToolOutputContent::InputText {
                    text: text.to_owned(),
                };
            }
        }
    }

    #[must_use]
    pub fn structured_result(&self) -> Value {
        match self {
            Self::Text(text) => Value::String(text.clone()),
            Self::Content(content) => serde_json::to_value(content).unwrap_or(Value::Null),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputContent {
    InputText {
        text: String,
    },

    InputImage {
        image_url: String,

        detail: ImageDetail,
    },

    #[serde(rename = "input_image")]
    InputImageFile {
        #[serde(serialize_with = "crate::responses::image_reference::serialize_file_id")]
        file_id: String,

        detail: ImageDetail,
    },

    InputAudio {
        audio_url: String,
    },

    InputFile {
        file_data: String,
    },

    EncryptedContent {
        encrypted_content: String,
    },
}

pub struct ToolOutput {
    pub output: ToolOutputBody,

    pub success: bool,

    pub metadata: Option<Box<RawValue>>,
    structured_result: Option<Value>,
    process_trace: Option<ToolProcessTrace>,
    receipts: Vec<Arc<dyn ToolReceipt>>,
}

pub trait ToolReceipt: Send + Sync {
    fn carried_by(&self, call_id: Option<&str>);
}

#[doc(hidden)]
#[derive(Deserialize, Serialize)]
pub struct ToolOutputWire {
    pub output: ToolOutputBody,
    pub success: bool,
    pub structured_result: Option<Box<RawValue>>,
    pub metadata: Option<Box<RawValue>>,
    pub process_trace: Option<ToolProcessTraceWire>,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct ToolProcessTrace {
    pub exit_code: Option<i32>,
    pub session_id: Option<i64>,
    pub original_token_count: Option<usize>,
    pub output_bytes: usize,
    pub wall_time_seconds: f64,
}

#[doc(hidden)]
#[derive(Deserialize, Serialize)]
pub struct ToolProcessTraceWire {
    pub exit_code: Option<i32>,
    pub session_id: Option<i64>,
    pub original_token_count: Option<usize>,
    pub output_bytes: usize,
    pub wall_time_seconds: f64,
}

pub type ToolError = Box<dyn std::error::Error + Send + Sync + 'static>;

pub type ToolResult = std::result::Result<ToolOutput, ToolError>;

impl ToolOutput {
    #[must_use]
    pub fn text(output: impl Into<String>) -> Self {
        Self {
            output: ToolOutputBody::Text(output.into()),
            success: true,
            metadata: None,
            structured_result: None,
            process_trace: None,
            receipts: Vec::new(),
        }
    }

    #[must_use]
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            output: ToolOutputBody::Text(error.into()),
            success: false,
            metadata: None,
            structured_result: None,
            process_trace: None,
            receipts: Vec::new(),
        }
    }

    #[must_use]
    pub fn json(output: &impl Serialize) -> Self {
        match serde_json::to_value(output) {
            Ok(output) => Self::from_json(output, true),
            Err(error) => Self::error(format!("failed to encode tool result: {error}")),
        }
    }

    #[must_use]
    pub fn from_json(output: Value, success: bool) -> Self {
        match serde_json::to_string(&output) {
            Ok(encoded) => Self {
                output: ToolOutputBody::Text(encoded),
                success,
                metadata: None,
                structured_result: Some(output),
                process_trace: None,
                receipts: Vec::new(),
            },
            Err(error) => Self::error(format!("failed to encode tool result: {error}")),
        }
    }

    #[must_use]
    pub const fn content(output: Vec<ToolOutputContent>) -> Self {
        Self {
            output: ToolOutputBody::Content(output),
            success: true,
            metadata: None,
            structured_result: None,
            process_trace: None,
            receipts: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: impl Serialize) -> Self {
        match to_raw_value(&metadata) {
            Ok(metadata) => self.metadata = Some(metadata),
            Err(error) => {
                self.output =
                    ToolOutputBody::Text(format!("failed to encode tool result metadata: {error}"));
                self.success = false;
            }
        }
        self
    }

    #[must_use]
    pub fn structured_result(&self) -> Value {
        if let Some(value) = &self.structured_result {
            return value.clone();
        }
        self.output.structured_result()
    }

    #[doc(hidden)]
    #[must_use]
    pub fn take_structured_result(&mut self) -> Value {
        self.structured_result
            .take()
            .unwrap_or_else(|| self.output.structured_result())
    }

    #[must_use]
    pub fn with_structured_result(mut self, value: Value) -> Self {
        self.structured_result = Some(value);
        self
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn with_process_trace(
        mut self,
        exit_code: Option<i32>,
        session_id: Option<i64>,
        original_token_count: Option<usize>,
        output_bytes: usize,
        wall_time_seconds: f64,
    ) -> Self {
        self.process_trace = Some(ToolProcessTrace {
            exit_code,
            session_id,
            original_token_count,
            output_bytes,
            wall_time_seconds,
        });
        self
    }

    #[must_use]
    pub fn with_receipt(mut self, receipt: Arc<dyn ToolReceipt>) -> Self {
        self.receipts.push(receipt);
        self
    }

    pub fn take_receipts(&mut self) -> Vec<Arc<dyn ToolReceipt>> {
        std::mem::take(&mut self.receipts)
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn process_trace(&self) -> Option<&ToolProcessTrace> {
        self.process_trace.as_ref()
    }

    #[doc(hidden)]
    pub fn into_wire(self) -> Result<ToolOutputWire, serde_json::Error> {
        Ok(ToolOutputWire {
            output: self.output,
            success: self.success,
            structured_result: self
                .structured_result
                .map(|value| to_raw_value(&value))
                .transpose()?,
            metadata: self.metadata,
            process_trace: self.process_trace.map(Into::into),
        })
    }

    #[doc(hidden)]
    pub fn from_wire(wire: ToolOutputWire) -> Result<Self, serde_json::Error> {
        Ok(Self {
            output: wire.output,
            success: wire.success,
            metadata: wire.metadata,
            structured_result: wire
                .structured_result
                .map(|value| serde_json::from_str(value.get()))
                .transpose()?,
            process_trace: wire.process_trace.map(Into::into),
            receipts: Vec::new(),
        })
    }
}

impl From<ToolProcessTrace> for ToolProcessTraceWire {
    fn from(trace: ToolProcessTrace) -> Self {
        Self {
            exit_code: trace.exit_code,
            session_id: trace.session_id,
            original_token_count: trace.original_token_count,
            output_bytes: trace.output_bytes,
            wall_time_seconds: trace.wall_time_seconds,
        }
    }
}

impl From<ToolProcessTraceWire> for ToolProcessTrace {
    fn from(trace: ToolProcessTraceWire) -> Self {
        Self {
            exit_code: trace.exit_code,
            session_id: trace.session_id,
            original_token_count: trace.original_token_count,
            output_bytes: trace.output_bytes,
            wall_time_seconds: trace.wall_time_seconds,
        }
    }
}

#[derive(Deserialize)]
#[serde(remote = "ToolOutputContent", tag = "type", rename_all = "snake_case")]
enum ToolOutputContentFields {
    InputText {
        text: String,
    },

    InputImage {
        image_url: String,

        detail: ImageDetail,
    },

    InputAudio {
        audio_url: String,
    },

    InputFile {
        file_data: String,
    },

    EncryptedContent {
        encrypted_content: String,
    },
}

impl<'de> Deserialize<'de> for ToolOutputContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("type").and_then(serde_json::Value::as_str) == Some("input_image") {
            let serde_json::Value::Object(mut object) = value else {
                return Err(serde::de::Error::custom("expected image object"));
            };
            object.remove("type");

            let detail = object.remove("detail");
            let reference: crate::responses::ImageReference =
                serde_json::from_value(serde_json::Value::Object(object))
                    .map_err(serde::de::Error::custom)?;
            let detail: ImageDetail =
                serde_json::from_value(detail.unwrap_or(serde_json::Value::Null))
                    .map_err(serde::de::Error::custom)?;
            return Ok(match reference {
                crate::responses::ImageReference::Inline { image_url } => {
                    Self::InputImage { image_url, detail }
                }
                crate::responses::ImageReference::File { file_id } => {
                    Self::InputImageFile { file_id, detail }
                }
            });
        }
        ToolOutputContentFields::deserialize(value).map_err(serde::de::Error::custom)
    }
}

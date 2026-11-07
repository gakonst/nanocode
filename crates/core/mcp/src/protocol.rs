use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

pub const INITIALIZE: &str = "initialize";
pub const INITIALIZED: &str = "notifications/initialized";
pub const CANCELLED: &str = "notifications/cancelled";
pub const PING: &str = "ping";
pub const TOOLS_LIST: &str = "tools/list";
pub const TOOLS_CALL: &str = "tools/call";
pub const RESOURCES_LIST: &str = "resources/list";
pub const RESOURCE_TEMPLATES_LIST: &str = "resources/templates/list";
pub const RESOURCES_READ: &str = "resources/read";
pub const ROOTS_LIST: &str = "roots/list";
pub const ELICITATION_CREATE: &str = "elicitation/create";

pub type JsonObject = Map<String, Value>;

pub const RESUME_META_KEY: &str = "io.nanocode/resume";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Implementation {
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: JsonObject,
    pub client_info: Implementation,
}

impl InitializeParams {
    #[must_use]
    pub fn new(client_info: Implementation) -> Self {
        Self {
            protocol_version: LATEST_PROTOCOL_VERSION.to_owned(),
            capabilities: JsonObject::new(),
            client_info,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Value>,
}

impl ServerCapabilities {
    #[must_use]
    pub const fn supports_tools(&self) -> bool {
        self.tools.is_some()
    }

    #[must_use]
    pub const fn supports_resources(&self) -> bool {
        self.resources.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<Implementation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

impl Tool {
    #[must_use]
    pub fn resumable(&self) -> bool {
        self.meta
            .as_ref()
            .and_then(|meta| meta.get(RESUME_META_KEY))
            .and_then(Value::as_bool)
            == Some(true)
    }

    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.annotations
            .as_ref()
            .is_some_and(|annotations| annotations.read_only_hint == Some(true))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PaginatedParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
    pub tools: Vec<Tool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
    pub resources: Vec<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourceTemplatesResult {
    pub resource_templates: Vec<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadResourceParams {
    pub uri: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadResourceResult {
    pub contents: Vec<JsonObject>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallToolParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

impl CallToolParams {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arguments: None,
            meta: None,
        }
    }

    #[must_use]
    pub fn with_arguments(mut self, arguments: JsonObject) -> Self {
        self.arguments = Some(arguments);
        self
    }

    pub fn insert_meta(&mut self, key: impl Into<String>, value: Value) {
        self.meta
            .get_or_insert_with(JsonObject::new)
            .insert(key.into(), value);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentBlock(pub JsonObject);

pub enum ContentKind<'a> {
    Text {
        text: &'a str,
        meta: Option<&'a JsonObject>,
    },
    Image {
        data: &'a str,
        mime_type: &'a str,
        meta: Option<&'a JsonObject>,
    },
    Audio {
        data: &'a str,
        mime_type: &'a str,
        meta: Option<&'a JsonObject>,
    },
    Other,
}

impl ContentBlock {
    #[must_use]
    pub fn content_type(&self) -> Option<&str> {
        self.0.get("type").and_then(Value::as_str)
    }

    #[must_use]
    pub fn meta(&self) -> Option<&JsonObject> {
        self.0.get("_meta").and_then(Value::as_object)
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }

    #[must_use]
    pub fn kind(&self) -> ContentKind<'_> {
        let meta = self.meta();
        match self.content_type() {
            Some("text") => self
                .string("text")
                .map_or(ContentKind::Other, |text| ContentKind::Text { text, meta }),
            Some(kind @ ("image" | "audio")) => {
                match (self.string("data"), self.string("mimeType")) {
                    (Some(data), Some(mime_type)) if kind == "image" => ContentKind::Image {
                        data,
                        mime_type,
                        meta,
                    },
                    (Some(data), Some(mime_type)) => ContentKind::Audio {
                        data,
                        mime_type,
                        meta,
                    },
                    _ => ContentKind::Other,
                }
            }
            _ => ContentKind::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonObject>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resumption {
    Answered,
    NotSeen,
    Unknown,
}

impl CallToolResult {
    #[must_use]
    pub fn is_error(&self) -> bool {
        self.is_error.unwrap_or(false)
    }

    #[must_use]
    pub fn resumption(&self) -> Resumption {
        match self
            .meta
            .as_ref()
            .and_then(|meta| meta.get(RESUME_META_KEY))
            .and_then(Value::as_str)
        {
            Some("not_seen") => Resumption::NotSeen,
            Some("unknown") => Resumption::Unknown,
            _ => Resumption::Answered,
        }
    }
}

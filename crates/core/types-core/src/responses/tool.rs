use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolDefinition {
    Function {
        name: Box<str>,

        description: Box<str>,

        strict: bool,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        defer_loading: Option<bool>,

        #[serde(default, rename = "async", skip_serializing_if = "is_false")]
        asynchronous: bool,

        parameters: JsonSchema,
        #[serde(default, skip_serializing)]
        output_schema: Option<JsonSchema>,
    },

    Namespace {
        name: Box<str>,

        description: Box<str>,

        #[serde(serialize_with = "serialize_namespace_tools")]
        tools: Vec<Self>,
    },

    Custom {
        name: Box<str>,

        description: Box<str>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        defer_loading: Option<bool>,

        #[serde(default, rename = "async", skip_serializing_if = "is_false")]
        asynchronous: bool,

        format: CustomToolFormat,
    },

    ToolSearch {
        execution: Box<str>,

        description: Box<str>,

        parameters: JsonSchema,
    },
}

const fn is_false(value: &bool) -> bool {
    !*value
}

fn serialize_namespace_tools<S>(tools: &[ToolDefinition], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if tools.iter().any(|tool| {
        !matches!(
            tool,
            ToolDefinition::Function { .. } | ToolDefinition::Custom { .. }
        )
    }) {
        return Err(serde::ser::Error::custom(
            "Responses namespaces may contain only function or custom definitions",
        ));
    }
    tools.serialize(serializer)
}

impl ToolDefinition {
    #[must_use]
    pub fn function(
        name: impl Into<Box<str>>,
        description: impl Into<Box<str>>,
        parameters: impl Into<JsonSchema>,
    ) -> Self {
        Self::Function {
            name: name.into(),
            description: description.into(),
            strict: false,
            defer_loading: None,
            asynchronous: false,
            parameters: parameters.into(),
            output_schema: None,
        }
    }

    #[must_use]
    pub fn custom(
        name: impl Into<Box<str>>,
        description: impl Into<Box<str>>,
        format: CustomToolFormat,
    ) -> Self {
        Self::Custom {
            name: name.into(),
            description: description.into(),
            defer_loading: None,
            asynchronous: false,
            format,
        }
    }

    #[must_use]
    pub fn namespace(
        name: impl Into<Box<str>>,
        description: impl Into<Box<str>>,
        tools: impl IntoIterator<Item = Self>,
    ) -> Self {
        Self::Namespace {
            name: name.into(),
            description: description.into(),
            tools: tools.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn tool_search(
        execution: impl Into<Box<str>>,
        description: impl Into<Box<str>>,
        parameters: impl Into<JsonSchema>,
    ) -> Self {
        Self::ToolSearch {
            execution: execution.into(),
            description: description.into(),
            parameters: parameters.into(),
        }
    }

    #[must_use]
    pub const fn with_strict_parameters(mut self) -> Self {
        if let Self::Function { strict, .. } = &mut self {
            *strict = true;
        }
        self
    }

    #[must_use]
    pub fn with_output_schema(mut self, output_schema: impl Into<JsonSchema>) -> Self {
        if let Self::Function {
            output_schema: current,
            ..
        } = &mut self
        {
            *current = Some(output_schema.into());
        }
        self
    }

    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "ToolDefinition owns drop types that const evaluation rejects"
    )]
    pub fn with_deferred_loading(mut self) -> Self {
        match &mut self {
            Self::Function { defer_loading, .. } | Self::Custom { defer_loading, .. } => {
                *defer_loading = Some(true);
            }
            Self::Namespace { .. } | Self::ToolSearch { .. } => {}
        }
        self
    }

    #[must_use]
    #[allow(
        clippy::missing_const_for_fn,
        reason = "ToolDefinition owns drop types that const evaluation rejects"
    )]
    pub fn with_async_execution(mut self) -> Self {
        match &mut self {
            Self::Function { asynchronous, .. } | Self::Custom { asynchronous, .. } => {
                *asynchronous = true;
            }
            Self::Namespace { .. } | Self::ToolSearch { .. } => {}
        }
        self
    }

    #[must_use]
    pub const fn is_async(&self) -> bool {
        match self {
            Self::Function { asynchronous, .. } | Self::Custom { asynchronous, .. } => {
                *asynchronous
            }
            Self::Namespace { .. } | Self::ToolSearch { .. } => false,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Function { name, .. }
            | Self::Namespace { name, .. }
            | Self::Custom { name, .. } => name,
            Self::ToolSearch { .. } => "tool_search",
        }
    }

    #[must_use]
    pub fn description(&self) -> &str {
        match self {
            Self::Function { description, .. }
            | Self::Namespace { description, .. }
            | Self::Custom { description, .. } => description,
            Self::ToolSearch { description, .. } => description,
        }
    }

    #[must_use]
    pub const fn parameters(&self) -> Option<&JsonSchema> {
        match self {
            Self::Function { parameters, .. } | Self::ToolSearch { parameters, .. } => {
                Some(parameters)
            }
            Self::Namespace { .. } | Self::Custom { .. } => None,
        }
    }

    #[must_use]
    pub const fn output_schema(&self) -> Option<&JsonSchema> {
        match self {
            Self::Function { output_schema, .. } => output_schema.as_ref(),
            Self::Namespace { .. } | Self::Custom { .. } | Self::ToolSearch { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CustomToolFormat {
    #[serde(rename = "type")]
    pub kind: Box<str>,

    pub syntax: Box<str>,

    pub definition: Box<str>,
}

impl CustomToolFormat {
    #[must_use]
    pub fn grammar(syntax: impl Into<Box<str>>, definition: impl Into<Box<str>>) -> Self {
        Self {
            kind: "grammar".into(),
            syntax: syntax.into(),
            definition: definition.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct JsonSchema(std::sync::Arc<Value>);

impl JsonSchema {
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

impl From<Value> for JsonSchema {
    fn from(value: Value) -> Self {
        Self(std::sync::Arc::new(value))
    }
}

impl PartialEq for JsonSchema {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct JsonValue(std::sync::Arc<Value>);

impl JsonValue {
    #[must_use]
    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

impl From<Value> for JsonValue {
    fn from(value: Value) -> Self {
        Self(std::sync::Arc::new(value))
    }
}

impl PartialEq for JsonValue {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

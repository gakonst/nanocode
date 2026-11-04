use super::ResponseItem;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub input_tokens_details: Option<InputTokenDetails>,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub output_tokens_details: Option<OutputTokenDetails>,
    #[serde(default)]
    pub total_tokens: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<crate::pricing::ReportedCost>,

    #[serde(
        default,
        with = "crate::pricing::usd_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub cost: Option<crate::pricing::UsdAmount>,
}

impl Usage {
    #[must_use]
    pub fn provider_cost(&self) -> Option<crate::pricing::ReportedCost> {
        self.reported_cost.or_else(|| {
            self.cost.map(|usd| crate::pricing::ReportedCost {
                usd,
                input_usd: None,
            })
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct InputTokenDetails {
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OutputTokenDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[doc(hidden)]
#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum ServerEvent {
    #[serde(rename = "response.created")]
    Created {
        #[serde(default)]
        response: Option<serde_json::Value>,
    },
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        #[serde(default)]
        output_index: Option<u32>,
        item: ResponseItem,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        output_index: Option<u32>,
        delta: String,
    },
    #[serde(rename = "response.custom_tool_call_input.delta")]
    ToolCallInputDelta {
        #[serde(default)]
        item_id: Option<String>,
        #[serde(default)]
        call_id: Option<String>,
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        delta: String,
        #[serde(default)]
        summary_index: i64,
    },
    #[serde(rename = "response.reasoning_summary.delta")]
    ReasoningSummaryDelta {
        delta: String,
        #[serde(default)]
        summary_index: Option<i64>,
    },
    #[serde(rename = "response.reasoning_summary_text.done")]
    ReasoningSummaryDone {
        item_id: String,
        text: String,
        summary_index: i64,
    },
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningContentDelta { delta: String, content_index: i64 },
    #[serde(rename = "response.reasoning_summary_part.added")]
    ReasoningSummaryPartAdded { summary_index: i64 },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        #[serde(default)]
        output_index: Option<u32>,
        item: ResponseItem,
    },
    #[serde(rename = "response.completed")]
    Completed { response: CompletedResponse },
    #[serde(rename = "response.failed")]
    Failed,
    #[serde(rename = "response.incomplete")]
    Incomplete,
    #[serde(rename = "error")]
    Error,
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ResponseEvent {
    Created,

    OutputItemAdded(ResponseItem),

    OutputTextDelta(String),

    ToolCallInputDelta {
        item_id: String,

        call_id: Option<String>,

        delta: String,
    },

    ReasoningSummaryDelta {
        delta: String,

        summary_index: i64,
    },

    ReasoningSummaryDone {
        item_id: String,

        text: String,

        summary_index: i64,
    },

    ReasoningContentDelta {
        delta: String,

        content_index: i64,
    },

    ReasoningSummaryPartAdded {
        summary_index: i64,
    },

    OutputItemDone(ResponseItem),

    Completed {
        usage: Option<Usage>,

        end_turn: Option<bool>,
    },
}

impl ServerEvent {
    #[doc(hidden)]
    pub fn normalized(&self) -> Option<ResponseEvent> {
        match self {
            Self::Created { .. } => Some(ResponseEvent::Created),
            Self::OutputItemAdded { item, .. } => {
                Some(ResponseEvent::OutputItemAdded(item.clone()))
            }
            Self::OutputTextDelta { delta, .. } => {
                Some(ResponseEvent::OutputTextDelta(delta.clone()))
            }
            Self::ToolCallInputDelta {
                item_id,
                call_id,
                delta,
            } => Some(ResponseEvent::ToolCallInputDelta {
                item_id: item_id.clone().or_else(|| call_id.clone())?,
                call_id: call_id.clone(),
                delta: delta.clone(),
            }),
            Self::ReasoningSummaryTextDelta {
                delta,
                summary_index,
            }
            | Self::ReasoningSummaryDelta {
                delta,
                summary_index: Some(summary_index),
            } => Some(ResponseEvent::ReasoningSummaryDelta {
                delta: delta.clone(),
                summary_index: *summary_index,
            }),
            Self::ReasoningSummaryDone {
                item_id,
                text,
                summary_index,
            } => Some(ResponseEvent::ReasoningSummaryDone {
                item_id: item_id.clone(),
                text: text.clone(),
                summary_index: *summary_index,
            }),
            Self::ReasoningContentDelta {
                delta,
                content_index,
            } => Some(ResponseEvent::ReasoningContentDelta {
                delta: delta.clone(),
                content_index: *content_index,
            }),
            Self::ReasoningSummaryPartAdded { summary_index } => {
                Some(ResponseEvent::ReasoningSummaryPartAdded {
                    summary_index: *summary_index,
                })
            }
            Self::OutputItemDone { item, .. } => Some(ResponseEvent::OutputItemDone(item.clone())),
            Self::Completed { response } => Some(ResponseEvent::Completed {
                usage: response.usage.clone(),
                end_turn: response.end_turn,
            }),
            Self::ReasoningSummaryDelta {
                summary_index: None,
                ..
            }
            | Self::Failed
            | Self::Incomplete
            | Self::Error
            | Self::Other => None,
        }
    }
}

#[doc(hidden)]
#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum WarmupServerEvent {
    #[serde(rename = "response.created")]
    Created { response: WarmupResponse },
    #[serde(rename = "response.completed")]
    Completed { response: WarmupResponse },
    #[serde(rename = "response.failed")]
    Failed,
    #[serde(rename = "response.incomplete")]
    Incomplete,
    #[serde(rename = "error")]
    Error,
    #[serde(other)]
    Other,
}

#[doc(hidden)]
#[derive(Deserialize)]
pub struct WarmupResponse {
    pub id: String,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[doc(hidden)]
#[derive(Deserialize)]
pub struct CompletedResponse {
    pub id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "completed_status")]
    pub status: String,
    #[serde(default)]
    pub end_turn: Option<bool>,
    #[serde(default)]
    pub output: Vec<ResponseItem>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

fn completed_status() -> String {
    "completed".to_owned()
}

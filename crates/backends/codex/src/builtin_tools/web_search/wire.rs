use crate::client::responses::ResponseItem;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub(crate) struct SearchRequest<'a> {
    pub(crate) id: &'a str,
    pub(crate) model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<&'a [ResponseItem]>,
    pub(crate) commands: &'a SearchCommands,
    pub(crate) settings: SearchSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, JsonSchema)]
pub(crate) struct SearchCommands {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) search_query: Option<Vec<SearchQuery>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image_query: Option<Vec<SearchQuery>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) open: Option<Vec<OpenOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) click: Option<Vec<ClickOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) find: Option<Vec<FindOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) screenshot: Option<Vec<ScreenshotOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finance: Option<Vec<FinanceOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) weather: Option<Vec<WeatherOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sports: Option<Vec<SportsOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) time: Option<Vec<TimeOperation>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) response_length: Option<SearchResponseLength>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub(crate) struct SearchQuery {
    pub(crate) q: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) recency: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) domains: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub(crate) struct OpenOperation {
    pub(crate) ref_id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) lineno: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub(crate) struct ClickOperation {
    pub(crate) ref_id: String,

    pub(crate) id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub(crate) struct FindOperation {
    pub(crate) ref_id: String,

    pub(crate) pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub(crate) struct ScreenshotOperation {
    pub(crate) ref_id: String,

    pub(crate) pageno: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub(crate) struct FinanceOperation {
    pub(crate) ticker: String,

    pub(crate) r#type: FinanceAssetType,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) market: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FinanceAssetType {
    Equity,
    Fund,
    Crypto,
    Index,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub(crate) struct WeatherOperation {
    pub(crate) location: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) duration: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
pub(crate) struct SportsOperation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool: Option<SportsToolName>,

    pub(crate) r#fn: SportsFunction,

    pub(crate) league: SportsLeague,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) team: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) opponent: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date_from: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) date_to: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) num_games: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) locale: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsToolName {
    Sports,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsFunction {
    Schedule,
    Standings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SportsLeague {
    Nba,
    Wnba,
    Nfl,
    Nhl,
    Mlb,
    Epl,
    Ncaamb,
    Ncaawb,
    Ipl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub(crate) struct TimeOperation {
    pub(crate) utc_offset: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchResponseLength {
    Short,
    Medium,
    Long,
}

#[derive(Serialize)]
pub(crate) struct SearchSettings {
    pub(crate) allowed_callers: [&'static str; 1],
    pub(crate) external_web_access: bool,
}

#[derive(Deserialize)]
pub(crate) struct SearchResponse {
    #[serde(default, rename = "encrypted_output")]
    pub(crate) _encrypted_output: Option<String>,
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) results: Option<Vec<Value>>,
}

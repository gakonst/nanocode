use std::{borrow::Cow, sync::Arc};

use crate::client::{
    CONTEXT_WINDOW_TOKENS, Model, OpenAiAuth, ReasoningMode, ResponsesHistory, ResponsesTransport,
    Thinking, pricing::ServiceTier, responses::StrictJsonSchema,
};

const SOL_SYSTEM_PROMPT: &str = include_str!("../../prompts/sol.md");
const LUNA_SYSTEM_PROMPT: &str = include_str!("../../prompts/luna.md");
const GLM_SYSTEM_PROMPT: &str = include_str!("../../prompts/glm.md");
const ASTRA_SYSTEM_PROMPT: &str = include_str!("../../prompts/astra.md");

pub(crate) fn prefixed_model_id(prefix: Option<&str>, model: Model) -> Cow<'static, str> {
    let name = match model {
        Model::Glm53 => "glm-5.3",
        _ => model.as_str(),
    };
    match prefix {
        Some(prefix) => Cow::Owned(format!("{prefix}/{name}")),
        None => Cow::Borrowed(model.as_str()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CyberAccessProgram {
    Standard,
    DaybreakBlue,
    DaybreakRed,
}

impl CyberAccessProgram {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::DaybreakBlue => "daybreak_blue",
            Self::DaybreakRed => "daybreak_red",
        }
    }
}

#[derive(Clone)]
pub struct ModelConfig {
    pub model: Model,

    pub model_id_prefix: Option<Arc<str>>,

    pub auth: OpenAiAuth,

    pub reasoning_mode: ReasoningMode,

    pub thinking: Thinking,

    #[doc(hidden)]
    pub thinking_explicit: bool,

    pub service_tier: ServiceTier,

    pub cyber_access_program: Option<CyberAccessProgram>,

    pub model_cyber_access_programs: Vec<(Model, CyberAccessProgram)>,

    pub context_window_tokens: u64,

    pub auto_compact_token_limit: Option<u64>,

    pub compaction_thinking: Option<Thinking>,

    pub responses_transport: ResponsesTransport,

    pub websocket_warmup: bool,

    pub raw_api_events: bool,

    pub tool_input_deltas: bool,

    pub responses_history: ResponsesHistory,

    pub store_responses: bool,

    pub strict_json_schema: Option<StrictJsonSchema>,

    pub websocket_url: String,

    pub api_base_url: String,

    pub host_transport: Option<Arc<dyn crate::client::transport::host::HostTransport>>,

    pub chat_gateway: Option<Arc<crate::client::transport::ChatGateway>>,

    #[cfg(not(target_family = "wasm"))]
    pub http_layer: Option<Arc<dyn nanocode_http::layer::Layer>>,

    pub responses_gateway: Option<Arc<crate::client::transport::ResponsesGateway>>,

    pub system_prompt: Option<Arc<str>>,

    pub model_system_prompts: Vec<(Model, Arc<str>)>,

    pub additional_instructions: Option<Arc<str>>,
}

const SKILLS_SECTION_HEADING: &str = "# Using skills";
const SKILL_CATALOG_HEADING: &str = "### Available skills";

fn without_section(prompt: &str, heading: &str) -> String {
    let mut kept = String::with_capacity(prompt.len());
    let mut skipping = false;
    for line in prompt.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text.starts_with("# ") {
            skipping = text == heading;
        }
        if !skipping {
            kept.push_str(line);
        }
    }
    let trimmed = kept.trim_end().len();
    kept.truncate(trimmed);
    kept
}

impl ModelConfig {
    pub(crate) fn wire_model_id(&self, model: Model) -> Cow<'static, str> {
        prefixed_model_id(self.model_id_prefix.as_deref(), model)
    }

    #[doc(hidden)]
    pub fn retarget(&mut self, model: Model) {
        if let Some(gateway) = self.responses_gateway.as_deref()
            && self.model_id_prefix.as_deref() == gateway.model_id_prefix_for(self.model)
        {
            self.model_id_prefix = gateway.model_id_prefix_for(model).map(Arc::from);
        }
        self.model = model;
    }

    #[must_use]
    pub fn cyber_access_program_for(&self, model: Model) -> Option<CyberAccessProgram> {
        self.model_cyber_access_programs
            .iter()
            .find(|(configured, _)| *configured == model)
            .map(|(_, program)| *program)
            .or(self.cyber_access_program)
    }

    #[must_use]
    pub const fn supports_reasoning_effort_updates(&self, model: Model) -> bool {
        model.supports_reasoning_effort_updates() && self.model_id_prefix.is_none()
    }

    #[must_use]
    pub const fn orchestration() -> &'static str {
        "local_code_mode"
    }

    #[must_use]
    #[doc(hidden)]
    pub fn base_system_prompt(&self) -> Cow<'_, str> {
        let base = self.system_prompt.as_deref().unwrap_or(match self.model {
            Model::Astra | Model::Terra => ASTRA_SYSTEM_PROMPT,
            Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_) => GLM_SYSTEM_PROMPT,
            Model::Sol | Model::DaybreakBlue => SOL_SYSTEM_PROMPT,
            Model::Luna => LUNA_SYSTEM_PROMPT,
            _ => ASTRA_SYSTEM_PROMPT,
        });
        let base = if self.system_prompt.is_none()
            && matches!(self.model, Model::Kimi | Model::Mimo | Model::Custom(_))
        {
            Cow::Owned(base.replacen(
                "powered by Z.ai GLM-5.3",
                &format!("powered by {}", self.model.as_str()),
                1,
            ))
        } else {
            Cow::Borrowed(base)
        };
        if self.system_prompt.is_none()
            && matches!(
                self.model,
                Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_)
            )
            && !self
                .additional_instructions
                .as_deref()
                .is_some_and(|instructions| instructions.contains(SKILL_CATALOG_HEADING))
        {
            Cow::Owned(without_section(&base, SKILLS_SECTION_HEADING))
        } else {
            base
        }
    }

    #[must_use]
    pub fn system_prompt(&self) -> Cow<'_, str> {
        Cow::Owned(format!(
            "{}\n\n{}",
            self.base_system_prompt(),
            self.host_instructions()
        ))
    }

    #[doc(hidden)]
    pub fn host_instructions(&self) -> String {
        let mut instructions = String::new();
        if let Some(additional) = self.additional_instructions.as_deref()
            && !additional.is_empty()
        {
            instructions.push_str(additional);
            instructions.push_str("\n\n");
        }
        instructions.push_str(&format!(
            "<runtime_model_identity>\nmodel_id: {}\n\
             This is the runtime-selected model serving this agent. When asked which model \
             you are, report this model ID. Nanocode and Codex are product or harness names, \
             not the underlying model. Other agents and models listed in tools can differ \
             from this agent. Use this current runtime identity over conflicting identity \
             claims in earlier conversation or inherited instructions.\n\
             </runtime_model_identity>",
            self.model.as_str(),
        ));
        instructions
    }

    #[must_use]
    pub fn search_endpoint(&self) -> String {
        format!("{}/alpha/search", self.api_base_url.trim_end_matches('/'))
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        #[cfg(not(target_family = "wasm"))]
        {
            let mut config = Self {
                auth: OpenAiAuth::chatgpt(),
                ..Self::api_key_defaults()
            };
            let mode = config.auth.mode();
            super::apply_mode_defaults(&mut config, mode);
            config
        }
        #[cfg(target_family = "wasm")]
        Self::api_key_defaults()
    }
}

impl ModelConfig {
    fn api_key_defaults() -> Self {
        Self {
            model: Model::default(),
            model_id_prefix: None,
            auth: OpenAiAuth::api_key(String::new()),
            reasoning_mode: ReasoningMode::default(),
            thinking: Thinking::default(),
            thinking_explicit: false,
            service_tier: ServiceTier::Standard,
            cyber_access_program: None,
            context_window_tokens: CONTEXT_WINDOW_TOKENS,
            auto_compact_token_limit: None,
            compaction_thinking: None,
            responses_transport: ResponsesTransport::default(),
            websocket_warmup: false,
            raw_api_events: false,
            tool_input_deltas: false,
            responses_history: ResponsesHistory::default(),
            store_responses: false,
            strict_json_schema: None,
            websocket_url: "wss://api.openai.com/v1/responses".to_owned(),
            api_base_url: "https://api.openai.com/v1".to_owned(),
            host_transport: None,
            chat_gateway: None,
            #[cfg(not(target_family = "wasm"))]
            http_layer: None,
            responses_gateway: None,
            system_prompt: None,
            model_system_prompts: Vec::new(),
            model_cyber_access_programs: Vec::new(),
            additional_instructions: None,
        }
    }
}

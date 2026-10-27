use std::{fmt, str::FromStr};

use nanocode_types_core::{BackendId, Model, ReasoningMode, Thinking};

use crate::{NanocodeError, ServiceTier};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HarnessFamily(nanocode_types_core::BackendId);
impl HarnessFamily {
    #[allow(non_upper_case_globals)]
    pub const Codex: Self = Self(nanocode_types_core::BackendId::Codex);

    #[allow(non_upper_case_globals)]
    pub const Claude: Self = Self(nanocode_types_core::BackendId::Claude);

    pub fn new(value: impl Into<String>) -> Self {
        Self(nanocode_types_core::BackendId::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub const fn builtin() -> [Self; 2] {
        [Self::Codex, Self::Claude]
    }

    pub fn models(&self) -> impl Iterator<Item = ModelSelection> + use<> {
        ModelSelection::for_family(self.clone())
    }

    pub fn default_model(&self) -> ModelSelection {
        if *self == Self::Codex {
            ModelSelection::Codex(Model::Sol)
        } else if *self == Self::Claude {
            ModelSelection::Claude(ClaudeModel::Opus55)
        } else {
            ModelSelection::new(self.clone(), "default")
        }
    }
}
impl From<nanocode_types_core::BackendId> for HarnessFamily {
    fn from(value: nanocode_types_core::BackendId) -> Self {
        Self(value)
    }
}
impl From<HarnessFamily> for nanocode_types_core::BackendId {
    fn from(value: HarnessFamily) -> Self {
        value.0
    }
}
impl fmt::Display for HarnessFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for HarnessFamily {
    type Err = std::convert::Infallible;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(value))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClaudeModel {
    Opus55,

    Sonnet55,

    Haiku55,

    Fable55,

    Fable51,

    Mythos51,

    Opus46,

    Sonnet46,

    Haiku45,
}

impl ClaudeModel {
    pub const ALL: [Self; 9] = [
        Self::Opus55,
        Self::Sonnet55,
        Self::Haiku55,
        Self::Fable55,
        Self::Fable51,
        Self::Mythos51,
        Self::Opus46,
        Self::Sonnet46,
        Self::Haiku45,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opus55 => "claude-opus-5-5",
            Self::Sonnet55 => "claude-sonnet-5-5",
            Self::Haiku55 => "claude-haiku-5-5",
            Self::Fable55 => "claude-fable-5-5",
            Self::Fable51 => "claude-fable-5-1",
            Self::Mythos51 => "claude-mythos-5-1",
            Self::Opus46 => "claude-opus-4-6",
            Self::Sonnet46 => "claude-sonnet-4-6",
            Self::Haiku45 => "claude-haiku-4-5",
        }
    }

    pub fn default_thinking(&self) -> Thinking {
        ModelSelection::Claude(*self)
            .capabilities(ModelTransport::Native)
            .default_thinking()
    }

    pub fn supports_thinking(&self, thinking: Thinking) -> bool {
        ModelSelection::Claude(*self)
            .capabilities(ModelTransport::Native)
            .supports_thinking(thinking)
    }
}

impl fmt::Display for ClaudeModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ClaudeModel {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "claude-opus-5-5" | "opus" => Ok(Self::Opus55),
            "claude-sonnet-5-5" | "sonnet" => Ok(Self::Sonnet55),
            "claude-haiku-5-5" | "haiku" => Ok(Self::Haiku55),
            "claude-fable-5-5" | "fable" => Ok(Self::Fable55),
            "claude-fable-5-1" => Ok(Self::Fable51),
            "claude-mythos-5-1" | "mythos" => Ok(Self::Mythos51),
            "claude-opus-4-6" => Ok(Self::Opus46),
            "claude-sonnet-4-6" => Ok(Self::Sonnet46),
            "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => Ok(Self::Haiku45),
            _ => Err(
                "unsupported Claude routing model; use opus, sonnet, fable, mythos, haiku or a supported Claude model ID",
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelSelection {
    pub backend: BackendId,

    pub model: String,
}
impl ModelSelection {
    pub fn new(backend: impl Into<HarnessFamily>, model: impl Into<String>) -> Self {
        Self {
            backend: backend.into().into(),
            model: model.into(),
        }
    }

    pub fn validate(&self) -> crate::Result<()> {
        if self.backend.as_str().trim().is_empty() || self.model.trim().is_empty() {
            return Err(NanocodeError::InvalidRequest(
                "backend and model identities must not be empty".into(),
            ));
        }
        Ok(())
    }

    #[allow(non_snake_case)]
    pub fn Codex(model: Model) -> Self {
        Self::new(HarnessFamily::Codex, model.as_str())
    }

    #[allow(non_snake_case)]
    pub fn Claude(model: ClaudeModel) -> Self {
        Self::new(HarnessFamily::Claude, model.as_str())
    }

    pub fn as_codex(&self) -> Option<Model> {
        (self.backend == BackendId::Codex)
            .then(|| self.model.parse().ok())
            .flatten()
    }

    pub fn as_claude(&self) -> Option<ClaudeModel> {
        (self.backend == BackendId::Claude)
            .then(|| self.model.parse().ok())
            .flatten()
    }

    pub fn family(&self) -> HarnessFamily {
        self.backend.clone().into()
    }

    pub fn as_str(&self) -> &str {
        &self.model
    }

    pub fn default_thinking(&self) -> Thinking {
        self.capabilities(ModelTransport::Native).default_thinking()
    }

    pub fn supports_thinking(&self, thinking: Thinking) -> bool {
        self.capabilities(ModelTransport::Native)
            .supports_thinking(thinking)
    }

    pub fn supports_fast_mode(&self) -> bool {
        self.capabilities(ModelTransport::Native).fast_mode()
    }

    pub fn supports_reasoning_mode(&self, mode: ReasoningMode) -> bool {
        self.capabilities(ModelTransport::Native)
            .supports_reasoning_mode(mode)
    }

    pub fn capabilities(&self, transport: ModelTransport) -> ModelCapabilities {
        use Thinking::{High, Low, Max, Medium, None, Xhigh};
        const fn set(levels: &[Thinking]) -> u8 {
            let mut bits = 0;
            let mut index = 0;
            while index < levels.len() {
                bits |= thinking_bit(levels[index]);
                index += 1;
            }
            bits
        }
        let (thinking, default_thinking, fast_mode, ultrafast, pro) =
            match (self.as_codex(), self.as_claude(), transport) {
                (Some(model), _, _) => {
                    let mut bits = 0;
                    let mut index = 0;
                    while index < Thinking::ALL.len() {
                        if model.supports_thinking(Thinking::ALL[index]) {
                            bits |= thinking_bit(Thinking::ALL[index]);
                        }
                        index += 1;
                    }
                    (
                        bits,
                        model.default_thinking(),
                        matches!(
                            model,
                            Model::Astra | Model::Sol | Model::Luna | Model::DaybreakBlue
                        ),
                        matches!(transport, ModelTransport::Native)
                            && matches!(model, Model::Astra | Model::Sol),
                        model.supports_reasoning_mode(ReasoningMode::Pro),
                    )
                }
                (_, Some(ClaudeModel::Haiku45), _) => (set(&[None]), None, false, false, false),

                (_, Some(model), ModelTransport::Native) => (
                    match model {
                        ClaudeModel::Opus46 | ClaudeModel::Sonnet46 => {
                            set(&[None, Low, Medium, High, Max])
                        }
                        ClaudeModel::Haiku55 => set(&[None, Low, Medium, High, Xhigh, Max]),
                        _ => set(&[Low, Medium, High, Xhigh, Max]),
                    },
                    match model {
                        ClaudeModel::Opus55 | ClaudeModel::Haiku55 => Medium,
                        _ => High,
                    },
                    matches!(model, ClaudeModel::Opus55),
                    false,
                    false,
                ),

                (_, Some(_), ModelTransport::Managed) => {
                    (set(&[Low, Medium, High]), Medium, false, false, false)
                }
                _ => (set(&Thinking::ALL), Medium, false, false, false),
            };
        ModelCapabilities {
            model: self.clone(),
            transport,
            thinking,
            default_thinking,
            fast_mode,
            ultrafast,
            pro,
        }
    }

    pub fn display_name(&self) -> &'static str {
        display_name(self)
    }

    pub fn for_family(family: HarnessFamily) -> impl Iterator<Item = Self> {
        Model::ALL
            .into_iter()
            .map(Self::Codex)
            .chain(ClaudeModel::ALL.into_iter().map(Self::Claude))
            .filter(move |model| model.family() == family)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ModelTransport {
    Native,

    Managed,
}

impl ModelTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Managed => "managed",
        }
    }
}

const fn thinking_bit(thinking: Thinking) -> u8 {
    1 << thinking as u8
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilities {
    model: ModelSelection,
    transport: ModelTransport,
    thinking: u8,
    default_thinking: Thinking,
    fast_mode: bool,
    ultrafast: bool,
    pro: bool,
}

impl ModelCapabilities {
    pub fn model(&self) -> ModelSelection {
        self.model.clone()
    }

    pub fn display_name(&self) -> &'static str {
        display_name(&self.model)
    }

    pub const fn transport(&self) -> ModelTransport {
        self.transport
    }

    pub const fn default_thinking(&self) -> Thinking {
        self.default_thinking
    }

    pub const fn supports_thinking(&self, thinking: Thinking) -> bool {
        self.thinking & thinking_bit(thinking) != 0
    }

    pub fn thinking(&self) -> impl Iterator<Item = Thinking> + use<> {
        let capabilities = self.clone();
        Thinking::ALL
            .into_iter()
            .filter(move |thinking| capabilities.supports_thinking(*thinking))
    }

    pub const fn fast_mode(&self) -> bool {
        self.fast_mode
    }

    pub const fn supports_service_tier(&self, tier: ServiceTier) -> bool {
        match tier {
            ServiceTier::Standard => true,
            ServiceTier::Priority | ServiceTier::Fast => self.fast_mode,
            ServiceTier::Ultrafast => self.ultrafast,
        }
    }

    pub fn service_tiers(&self) -> impl Iterator<Item = ServiceTier> + use<> {
        let capabilities = self.clone();
        [
            ServiceTier::Standard,
            ServiceTier::Fast,
            ServiceTier::Ultrafast,
        ]
        .into_iter()
        .filter(move |tier| capabilities.supports_service_tier(*tier))
    }

    pub const fn supports_reasoning_mode(&self, mode: ReasoningMode) -> bool {
        match mode {
            ReasoningMode::Standard => true,
            ReasoningMode::Pro => self.pro,
        }
    }

    pub fn reasoning_modes(&self) -> impl Iterator<Item = ReasoningMode> + use<> {
        let capabilities = self.clone();
        [ReasoningMode::Standard, ReasoningMode::Pro]
            .into_iter()
            .filter(move |mode| capabilities.supports_reasoning_mode(*mode))
    }

    pub const fn normalize_thinking(&self, thinking: Thinking) -> Thinking {
        if self.supports_thinking(thinking) {
            thinking
        } else {
            self.default_thinking
        }
    }

    pub const fn normalize_service_tier(&self, tier: ServiceTier) -> ServiceTier {
        if self.supports_service_tier(tier) {
            tier
        } else {
            ServiceTier::Standard
        }
    }

    pub const fn normalize_reasoning_mode(&self, mode: ReasoningMode) -> ReasoningMode {
        if self.supports_reasoning_mode(mode) {
            mode
        } else {
            ReasoningMode::Standard
        }
    }

    pub fn check_thinking(&self, thinking: Thinking) -> crate::Result<()> {
        if self.supports_thinking(thinking) {
            return Ok(());
        }
        Err(NanocodeError::InvalidRequest(format!(
            "{} requires a supported thinking level{}; supported thinking: {} ({} does not support {thinking} thinking)",
            display_name(&self.model),
            self.transport_suffix(),
            join(self.thinking().map(Thinking::as_str)),
            self.model,
        )))
    }

    pub fn check_service_tier(&self, tier: ServiceTier) -> crate::Result<()> {
        if self.supports_service_tier(tier) {
            return Ok(());
        }
        Err(self.unsupported(
            &format!("the {} service tier", tier.as_str()),
            &format!(
                "supported tiers: {}",
                join(self.service_tiers().map(ServiceTier::as_str))
            ),
        ))
    }

    pub fn check_fast_mode(&self, enabled: bool) -> crate::Result<()> {
        if !enabled || self.fast_mode {
            return Ok(());
        }
        Err(self.unsupported(
            "fast mode",
            &format!(
                "supported tiers: {}",
                join(self.service_tiers().map(ServiceTier::as_str))
            ),
        ))
    }

    pub fn check_reasoning_mode(&self, mode: ReasoningMode) -> crate::Result<()> {
        if self.supports_reasoning_mode(mode) {
            return Ok(());
        }
        Err(self.unsupported(
            &format!("{mode} reasoning mode"),
            &format!(
                "supported modes: {}",
                join(self.reasoning_modes().map(ReasoningMode::as_str))
            ),
        ))
    }

    const fn transport_suffix(&self) -> &'static str {
        match self.transport {
            ModelTransport::Native => "",
            ModelTransport::Managed => " on the managed service",
        }
    }

    fn unsupported(&self, setting: &str, supported: &str) -> NanocodeError {
        NanocodeError::InvalidRequest(format!(
            "{} ({}) does not support {setting}{}; {supported}",
            display_name(&self.model),
            self.model,
            self.transport_suffix(),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelInfo {
    pub harness: HarnessFamily,
    pub id: String,
    pub name: &'static str,
    pub efforts: Vec<Thinking>,
    pub effort: Thinking,
    pub fast: bool,
    pub service_tiers: Vec<ServiceTier>,
    pub reasoning_modes: Vec<ReasoningMode>,
}

impl ModelInfo {
    pub fn new(capabilities: &ModelCapabilities) -> Self {
        Self {
            harness: capabilities.model.family(),
            id: capabilities.model.model.clone(),
            name: capabilities.display_name(),
            efforts: capabilities.thinking().collect(),
            effort: capabilities.default_thinking(),
            fast: capabilities.fast_mode(),
            service_tiers: capabilities.service_tiers().collect(),
            reasoning_modes: capabilities.reasoning_modes().collect(),
        }
    }

    pub fn of(model: &ModelSelection) -> Self {
        Self::new(&model.capabilities(ModelTransport::Native))
    }

    pub fn selection(&self) -> ModelSelection {
        ModelSelection::new(self.harness.clone(), self.id.clone())
    }

    pub fn runs(&self, thinking: Thinking, fast: bool) -> bool {
        self.efforts.contains(&thinking) && (!fast || self.fast)
    }
}

pub fn model_catalog(transport: ModelTransport) -> impl Iterator<Item = ModelInfo> {
    HarnessFamily::builtin()
        .into_iter()
        .flat_map(|family| family.models())
        .map(move |model| ModelInfo::new(&model.capabilities(transport)))
}

fn display_name(model: &ModelSelection) -> &'static str {
    match (model.as_codex(), model.as_claude()) {
        (Some(Model::Sol), _) => "GPT-6.1 Sol",
        (Some(Model::Luna), _) => "GPT-6 Luna",
        (Some(Model::Astra), _) => "GPT-6 Astra",
        (Some(Model::Glm53), _) => "GLM-5.3",
        (Some(Model::Kimi), _) => "Kimi K3",
        (Some(Model::Mimo), _) => "MiMo V2.6 Pro",
        (Some(Model::DaybreakBlue), _) => "GPT Daybreak Blue",
        (Some(Model::Custom(model)), _) => model.as_str(),
        (_, Some(ClaudeModel::Opus55)) => "Claude Opus 5.5",
        (_, Some(ClaudeModel::Sonnet55)) => "Claude Sonnet 5.5",
        (_, Some(ClaudeModel::Haiku55)) => "Claude Haiku 5.5",
        (_, Some(ClaudeModel::Fable55)) => "Claude Fable 5.5",
        (_, Some(ClaudeModel::Fable51)) => "Claude Fable 5.1",
        (_, Some(ClaudeModel::Mythos51)) => "Claude Mythos 5.1",
        (_, Some(ClaudeModel::Opus46)) => "Claude Opus 4.6",
        (_, Some(ClaudeModel::Sonnet46)) => "Claude Sonnet 4.6",
        (_, Some(ClaudeModel::Haiku45)) => "Claude Haiku 4.5",
        _ => "The selected model",
    }
}

fn join<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values.collect::<Vec<_>>().join(", ")
}

impl From<Model> for ModelSelection {
    fn from(model: Model) -> Self {
        Self::Codex(model)
    }
}
impl Default for ModelSelection {
    fn default() -> Self {
        Self::Codex(Model::default())
    }
}
impl From<ClaudeModel> for ModelSelection {
    fn from(model: ClaudeModel) -> Self {
        Self::Claude(model)
    }
}
impl PartialEq<Model> for ModelSelection {
    fn eq(&self, model: &Model) -> bool {
        self == &Self::Codex(*model)
    }
}
impl PartialEq<ModelSelection> for Model {
    fn eq(&self, model: &ModelSelection) -> bool {
        model == self
    }
}
impl fmt::Display for ModelSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for ModelSelection {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(model) = Model::known(value) {
            return Ok(Self::Codex(model));
        }
        if let Ok(model) = value.parse::<ClaudeModel>() {
            return Ok(Self::Claude(model));
        }
        if value.trim().is_empty() {
            return Err("model identities must not be empty");
        }
        Ok(Self::new(HarnessFamily::Codex, value))
    }
}

impl Serialize for ModelSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self
            .as_codex()
            .is_some_and(|model| model.as_str() == self.as_str())
            || self
                .as_claude()
                .is_some_and(|model| model.as_str() == self.as_str())
        {
            serializer.serialize_str(self.as_str())
        } else {
            QualifiedModel {
                backend: self.backend.clone(),
                model: self.model.clone(),
            }
            .serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for ModelSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Known(String),
            Open(QualifiedModel),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Known(value) => value.parse().map_err(de::Error::custom),
            Wire::Open(value) => {
                let model = Self::new(value.backend, value.model);
                model.validate().map_err(de::Error::custom)?;
                Ok(model)
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct QualifiedModel {
    backend: BackendId,
    model: String,
}

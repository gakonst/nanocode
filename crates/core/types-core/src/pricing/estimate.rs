use serde::{Deserialize, Serialize};

use super::UsdAmount;
use crate::{Model, responses::Usage};

const SOL_STANDARD: TokenRates = TokenRates {
    input: 2_000,
    cached_input: 100,
    cache_write_input: 2_500,
    output: 10_000,
};
const SOL_PRIORITY: TokenRates = TokenRates {
    input: 4_000,
    cached_input: 200,
    cache_write_input: 5_000,
    output: 20_000,
};
const SOL_LONG_CONTEXT_STANDARD: TokenRates = TokenRates {
    input: 4_000,
    cached_input: 200,
    cache_write_input: 5_000,
    output: 15_000,
};
const SOL_LONG_CONTEXT_PRIORITY: TokenRates = TokenRates {
    input: 8_000,
    cached_input: 400,
    cache_write_input: 10_000,
    output: 30_000,
};
const TERRA_STANDARD: TokenRates = TokenRates {
    input: 1_000,
    cached_input: 50,
    cache_write_input: 1_250,
    output: 5_000,
};
const TERRA_PRIORITY: TokenRates = TokenRates {
    input: 2_000,
    cached_input: 100,
    cache_write_input: 2_500,
    output: 10_000,
};
const TERRA_LONG_CONTEXT_STANDARD: TokenRates = TokenRates {
    input: 2_000,
    cached_input: 100,
    cache_write_input: 2_500,
    output: 7_500,
};
const TERRA_LONG_CONTEXT_PRIORITY: TokenRates = TokenRates {
    input: 4_000,
    cached_input: 200,
    cache_write_input: 5_000,
    output: 15_000,
};
const LUNA_STANDARD: TokenRates = TokenRates {
    input: 100,
    cached_input: 10,
    cache_write_input: 125,
    output: 500,
};
const LUNA_PRIORITY: TokenRates = TokenRates {
    input: 200,
    cached_input: 20,
    cache_write_input: 250,
    output: 1_000,
};
const LUNA_LONG_CONTEXT_STANDARD: TokenRates = TokenRates {
    input: 200,
    cached_input: 20,
    cache_write_input: 250,
    output: 750,
};
const LUNA_LONG_CONTEXT_PRIORITY: TokenRates = TokenRates {
    input: 400,
    cached_input: 40,
    cache_write_input: 500,
    output: 1_500,
};
const ASTRA_STANDARD: TokenRates = TokenRates {
    input: 10_000,
    cached_input: 1_000,
    cache_write_input: 12_500,
    output: 50_000,
};
const ASTRA_PRIORITY: TokenRates = TokenRates {
    input: 20_000,
    cached_input: 2_000,
    cache_write_input: 25_000,
    output: 100_000,
};
const ASTRA_LONG_CONTEXT_STANDARD: TokenRates = TokenRates {
    input: 20_000,
    cached_input: 2_000,
    cache_write_input: 25_000,
    output: 75_000,
};
const ASTRA_LONG_CONTEXT_PRIORITY: TokenRates = TokenRates {
    input: 40_000,
    cached_input: 4_000,
    cache_write_input: 50_000,
    output: 150_000,
};

const SOL_ULTRAFAST: TokenRates = TokenRates {
    input: 12_000,
    cached_input: 600,
    cache_write_input: 15_000,
    output: 60_000,
};
const SOL_LONG_CONTEXT_ULTRAFAST: TokenRates = TokenRates {
    input: 24_000,
    cached_input: 1_200,
    cache_write_input: 30_000,
    output: 90_000,
};
const ASTRA_ULTRAFAST: TokenRates = TokenRates {
    input: 60_000,
    cached_input: 6_000,
    cache_write_input: 75_000,
    output: 300_000,
};
const ASTRA_LONG_CONTEXT_ULTRAFAST: TokenRates = TokenRates {
    input: 120_000,
    cached_input: 12_000,
    cache_write_input: 150_000,
    output: 450_000,
};

const GLM53_STANDARD: TokenRates = TokenRates {
    input: 1_400,
    cached_input: 260,
    cache_write_input: 1_400,
    output: 4_400,
};

const KIMI_STANDARD: TokenRates = TokenRates {
    input: 3_000,
    cached_input: 300,
    cache_write_input: 3_000,
    output: 15_000,
};
const MIMO_STANDARD: TokenRates = TokenRates {
    input: 435,
    cached_input: 4,
    cache_write_input: 435,
    output: 870,
};
const UNPRICED: TokenRates = TokenRates {
    input: 0,
    cached_input: 0,
    cache_write_input: 0,
    output: 0,
};
const LONG_CONTEXT_THRESHOLD: u64 = 272_000;

#[derive(Clone, Copy)]
struct TokenRates {
    input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
}

impl TokenRates {
    const fn for_model(model: Model, service_tier: ServiceTier, input_tokens: u64) -> Self {
        use ServiceTier::{Fast, Priority, Standard, Ultrafast};

        let long = input_tokens > LONG_CONTEXT_THRESHOLD;
        match (model, service_tier, long) {
            (Model::Glm53, _, _) => GLM53_STANDARD,
            (Model::Kimi, _, _) => KIMI_STANDARD,
            (Model::Mimo, _, _) => MIMO_STANDARD,
            (Model::Custom(_) | Model::DaybreakBlue, _, _) => UNPRICED,
            (Model::Sol, Standard, false) => SOL_STANDARD,
            (Model::Sol, Priority | Fast, false) => SOL_PRIORITY,
            (Model::Sol, Ultrafast, false) => SOL_ULTRAFAST,
            (Model::Sol, Standard, true) => SOL_LONG_CONTEXT_STANDARD,
            (Model::Sol, Priority | Fast, true) => SOL_LONG_CONTEXT_PRIORITY,
            (Model::Sol, Ultrafast, true) => SOL_LONG_CONTEXT_ULTRAFAST,
            (Model::Terra, Standard, false) => TERRA_STANDARD,
            (Model::Terra, Priority | Fast | Ultrafast, false) => TERRA_PRIORITY,
            (Model::Terra, Standard, true) => TERRA_LONG_CONTEXT_STANDARD,
            (Model::Terra, Priority | Fast | Ultrafast, true) => TERRA_LONG_CONTEXT_PRIORITY,
            (Model::Luna, Standard, false) => LUNA_STANDARD,
            (Model::Luna, Priority | Fast | Ultrafast, false) => LUNA_PRIORITY,
            (Model::Luna, Standard, true) => LUNA_LONG_CONTEXT_STANDARD,
            (Model::Luna, Priority | Fast | Ultrafast, true) => LUNA_LONG_CONTEXT_PRIORITY,
            (Model::Astra, Standard, false) => ASTRA_STANDARD,
            (Model::Astra, Priority | Fast, false) => ASTRA_PRIORITY,
            (Model::Astra, Ultrafast, false) => ASTRA_ULTRAFAST,
            (Model::Astra, Standard, true) => ASTRA_LONG_CONTEXT_STANDARD,
            (Model::Astra, Priority | Fast, true) => ASTRA_LONG_CONTEXT_PRIORITY,
            (Model::Astra, Ultrafast, true) => ASTRA_LONG_CONTEXT_ULTRAFAST,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    #[default]
    Standard,

    Priority,

    Fast,

    Ultrafast,
}

impl ServiceTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Priority => "priority",
            Self::Fast => "fast",
            Self::Ultrafast => "ultrafast",
        }
    }

    #[must_use]
    pub const fn from_fast_mode(enabled: bool) -> Self {
        if enabled { Self::Fast } else { Self::Standard }
    }

    #[must_use]
    pub const fn effective_for_model(self, model: Model) -> Self {
        match (model, self) {
            (Model::Glm53 | Model::Kimi | Model::Mimo, _) => Self::Standard,
            (Model::Terra | Model::Luna, Self::Ultrafast) => Self::Fast,
            _ => self,
        }
    }

    #[must_use]
    pub const fn for_model(model: Model, fast_mode: bool) -> Self {
        Self::from_fast_mode(fast_mode).effective_for_model(model)
    }

    #[doc(hidden)]
    pub const fn request_value(self, model: Model) -> Option<&'static str> {
        match self.effective_for_model(model) {
            Self::Standard => None,
            Self::Priority | Self::Fast => Some("priority"),
            Self::Ultrafast => Some("ultrafast"),
        }
    }
}

impl From<bool> for ServiceTier {
    fn from(enabled: bool) -> Self {
        Self::from_fast_mode(enabled)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EstimatedUsdCost {
    #[serde(rename = "usd")]
    amount: UsdAmount,
    #[serde(rename = "input_usd")]
    input: UsdAmount,
    #[serde(rename = "cached_input_usd")]
    cached_input: UsdAmount,
    #[serde(rename = "cache_write_input_usd")]
    cache_write_input: UsdAmount,
    #[serde(rename = "output_usd")]
    output: UsdAmount,
    #[serde(default)]
    service_tier: ServiceTier,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReportedCost {
    pub usd: UsdAmount,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_usd: Option<UsdAmount>,
}

impl ReportedCost {
    #[must_use]
    pub fn as_cost(self, service_tier: ServiceTier) -> EstimatedUsdCost {
        let input = self.input_usd.unwrap_or_default().min(self.usd);
        EstimatedUsdCost {
            amount: self.usd,
            input,
            cached_input: UsdAmount::default(),
            cache_write_input: UsdAmount::default(),
            output: UsdAmount::from_nano_usd(self.usd.nano_usd() - input.nano_usd()),
            service_tier,
        }
    }
}

impl EstimatedUsdCost {
    #[must_use]
    pub const fn new(
        input: UsdAmount,
        cached_input: UsdAmount,
        cache_write_input: UsdAmount,
        output: UsdAmount,
        service_tier: ServiceTier,
    ) -> Self {
        Self {
            amount: input
                .saturating_add(cached_input)
                .saturating_add(cache_write_input)
                .saturating_add(output),
            input,
            cached_input,
            cache_write_input,
            output,
            service_tier,
        }
    }

    #[must_use]
    pub const fn amount(&self) -> UsdAmount {
        self.amount
    }

    #[must_use]
    pub const fn input(&self) -> UsdAmount {
        self.input
    }

    #[must_use]
    pub const fn cached_input(&self) -> UsdAmount {
        self.cached_input
    }

    #[must_use]
    pub const fn cache_write_input(&self) -> UsdAmount {
        self.cache_write_input
    }

    #[must_use]
    pub const fn output(&self) -> UsdAmount {
        self.output
    }

    #[must_use]
    pub const fn service_tier(&self) -> ServiceTier {
        self.service_tier
    }

    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        debug_assert_eq!(self.service_tier, other.service_tier);
        Self {
            amount: self.amount.saturating_add(other.amount),
            input: self.input.saturating_add(other.input),
            cached_input: self.cached_input.saturating_add(other.cached_input),
            cache_write_input: self
                .cache_write_input
                .saturating_add(other.cache_write_input),
            output: self.output.saturating_add(other.output),
            service_tier: self.service_tier,
        }
    }
}

#[must_use]
pub fn estimate(usage: &Usage, service_tier: ServiceTier) -> EstimatedUsdCost {
    estimate_for_model(usage, Model::Sol, service_tier)
}

#[must_use]
pub fn estimate_for_model(
    usage: &Usage,
    model: Model,
    service_tier: ServiceTier,
) -> EstimatedUsdCost {
    if let Some(reported) = usage.provider_cost() {
        return reported.as_cost(service_tier.effective_for_model(model));
    }
    let cached_input_tokens = usage
        .input_tokens_details
        .as_ref()
        .map_or(0, |details| details.cached_tokens);
    let cache_write_input_tokens = usage
        .input_tokens_details
        .as_ref()
        .map_or(0, |details| details.cache_write_tokens);
    estimate_tokens(
        usage.input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        usage.output_tokens,
        model,
        service_tier,
    )
}

#[doc(hidden)]
pub fn estimate_tokens(
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    model: Model,
    service_tier: ServiceTier,
) -> EstimatedUsdCost {
    let service_tier = service_tier.effective_for_model(model);
    let rates = TokenRates::for_model(model, service_tier, input_tokens);
    let cached_input_tokens = cached_input_tokens.min(input_tokens);
    let remaining_input = input_tokens.saturating_sub(cached_input_tokens);
    let cache_write_input_tokens = cache_write_input_tokens.min(remaining_input);
    let ordinary_input_tokens = remaining_input.saturating_sub(cache_write_input_tokens);

    let input = UsdAmount::saturating_mul(ordinary_input_tokens, rates.input);
    let cached_input = UsdAmount::saturating_mul(cached_input_tokens, rates.cached_input);
    let cache_write_input =
        UsdAmount::saturating_mul(cache_write_input_tokens, rates.cache_write_input);
    let output = UsdAmount::saturating_mul(output_tokens, rates.output);
    EstimatedUsdCost::new(input, cached_input, cache_write_input, output, service_tier)
}

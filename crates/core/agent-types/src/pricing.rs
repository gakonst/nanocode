use crate::{ClaudeModel, EstimatedUsdCost, ServiceTier, UsdAmount};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClaudeTokens {
    pub input_tokens: u64,

    pub cache_read_input_tokens: u64,

    pub cache_write_5m_input_tokens: u64,

    pub cache_write_1h_input_tokens: u64,

    pub output_tokens: u64,
}

impl ClaudeTokens {
    #[must_use]
    pub const fn prompt_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_write_5m_input_tokens)
            .saturating_add(self.cache_write_1h_input_tokens)
    }
}

#[derive(Clone, Copy)]
struct Rates {
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
}

const fn rates(
    input: u64,
    output: u64,
    cache_write_5m: u64,
    cache_write_1h: u64,
    cache_read: u64,
) -> Rates {
    Rates {
        input,
        output,
        cache_write_5m,
        cache_write_1h,
        cache_read,
    }
}

const HAIKU_55_STANDARD_PROMPT_TOKENS: u64 = 100_000;

impl ClaudeModel {
    #[must_use]
    pub fn estimate_cost(self, tokens: ClaudeTokens, fast_mode: bool) -> Option<EstimatedUsdCost> {
        let rates = self.rates(tokens.prompt_tokens(), fast_mode)?;
        let cache_write =
            UsdAmount::saturating_mul(tokens.cache_write_5m_input_tokens, rates.cache_write_5m)
                .saturating_add(UsdAmount::saturating_mul(
                    tokens.cache_write_1h_input_tokens,
                    rates.cache_write_1h,
                ));
        Some(EstimatedUsdCost::new(
            UsdAmount::saturating_mul(tokens.input_tokens, rates.input),
            UsdAmount::saturating_mul(tokens.cache_read_input_tokens, rates.cache_read),
            cache_write,
            UsdAmount::saturating_mul(tokens.output_tokens, rates.output),
            ServiceTier::from_fast_mode(fast_mode),
        ))
    }

    const fn rates(self, prompt_tokens: u64, fast_mode: bool) -> Option<Rates> {
        Some(match (self, fast_mode) {
            (Self::Fable55 | Self::Fable51 | Self::Mythos51, false) => {
                rates(10_000, 50_000, 12_500, 20_000, 250)
            }
            (Self::Opus55, false) => rates(4_000, 20_000, 5_000, 8_000, 200),
            (Self::Opus55, true) => rates(8_000, 40_000, 10_000, 16_000, 400),
            (Self::Sonnet55, false) => rates(2_000, 10_000, 2_500, 4_000, 100),
            (Self::Haiku55, false) if prompt_tokens > HAIKU_55_STANDARD_PROMPT_TOKENS => {
                rates(500, 2_500, 625, 1_000, 50)
            }
            (Self::Haiku55, false) => rates(100, 500, 125, 200, 10),
            (Self::Opus46, false) => rates(5_000, 25_000, 6_250, 10_000, 500),
            (Self::Sonnet46, false) => rates(3_000, 15_000, 3_750, 6_000, 300),
            (Self::Haiku45, false) => rates(1_000, 5_000, 1_250, 2_000, 100),
            (_, true) => return None,
        })
    }
}

use crate::session::Session;

/// Per-million-token pricing used for cost estimation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

/// Anthropic list prices in USD per million tokens: input, output, 5-minute
/// cache write (1.25x input) and cache read.
const fn anthropic_pricing(
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
) -> ModelPricing {
    ModelPricing {
        input_cost_per_million: input,
        output_cost_per_million: output,
        cache_creation_cost_per_million: cache_write,
        cache_read_cost_per_million: cache_read,
    }
}

const FABLE_5_1: ModelPricing = anthropic_pricing(10.0, 50.0, 12.5, 0.25);
const FABLE_5: ModelPricing = anthropic_pricing(10.0, 50.0, 12.5, 1.0);
const OPUS_5_5: ModelPricing = anthropic_pricing(4.0, 20.0, 5.0, 0.2);
/// Opus 4.5 through Opus 5.
const OPUS_4_5: ModelPricing = anthropic_pricing(5.0, 25.0, 6.25, 0.5);
/// Opus 3, 4 and 4.1.
const OPUS_4: ModelPricing = anthropic_pricing(15.0, 75.0, 18.75, 1.5);
const SONNET_5: ModelPricing = anthropic_pricing(2.0, 10.0, 2.5, 0.2);
/// Sonnet 3.5 through 4.6.
const SONNET_4: ModelPricing = anthropic_pricing(3.0, 15.0, 3.75, 0.3);
/// Haiku 5.5 for prompts up to 100K tokens (longer prompts cost 5x).
const HAIKU_5_5: ModelPricing = anthropic_pricing(0.1, 0.5, 0.125, 0.01);
const HAIKU_4_5: ModelPricing = anthropic_pricing(1.0, 5.0, 1.25, 0.1);
const HAIKU_3_5: ModelPricing = anthropic_pricing(0.8, 4.0, 1.0, 0.08);
const HAIKU_3: ModelPricing = anthropic_pricing(0.25, 1.25, 0.3, 0.03);

impl ModelPricing {
    /// Claude Sonnet 4.x list pricing ($3 / $15 per million tokens).
    #[must_use]
    pub const fn default_sonnet_tier() -> Self {
        SONNET_4
    }
}

/// Token counters accumulated for a conversation turn or session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

/// Estimated dollar cost derived from a [`TokenUsage`] sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageCostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
}

impl UsageCostEstimate {
    #[must_use]
    pub fn total_cost_usd(self) -> f64 {
        self.input_cost_usd
            + self.output_cost_usd
            + self.cache_creation_cost_usd
            + self.cache_read_cost_usd
    }
}

/// Returns the Anthropic list pricing of a Claude model id or alias, in any
/// of the usual spellings (`claude-sonnet-4-6`, `claude-3-5-haiku-20241022`,
/// `anthropic/claude-opus-4.1`, `anthropic.claude-opus-5-5`, `opus`).
///
/// Every other model returns `None`: gateway and third-party models (`auto`,
/// `groq/openai/gpt-oss-120b`, `gemini/...`, `cohere/north-mini-code:free`)
/// have no price the client knows, and many cost nothing, so callers must
/// report their cost as unknown instead of substituting a Claude rate (see
/// [`TokenUsage::estimate_cost_usd_for_model`]).
#[must_use]
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    let normalized = model.to_ascii_lowercase();
    let tokens = normalized
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let position = tokens
        .iter()
        .position(|token| matches!(*token, "opus" | "sonnet" | "haiku" | "fable" | "mythos"))?;
    // `claude-opus-4-6` puts the version after the family, older ids such as
    // `claude-3-5-haiku` before it. Bare aliases have no version and price as
    // the models they resolve to (`claude-opus-4-6`, `claude-sonnet-4-6`,
    // `claude-haiku-4-5`).
    let version =
        version_after(&tokens[position + 1..]).or_else(|| version_before(&tokens[..position]));
    let at_least = |minimum: (u32, u32)| version.is_some_and(|version| version >= minimum);
    let pricing = match tokens[position] {
        "fable" | "mythos" if version.is_some() && !at_least((5, 1)) => FABLE_5,
        "fable" | "mythos" => FABLE_5_1,
        "opus" if at_least((5, 5)) => OPUS_5_5,
        "opus" if version.is_none() || at_least((4, 5)) => OPUS_4_5,
        "opus" => OPUS_4,
        "sonnet" if at_least((5, 0)) => SONNET_5,
        "sonnet" => SONNET_4,
        _ if at_least((5, 0)) => HAIKU_5_5,
        _ if version.is_none() || at_least((4, 0)) => HAIKU_4_5,
        _ if at_least((3, 5)) => HAIKU_3_5,
        _ => HAIKU_3,
    };
    Some(pricing)
}

/// A version component: a number of at most two digits (dates such as
/// `20250514` are not versions).
fn version_number(token: &str) -> Option<u32> {
    if token.len() <= 2 {
        token.parse().ok()
    } else {
        None
    }
}

/// `4-6` in `claude-opus-4-6-...`.
fn version_after(tokens: &[&str]) -> Option<(u32, u32)> {
    let mut parts = tokens.iter().map_while(|token| version_number(token));
    let major = parts.next()?;
    Some((major, parts.next().unwrap_or(0)))
}

/// `3-5` in `claude-3-5-haiku-...`: the run of version numbers that ends
/// right before the family name.
fn version_before(tokens: &[&str]) -> Option<(u32, u32)> {
    let run = tokens
        .iter()
        .rev()
        .map_while(|token| version_number(token))
        .take(2)
        .collect::<Vec<_>>();
    match run.as_slice() {
        [minor, major] => Some((*major, *minor)),
        [major] => Some((*major, 0)),
        _ => None,
    }
}

impl TokenUsage {
    #[must_use]
    pub fn total_tokens(self) -> u32 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }

    /// Cost at Claude Sonnet 4.x rates, whatever model produced the usage.
    /// Prefer [`Self::estimate_cost_usd_for_model`].
    #[must_use]
    pub fn estimate_cost_usd(self) -> UsageCostEstimate {
        self.estimate_cost_usd_with_pricing(ModelPricing::default_sonnet_tier())
    }

    /// Estimated cost at `model`'s Anthropic list price, or `None` when the
    /// model has no known price (gateway and third-party models, see
    /// [`pricing_for_model`]). Report `None` as an unknown cost, or as zero
    /// where a number is required; never as a Claude rate.
    #[must_use]
    pub fn estimate_cost_usd_for_model(self, model: &str) -> Option<UsageCostEstimate> {
        pricing_for_model(model).map(|pricing| self.estimate_cost_usd_with_pricing(pricing))
    }

    #[must_use]
    pub fn estimate_cost_usd_with_pricing(self, pricing: ModelPricing) -> UsageCostEstimate {
        UsageCostEstimate {
            input_cost_usd: cost_for_tokens(self.input_tokens, pricing.input_cost_per_million),
            output_cost_usd: cost_for_tokens(self.output_tokens, pricing.output_cost_per_million),
            cache_creation_cost_usd: cost_for_tokens(
                self.cache_creation_input_tokens,
                pricing.cache_creation_cost_per_million,
            ),
            cache_read_cost_usd: cost_for_tokens(
                self.cache_read_input_tokens,
                pricing.cache_read_cost_per_million,
            ),
        }
    }

    #[must_use]
    pub fn summary_lines(self, label: &str) -> Vec<String> {
        self.summary_lines_for_model(label, None)
    }

    /// Usage lines with the cost at `model`'s price. Without a model, or for
    /// a model with no known price, the cost is reported as unknown.
    #[must_use]
    pub fn summary_lines_for_model(self, label: &str, model: Option<&str>) -> Vec<String> {
        let cost = model.and_then(|model| self.estimate_cost_usd_for_model(model));
        let model_suffix =
            model.map_or_else(String::new, |model_name| format!(" model={model_name}"));
        let pricing_suffix = if cost.is_none() && model.is_some() {
            " pricing=unknown"
        } else {
            ""
        };
        let usage_line = format!(
            "{label}: total_tokens={} input={} output={} cache_write={} cache_read={} estimated_cost={}{}{}",
            self.total_tokens(),
            self.input_tokens,
            self.output_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
            cost.map_or_else(|| "unknown".to_string(), |cost| format_usd(cost.total_cost_usd())),
            model_suffix,
            pricing_suffix,
        );
        let breakdown_line = cost.map_or_else(
            || "  cost breakdown: unavailable (no known pricing)".to_string(),
            |cost| {
                format!(
                    "  cost breakdown: input={} output={} cache_write={} cache_read={}",
                    format_usd(cost.input_cost_usd),
                    format_usd(cost.output_cost_usd),
                    format_usd(cost.cache_creation_cost_usd),
                    format_usd(cost.cache_read_cost_usd),
                )
            },
        );
        vec![usage_line, breakdown_line]
    }
}

fn cost_for_tokens(tokens: u32, usd_per_million_tokens: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * usd_per_million_tokens
}

#[must_use]
/// Formats a dollar-denominated value for CLI display.
pub fn format_usd(amount: f64) -> String {
    format!("${amount:.4}")
}

/// Aggregates token usage across a running session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTracker {
    latest_turn: TokenUsage,
    cumulative: TokenUsage,
    turns: u32,
}

impl UsageTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        let mut tracker = Self::new();
        for message in &session.messages {
            if let Some(usage) = message.usage {
                tracker.record(usage);
            }
        }
        tracker
    }

    pub fn record(&mut self, usage: TokenUsage) {
        self.latest_turn = usage;
        self.cumulative.input_tokens += usage.input_tokens;
        self.cumulative.output_tokens += usage.output_tokens;
        self.cumulative.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        self.cumulative.cache_read_input_tokens += usage.cache_read_input_tokens;
        self.turns += 1;
    }

    #[must_use]
    pub fn current_turn_usage(&self) -> TokenUsage {
        self.latest_turn
    }

    #[must_use]
    pub fn cumulative_usage(&self) -> TokenUsage {
        self.cumulative
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }
}

#[cfg(test)]
mod tests {
    use super::{format_usd, pricing_for_model, TokenUsage, UsageTracker};
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn tracks_true_cumulative_usage() {
        let mut tracker = UsageTracker::new();
        tracker.record(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: 2,
            cache_read_input_tokens: 1,
        });
        tracker.record(TokenUsage {
            input_tokens: 20,
            output_tokens: 6,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 2,
        });

        assert_eq!(tracker.turns(), 2);
        assert_eq!(tracker.current_turn_usage().input_tokens, 20);
        assert_eq!(tracker.current_turn_usage().output_tokens, 6);
        assert_eq!(tracker.cumulative_usage().output_tokens, 10);
        assert_eq!(tracker.cumulative_usage().input_tokens, 30);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 48);
    }

    #[test]
    fn computes_cost_summary_lines() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 100_000,
            cache_read_input_tokens: 200_000,
        };

        let cost = usage.estimate_cost_usd();
        assert_eq!(format_usd(cost.input_cost_usd), "$3.0000");
        assert_eq!(format_usd(cost.output_cost_usd), "$7.5000");
        let lines = usage.summary_lines_for_model("usage", Some("claude-sonnet-4-20250514"));
        assert!(lines[0].contains("estimated_cost=$10.9350"), "{}", lines[0]);
        assert!(lines[0].contains("model=claude-sonnet-4-20250514"));
        assert!(lines[1].contains("cache_read=$0.0600"), "{}", lines[1]);
    }

    #[test]
    fn supports_model_specific_pricing() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let cost = |model: &str| {
            format_usd(
                usage
                    .estimate_cost_usd_for_model(model)
                    .unwrap_or_else(|| panic!("{model} should have pricing"))
                    .total_cost_usd(),
            )
        };

        // $1 / $5
        assert_eq!(cost("claude-haiku-4-5-20251001"), "$3.5000");
        assert_eq!(cost("haiku"), "$3.5000");
        // $5 / $25 from Opus 4.5 on; $15 / $75 for Opus 4 and 4.1.
        assert_eq!(cost("claude-opus-4-6"), "$17.5000");
        assert_eq!(cost("claude-opus-4-5@20251101"), "$17.5000");
        assert_eq!(cost("opus"), "$17.5000");
        assert_eq!(cost("claude-opus-4-1-20250805"), "$52.5000");
        assert_eq!(cost("claude-opus-4-20250514"), "$52.5000");
        assert_eq!(cost("claude-3-opus-20240229"), "$52.5000");
        // $4 / $20
        assert_eq!(cost("anthropic.claude-opus-5-5"), "$14.0000");
        // $3 / $15 through Sonnet 4.6, $2 / $10 from Sonnet 5.
        assert_eq!(cost("claude-sonnet-4-6"), "$10.5000");
        assert_eq!(cost("anthropic/claude-sonnet-4.5"), "$10.5000");
        assert_eq!(cost("claude-3-7-sonnet-latest"), "$10.5000");
        assert_eq!(cost("sonnet"), "$10.5000");
        assert_eq!(cost("claude-sonnet-5-5"), "$7.0000");
        // Older Haiku generations.
        assert_eq!(cost("claude-3-5-haiku-20241022"), "$2.8000");
        assert_eq!(cost("claude-3-haiku-20240307"), "$0.8750");
        assert_eq!(cost("claude-haiku-5-5"), "$0.3500");
        // $10 / $50
        assert_eq!(cost("claude-fable-5-1"), "$35.0000");
    }

    #[test]
    fn cache_read_rates_follow_the_model_generation() {
        let read = |model: &str| {
            pricing_for_model(model)
                .expect("claude pricing")
                .cache_read_cost_per_million
        };
        assert!((read("claude-fable-5-1") - 0.25).abs() < f64::EPSILON);
        assert!((read("claude-fable-5") - 1.0).abs() < f64::EPSILON);
        assert!((read("claude-opus-5-5") - 0.2).abs() < f64::EPSILON);
        assert!((read("claude-sonnet-4-6") - 0.3).abs() < f64::EPSILON);
    }

    /// Free gateway models must not be billed at a Claude rate: the gateway
    /// records their cost as zero.
    #[test]
    fn gateway_and_unknown_models_have_no_cost_estimate() {
        let usage = TokenUsage {
            input_tokens: 100_000,
            output_tokens: 100_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        for model in [
            "auto",
            "gemini/gemini-3.6-flash",
            "groq/openai/gpt-oss-120b",
            "nvidia/nemotron-ultra",
            "cohere/north-mini-code:free",
            "custom-model",
            "gpt-5",
        ] {
            assert_eq!(pricing_for_model(model), None, "{model}");
            assert_eq!(usage.estimate_cost_usd_for_model(model), None, "{model}");
        }
    }

    #[test]
    fn marks_unknown_model_pricing_as_unknown() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let lines = usage.summary_lines_for_model("usage", Some("custom-model"));
        assert!(lines[0].contains("estimated_cost=unknown"), "{}", lines[0]);
        assert!(lines[0].contains("pricing=unknown"));
        assert!(!lines[1].contains('$'), "{}", lines[1]);
        let without_model = usage.summary_lines("usage");
        assert!(without_model[0].contains("estimated_cost=unknown"));
    }

    #[test]
    fn reconstructs_usage_from_session_messages() {
        let mut session = Session::new();
        session.messages = vec![ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            usage: Some(TokenUsage {
                input_tokens: 5,
                output_tokens: 2,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 0,
            }),
        }];

        let tracker = UsageTracker::from_session(&session);
        assert_eq!(tracker.turns(), 1);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 8);
    }
}

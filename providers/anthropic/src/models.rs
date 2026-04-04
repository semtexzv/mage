//! Anthropic model catalog.

use llm::types::{InputModality, Model, ModelCost};

/// Macro to reduce boilerplate for model definitions.
macro_rules! model {
    ($id:expr, $name:expr, reasoning: $reasoning:expr, cost: ($inp:expr, $out:expr, $cr:expr, $cw:expr), ctx: $ctx:expr, max: $max:expr) => {
        Model {
            id: $id.into(),
            name: $name.into(),
            api: "anthropic".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: $reasoning,
            input: vec![InputModality::Text, InputModality::Image],
            cost: ModelCost {
                input: $inp,
                output: $out,
                cache_read: $cr,
                cache_write: $cw,
            },
            context_window: $ctx,
            max_out: $max,
            headers: None,
        }
    };
}

/// Returns all known Anthropic models.
pub fn anthropic_models() -> Vec<Model> {
    vec![
        // ── Claude 4.6 Opus ──
        model!("claude-opus-4-6", "Claude Opus 4.6 (latest)",
            reasoning: true,
            cost: (5.0, 25.0, 0.5, 6.25),
            ctx: 200_000, max: 128_000),

        // ── Claude 4.5 Opus ──
        model!("claude-opus-4-5", "Claude Opus 4.5 (latest)",
            reasoning: true,
            cost: (5.0, 25.0, 0.5, 6.25),
            ctx: 200_000, max: 64_000),
        // ── Claude 4 Opus ──
        model!("claude-opus-4-1", "Claude Opus 4.1 (latest)",
            reasoning: true,
            cost: (15.0, 75.0, 1.5, 18.75),
            ctx: 200_000, max: 32_000),
        model!("claude-opus-4-1-20250805", "Claude Opus 4.1",
            reasoning: true,
            cost: (15.0, 75.0, 1.5, 18.75),
            ctx: 200_000, max: 32_000),
        model!("claude-opus-4-0", "Claude Opus 4 (latest)",
            reasoning: true,
            cost: (15.0, 75.0, 1.5, 18.75),
            ctx: 200_000, max: 32_000),
        model!("claude-opus-4-20250514", "Claude Opus 4",
            reasoning: true,
            cost: (15.0, 75.0, 1.5, 18.75),
            ctx: 200_000, max: 32_000),

        // ── Claude 4.5 Sonnet ──
        model!("claude-sonnet-4-5-20250514", "Claude Sonnet 4.5",
            reasoning: true,
            cost: (3.0, 15.0, 0.3, 3.75),
            ctx: 200_000, max: 64_000),
        model!("claude-sonnet-4-5", "Claude Sonnet 4.5 (latest)",
            reasoning: true,
            cost: (3.0, 15.0, 0.3, 3.75),
            ctx: 200_000, max: 64_000),

        // ── Claude 4 Sonnet ──
        model!("claude-sonnet-4-20250514", "Claude Sonnet 4",
            reasoning: true,
            cost: (3.0, 15.0, 0.3, 3.75),
            ctx: 200_000, max: 64_000),
        model!("claude-sonnet-4", "Claude Sonnet 4 (latest)",
            reasoning: true,
            cost: (3.0, 15.0, 0.3, 3.75),
            ctx: 200_000, max: 64_000),

        // ── Claude 4.5 Haiku ──
        model!("claude-haiku-4-5-20251001", "Claude Haiku 4.5",
            reasoning: true,
            cost: (1.0, 5.0, 0.1, 1.25),
            ctx: 200_000, max: 64_000),
        model!("claude-haiku-4-5", "Claude Haiku 4.5 (latest)",
            reasoning: true,
            cost: (1.0, 5.0, 0.1, 1.25),
            ctx: 200_000, max: 64_000),
    ]
}

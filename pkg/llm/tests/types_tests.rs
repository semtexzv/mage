use llm::{ModelCost, Usage};

#[test]
fn compute_cost() {
    let mut usage = Usage {
        input: 1000,
        output: 500,
        cache_read: 200,
        cache_write: 100,
        total_tokens: 1800,
        ..Usage::default()
    };
    let cost_per_token = ModelCost {
        input: 0.003,
        output: 0.015,
        cache_read: 0.0003,
        cache_write: 0.00375,
    };
    usage.compute_cost(&cost_per_token);
    assert!((usage.cost.input - 3.0).abs() < 1e-10);
    assert!((usage.cost.output - 7.5).abs() < 1e-10);
    assert!((usage.cost.cache_read - 0.06).abs() < 1e-10);
    assert!((usage.cost.cache_write - 0.375).abs() < 1e-10);
    assert!((usage.cost.total - 10.935).abs() < 1e-10);
}

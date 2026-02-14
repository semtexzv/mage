use llm::json_accum::JsonAccumulator;
use serde_json::json;

#[test]
fn accumulate_and_finish() {
    let mut acc = JsonAccumulator::new();
    acc.push(r#"{"path"#);
    acc.push(r#"": "/tmp/test"#);
    acc.push(r#""}"#);
    assert_eq!(acc.finish(), json!({"path": "/tmp/test"}));
}

#[test]
fn finish_empty() {
    let acc = JsonAccumulator::new();
    assert_eq!(acc.finish(), serde_json::Value::Null);
}

#[test]
fn partial_parse_complete() {
    let mut acc = JsonAccumulator::new();
    acc.push(r#"{"key": "value"}"#);
    assert_eq!(acc.partial_parse(), json!({"key": "value"}));
}

#[test]
fn partial_parse_incomplete_object() {
    let mut acc = JsonAccumulator::new();
    acc.push(r#"{"key": "val"#);
    assert_eq!(acc.partial_parse(), json!({"key": "val"}));
}

#[test]
fn partial_parse_incomplete_nested() {
    let mut acc = JsonAccumulator::new();
    acc.push(r#"{"a": [1, 2"#);
    assert_eq!(acc.partial_parse(), json!({"a": [1, 2]}));
}

#[test]
fn partial_parse_open_brace() {
    let mut acc = JsonAccumulator::new();
    acc.push("{");
    assert_eq!(acc.partial_parse(), json!({}));
}

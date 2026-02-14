use anthropic::sse::SseParser;

#[test]
fn basic_event() {
    let mut parser = SseParser::new();
    let events = parser.feed(b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("message_start"));
    assert_eq!(events[0].data, "{\"type\":\"message_start\"}");
}

#[test]
fn no_event_field() {
    let mut parser = SseParser::new();
    let events = parser.feed(b"data: hello\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, None);
    assert_eq!(events[0].data, "hello");
}

#[test]
fn multi_data_lines() {
    let mut parser = SseParser::new();
    let events = parser.feed(b"data: line1\ndata: line2\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "line1\nline2");
}

#[test]
fn multiple_events() {
    let mut parser = SseParser::new();
    let events = parser.feed(b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\n");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event.as_deref(), Some("a"));
    assert_eq!(events[1].event.as_deref(), Some("b"));
}

#[test]
fn chunked_input() {
    let mut parser = SseParser::new();
    let e1 = parser.feed(b"event: test\n");
    assert!(e1.is_empty());
    let e2 = parser.feed(b"data: hel");
    assert!(e2.is_empty());
    let e3 = parser.feed(b"lo\n\n");
    assert_eq!(e3.len(), 1);
    assert_eq!(e3[0].data, "hello");
}

#[test]
fn comments_ignored() {
    let mut parser = SseParser::new();
    let events = parser.feed(b": this is a comment\ndata: real\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, "real");
}

#[test]
fn empty_lines_between_events() {
    let mut parser = SseParser::new();
    let events = parser.feed(b"data: first\n\n\n\ndata: second\n\n");
    assert_eq!(events.len(), 2);
}

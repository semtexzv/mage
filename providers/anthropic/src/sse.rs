//! Server-Sent Events (SSE) line parser.
//!
//! Converts a byte stream into SSE events. Each event has an optional
//! `event` field and a `data` field. Events are delimited by blank lines.

/// A parsed SSE event.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// The `event:` field, if present.
    pub event: Option<String>,
    /// The `data:` field (concatenated if multiple `data:` lines).
    pub data: String,
}

/// Incremental SSE parser. Feed it lines (or chunks containing lines),
/// and it yields complete events.
pub struct SseParser {
    event_type: Option<String>,
    data_buf: String,
    line_buf: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            event_type: None,
            data_buf: String::new(),
            line_buf: String::new(),
        }
    }

    /// Feed a chunk of bytes. Returns an iterator of complete events.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let text = String::from_utf8_lossy(chunk);

        for ch in text.chars() {
            if ch == '\n' {
                self.process_line(&mut events);
                self.line_buf.clear();
            } else if ch != '\r' {
                self.line_buf.push(ch);
            }
        }

        events
    }

    /// Flush any remaining buffered data as an event (call at stream end).
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.line_buf.is_empty() {
            let mut events = Vec::new();
            self.process_line(&mut events);
            self.line_buf.clear();
            if !self.data_buf.is_empty() {
                let event = SseEvent {
                    event: self.event_type.take(),
                    data: std::mem::take(&mut self.data_buf),
                };
                return Some(event);
            }
            return events.into_iter().next();
        }
        if !self.data_buf.is_empty() {
            return Some(SseEvent {
                event: self.event_type.take(),
                data: std::mem::take(&mut self.data_buf),
            });
        }
        None
    }

    fn process_line(&mut self, events: &mut Vec<SseEvent>) {
        let line = &self.line_buf;

        if line.is_empty() {
            // Blank line = event boundary
            if !self.data_buf.is_empty() {
                events.push(SseEvent {
                    event: self.event_type.take(),
                    data: std::mem::take(&mut self.data_buf),
                });
            } else {
                self.event_type = None;
            }
            return;
        }

        if line.starts_with(':') {
            // Comment, ignore
            return;
        }

        if let Some(value) = line.strip_prefix("event:") {
            self.event_type = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("data:") {
            let value = value.trim_start();
            if !self.data_buf.is_empty() {
                self.data_buf.push('\n');
            }
            self.data_buf.push_str(value);
        } else if let Some(value) = line.strip_prefix("id:") {
            // We don't use event IDs, but parse them to avoid treating as unknown
            let _ = value;
        } else if let Some(value) = line.strip_prefix("retry:") {
            // We don't use retry, but parse
            let _ = value;
        }
        // Unknown fields are ignored per spec
    }
}
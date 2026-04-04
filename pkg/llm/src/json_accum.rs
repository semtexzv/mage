//! Accumulate streaming JSON fragments into a complete `serde_json::Value`.
//!
//! LLM providers stream tool call arguments as partial JSON strings.
//! This module concatenates fragments and supports both final parsing
//! and best-effort partial parsing for live UI display.

/// Accumulates JSON string fragments.
pub struct JsonAccumulator {
    buf: String,
}

impl JsonAccumulator {
    pub fn new() -> Self {
        Self { buf: String::new() }
    }

    /// Append a JSON fragment.
    pub fn push(&mut self, fragment: &str) {
        self.buf.push_str(fragment);
    }

    /// Parse the accumulated JSON and return the result.
    /// Returns an empty object `{}` if the buffer is empty or unparseable.
    /// (The API requires tool_use input to be a valid dictionary, never null.)
    pub fn finish(&self) -> serde_json::Value {
        if self.buf.is_empty() {
            return serde_json::Value::Object(Default::default());
        }
        serde_json::from_str(&self.buf)
            .unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
    }

    /// Attempt a best-effort parse of the current buffer.
    /// Useful for UI streaming of partially received JSON.
    /// Returns `Value::Null` if not parseable yet.
    pub fn partial_parse(&self) -> serde_json::Value {
        // Try parsing as-is first
        if let Ok(v) = serde_json::from_str(&self.buf) {
            return v;
        }
        // Try closing open braces/brackets
        try_close_json(&self.buf).unwrap_or(serde_json::Value::Null)
    }

    /// The raw accumulated string.
    pub fn raw(&self) -> &str {
        &self.buf
    }
}

/// Best-effort: close unclosed braces and brackets, then parse.
fn try_close_json(partial: &str) -> Option<serde_json::Value> {
    let mut closers = Vec::new();
    let mut in_string = false;
    let mut escape = false;

    for ch in partial.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => closers.push('}'),
            '[' => closers.push(']'),
            '}' | ']' => { closers.pop(); }
            _ => {}
        }
    }

    // If we're inside a string, close it first
    let mut attempt = partial.to_owned();
    if in_string {
        attempt.push('"');
    }
    // Close any open structures
    for closer in closers.into_iter().rev() {
        attempt.push(closer);
    }

    serde_json::from_str(&attempt).ok()
}
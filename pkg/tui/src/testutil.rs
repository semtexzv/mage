//! Test utilities — not part of the public API.

use crate::renderer::Terminal;

/// In-memory terminal for testing.
pub struct TestTerminal {
    pub output: String,
    pub cols: u16,
    pub rows: u16,
}

impl TestTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            output: String::new(),
            cols,
            rows,
        }
    }
}

impl Terminal for TestTerminal {
    fn write(&mut self, s: &str) {
        self.output.push_str(s);
    }
    fn flush(&mut self) {}
    fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }
}

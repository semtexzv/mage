//! Read tool — reads file contents with line numbers.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

pub struct ReadModule;

#[async_trait(?Send)]
impl Module for ReadModule {
    fn name(&self) -> &str { "read" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Read".into(),
                description: "Read a file from the filesystem. Returns contents with line numbers.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file to read"
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Line number to start reading from (0-based). Only provide for large files."
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Number of lines to read. Defaults to 2000."
                        }
                    },
                    "required": ["file_path"]
                }),
            },
            handler: Rc::new(ReadHandler),
        }]
    }
}

struct ReadHandler;

#[async_trait(?Send)]
impl ToolHandler for ReadHandler {
    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::failure("file_path is required"),
        };

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(2000) as usize;

        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Failed to read {path}: {e}")),
        };

        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();

        if offset >= total && total > 0 {
            return ToolResult::failure(format!(
                "Offset {offset} is beyond end of file ({total} lines)"
            ));
        }

        let end = (offset + limit).min(total);
        let selected = &lines[offset..end];

        let mut output = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = offset + i + 1; // 1-based
            output.push_str(&format!("{line_num}\t{line}\n"));
        }

        if end < total {
            output.push_str(&format!(
                "\n... ({} more lines, {} total)\n",
                total - end,
                total
            ));
        }

        ToolResult::success(output)
    }

    fn is_concurrent_safe(&self, _args: &serde_json::Value) -> bool {
        true
    }
}

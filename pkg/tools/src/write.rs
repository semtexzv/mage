//! Write tool — create or overwrite a file.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

pub struct WriteModule;

#[async_trait(?Send)]
impl Module for WriteModule {
    fn name(&self) -> &str { "write" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Write".into(),
                description: "Write content to a file. Creates the file if it doesn't exist, \
                    overwrites if it does. Prefer Edit for modifying existing files.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "The full content to write to the file"
                        }
                    },
                    "required": ["file_path", "content"]
                }),
            },
            handler: Rc::new(WriteHandler),
        }]
    }
}

struct WriteHandler;

#[async_trait(?Send)]
impl ToolHandler for WriteHandler {
    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::failure("file_path is required"),
        };
        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return ToolResult::failure("content is required"),
        };

        // Ensure parent directory exists
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.exists() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return ToolResult::failure(format!(
                        "Failed to create directory {}: {e}",
                        parent.display()
                    ));
                }
            }
        }

        if let Err(e) = tokio::fs::write(path, content).await {
            return ToolResult::failure(format!("Failed to write {path}: {e}"));
        }

        let lines = content.lines().count();
        ToolResult::success(format!("Wrote {path} ({lines} lines)"))
    }
}

//! Edit tool — exact string replacement in files.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

pub struct EditModule;

#[async_trait(?Send)]
impl Module for EditModule {
    fn name(&self) -> &str { "edit" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Edit".into(),
                description: "Perform exact string replacement in a file. The old_string must \
                    be unique in the file unless replace_all is true.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Absolute path to the file to edit"
                        },
                        "old_string": {
                            "type": "string",
                            "description": "The exact text to find and replace"
                        },
                        "new_string": {
                            "type": "string",
                            "description": "The replacement text"
                        },
                        "replace_all": {
                            "type": "boolean",
                            "description": "Replace all occurrences (default: false)"
                        }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            handler: Rc::new(EditHandler),
        }]
    }
}

struct EditHandler;

#[async_trait(?Send)]
impl ToolHandler for EditHandler {
    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let path = match args.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::failure("file_path is required"),
        };
        let old_string = match args.get("old_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::failure("old_string is required"),
        };
        let new_string = match args.get("new_string").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::failure("new_string is required"),
        };
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string == new_string {
            return ToolResult::failure("old_string and new_string must be different");
        }

        // Read file
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Failed to read {path}: {e}")),
        };

        // Check occurrences
        let count = content.matches(old_string).count();
        if count == 0 {
            return ToolResult::failure(format!(
                "old_string not found in {path}. Make sure it matches exactly, including whitespace."
            ));
        }
        if count > 1 && !replace_all {
            return ToolResult::failure(format!(
                "old_string found {count} times in {path}. \
                 Use replace_all: true to replace all, or provide more context to make it unique."
            ));
        }

        // Perform replacement
        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        // Write back
        if let Err(e) = tokio::fs::write(path, &new_content).await {
            return ToolResult::failure(format!("Failed to write {path}: {e}"));
        }

        let msg = if replace_all && count > 1 {
            format!("Replaced {count} occurrences in {path}")
        } else {
            format!("Edited {path}")
        };
        ToolResult::success(msg)
    }
}

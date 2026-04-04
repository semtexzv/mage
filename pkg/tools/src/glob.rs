//! Glob tool — fast file pattern matching.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;
use walkdir::WalkDir;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

/// Directories to always skip during traversal.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "dist",
    "build",
    ".next",
    ".nuxt",
];

pub struct GlobModule;

#[async_trait(?Send)]
impl Module for GlobModule {
    fn name(&self) -> &str { "glob" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Glob".into(),
                description: "Find files matching a glob pattern. Returns paths sorted by \
                    modification time (most recent first).".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern to match (e.g. \"**/*.rs\", \"src/**/*.ts\")"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search in (default: current working directory)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            handler: Rc::new(GlobHandler),
        }]
    }
}

struct GlobHandler;

#[async_trait(?Send)]
impl ToolHandler for GlobHandler {
    async fn execute(&self, args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let pattern = match args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return ToolResult::failure("pattern is required"),
        };

        let base_dir = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let glob = match globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
        {
            Ok(g) => g.compile_matcher(),
            Err(e) => return ToolResult::failure(format!("Invalid glob pattern: {e}")),
        };

        // Walk the directory tree (blocking — runs on the tokio thread pool).
        let base = base_dir.to_owned();
        let result = tokio::task::spawn_blocking(move || {
            let mut matches: Vec<(String, std::time::SystemTime)> = Vec::new();

            for entry in WalkDir::new(&base)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    // Skip hidden/ignored directories.
                    if e.file_type().is_dir() {
                        let name = e.file_name().to_string_lossy();
                        return !SKIP_DIRS.contains(&&*name);
                    }
                    true
                })
            {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                if entry.file_type().is_dir() {
                    continue;
                }

                // Match against the relative path from base.
                let rel = entry
                    .path()
                    .strip_prefix(&base)
                    .unwrap_or(entry.path());

                if glob.is_match(rel) {
                    let mtime = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    matches.push((entry.path().display().to_string(), mtime));
                }
            }

            // Sort by modification time, most recent first.
            matches.sort_by(|a, b| b.1.cmp(&a.1));
            matches
        })
        .await;

        let matches = match result {
            Ok(m) => m,
            Err(e) => return ToolResult::failure(format!("Glob search failed: {e}")),
        };

        if matches.is_empty() {
            return ToolResult::success("No files found matching the pattern.");
        }

        let output: String = matches.iter().map(|(p, _)| format!("{p}\n")).collect();
        ToolResult::success(format!("{} files found:\n{output}", matches.len()))
    }

    fn is_concurrent_safe(&self, _args: &serde_json::Value) -> bool {
        true
    }
}
